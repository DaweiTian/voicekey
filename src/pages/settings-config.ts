import { LLM_REFINE, TRANSLATION } from '@/shared/constants'
import { normalizeLLMRefineConfig, resolveLLMConnection } from '@/shared/llm-config'
import type { AppConfig, LLMRefineConfig } from '@/shared/types'

const LLM_CONNECTION_KEYS = [
  'openai',
  'deepseek',
  'openrouter',
  'tokendance',
  'mimo',
  'custom',
] as const

export type RefineFeatureFlags = {
  enabled: boolean
  translateOutput: boolean
  translationEnabled: boolean
}

export type MicrophoneDeviceIdentity = {
  deviceId: string
  label: string
}

function normalizeMicrophoneLabel(label: string): string {
  return label.trim().replace(/\s+/g, ' ').toLowerCase()
}

/**
 * A Chromium media-device ID can change when its origin/salt or the OS audio endpoint changes.
 * Rebind only when the stored ID is gone and one current device has the same exposed label.
 * Duplicate labels intentionally require an explicit user choice.
 */
export function resolveMicrophoneDeviceMigration(
  selectedDeviceId: string,
  selectedDeviceLabel: string,
  devices: MicrophoneDeviceIdentity[],
): MicrophoneDeviceIdentity | null {
  const normalizedDeviceId = selectedDeviceId.trim()
  const normalizedLabel = normalizeMicrophoneLabel(selectedDeviceLabel)
  if (!normalizedDeviceId || !normalizedLabel) return null
  if (devices.some((device) => device.deviceId === normalizedDeviceId)) return null

  const labelMatches = devices.filter(
    (device) => device.deviceId && normalizeMicrophoneLabel(device.label) === normalizedLabel,
  )
  return labelMatches.length === 1 ? labelMatches[0] : null
}

export function isRefineConfigComplete(config: LLMRefineConfig): boolean {
  const connection = resolveLLMConnection(config)
  return Boolean(connection.endpoint.trim() && connection.model.trim() && connection.apiKey.trim())
}

/** Maps a connection fingerprint to the timestamp of its last successful test. */
export type RefineConnectionValidationCache = Map<string, number>

/**
 * Stable identity of the resolved connection. Never log or persist this value: it
 * embeds the renderer-visible API key so that editing any connection field naturally
 * misses the cache and triggers a fresh test.
 */
export function getRefineConnectionFingerprint(config: LLMRefineConfig): string {
  const connection = resolveLLMConnection(config)
  return [connection.provider, connection.endpoint, connection.model, connection.apiKey].join('\n')
}

export function isRefineConnectionCacheFresh(
  cache: RefineConnectionValidationCache,
  fingerprint: string,
  now: number,
): boolean {
  const validatedAt = cache.get(fingerprint)
  if (validatedAt === undefined) return false
  if (now - validatedAt > LLM_REFINE.CONNECTION_CACHE_TTL_MS) {
    cache.delete(fingerprint)
    return false
  }
  return true
}

export function markRefineConnectionValidated(
  cache: RefineConnectionValidationCache,
  fingerprint: string,
  now: number,
): void {
  cache.set(fingerprint, now)
}

export function invalidateRefineConnection(
  cache: RefineConnectionValidationCache,
  fingerprint: string,
): void {
  cache.delete(fingerprint)
}

export function readRefineFeatureFlags(config: AppConfig): RefineFeatureFlags {
  const refine = normalizeLLMRefineConfig(config.llmRefine)
  return {
    enabled: refine.enabled,
    translateOutput: refine.translateOutput,
    translationEnabled: config.translation.enabled,
  }
}

export function anyRefineFeatureOn(flags: RefineFeatureFlags): boolean {
  return flags.enabled || flags.translateOutput || flags.translationEnabled
}

export function applyRefineFeatureFlags(
  config: AppConfig,
  refine: LLMRefineConfig,
  flags: RefineFeatureFlags,
): AppConfig {
  return {
    ...config,
    llmRefine: normalizeLLMRefineConfig({
      ...refine,
      enabled: flags.enabled,
      translateOutput: flags.translateOutput,
    }),
    translation: {
      ...config.translation,
      enabled: flags.translationEnabled,
    },
  }
}

export type ReconcileRefineFeaturesResult = {
  config: AppConfig
  /** Snapshot that should remain in the settings-page ref after this reconcile. */
  snapshot: RefineFeatureFlags | null
  /** True when restored/kept features are on and the new connection should be re-checked. */
  shouldReverifyConnection: boolean
}

/**
 * After a connection-affecting change (provider switch, etc.):
 * - complete target: restore snapshot if present, otherwise keep current feature flags
 * - incomplete target: force features off; snapshot prior flags once so a later complete
 *   target can restore the user's intent
 */
export function reconcileRefineFeaturesAfterConnectionChange(
  prev: AppConfig,
  nextRefine: LLMRefineConfig,
  snapshot: RefineFeatureFlags | null,
): ReconcileRefineFeaturesResult {
  const normalizedNext = normalizeLLMRefineConfig(nextRefine)
  const currentFlags = readRefineFeatureFlags(prev)

  if (isRefineConfigComplete(normalizedNext)) {
    const restoredFlags = snapshot ?? currentFlags
    return {
      config: applyRefineFeatureFlags(prev, normalizedNext, restoredFlags),
      snapshot: null,
      shouldReverifyConnection: anyRefineFeatureOn(restoredFlags),
    }
  }

  const nextSnapshot = snapshot ?? (anyRefineFeatureOn(currentFlags) ? currentFlags : null)

  return {
    config: applyRefineFeatureFlags(prev, normalizedNext, {
      enabled: false,
      translateOutput: false,
      translationEnabled: false,
    }),
    snapshot: nextSnapshot,
    shouldReverifyConnection: false,
  }
}

export function normalizeRendererConfig(loadedConfig: AppConfig): AppConfig {
  return {
    ...loadedConfig,
    asr: {
      ...loadedConfig.asr,
      lowVolumeMode: loadedConfig.asr?.lowVolumeMode ?? true,
      microphoneDeviceId: loadedConfig.asr?.microphoneDeviceId ?? '',
      microphoneDeviceLabel: loadedConfig.asr?.microphoneDeviceLabel ?? '',
      streamingEnabled: loadedConfig.asr?.streamingEnabled ?? false,
    },
    llmRefine: normalizeLLMRefineConfig(loadedConfig.llmRefine),
    translation: {
      enabled: loadedConfig.translation?.enabled ?? TRANSLATION.ENABLED,
      targetLanguage: loadedConfig.translation?.targetLanguage || TRANSLATION.TARGET_LANGUAGE,
    },
    mouse: {
      middleClickPtt: loadedConfig.mouse?.middleClickPtt ?? false,
    },
  }
}

/**
 * Apply only the persisted secret placeholders after an autosave. Values edited again while the
 * IPC request was in flight are left untouched, so refreshing the masked state cannot lose input.
 */
export function applyPersistedSecretState(
  currentConfig: AppConfig,
  savedPatch: Partial<AppConfig>,
  persistedConfig: AppConfig,
): AppConfig {
  let llmRefine = normalizeLLMRefineConfig(currentConfig.llmRefine)
  if (savedPatch.llmRefine) {
    const savedLLMRefine = normalizeLLMRefineConfig(savedPatch.llmRefine)
    const persistedLLMRefine = normalizeLLMRefineConfig(persistedConfig.llmRefine)
    const nextLLMRefine = {
      ...llmRefine,
      openai: { ...llmRefine.openai },
      deepseek: { ...llmRefine.deepseek },
      openrouter: { ...llmRefine.openrouter },
      tokendance: { ...llmRefine.tokendance },
      mimo: { ...llmRefine.mimo },
      custom: { ...llmRefine.custom },
    }

    for (const provider of LLM_CONNECTION_KEYS) {
      if (nextLLMRefine[provider].apiKey === savedLLMRefine[provider].apiKey) {
        nextLLMRefine[provider].apiKey = persistedLLMRefine[provider].apiKey
      }
    }
    llmRefine = normalizeLLMRefineConfig(nextLLMRefine)
  }

  // Mirror the masked state of the MiMo ASR key only when the renderer value still matches what
  // was sent, so edits made while the save was in flight are never overwritten.
  let asr = currentConfig.asr
  if (savedPatch.asr) {
    const savedMimoApiKey = savedPatch.asr.mimoApiKey ?? ''
    if ((currentConfig.asr?.mimoApiKey ?? '') === savedMimoApiKey) {
      asr = { ...currentConfig.asr, mimoApiKey: persistedConfig.asr?.mimoApiKey ?? '' }
    }
  }

  return {
    ...currentConfig,
    llmRefine,
    asr,
  }
}

/**
 * Sync the renderer hotkey state with what the main process actually persisted. When
 * `set_config` rolled the accelerators back (`hotkeyRolledBack`, rebind failure) the
 * persisted values replace the rejected ones so `isHotkeyConfigDirty` clears and the
 * invalid value is never re-sent on later autosaves (which would re-trigger the error
 * toast every time). Outside a rollback, persisted values only replace fields the
 * editor still holds from this save, so edits made while the request was in flight are
 * never overwritten.
 */
export function applyPersistedHotkeyState(
  currentConfig: AppConfig,
  savedPatch: Partial<AppConfig>,
  persistedConfig: AppConfig,
  hotkeyRolledBack: boolean,
): AppConfig {
  const savedHotkey = savedPatch.hotkey
  if (!savedHotkey && !hotkeyRolledBack) return currentConfig

  // Rollback without a hotkey patch (defensive): the persisted state wins outright.
  const replaceAll = hotkeyRolledBack && !savedHotkey
  const shouldReplace = (current: string, saved: string | undefined): boolean =>
    replaceAll || (saved !== undefined && current === saved)

  return {
    ...currentConfig,
    hotkey: {
      pttKey: shouldReplace(currentConfig.hotkey.pttKey, savedHotkey?.pttKey)
        ? persistedConfig.hotkey.pttKey
        : currentConfig.hotkey.pttKey,
      toggleSettings: shouldReplace(
        currentConfig.hotkey.toggleSettings,
        savedHotkey?.toggleSettings,
      )
        ? persistedConfig.hotkey.toggleSettings
        : currentConfig.hotkey.toggleSettings,
      translateKey: shouldReplace(currentConfig.hotkey.translateKey, savedHotkey?.translateKey)
        ? persistedConfig.hotkey.translateKey
        : currentConfig.hotkey.translateKey,
    },
  }
}
