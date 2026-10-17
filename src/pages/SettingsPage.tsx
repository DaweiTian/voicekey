import { ReactNode, useCallback, useEffect, useRef, useState } from 'react'
import {
  CheckCircle2,
  XCircle,
  AlertTriangle,
  Eye,
  EyeOff,
  Sparkles,
  SlidersHorizontal,
  Mic,
  ScrollText,
  Activity,
  Info,
  Plug,
  RefreshCw,
  Cloud,
  HardDrive,
  Languages,
  Monitor,
  KeyRound,
  MousePointer,
  Palette,
  Sun,
  Moon,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { type LanguageSetting } from '@/shared/i18n'
import {
  LOG_FILE_MAX_SIZE_MB,
  LOG_RETENTION_DAYS,
  LLM_PROVIDERS,
  MICROPHONE_INPUT,
  STORED_SECRET_PLACEHOLDER,
  TRANSLATION,
  TARGET_LANGUAGES,
} from '@/shared/constants'
import { normalizeRefineBaseUrl } from '@/shared/refine-url'
import {
  CLOUD_ASR_DIALECTS,
  CLOUD_ASR_PRESETS,
  applyCloudAsrPreset,
  getCloudAsrPreset,
} from '@/shared/cloud-asr'
import { storageKindLabelKey, storageSwitchLabelKey } from '@/shared/storage'
import {
  defaultLLMRefineConfig,
  normalizeFillerCleanup,
  normalizeLLMRefineConfig,
  normalizeUserGlossary,
  resolveLLMConnection,
} from '@/shared/llm-config'
import type {
  AppConfig,
  CloudDialect,
  ConfigSecretRequest,
  FillerCleanupMode,
  LLMRefineConfig,
  LLMProvider,
  LinuxIntegrationStatus,
  StorageStatus,
  TranslationConfig,
  UpdateInfo,
} from '@/shared/types'
import { LogViewerDialog } from '@/components/LogViewerDialog'
import { HotkeySettings } from '@/components/HotkeySettings'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import { useTheme } from '@/lib/useTheme'
import { validateHotkey } from '@/lib/hotkey-utils'
import { ChangelogDialog } from '@/components/ChangelogDialog'
import { AccentSelector } from '@/components/AccentSelector'
import {
  applyPersistedHotkeyState,
  applyPersistedSecretState,
  getRefineConnectionFingerprint,
  invalidateRefineConnection,
  isRefineConfigComplete,
  isRefineConnectionCacheFresh,
  markRefineConnectionValidated,
  normalizeRendererConfig,
  reconcileRefineFeaturesAfterConnectionChange,
  resolveMicrophoneDeviceMigration,
  type RefineConnectionValidationCache,
  type RefineFeatureFlags,
} from './settings-config'

const AUTO_SAVE_DELAY_MS = 700
const NO_MICROPHONE_SELECT_VALUE = '__no-microphones__'

type AudioInputDevice = {
  deviceId: string
  label: string
  identityLabel: string
}

const defaultTranslationConfig: TranslationConfig = {
  enabled: TRANSLATION.ENABLED,
  targetLanguage: TRANSLATION.TARGET_LANGUAGE,
}

type TestStatus = {
  type: 'success' | 'error'
  message: string
} | null

type SaveStatus = {
  state: 'saving' | 'success' | 'error' | 'invalid'
  message: string
} | null

function isAppPreferencesDirty(current: AppConfig['app'], original: AppConfig['app']): boolean {
  return (current.autoLaunch ?? false) !== (original.autoLaunch ?? false)
}

function isAsrConfigDirty(current: AppConfig['asr'], original: AppConfig['asr']): boolean {
  return (
    (current.lowVolumeMode ?? true) !== (original.lowVolumeMode ?? true) ||
    (current.microphoneDeviceId ?? '') !== (original.microphoneDeviceId ?? '') ||
    (current.microphoneDeviceLabel ?? '') !== (original.microphoneDeviceLabel ?? '') ||
    (current.cloudPreset ?? 'mimo') !== (original.cloudPreset ?? 'mimo') ||
    (current.cloudDialect ?? 'chat_asr_data_url') !==
      (original.cloudDialect ?? 'chat_asr_data_url') ||
    (current.cloudEndpoint ?? '') !== (original.cloudEndpoint ?? '') ||
    (current.cloudModel ?? '') !== (original.cloudModel ?? '') ||
    // A stored key is masked as the placeholder in both snapshots, so equal placeholders mean the
    // key was left untouched; any other difference (including clearing or a new key) is a change.
    (current.cloudApiKey ?? '') !== (original.cloudApiKey ?? '')
  )
}

function isLlmRefineDirty(current: LLMRefineConfig, original: LLMRefineConfig): boolean {
  return (
    JSON.stringify(normalizeLLMRefineConfig(current)) !==
    JSON.stringify(normalizeLLMRefineConfig(original))
  )
}

function isHotkeyConfigDirty(current: AppConfig['hotkey'], original: AppConfig['hotkey']): boolean {
  return (
    current.pttKey !== original.pttKey ||
    current.toggleSettings !== original.toggleSettings ||
    current.translateKey !== original.translateKey
  )
}

function isTranslationConfigDirty(
  current: TranslationConfig,
  original: TranslationConfig,
): boolean {
  return current.enabled !== original.enabled || current.targetLanguage !== original.targetLanguage
}

function isMouseConfigDirty(current: AppConfig['mouse'], original: AppConfig['mouse']): boolean {
  return (current?.middleClickPtt ?? false) !== (original?.middleClickPtt ?? false)
}

/**
 * A stored API key is shown as a selectable placeholder. If the user inserts text without
 * replacing the whole selection (paste/mid-string caret), the placeholder plus the new text would
 * otherwise be saved as the key. Strip the placeholder characters so only the fresh input remains.
 */
function stripStoredSecretPlaceholder(nextValue: string, previousValue: string): string {
  if (
    previousValue === STORED_SECRET_PLACEHOLDER &&
    nextValue.includes(STORED_SECRET_PLACEHOLDER)
  ) {
    return nextValue.split(STORED_SECRET_PLACEHOLDER).join('')
  }
  return nextValue
}

function mergeConfigPatch(config: AppConfig, patch: Partial<AppConfig>): AppConfig {
  return {
    ...config,
    app: patch.app ? { ...config.app, ...patch.app } : config.app,
    asr: patch.asr ? { ...config.asr, ...patch.asr } : config.asr,
    llmRefine: patch.llmRefine
      ? normalizeLLMRefineConfig({ ...config.llmRefine, ...patch.llmRefine })
      : config.llmRefine,
    hotkey: patch.hotkey ? { ...config.hotkey, ...patch.hotkey } : config.hotkey,
    translation: patch.translation
      ? { ...config.translation, ...patch.translation }
      : config.translation,
    mouse: patch.mouse ? { ...config.mouse, ...patch.mouse } : config.mouse,
  }
}

function InlineFeedback({
  status,
  className = '',
  testId,
}: {
  status: TestStatus | SaveStatus
  className?: string
  testId?: string
}) {
  if (!status) return null

  const isSaveStatus = 'state' in status
  const isSuccess = isSaveStatus ? status.state === 'success' : status.type === 'success'
  const isSaving = isSaveStatus ? status.state === 'saving' : false
  const isError = isSaveStatus
    ? status.state === 'error' || status.state === 'invalid'
    : status.type === 'error'

  return (
    <Alert variant={isError ? 'destructive' : 'default'} className={className} data-testid={testId}>
      {isSaving ? (
        <div className="mt-0.5 h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : isSuccess ? (
        <CheckCircle2 className="h-4 w-4 text-green-600 dark:text-green-500" />
      ) : (
        <XCircle className="h-4 w-4" />
      )}
      <AlertDescription className={isSuccess ? 'text-green-600 dark:text-green-500' : ''}>
        {status.message}
      </AlertDescription>
    </Alert>
  )
}

function SectionCard({
  icon,
  title,
  desc,
  children,
}: {
  icon: ReactNode
  title: string
  desc?: string
  children: ReactNode
}) {
  return (
    <section className="rounded-2xl border bg-card p-5 shadow-sm">
      <div className="mb-5 flex items-center gap-3">
        <span className="grid h-9 w-9 shrink-0 place-items-center rounded-[10px] bg-accent text-accent-foreground">
          {icon}
        </span>
        <div>
          <h2 className="text-[15px] font-semibold text-foreground">{title}</h2>
          {desc ? <p className="mt-0.5 text-xs text-muted-foreground">{desc}</p> : null}
        </div>
      </div>
      {children}
    </section>
  )
}

function ToggleRow({
  title,
  desc,
  checked,
  onChange,
  disabled = false,
}: {
  title: string
  desc?: string
  checked: boolean
  onChange: (value: boolean) => void
  disabled?: boolean
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="text-[13px] font-semibold text-foreground">{title}</div>
        {desc ? (
          <div className="mt-0.5 max-w-[420px] text-xs leading-relaxed text-muted-foreground">
            {desc}
          </div>
        ) : null}
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onChange}
        disabled={disabled}
        className="no-drag cursor-pointer disabled:cursor-not-allowed"
      />
    </div>
  )
}

function Segmented({
  value,
  onChange,
  options,
}: {
  value: string
  onChange: (value: string) => void
  options: { value: string; label: string }[]
}) {
  return (
    <div className="inline-flex rounded-lg border bg-secondary p-0.5">
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          onClick={() => onChange(option.value)}
          className={cn(
            'no-drag cursor-pointer rounded-md px-5 py-1.5 text-xs font-semibold transition-colors',
            value === option.value
              ? 'bg-card text-primary shadow-sm'
              : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}

export default function SettingsPage() {
  const { t } = useTranslation()
  const { theme, setTheme, accent, setAccent } = useTheme()
  const [config, setConfig] = useState<AppConfig>({
    app: {
      language: 'system',
    },
    asr: {
      lowVolumeMode: true,
      microphoneDeviceId: '',
      microphoneDeviceLabel: '',
    },
    llmRefine: defaultLLMRefineConfig,
    hotkey: {
      pttKey: '',
      toggleSettings: '',
      translateKey: '',
    },
    translation: defaultTranslationConfig,
    mouse: { middleClickPtt: false },
  })

  const [originalConfig, setOriginalConfig] = useState<AppConfig | null>(null)
  const [isConfigLoading, setIsConfigLoading] = useState(true)
  const [asrTestStatus, setAsrTestStatus] = useState<TestStatus>(null)
  const [saveStatus, setSaveStatus] = useState<SaveStatus>(null)
  const [visibleSecret, setVisibleSecret] = useState<{ id: string; value?: string } | null>(null)
  const [revealingSecretId, setRevealingSecretId] = useState<string | null>(null)
  const [logDialogOpen, setLogDialogOpen] = useState(false)
  const [changelogOpen, setChangelogOpen] = useState(false)
  const [testingRefine, setTestingRefine] = useState(false)
  const [refineTestStatus, setRefineTestStatus] = useState<TestStatus>(null)
  const [connectingTokenDance, setConnectingTokenDance] = useState(false)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [microphoneDevices, setMicrophoneDevices] = useState<AudioInputDevice[]>([])
  const [loadingMicrophones, setLoadingMicrophones] = useState(false)
  const [microphoneError, setMicrophoneError] = useState<string | null>(null)
  const [linuxIntegrationStatus, setLinuxIntegrationStatus] =
    useState<LinuxIntegrationStatus | null>(null)
  const [linuxIntegrationBusy, setLinuxIntegrationBusy] = useState(false)
  const [linuxIntegrationError, setLinuxIntegrationError] = useState<string | null>(null)
  const hasLoadedConfig = useRef(false)
  const hasLoadedUpdateStatus = useRef(false)
  const latestConfigRef = useRef(config)
  const latestOriginalConfigRef = useRef<AppConfig | null>(null)
  const autoSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const isAutoSavingRef = useRef(false)
  const shouldRunAutoSaveAgainRef = useRef(false)
  const flushAutoSaveRef = useRef<() => Promise<void>>(async () => {})
  const revealRequestIdRef = useRef(0)
  const microphoneLoadRequestIdRef = useRef(0)
  /**
   * Recently validated LLM connections keyed by fingerprint. Switching providers back
   * and forth within the TTL reuses the cached result instead of re-testing.
   */
  const refineConnectionValidationCacheRef = useRef<RefineConnectionValidationCache>(new Map())
  const refineConnectionTestInFlightRef = useRef(false)
  /** Intent preserved when a connection change forces refine-related features off. */
  const refineFeatureFlagsSnapshotRef = useRef<RefineFeatureFlags | null>(null)

  useEffect(() => {
    latestConfigRef.current = config
  }, [config])

  useEffect(() => {
    latestOriginalConfigRef.current = originalConfig
  }, [originalConfig])

  const refreshLinuxIntegrationStatus = useCallback(async () => {
    if (window.electronAPI.platform !== 'linux') return
    try {
      const status = await window.electronAPI.getLinuxIntegrationStatus()
      setLinuxIntegrationStatus(status)
      setLinuxIntegrationError(status.error ?? null)
    } catch (error) {
      setLinuxIntegrationError(error instanceof Error ? error.message : String(error))
    }
  }, [])

  useEffect(() => {
    if (hasLoadedConfig.current) return
    hasLoadedConfig.current = true

    const loadConfig = async () => {
      try {
        const loadedConfig = await window.electronAPI.getConfig()
        const normalizedConfig = normalizeRendererConfig(loadedConfig)
        refineFeatureFlagsSnapshotRef.current = null
        refineConnectionValidationCacheRef.current.clear()
        setConfig(normalizedConfig)
        setOriginalConfig(normalizedConfig)
      } catch (error) {
        console.error('Failed to load config:', error)
      } finally {
        setIsConfigLoading(false)
      }
    }

    loadConfig()
  }, [])

  useEffect(() => {
    void refreshLinuxIntegrationStatus()
  }, [refreshLinuxIntegrationStatus])

  useEffect(() => {
    if (window.electronAPI.platform !== 'linux' || isConfigLoading) return
    const timer = setTimeout(() => {
      void refreshLinuxIntegrationStatus()
    }, AUTO_SAVE_DELAY_MS + 500)
    return () => clearTimeout(timer)
  }, [
    config.hotkey.pttKey,
    config.hotkey.toggleSettings,
    config.hotkey.translateKey,
    config.translation.enabled,
    isConfigLoading,
    refreshLinuxIntegrationStatus,
  ])

  const loadMicrophoneDevices = useCallback(async () => {
    const requestId = ++microphoneLoadRequestIdRef.current
    const isLatestRequest = () => microphoneLoadRequestIdRef.current === requestId

    if (!navigator.mediaDevices?.enumerateDevices) {
      if (isLatestRequest()) {
        setMicrophoneDevices([])
        setMicrophoneError(t('settings.microphone.unsupported'))
        setLoadingMicrophones(false)
      }
      return
    }

    setLoadingMicrophones(true)
    setMicrophoneError(null)

    try {
      let devices = await navigator.mediaDevices.enumerateDevices()
      let rawAudioInputs = devices.filter((device) => device.kind === 'audioinput')
      let audioInputs = rawAudioInputs.filter(
        (device) =>
          device.deviceId && device.deviceId !== 'default' && device.deviceId !== 'communications',
      )

      if (
        rawAudioInputs.length > 0 &&
        (audioInputs.length === 0 || audioInputs.every((device) => !device.label))
      ) {
        try {
          const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
          stream.getTracks().forEach((track) => track.stop())
          devices = await navigator.mediaDevices.enumerateDevices()
          rawAudioInputs = devices.filter((device) => device.kind === 'audioinput')
          audioInputs = rawAudioInputs.filter(
            (device) =>
              device.deviceId &&
              device.deviceId !== 'default' &&
              device.deviceId !== 'communications',
          )
        } catch {
          if (isLatestRequest()) {
            setMicrophoneError(t('settings.microphone.permissionHelp'))
          }
        }
      }

      const seenDeviceIds = new Set<string>()
      const nextDevices = audioInputs.reduce<AudioInputDevice[]>((result, device) => {
        if (seenDeviceIds.has(device.deviceId)) return result
        seenDeviceIds.add(device.deviceId)
        result.push({
          deviceId: device.deviceId,
          identityLabel: device.label,
          label:
            device.label ||
            t('settings.microphone.deviceFallback', {
              index: result.length + 1,
            }),
        })
        return result
      }, [])

      if (isLatestRequest()) {
        setMicrophoneDevices(nextDevices)
      }
    } catch (error) {
      console.error('Failed to enumerate microphone devices:', error)
      if (isLatestRequest()) {
        setMicrophoneDevices([])
        setMicrophoneError(t('settings.microphone.detectFailed'))
      }
    } finally {
      if (isLatestRequest()) {
        setLoadingMicrophones(false)
      }
    }
  }, [t])

  useEffect(() => {
    if (isConfigLoading) return

    const deviceIdentities = microphoneDevices.map((device) => ({
      deviceId: device.deviceId,
      label: device.identityLabel,
    }))
    const migratedDevice = resolveMicrophoneDeviceMigration(
      config.asr.microphoneDeviceId ?? '',
      config.asr.microphoneDeviceLabel ?? '',
      deviceIdentities,
    )
    if (!migratedDevice) return

    setConfig((prev) => {
      const latestMigration = resolveMicrophoneDeviceMigration(
        prev.asr.microphoneDeviceId ?? '',
        prev.asr.microphoneDeviceLabel ?? '',
        deviceIdentities,
      )
      if (latestMigration?.deviceId !== migratedDevice.deviceId) return prev

      return {
        ...prev,
        asr: {
          ...prev.asr,
          microphoneDeviceId: latestMigration.deviceId,
          microphoneDeviceLabel: latestMigration.label,
        },
      }
    })
  }, [
    config.asr.microphoneDeviceId,
    config.asr.microphoneDeviceLabel,
    isConfigLoading,
    microphoneDevices,
  ])

  useEffect(() => {
    void loadMicrophoneDevices()

    const mediaDevices = navigator.mediaDevices
    if (!mediaDevices?.addEventListener) return

    const handleDeviceChange = () => {
      void loadMicrophoneDevices()
    }

    mediaDevices.addEventListener('devicechange', handleDeviceChange)
    return () => {
      mediaDevices.removeEventListener('devicechange', handleDeviceChange)
      microphoneLoadRequestIdRef.current += 1
    }
  }, [loadMicrophoneDevices])

  const clearAutoSaveTimer = () => {
    if (autoSaveTimerRef.current) {
      clearTimeout(autoSaveTimerRef.current)
      autoSaveTimerRef.current = null
    }
  }

  const handleAppLanguageChange = (value: string) => {
    const setting = value as LanguageSetting
    setConfig((prev) => ({
      ...prev,
      app: {
        ...prev.app,
        language: setting,
      },
    }))
    setOriginalConfig((prev) =>
      prev
        ? {
            ...prev,
            app: {
              ...prev.app,
              language: setting,
            },
          }
        : prev,
    )
    setSaveStatus({ state: 'saving', message: t('settings.autoSave.saving') })
    void window.electronAPI
      .setConfig({ app: { language: setting } })
      .then(() => {
        setSaveStatus({ state: 'success', message: t('settings.autoSave.saved') })
      })
      .catch((error) => {
        console.error('Failed to persist app language:', error)
        const errorMessage = error instanceof Error ? error.message : t('common.unknownError')
        setSaveStatus({
          state: 'error',
          message: t('settings.autoSave.error', { message: errorMessage }),
        })
      })
  }

  const getHotkeyErrorMessage = (hotkey: AppConfig['hotkey']): string | null => {
    const pttValidation = validateHotkey(hotkey.pttKey, { allowModifierOnly: true })
    const settingsValidation = validateHotkey(hotkey.toggleSettings, { allowModifierOnly: false })
    const translateValidation = validateHotkey(hotkey.translateKey, { allowModifierOnly: false })

    if (!pttValidation.valid || !settingsValidation.valid || !translateValidation.valid) {
      return t('settings.result.hotkeyInvalid')
    }

    const keys = [hotkey.pttKey, hotkey.toggleSettings, hotkey.translateKey]
    if (new Set(keys).size !== keys.length) {
      return t('settings.result.hotkeyInvalid')
    }

    return null
  }

  const getRefineErrorMessage = (refineConfig: LLMRefineConfig): string | null => {
    if (
      (refineConfig.enabled || refineConfig.translateOutput) &&
      !isRefineConfigComplete(refineConfig)
    ) {
      return t('settings.result.refineConfigRequired')
    }
    return null
  }

  const flushAutoSave = async () => {
    clearAutoSaveTimer()

    if (isAutoSavingRef.current) {
      shouldRunAutoSaveAgainRef.current = true
      return
    }

    const currentConfig = latestConfigRef.current
    const currentOriginalConfig = latestOriginalConfigRef.current

    if (!currentOriginalConfig) return

    const normalizedRefineConfig = normalizeLLMRefineConfig(currentConfig.llmRefine)
    const appDirty = isAppPreferencesDirty(currentConfig.app, currentOriginalConfig.app)
    const asrDirty = isAsrConfigDirty(currentConfig.asr, currentOriginalConfig.asr)
    const refineDirty = isLlmRefineDirty(normalizedRefineConfig, currentOriginalConfig.llmRefine)
    const hotkeyDirty = isHotkeyConfigDirty(currentConfig.hotkey, currentOriginalConfig.hotkey)
    const translationDirty = isTranslationConfigDirty(
      currentConfig.translation,
      currentOriginalConfig.translation,
    )
    const mouseDirty = isMouseConfigDirty(currentConfig.mouse, currentOriginalConfig.mouse)
    const refineError = refineDirty ? getRefineErrorMessage(normalizedRefineConfig) : null
    const hotkeyError = hotkeyDirty ? getHotkeyErrorMessage(currentConfig.hotkey) : null

    const patch: Partial<AppConfig> = {}

    if (appDirty) {
      patch.app = {
        language: currentConfig.app.language,
        autoLaunch: currentConfig.app.autoLaunch ?? false,
      }
    }

    if (asrDirty) {
      // Send the complete ASR object (original fields + edits) so the Rust merge cannot fall back
      // to defaults for fields the renderer does not include, and stored-masked keys survive.
      patch.asr = { ...currentOriginalConfig.asr, ...currentConfig.asr }
    }

    if (refineDirty && !refineError) {
      patch.llmRefine = { ...currentOriginalConfig.llmRefine, ...normalizedRefineConfig }
    }

    if (hotkeyDirty && !hotkeyError) {
      patch.hotkey = currentConfig.hotkey
    }

    if (translationDirty) {
      patch.translation = currentConfig.translation
    }

    if (mouseDirty) {
      patch.mouse = { middleClickPtt: currentConfig.mouse?.middleClickPtt ?? false }
    }

    const invalidMessage = hotkeyError ?? refineError

    if (Object.keys(patch).length === 0) {
      if (invalidMessage) {
        setSaveStatus({ state: 'invalid', message: invalidMessage })
      }
      return
    }

    isAutoSavingRef.current = true
    shouldRunAutoSaveAgainRef.current = false
    setSaveStatus({ state: 'saving', message: t('settings.autoSave.saving') })

    try {
      // set_config 返回持久化（脱敏）后的完整配置；rebind 失败时 hotkey 已在主进程
      // 回滚并通过 hotkeyRolledBack 标记，渲染层必须据此同步，避免脏值反复重发。
      const saveResult = await window.electronAPI.setConfig(patch)
      try {
        // Re-read from the main process so newly entered secrets immediately become placeholders
        // and legacy-encryption status reflects the persisted state.
        const persistedConfig = normalizeRendererConfig(await window.electronAPI.getConfig())
        latestOriginalConfigRef.current = persistedConfig
        setOriginalConfig(persistedConfig)
        setConfig((current) => {
          let next = applyPersistedSecretState(current, patch, persistedConfig)
          next = applyPersistedHotkeyState(
            next,
            patch,
            saveResult.config,
            saveResult.hotkeyRolledBack,
          )
          latestConfigRef.current = next
          return next
        })
      } catch (reloadError) {
        // The save already succeeded. Keep the previous behavior as a fallback and let a later
        // settings load obtain the masked values rather than reporting a false save failure.
        console.error('Failed to refresh masked config after save:', reloadError)
        setOriginalConfig((prev) => {
          if (!prev) return prev
          // 快捷键以 set_config 的持久化结果为准（rebind 失败时是回滚后的值）
          const merged = {
            ...mergeConfigPatch(prev, patch),
            hotkey: { ...saveResult.config.hotkey },
          }
          latestOriginalConfigRef.current = merged
          return merged
        })
        setConfig((current) => {
          const next = applyPersistedHotkeyState(
            current,
            patch,
            saveResult.config,
            saveResult.hotkeyRolledBack,
          )
          latestConfigRef.current = next
          return next
        })
      }

      setVisibleSecret(null)
      setRevealingSecretId(null)
      revealRequestIdRef.current += 1

      if (invalidMessage) {
        setSaveStatus({ state: 'invalid', message: invalidMessage })
      } else {
        setSaveStatus({ state: 'success', message: t('settings.autoSave.saved') })
      }
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : t('common.unknownError')
      setSaveStatus({
        state: 'error',
        message: t('settings.autoSave.error', { message: errorMessage }),
      })
    } finally {
      isAutoSavingRef.current = false
      if (shouldRunAutoSaveAgainRef.current) {
        shouldRunAutoSaveAgainRef.current = false
        void flushAutoSave()
      }
    }
  }

  flushAutoSaveRef.current = flushAutoSave

  useEffect(() => {
    if (isConfigLoading || !originalConfig) return

    const normalizedRefineConfig = normalizeLLMRefineConfig(config.llmRefine)
    const hasPendingChanges =
      isAppPreferencesDirty(config.app, originalConfig.app) ||
      isAsrConfigDirty(config.asr, originalConfig.asr) ||
      isLlmRefineDirty(normalizedRefineConfig, originalConfig.llmRefine) ||
      isHotkeyConfigDirty(config.hotkey, originalConfig.hotkey) ||
      isTranslationConfigDirty(config.translation, originalConfig.translation) ||
      isMouseConfigDirty(config.mouse, originalConfig.mouse)

    if (!hasPendingChanges) return

    clearAutoSaveTimer()
    autoSaveTimerRef.current = setTimeout(() => {
      void flushAutoSaveRef.current()
    }, AUTO_SAVE_DELAY_MS)

    return () => {
      clearAutoSaveTimer()
    }
  }, [config, originalConfig, isConfigLoading])

  const disableRefineDependentFeatures = () => {
    setConfig((prev) => {
      const normalizedRefineConfig = normalizeLLMRefineConfig(prev.llmRefine)
      if (
        !normalizedRefineConfig.enabled &&
        !normalizedRefineConfig.translateOutput &&
        !prev.translation.enabled
      ) {
        return prev
      }

      const next = {
        ...prev,
        llmRefine: {
          ...prev.llmRefine,
          enabled: false,
          translateOutput: false,
        },
        translation: {
          ...prev.translation,
          enabled: false,
        },
      }
      latestConfigRef.current = next
      return next
    })
  }

  const clearRefineFeatureFlagsSnapshot = () => {
    refineFeatureFlagsSnapshotRef.current = null
  }

  const verifyRefineConnection = async (forFeatureEnable: boolean): Promise<boolean> => {
    if (refineConnectionTestInFlightRef.current) return false

    const normalizedRefineConfig = normalizeLLMRefineConfig(latestConfigRef.current.llmRefine)

    if (!isRefineConfigComplete(normalizedRefineConfig)) {
      invalidateRefineConnection(
        refineConnectionValidationCacheRef.current,
        getRefineConnectionFingerprint(normalizedRefineConfig),
      )
      const message = t(
        forFeatureEnable
          ? 'settings.result.refineConfigBeforeEnable'
          : 'settings.result.refineConfigRequired',
      )
      setRefineTestStatus({ type: 'error', message })
      disableRefineDependentFeatures()
      if (forFeatureEnable) {
        toast.warning(message)
      }
      return false
    }

    const connectionFingerprint = getRefineConnectionFingerprint(normalizedRefineConfig)

    if (
      forFeatureEnable &&
      isRefineConnectionCacheFresh(
        refineConnectionValidationCacheRef.current,
        connectionFingerprint,
        Date.now(),
      )
    ) {
      return true
    }

    refineConnectionTestInFlightRef.current = true
    setTestingRefine(true)
    setRefineTestStatus(null)
    try {
      const result = await window.electronAPI.testRefineConnection(normalizedRefineConfig)
      const testedConnection = resolveLLMConnection(normalizedRefineConfig)
      const currentConnection = resolveLLMConnection(
        normalizeLLMRefineConfig(latestConfigRef.current.llmRefine),
      )
      const connectionChanged =
        testedConnection.provider !== currentConnection.provider ||
        testedConnection.endpoint !== currentConnection.endpoint ||
        testedConnection.model !== currentConnection.model ||
        (currentConnection.apiKey !== STORED_SECRET_PLACEHOLDER &&
          testedConnection.apiKey !== currentConnection.apiKey)

      // Record the outcome for the connection that was actually tested, even if the
      // user switched to another connection while the request was in flight.
      if (result.ok) {
        markRefineConnectionValidated(
          refineConnectionValidationCacheRef.current,
          connectionFingerprint,
          Date.now(),
        )
      } else {
        invalidateRefineConnection(
          refineConnectionValidationCacheRef.current,
          connectionFingerprint,
        )
      }

      if (connectionChanged) return false

      if (result.ok) {
        setRefineTestStatus({
          type: 'success',
          message: t('settings.result.refineConnectionSuccess'),
        })
        return true
      } else if (result.message) {
        setRefineTestStatus({
          type: 'error',
          message: t('settings.result.refineTestFailed', { message: result.message }),
        })
      } else {
        setRefineTestStatus({
          type: 'error',
          message: t('settings.result.refineConnectionFailed'),
        })
      }
      disableRefineDependentFeatures()
      if (forFeatureEnable) {
        toast.error(t('settings.result.refineConnectionBeforeEnableFailed'), {
          description: result.message,
        })
      }
      return false
    } catch (error) {
      invalidateRefineConnection(refineConnectionValidationCacheRef.current, connectionFingerprint)
      const errorMessage = error instanceof Error ? error.message : t('common.unknownError')
      setRefineTestStatus({
        type: 'error',
        message: t('settings.result.refineTestFailed', { message: errorMessage }),
      })
      disableRefineDependentFeatures()
      if (forFeatureEnable) {
        toast.error(t('settings.result.refineConnectionBeforeEnableFailed'), {
          description: errorMessage,
        })
      }
      return false
    } finally {
      refineConnectionTestInFlightRef.current = false
      setTestingRefine(false)
    }
  }

  const handleTestRefineConnection = () => {
    void verifyRefineConnection(false)
  }

  const handleRefineEnabledChange = async (checked: boolean) => {
    if (!checked) {
      clearRefineFeatureFlagsSnapshot()
      setConfig((prev) => ({
        ...prev,
        llmRefine: { ...prev.llmRefine, enabled: false },
      }))
      return
    }

    if (!(await verifyRefineConnection(true))) return
    clearRefineFeatureFlagsSnapshot()
    setConfig((prev) => ({
      ...prev,
      llmRefine: { ...prev.llmRefine, enabled: true },
    }))
  }

  const handleTranslationEnabledChange = async (checked: boolean) => {
    if (!checked) {
      clearRefineFeatureFlagsSnapshot()
      setConfig((prev) => ({
        ...prev,
        translation: { ...prev.translation, enabled: false },
      }))
      return
    }

    if (!(await verifyRefineConnection(true))) return
    clearRefineFeatureFlagsSnapshot()
    setConfig((prev) => ({
      ...prev,
      translation: { ...prev.translation, enabled: true },
    }))
  }

  const handleTranslateOutputChange = async (checked: boolean) => {
    if (!checked) {
      clearRefineFeatureFlagsSnapshot()
      setConfig((prev) => ({
        ...prev,
        llmRefine: { ...prev.llmRefine, translateOutput: false },
      }))
      return
    }

    if (!(await verifyRefineConnection(true))) return
    clearRefineFeatureFlagsSnapshot()
    setConfig((prev) => ({
      ...prev,
      llmRefine: { ...prev.llmRefine, translateOutput: true },
    }))
  }

  const [userGlossaryDraft, setUserGlossaryDraft] = useState('')

  useEffect(() => {
    if (isConfigLoading) return
    setUserGlossaryDraft(normalizedLLMRefineConfig.userGlossary.join('\n'))
    // 只在配置加载完成后同步一次草稿，避免打字被归一化回写打断。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isConfigLoading])

  const handleFillerCleanupChange = (value: FillerCleanupMode) => {
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        fillerCleanup: normalizeFillerCleanup(value),
      }),
    }))
  }

  const handleUserGlossaryChange = (raw: string) => {
    setUserGlossaryDraft(raw)
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        userGlossary: normalizeUserGlossary(raw.split('\n')),
      }),
    }))
  }

  const handleMicrophoneChange = (value: string) => {
    if (value === NO_MICROPHONE_SELECT_VALUE) return

    if (value === MICROPHONE_INPUT.SYSTEM_DEFAULT_ID) {
      setConfig((prev) => ({
        ...prev,
        asr: {
          ...prev.asr,
          microphoneDeviceId: '',
          microphoneDeviceLabel: '',
        },
      }))
      return
    }

    const device = microphoneDevices.find((item) => item.deviceId === value)
    setConfig((prev) => ({
      ...prev,
      asr: {
        ...prev.asr,
        microphoneDeviceId: value,
        microphoneDeviceLabel: device?.label ?? prev.asr.microphoneDeviceLabel ?? '',
      },
    }))
  }

  const handleLLMProviderChange = (value: string) => {
    const provider = value as LLMProvider
    setVisibleSecret(null)
    setRevealingSecretId(null)
    revealRequestIdRef.current += 1

    const prev = latestConfigRef.current
    const nextRefine = normalizeLLMRefineConfig({
      ...prev.llmRefine,
      provider,
    })
    const result = reconcileRefineFeaturesAfterConnectionChange(
      prev,
      nextRefine,
      refineFeatureFlagsSnapshotRef.current,
    )
    refineFeatureFlagsSnapshotRef.current = result.snapshot
    latestConfigRef.current = result.config
    setConfig(result.config)

    if (result.shouldReverifyConnection) {
      void verifyRefineConnection(true).then((ok) => {
        if (!ok) {
          clearRefineFeatureFlagsSnapshot()
        }
      })
    }
  }

  const handleOpenAIApiKeyChange = (value: string) => {
    disableRefineDependentFeatures()
    setVisibleSecret((current) =>
      current?.id === 'llm-refine:openai' ? { id: current.id } : current,
    )
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        openai: {
          ...prev.llmRefine.openai,
          apiKey: value,
        },
      }),
    }))
  }

  const handleOpenAIModelChange = (value: string) => {
    if (value !== LLM_PROVIDERS.DEFAULT_OPENAI_MODEL) return

    disableRefineDependentFeatures()
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        openai: {
          ...prev.llmRefine.openai,
          model: value,
        },
      }),
    }))
  }

  const handleDeepSeekApiKeyChange = (value: string) => {
    disableRefineDependentFeatures()
    setVisibleSecret((current) =>
      current?.id === 'llm-refine:deepseek' ? { id: current.id } : current,
    )
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        deepseek: {
          ...prev.llmRefine.deepseek,
          apiKey: value,
        },
      }),
    }))
  }

  const handleDeepSeekModelChange = (value: string) => {
    const model = LLM_PROVIDERS.DEEPSEEK_MODELS.find((option) => option === value)
    if (!model) return

    disableRefineDependentFeatures()
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        deepseek: {
          ...prev.llmRefine.deepseek,
          model,
        },
      }),
    }))
  }

  const handleOpenRouterApiKeyChange = (value: string) => {
    disableRefineDependentFeatures()
    setVisibleSecret((current) =>
      current?.id === 'llm-refine:openrouter' ? { id: current.id } : current,
    )
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        openrouter: {
          ...prev.llmRefine.openrouter,
          apiKey: value,
        },
      }),
    }))
  }

  const handleOpenRouterModelChange = (value: string) => {
    const model = LLM_PROVIDERS.OPENROUTER_MODELS.find((option) => option.id === value)?.id
    if (!model) return

    disableRefineDependentFeatures()
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        openrouter: {
          ...prev.llmRefine.openrouter,
          model,
        },
      }),
    }))
  }

  const handleTokenDanceApiKeyChange = (value: string) => {
    disableRefineDependentFeatures()
    setVisibleSecret((current) =>
      current?.id === 'llm-refine:tokendance' ? { id: current.id } : current,
    )
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        tokendance: {
          ...prev.llmRefine.tokendance,
          apiKey: value,
        },
      }),
    }))
  }

  const handleTokenDanceModelChange = (value: string) => {
    const model = LLM_PROVIDERS.TOKENDANCE_MODELS.find((option) => option.id === value)?.id
    if (!model) return

    disableRefineDependentFeatures()
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        tokendance: {
          ...prev.llmRefine.tokendance,
          model,
        },
      }),
    }))
  }

  const handleTokenDanceConnect = async () => {
    if (connectingTokenDance) return

    setConnectingTokenDance(true)
    setRefineTestStatus(null)
    try {
      // Persist pending edits first so the reload below reflects the latest settings.
      await flushAutoSaveRef.current()

      const result = await window.electronAPI.connectTokenDance()
      if (!result.ok) {
        setRefineTestStatus({
          type: 'error',
          message: t('settings.tokenDanceConnectFailed', {
            message: result.message ?? t('common.unknownError'),
          }),
        })
        return
      }

      // The main process stored the authorized key; reload so it shows as the placeholder.
      const persistedConfig = normalizeRendererConfig(await window.electronAPI.getConfig())
      latestOriginalConfigRef.current = persistedConfig
      setOriginalConfig(persistedConfig)
      setConfig((current) => {
        const next = { ...current, llmRefine: persistedConfig.llmRefine }
        latestConfigRef.current = next
        return next
      })
      setVisibleSecret(null)

      // The stored key changed behind the same masked placeholder, so any cached
      // validation for this connection fingerprint belongs to the previous key.
      invalidateRefineConnection(
        refineConnectionValidationCacheRef.current,
        getRefineConnectionFingerprint(normalizeLLMRefineConfig(persistedConfig.llmRefine)),
      )

      setRefineTestStatus({
        type: 'success',
        message: t('settings.tokenDanceConnectSuccess'),
      })
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : t('common.unknownError')
      setRefineTestStatus({
        type: 'error',
        message: t('settings.tokenDanceConnectFailed', { message: errorMessage }),
      })
    } finally {
      setConnectingTokenDance(false)
    }
  }

  const handleCustomConfigChange = (key: 'endpoint' | 'model' | 'apiKey', value: string) => {
    disableRefineDependentFeatures()
    if (key === 'apiKey') {
      setVisibleSecret((current) =>
        current?.id === 'llm-refine:custom-compatible' ? { id: current.id } : current,
      )
    }
    setConfig((prev) => ({
      ...prev,
      llmRefine: {
        ...prev.llmRefine,
        custom: {
          ...prev.llmRefine.custom,
          [key]: value,
        },
      },
    }))
  }

  const handleMimoConfigChange = (key: 'endpoint' | 'model' | 'apiKey', value: string) => {
    disableRefineDependentFeatures()
    if (key === 'apiKey') {
      setVisibleSecret((current) =>
        current?.id === 'llm-refine:mimo' ? { id: current.id } : current,
      )
    }
    setConfig((prev) => ({
      ...prev,
      llmRefine: {
        ...prev.llmRefine,
        mimo: {
          model: prev.llmRefine.mimo?.model ?? 'mimo-v2.5',
          apiKey: prev.llmRefine.mimo?.apiKey ?? '',
          endpoint: prev.llmRefine.mimo?.endpoint ?? 'https://api.xiaomimimo.com/v1',
          [key]: value,
        },
      },
    }))
  }

  const handleSecretVisibilityToggle = async (
    request: ConfigSecretRequest,
    secretId: string,
    currentValue: string,
  ) => {
    if (visibleSecret?.id === secretId) {
      revealRequestIdRef.current += 1
      setVisibleSecret(null)
      return
    }

    if (currentValue !== STORED_SECRET_PLACEHOLDER) {
      revealRequestIdRef.current += 1
      setVisibleSecret({ id: secretId })
      return
    }

    const requestId = revealRequestIdRef.current + 1
    revealRequestIdRef.current = requestId
    setVisibleSecret(null)
    setRevealingSecretId(secretId)
    try {
      const secret = await window.electronAPI.getConfigSecret(request)
      if (revealRequestIdRef.current !== requestId) return
      setVisibleSecret({ id: secretId, value: secret })
    } catch {
      window.electronAPI.log({
        level: 'error',
        message: 'Failed to reveal saved API key',
        scope: 'settings',
      })
    } finally {
      if (revealRequestIdRef.current === requestId) {
        setRevealingSecretId(null)
      }
    }
  }

  const handleCustomEndpointBlur = () => {
    setConfig((prev) => ({
      ...prev,
      llmRefine: normalizeLLMRefineConfig({
        ...prev.llmRefine,
        custom: {
          ...prev.llmRefine.custom,
          endpoint: normalizeRefineBaseUrl(prev.llmRefine.custom.endpoint),
        },
      }),
    }))
  }

  const handleInstallLinuxIntegration = async () => {
    const hotkeyError = getHotkeyErrorMessage(config.hotkey)
    if (hotkeyError) {
      setLinuxIntegrationError(hotkeyError)
      return
    }

    setLinuxIntegrationBusy(true)
    setLinuxIntegrationError(null)
    try {
      clearAutoSaveTimer()
      const saveResult = await window.electronAPI.setConfig({
        hotkey: config.hotkey,
        translation: config.translation,
      })
      // 快捷键以持久化结果为准（rebind 失败时已回滚），保持 dirty 判定与实际生效值一致
      setOriginalConfig((current) => {
        if (!current) return current
        const next = {
          ...current,
          hotkey: { ...saveResult.config.hotkey },
          translation: config.translation,
        }
        latestOriginalConfigRef.current = next
        return next
      })
      setConfig((current) => {
        const next = applyPersistedHotkeyState(
          current,
          { hotkey: config.hotkey },
          saveResult.config,
          saveResult.hotkeyRolledBack,
        )
        latestConfigRef.current = next
        return next
      })
      const status = await window.electronAPI.installLinuxIntegration()
      setLinuxIntegrationStatus(status)
      setSaveStatus({ state: 'success', message: t('settings.autoSave.saved') })
      toast.success(t('settings.linuxIntegration.installSuccess'))
    } catch (error) {
      setLinuxIntegrationError(error instanceof Error ? error.message : String(error))
    } finally {
      setLinuxIntegrationBusy(false)
    }
  }

  const handleRemoveLinuxIntegration = async () => {
    setLinuxIntegrationBusy(true)
    setLinuxIntegrationError(null)
    try {
      const status = await window.electronAPI.removeLinuxIntegration()
      setLinuxIntegrationStatus(status)
      toast.success(t('settings.linuxIntegration.removeSuccess'))
    } catch (error) {
      setLinuxIntegrationError(error instanceof Error ? error.message : String(error))
    } finally {
      setLinuxIntegrationBusy(false)
    }
  }

  const selectedMicrophoneDeviceId = config.asr.microphoneDeviceId?.trim() ?? ''
  const selectedMicrophoneValue = selectedMicrophoneDeviceId || MICROPHONE_INPUT.SYSTEM_DEFAULT_ID
  const selectedMicrophone = microphoneDevices.find(
    (device) => device.deviceId === selectedMicrophoneDeviceId,
  )
  const selectedMicrophoneUnavailable = Boolean(selectedMicrophoneDeviceId && !selectedMicrophone)
  const selectedMicrophoneLabel =
    config.asr.microphoneDeviceLabel?.trim() || t('settings.microphone.unknownDevice')
  const normalizedLLMRefineConfig = normalizeLLMRefineConfig(config.llmRefine)
  const activeLLMConnection = resolveLLMConnection(normalizedLLMRefineConfig)
  const currentLLMProvider = normalizedLLMRefineConfig.provider
  const refineSecretId = `llm-refine:${currentLLMProvider}`
  const isRefineApiKeyVisible = visibleSecret?.id === refineSecretId
  const displayedRefineApiKey =
    isRefineApiKeyVisible && activeLLMConnection.apiKey === STORED_SECRET_PLACEHOLDER
      ? (visibleSecret.value ?? activeLLMConnection.apiKey)
      : activeLLMConnection.apiKey
  const isCustomLLMProvider = currentLLMProvider === 'custom-compatible'
  const builtInDeepSeekModels: string[] = [...LLM_PROVIDERS.DEEPSEEK_MODELS]
  const llmProviderOptions = [
    { value: 'mimo', label: 'MiMo' },
    { value: 'deepseek', label: 'DeepSeek' },
    { value: 'tokendance', label: 'TokenDance' },
    { value: 'openai', label: 'OpenAI' },
    { value: 'openrouter', label: 'OpenRouter' },
    ...(isCustomLLMProvider
      ? [{ value: 'custom-compatible', label: t('settings.llmProviderCustom') }]
      : []),
  ]
  const llmRefineEnabled = normalizedLLMRefineConfig.enabled
  const translateOutput = normalizedLLMRefineConfig.translateOutput
  const canTestRefine = isRefineConfigComplete(normalizedLLMRefineConfig)
  const hotkeyValidationMessage =
    originalConfig && isHotkeyConfigDirty(config.hotkey, originalConfig.hotkey)
      ? getHotkeyErrorMessage(config.hotkey)
      : null
  const refineValidationMessage =
    originalConfig && isLlmRefineDirty(normalizedLLMRefineConfig, originalConfig.llmRefine)
      ? getRefineErrorMessage(normalizedLLMRefineConfig)
      : null

  useEffect(() => {
    setAsrTestStatus(null)
  }, [config.asr])

  useEffect(() => {
    setRefineTestStatus(null)
  }, [config.llmRefine])

  useEffect(() => {
    if (
      isConfigLoading ||
      canTestRefine ||
      (!llmRefineEnabled && !translateOutput && !config.translation.enabled)
    ) {
      return
    }

    setConfig((prev) => {
      const result = reconcileRefineFeaturesAfterConnectionChange(
        prev,
        prev.llmRefine,
        refineFeatureFlagsSnapshotRef.current,
      )
      refineFeatureFlagsSnapshotRef.current = result.snapshot
      latestConfigRef.current = result.config
      return result.config
    })
  }, [
    canTestRefine,
    config.translation.enabled,
    isConfigLoading,
    llmRefineEnabled,
    translateOutput,
  ])

  useEffect(() => {
    return () => {
      clearAutoSaveTimer()
      if (!isConfigLoading && latestOriginalConfigRef.current) {
        const currentConfig = latestConfigRef.current
        const currentOriginalConfig = latestOriginalConfigRef.current
        const normalizedRefineConfig = normalizeLLMRefineConfig(currentConfig.llmRefine)
        const hasPendingChanges =
          isAppPreferencesDirty(currentConfig.app, currentOriginalConfig.app) ||
          isAsrConfigDirty(currentConfig.asr, currentOriginalConfig.asr) ||
          isLlmRefineDirty(normalizedRefineConfig, currentOriginalConfig.llmRefine) ||
          isHotkeyConfigDirty(currentConfig.hotkey, currentOriginalConfig.hotkey) ||
          isTranslationConfigDirty(currentConfig.translation, currentOriginalConfig.translation)

        if (hasPendingChanges) {
          void flushAutoSaveRef.current()
        }
      }
    }
  }, [isConfigLoading])

  useEffect(() => {
    if (hasLoadedUpdateStatus.current) return
    hasLoadedUpdateStatus.current = true

    const loadUpdateStatus = async () => {
      try {
        const info = await window.electronAPI.getUpdateStatus()
        if (info) {
          setUpdateInfo(info)
        }
      } catch (error) {
        console.error('Failed to load update status:', error)
      }
    }

    loadUpdateStatus()
  }, [])

  const handleCheckUpdate = async () => {
    setCheckingUpdate(true)
    setUpdateInfo(null)
    try {
      const info = await window.electronAPI.checkForUpdates()
      setUpdateInfo(info)
    } catch (error) {
      console.error('Update check failed:', error)
      setUpdateInfo({
        hasUpdate: false,
        latestVersion: '',
        releaseUrl: '',
        releaseNotes: '',
        error: 'failed',
      })
    } finally {
      setCheckingUpdate(false)
    }
  }

  const handleOpenRelease = () => {
    if (updateInfo?.releaseUrl) {
      window.electronAPI.openExternal(updateInfo.releaseUrl)
    }
  }

  // Cloud ASR readiness mirrors the credential fallback chain the Rust side
  // uses (`cloud_asr_credentials`).
  const asrHealthReady = Boolean(
    (config.asr.cloudApiKey ?? '').trim() ||
    config.llmRefine.mimo.apiKey.trim() ||
    config.llmRefine.apiKey.trim(),
  )
  const asrHealthStatus = asrHealthReady
    ? t('settings.health.asrCloudReady')
    : t('settings.health.asrCloudMissing')

  const cloudPreset = config.asr.cloudPreset || 'mimo'
  const cloudDialect = config.asr.cloudDialect || 'chat_asr_data_url'
  const cloudModelPlaceholder = getCloudAsrPreset(cloudPreset).modelPlaceholder
  const cloudDialectHelpKey =
    CLOUD_ASR_DIALECTS.find((entry) => entry.id === cloudDialect)?.helpKey ??
    'settings.cloudDialectDataUrlHelp'

  const translationActive = config.translation.enabled || translateOutput
  const activeTargetLanguage = TARGET_LANGUAGES.find(
    (lang) => lang.value === config.translation.targetLanguage,
  )
  const translationModes = [
    translateOutput ? t('settings.health.dictationTranslation') : null,
    config.translation.enabled ? t('settings.health.selectedTextTranslation') : null,
  ]
    .filter(Boolean)
    .join(' · ')
  const targetLanguageLabel = activeTargetLanguage
    ? t(`settings.translation.languages.${activeTargetLanguage.value}`)
    : config.translation.targetLanguage
  const linuxIntegrationReady = Boolean(
    linuxIntegrationStatus?.available &&
    linuxIntegrationStatus.installed &&
    linuxIntegrationStatus.connected &&
    !linuxIntegrationStatus.needsRepair,
  )
  const linuxBackendReady = linuxIntegrationReady || linuxIntegrationStatus?.session === 'x11'

  // 存储位置（便携模式切换）：挂载时读一次当前根；切换成功后提示重启生效——根路径在
  // Rust 侧一次进程内解析固定（`config::resolved_root`），切换只在下次启动生效。
  const [storageStatus, setStorageStatus] = useState<StorageStatus | null>(null)
  const [storageBusy, setStorageBusy] = useState(false)
  const [storageMessage, setStorageMessage] = useState<{
    type: 'success' | 'error'
    text: string
  } | null>(null)

  useEffect(() => {
    window.electronAPI
      .getStorageStatus()
      .then(setStorageStatus)
      .catch((error) => console.error('Failed to load storage status:', error))
  }, [])

  const handleSwitchStorage = async () => {
    if (!storageStatus || storageBusy) return
    setStorageBusy(true)
    setStorageMessage(null)
    try {
      await window.electronAPI.switchStorageRoot(storageStatus.target)
      setStorageMessage({ type: 'success', text: t('settings.storage.restartNotice') })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setStorageMessage({ type: 'error', text: t('settings.storage.failed', { message }) })
    } finally {
      setStorageBusy(false)
    }
  }

  return (
    <div className="mx-auto max-w-[1040px] px-8 py-7">
      {/* 页头 */}
      <div className="mb-6">
        <span className="inline-flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.12em] text-primary">
          <SlidersHorizontal className="h-3 w-3" />
          {t('settings.eyebrow')}
        </span>
        <h1 className="mt-2 font-display text-[27px] font-bold tracking-tight text-foreground">
          {t('settings.title')}
        </h1>
        <p className="mt-2 text-sm text-muted-foreground">{t('settings.subtitle')}</p>
      </div>

      <div className="grid items-start gap-5 xl:grid-cols-[minmax(0,1fr)_256px]">
        <div className="flex min-w-0 flex-col gap-4">
          {/* 通用 */}
          <SectionCard
            icon={<SlidersHorizontal className="h-[18px] w-[18px]" />}
            title={t('settings.appPreferences')}
            desc={t('settings.descGeneral')}
          >
            <div className="space-y-2">
              <Label htmlFor="appLanguage">{t('settings.appLanguage')}</Label>
              <Select value={config.app.language} onValueChange={handleAppLanguageChange}>
                <SelectTrigger id="appLanguage" className="no-drag w-full cursor-pointer">
                  <SelectValue placeholder={t('settings.languagePlaceholder')} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="system">{t('settings.systemLanguage')}</SelectItem>
                  <SelectItem value="zh">{t('settings.languageChinese')}</SelectItem>
                  <SelectItem value="en">{t('settings.languageEnglish')}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="mt-4">
              <ToggleRow
                title={t('settings.autoLaunch')}
                desc={t('settings.autoLaunchHelp')}
                checked={config.app.autoLaunch ?? false}
                onChange={(checked) =>
                  setConfig((prev) => ({ ...prev, app: { ...prev.app, autoLaunch: checked } }))
                }
              />
            </div>
          </SectionCard>

          {/* 外观 */}
          <SectionCard
            icon={<Palette className="h-[18px] w-[18px]" />}
            title={t('settings.appearance.title')}
            desc={t('settings.appearance.description')}
          >
            <div className="space-y-5">
              <div className="space-y-2">
                <Label>{t('settings.appearance.themeMode')}</Label>
                <div className="no-drag inline-flex items-center gap-0.5 rounded-full border bg-secondary p-0.5">
                  <button
                    type="button"
                    onClick={() => setTheme('light')}
                    title={t('theme.light')}
                    className={cn(
                      'grid h-[26px] w-[30px] cursor-pointer place-items-center rounded-full transition-colors',
                      theme === 'light'
                        ? 'bg-card text-primary shadow-sm'
                        : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    <Sun className="h-[14px] w-[14px]" />
                  </button>
                  <button
                    type="button"
                    onClick={() => setTheme('dark')}
                    title={t('theme.dark')}
                    className={cn(
                      'grid h-[26px] w-[30px] cursor-pointer place-items-center rounded-full transition-colors',
                      theme === 'dark'
                        ? 'bg-card text-primary shadow-sm'
                        : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    <Moon className="h-[14px] w-[14px]" />
                  </button>
                </div>
              </div>
              <div className="space-y-2">
                <Label>{t('settings.appearance.accentColor')}</Label>
                <AccentSelector value={accent} onChange={setAccent} />
              </div>
            </div>
          </SectionCard>

          {/* 语音识别 */}
          <SectionCard
            icon={<Mic className="h-[18px] w-[18px]" />}
            title={t('settings.asrConfig')}
            desc={t('settings.descAsr')}
          >
            <div className="space-y-2">
              <Label htmlFor="microphoneDevice">{t('settings.microphone.label')}</Label>
              <div className="flex items-center gap-2">
                <Select value={selectedMicrophoneValue} onValueChange={handleMicrophoneChange}>
                  <SelectTrigger
                    id="microphoneDevice"
                    className="no-drag min-w-0 flex-1 cursor-pointer"
                  >
                    <SelectValue placeholder={t('settings.microphone.placeholder')} />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={MICROPHONE_INPUT.SYSTEM_DEFAULT_ID}>
                      {t('settings.microphone.systemDefault')}
                    </SelectItem>
                    {microphoneDevices.map((device) => (
                      <SelectItem key={device.deviceId} value={device.deviceId}>
                        {device.label}
                      </SelectItem>
                    ))}
                    {selectedMicrophoneUnavailable ? (
                      <SelectItem value={selectedMicrophoneDeviceId}>
                        {t('settings.microphone.unavailableOption', {
                          label: selectedMicrophoneLabel,
                        })}
                      </SelectItem>
                    ) : null}
                    {microphoneDevices.length === 0 ? (
                      <SelectItem value={NO_MICROPHONE_SELECT_VALUE} disabled>
                        {t('settings.microphone.noDevices')}
                      </SelectItem>
                    ) : null}
                  </SelectContent>
                </Select>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => void loadMicrophoneDevices()}
                  disabled={loadingMicrophones}
                  className="no-drag shrink-0 cursor-pointer"
                >
                  <RefreshCw className={cn('h-4 w-4', loadingMicrophones && 'animate-spin')} />
                  {loadingMicrophones
                    ? t('settings.microphone.detecting')
                    : t('settings.microphone.refresh')}
                </Button>
              </div>
              {microphoneError ? (
                <p className="text-xs leading-relaxed text-destructive">{microphoneError}</p>
              ) : (
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {t('settings.microphone.help')}
                </p>
              )}
              {selectedMicrophoneUnavailable ? (
                <div className="flex items-start gap-2 rounded-lg border border-yellow-500/30 bg-yellow-500/10 px-3 py-2.5 text-xs leading-relaxed text-muted-foreground">
                  <AlertTriangle className="mt-px h-4 w-4 shrink-0 text-yellow-600 dark:text-yellow-500" />
                  {t('settings.microphone.unavailableHelp')}
                </div>
              ) : null}
            </div>

            <div className="mt-4 space-y-3">
              <div className="space-y-4 rounded-lg border border-border/60 p-3">
                <p className="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                  <Cloud className="h-3.5 w-3.5" />
                  {t('settings.cloudAsrTitle')}
                </p>
                <div className="space-y-2">
                  <Label htmlFor="cloudPreset">{t('settings.cloudPresetLabel')}</Label>
                  <Select
                    value={cloudPreset}
                    onValueChange={(presetId) =>
                      setConfig((prev) => ({
                        ...prev,
                        asr: applyCloudAsrPreset(prev.asr, presetId),
                      }))
                    }
                  >
                    <SelectTrigger id="cloudPreset" className="no-drag cursor-pointer">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {CLOUD_ASR_PRESETS.map((preset) => (
                        <SelectItem key={preset.id} value={preset.id}>
                          {t(preset.labelKey)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">{t('settings.cloudPresetHelp')}</p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="asrCloudEndpoint">
                    {t('settings.cloudEndpointLabel')} <span className="text-primary">*</span>
                  </Label>
                  <Input
                    id="asrCloudEndpoint"
                    type="text"
                    value={config.asr.cloudEndpoint ?? ''}
                    onChange={(e) =>
                      setConfig((prev) => ({
                        ...prev,
                        asr: { ...prev.asr, cloudEndpoint: e.target.value },
                      }))
                    }
                    placeholder="https://api.xiaomimimo.com/v1"
                    className="no-drag font-mono"
                  />
                  <p className="text-xs text-muted-foreground">{t('settings.cloudEndpointHelp')}</p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="asrCloudModel">{t('settings.cloudModelLabel')}</Label>
                  <Input
                    id="asrCloudModel"
                    type="text"
                    value={config.asr.cloudModel ?? ''}
                    onChange={(e) =>
                      setConfig((prev) => ({
                        ...prev,
                        asr: { ...prev.asr, cloudModel: e.target.value },
                      }))
                    }
                    placeholder={cloudModelPlaceholder}
                    className="no-drag font-mono"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="asrCloudApiKey">
                    {t('settings.cloudApiKeyLabel')} <span className="text-primary">*</span>
                  </Label>
                  <div className="relative">
                    <Input
                      id="asrCloudApiKey"
                      type={visibleSecret?.id === 'asr:mimo' ? 'text' : 'password'}
                      value={
                        visibleSecret?.id === 'asr:mimo'
                          ? (visibleSecret.value ?? config.asr.cloudApiKey ?? '')
                          : (config.asr.cloudApiKey ?? '')
                      }
                      onChange={(e) => {
                        const nextValue = stripStoredSecretPlaceholder(
                          e.target.value,
                          config.asr.cloudApiKey ?? '',
                        )
                        setVisibleSecret((current) =>
                          current?.id === 'asr:mimo' ? { id: current.id } : current,
                        )
                        setConfig((prev) => ({
                          ...prev,
                          asr: { ...prev.asr, cloudApiKey: nextValue },
                        }))
                      }}
                      placeholder={t('settings.cloudApiKeyPlaceholder')}
                      className="no-drag pr-10 font-mono"
                    />
                    <button
                      type="button"
                      onClick={() =>
                        void handleSecretVisibilityToggle(
                          { scope: 'asr', provider: 'mimo' },
                          'asr:mimo',
                          config.asr.cloudApiKey ?? '',
                        )
                      }
                      disabled={revealingSecretId === 'asr:mimo'}
                      aria-label={t('settings.showRefineKey')}
                      className="no-drag absolute inset-y-0 right-0 flex items-center pr-3 text-muted-foreground hover:text-foreground"
                    >
                      {visibleSecret?.id === 'asr:mimo' ? (
                        <EyeOff className="h-4 w-4" />
                      ) : (
                        <Eye className="h-4 w-4" />
                      )}
                    </button>
                  </div>
                  <p className="text-xs text-muted-foreground">{t('settings.cloudApiKeyHelp')}</p>
                </div>
                {cloudPreset === 'custom' ? (
                  <div className="space-y-2">
                    <Label htmlFor="cloudDialect">{t('settings.cloudDialectLabel')}</Label>
                    <Select
                      value={cloudDialect}
                      onValueChange={(dialect) =>
                        setConfig((prev) => ({
                          ...prev,
                          asr: { ...prev.asr, cloudDialect: dialect as CloudDialect },
                        }))
                      }
                    >
                      <SelectTrigger id="cloudDialect" className="no-drag cursor-pointer">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {CLOUD_ASR_DIALECTS.map((dialect) => (
                          <SelectItem key={dialect.id} value={dialect.id}>
                            {t(dialect.labelKey)}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <p className="text-xs text-muted-foreground">{t(cloudDialectHelpKey)}</p>
                  </div>
                ) : null}
              </div>
            </div>

            <InlineFeedback status={asrTestStatus} testId="asr-test-status" />

            <div className="mt-4 flex items-start gap-2 rounded-lg border border-yellow-500/30 bg-yellow-500/10 px-3 py-2.5 text-xs leading-relaxed text-muted-foreground">
              <AlertTriangle className="mt-px h-4 w-4 shrink-0 text-yellow-600 dark:text-yellow-500" />
              {t('settings.durationWarning')}
            </div>

            <div className="mt-4 border-t pt-4">
              <ToggleRow
                title={t('settings.lowVolumeMode')}
                desc={t('settings.lowVolumeModeHelp')}
                checked={config.asr.lowVolumeMode ?? true}
                onChange={(checked) =>
                  setConfig((prev) => ({ ...prev, asr: { ...prev.asr, lowVolumeMode: checked } }))
                }
              />
            </div>
          </SectionCard>

          {/* 润色与翻译 */}
          <SectionCard
            icon={<Sparkles className="h-[18px] w-[18px]" />}
            title={t('settings.refineAndTranslation')}
            desc={t('settings.descRefine')}
          >
            {/* 共享 LLM 连接 */}
            <p className="mb-4 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
              <Plug className="h-3.5 w-3.5" />
              {t('settings.llmConnectionHelp')}
            </p>

            <div className="space-y-2">
              <Label>{t('settings.llmProvider')}</Label>
              <Segmented
                value={currentLLMProvider}
                onChange={handleLLMProviderChange}
                options={llmProviderOptions}
              />
              <p className="text-xs text-muted-foreground">
                {t('settings.llmProviderHelp', { endpoint: activeLLMConnection.endpoint })}
              </p>
            </div>

            {currentLLMProvider === 'mimo' && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="mimoEndpoint">
                    {t('settings.refineEndpoint')} <span className="text-primary">*</span>
                  </Label>
                  <Input
                    id="mimoEndpoint"
                    type="text"
                    value={
                      normalizedLLMRefineConfig.mimo?.endpoint ?? 'https://api.xiaomimimo.com/v1'
                    }
                    onChange={(e) => handleMimoConfigChange('endpoint', e.target.value)}
                    placeholder="https://api.xiaomimimo.com/v1"
                    className="no-drag font-mono"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="mimoModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Input
                    id="mimoModel"
                    value={normalizedLLMRefineConfig.mimo?.model ?? 'mimo-v2.5'}
                    onChange={(e) => handleMimoConfigChange('model', e.target.value)}
                    placeholder="mimo-v2.5"
                  />
                  <p className="text-xs text-muted-foreground">
                    {t('settings.mimoModelHelp', {
                      url: 'https://mimo.mi.com/docs/zh-CN/quick-start/summary/model',
                    })}
                  </p>
                </div>
              </div>
            )}

            {currentLLMProvider === 'openai' && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="openAIModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Select
                    value={normalizedLLMRefineConfig.openai.model}
                    onValueChange={handleOpenAIModelChange}
                  >
                    <SelectTrigger id="openAIModel" className="no-drag w-full cursor-pointer">
                      <SelectValue placeholder={LLM_PROVIDERS.DEFAULT_OPENAI_MODEL} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={LLM_PROVIDERS.DEFAULT_OPENAI_MODEL}>
                        {LLM_PROVIDERS.DEFAULT_OPENAI_MODEL}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
            )}

            {currentLLMProvider === 'deepseek' && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="deepSeekModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Select
                    value={normalizedLLMRefineConfig.deepseek.model}
                    onValueChange={handleDeepSeekModelChange}
                  >
                    <SelectTrigger id="deepSeekModel" className="no-drag w-full cursor-pointer">
                      <SelectValue placeholder="DeepSeek · V4.1 Flash" />
                    </SelectTrigger>
                    <SelectContent>
                      {builtInDeepSeekModels.map((model) => (
                        <SelectItem key={model} value={model}>
                          DeepSeek · V4.1 Flash
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>
            )}

            {currentLLMProvider === 'openrouter' && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="openRouterModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Select
                    value={normalizedLLMRefineConfig.openrouter.model}
                    onValueChange={handleOpenRouterModelChange}
                  >
                    <SelectTrigger id="openRouterModel" className="no-drag w-full cursor-pointer">
                      <SelectValue placeholder={LLM_PROVIDERS.DEFAULT_OPENROUTER_MODEL} />
                    </SelectTrigger>
                    <SelectContent>
                      {LLM_PROVIDERS.OPENROUTER_MODELS.map((model) => (
                        <SelectItem key={model.id} value={model.id}>
                          {model.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>
            )}

            {currentLLMProvider === 'tokendance' && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="tokenDanceModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Select
                    value={normalizedLLMRefineConfig.tokendance.model}
                    onValueChange={handleTokenDanceModelChange}
                  >
                    <SelectTrigger id="tokenDanceModel" className="no-drag w-full cursor-pointer">
                      <SelectValue placeholder={LLM_PROVIDERS.DEFAULT_TOKENDANCE_MODEL} />
                    </SelectTrigger>
                    <SelectContent>
                      {LLM_PROVIDERS.TOKENDANCE_MODELS.map((model) => (
                        <SelectItem key={model.id} value={model.id}>
                          {model.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>

                <div className="space-y-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void handleTokenDanceConnect()}
                    disabled={connectingTokenDance}
                    className="no-drag cursor-pointer"
                  >
                    <KeyRound className="h-4 w-4" />
                    {connectingTokenDance
                      ? t('settings.tokenDanceConnecting')
                      : t('settings.tokenDanceConnect')}
                  </Button>
                  <p className="text-xs text-muted-foreground">
                    {t('settings.tokenDanceConnectHelp')}
                  </p>
                </div>
              </div>
            )}

            {isCustomLLMProvider && (
              <div className="mt-4 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="customRefineEndpoint">
                    {t('settings.refineEndpoint')} <span className="text-primary">*</span>
                  </Label>
                  <Input
                    id="customRefineEndpoint"
                    type="text"
                    value={config.llmRefine.custom.endpoint}
                    onChange={(e) => handleCustomConfigChange('endpoint', e.target.value)}
                    onBlur={handleCustomEndpointBlur}
                    placeholder={t('settings.refineEndpointPlaceholder')}
                    className="no-drag font-mono"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="customRefineModel">
                    {t('settings.refineModel')} <span className="text-primary">*</span>
                  </Label>
                  <Input
                    id="customRefineModel"
                    type="text"
                    value={config.llmRefine.custom.model}
                    onChange={(e) => handleCustomConfigChange('model', e.target.value)}
                    placeholder={t('settings.refineModelPlaceholder')}
                    className="no-drag font-mono"
                  />
                </div>
              </div>
            )}

            <div className="mt-4 space-y-2">
              <Label htmlFor="refineApiKey">
                {t('settings.refineApiKey')} <span className="text-primary">*</span>
              </Label>
              <div className="relative">
                <Input
                  id="refineApiKey"
                  type={isRefineApiKeyVisible ? 'text' : 'password'}
                  value={displayedRefineApiKey}
                  onChange={(e) => {
                    const nextValue = stripStoredSecretPlaceholder(
                      e.target.value,
                      displayedRefineApiKey,
                    )
                    if (currentLLMProvider === 'openai') {
                      handleOpenAIApiKeyChange(nextValue)
                    } else if (currentLLMProvider === 'deepseek') {
                      handleDeepSeekApiKeyChange(nextValue)
                    } else if (currentLLMProvider === 'openrouter') {
                      handleOpenRouterApiKeyChange(nextValue)
                    } else if (currentLLMProvider === 'tokendance') {
                      handleTokenDanceApiKeyChange(nextValue)
                    } else if (currentLLMProvider === 'mimo') {
                      handleMimoConfigChange('apiKey', nextValue)
                    } else {
                      handleCustomConfigChange('apiKey', nextValue)
                    }
                  }}
                  onFocus={(event) => {
                    if (activeLLMConnection.apiKey === STORED_SECRET_PLACEHOLDER) {
                      event.currentTarget.select()
                    }
                  }}
                  placeholder={t(
                    currentLLMProvider === 'openai'
                      ? 'settings.openAIApiKeyPlaceholder'
                      : currentLLMProvider === 'openrouter'
                        ? 'settings.openRouterApiKeyPlaceholder'
                        : currentLLMProvider === 'tokendance'
                          ? 'settings.tokenDanceApiKeyPlaceholder'
                          : currentLLMProvider === 'deepseek'
                            ? 'settings.deepSeekApiKeyPlaceholder'
                            : 'settings.refineApiKeyPlaceholder',
                  )}
                  className="no-drag pr-10 font-mono"
                />
                <button
                  type="button"
                  onClick={() =>
                    void handleSecretVisibilityToggle(
                      { scope: 'llm-refine', provider: currentLLMProvider },
                      refineSecretId,
                      activeLLMConnection.apiKey,
                    )
                  }
                  disabled={revealingSecretId === refineSecretId}
                  aria-label={
                    isRefineApiKeyVisible
                      ? t('settings.hideRefineKey')
                      : t('settings.showRefineKey')
                  }
                  className="no-drag absolute inset-y-0 right-0 flex items-center pr-3 text-muted-foreground hover:text-foreground"
                >
                  {isRefineApiKeyVisible ? (
                    <EyeOff className="h-4 w-4" />
                  ) : (
                    <Eye className="h-4 w-4" />
                  )}
                </button>
              </div>
            </div>

            {refineValidationMessage && (
              <Alert variant="destructive" className="mt-4" data-testid="refine-validation-status">
                <AlertTriangle className="h-4 w-4" />
                <AlertDescription>{refineValidationMessage}</AlertDescription>
              </Alert>
            )}

            <div className="mt-4 space-y-3">
              <Button
                variant="outline"
                size="sm"
                onClick={handleTestRefineConnection}
                disabled={testingRefine || !canTestRefine}
                className="no-drag cursor-pointer"
              >
                <Plug className="h-4 w-4" />
                {testingRefine
                  ? t('settings.testingRefineConnection')
                  : t('settings.testRefineConnection')}
              </Button>
              <InlineFeedback status={refineTestStatus} testId="refine-test-status" />
            </div>

            {/* 文本润色 */}
            <div className="mt-5 border-t pt-5">
              <p className="mb-4 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                <Sparkles className="h-3.5 w-3.5" />
                {t('settings.llmRefineConfig')}
              </p>
              <ToggleRow
                title={t('settings.llmRefineEnabled')}
                desc={t('settings.llmRefineEnabledHelp')}
                checked={llmRefineEnabled}
                disabled={testingRefine}
                onChange={(checked) => void handleRefineEnabledChange(checked)}
              />

              <div className="mt-4 space-y-2">
                <Label htmlFor="fillerCleanup">{t('settings.fillerCleanup')}</Label>
                <Select
                  value={normalizedLLMRefineConfig.fillerCleanup}
                  onValueChange={(value) => handleFillerCleanupChange(value as FillerCleanupMode)}
                >
                  <SelectTrigger id="fillerCleanup" className="no-drag w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="off">{t('settings.fillerCleanupOff')}</SelectItem>
                    <SelectItem value="rules">{t('settings.fillerCleanupRules')}</SelectItem>
                    <SelectItem value="llm">{t('settings.fillerCleanupLlm')}</SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">{t('settings.fillerCleanupHelp')}</p>
                {normalizedLLMRefineConfig.fillerCleanup === 'llm' && !llmRefineEnabled && (
                  <p className="text-xs text-muted-foreground">
                    {t('settings.fillerCleanupLlmHint')}
                  </p>
                )}
              </div>

              <div className="mt-4 space-y-2">
                <Label htmlFor="userGlossary">{t('settings.userGlossary')}</Label>
                <Textarea
                  id="userGlossary"
                  className="no-drag min-h-24 font-mono text-xs"
                  placeholder={t('settings.userGlossaryPlaceholder')}
                  value={userGlossaryDraft}
                  onChange={(event) => handleUserGlossaryChange(event.target.value)}
                />
                <p className="text-xs text-muted-foreground">{t('settings.userGlossaryHelp')}</p>
              </div>
            </div>

            {/* 翻译 */}
            <div className="mt-5 border-t pt-5">
              <p className="mb-4 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                <Languages className="h-3.5 w-3.5" />
                {t('settings.translation.title')}
              </p>

              <div className="space-y-2">
                <Label htmlFor="targetLanguage">{t('settings.translation.targetLanguage')}</Label>
                <Select
                  value={config.translation.targetLanguage}
                  onValueChange={(value) =>
                    setConfig((prev) => ({
                      ...prev,
                      translation: { ...prev.translation, targetLanguage: value },
                    }))
                  }
                >
                  <SelectTrigger id="targetLanguage" className="no-drag w-full cursor-pointer">
                    <SelectValue placeholder="English" />
                  </SelectTrigger>
                  <SelectContent>
                    {TARGET_LANGUAGES.map((lang) => (
                      <SelectItem key={lang.value} value={lang.value}>
                        {t(`settings.translation.languages.${lang.value}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="mt-4">
                <ToggleRow
                  title={t('settings.translateOutput')}
                  desc={t('settings.translateOutputHelp')}
                  checked={translateOutput}
                  disabled={testingRefine}
                  onChange={(checked) => void handleTranslateOutputChange(checked)}
                />
              </div>

              <div className="mt-4">
                <ToggleRow
                  title={t('settings.translation.enable')}
                  desc={t('settings.translation.enableHelp')}
                  checked={config.translation.enabled}
                  disabled={testingRefine}
                  onChange={(checked) => void handleTranslationEnabledChange(checked)}
                />
              </div>
            </div>
          </SectionCard>

          {/* 快捷键 */}
          <div className="space-y-3">
            <HotkeySettings
              value={config.hotkey}
              originalValue={originalConfig?.hotkey ?? null}
              isLoading={isConfigLoading}
              onChange={(hotkey) => setConfig((prev) => ({ ...prev, hotkey }))}
            />
            {hotkeyValidationMessage && (
              <Alert variant="destructive" data-testid="hotkey-validation-status">
                <AlertTriangle className="h-4 w-4" />
                <AlertDescription>{hotkeyValidationMessage}</AlertDescription>
              </Alert>
            )}
          </div>

          {/* 鼠标触发（仅 Windows：中键 PTT 依赖 WH_MOUSE_LL 钩子） */}
          {window.electronAPI.platform === 'win32' && (
            <SectionCard
              icon={<MousePointer className="h-[18px] w-[18px]" />}
              title={t('settings.mouse.title')}
              desc={t('settings.mouse.description')}
            >
              <ToggleRow
                title={t('settings.mouse.middleClickPtt')}
                desc={t('settings.mouse.middleClickPttHelp')}
                checked={config.mouse?.middleClickPtt ?? false}
                disabled={isConfigLoading}
                onChange={(checked) =>
                  setConfig((prev) => ({
                    ...prev,
                    mouse: { ...prev.mouse, middleClickPtt: checked },
                  }))
                }
              />
            </SectionCard>
          )}

          {window.electronAPI.platform === 'linux' && (
            <SectionCard
              icon={<Monitor className="h-[18px] w-[18px]" />}
              title={t('settings.linuxIntegration.title')}
              desc={t('settings.linuxIntegration.description')}
            >
              <div className="space-y-4">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <p className="text-sm font-semibold text-foreground">
                      {linuxIntegrationStatus?.session === 'x11'
                        ? t('settings.linuxIntegration.x11Ready')
                        : linuxIntegrationReady
                          ? t('settings.linuxIntegration.ready')
                          : linuxIntegrationStatus?.installed
                            ? t('settings.linuxIntegration.installedNeedsRepair')
                            : t('settings.linuxIntegration.notInstalled')}
                    </p>
                    <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                      {t('settings.linuxIntegration.session', {
                        session: linuxIntegrationStatus
                          ? t(
                              `settings.linuxIntegration.sessions.${linuxIntegrationStatus.session}`,
                            )
                          : '—',
                      })}
                    </p>
                  </div>
                  <span
                    className={cn(
                      'shrink-0 rounded-full px-2.5 py-1 text-[11px] font-semibold',
                      linuxBackendReady
                        ? 'bg-green-500/10 text-green-600 dark:text-green-500'
                        : 'bg-amber-500/10 text-amber-700 dark:text-amber-400',
                    )}
                  >
                    {linuxBackendReady
                      ? t('settings.linuxIntegration.connected')
                      : t('settings.linuxIntegration.actionRequired')}
                  </span>
                </div>

                {linuxIntegrationStatus && !linuxIntegrationStatus.available && (
                  <Alert>
                    <Info className="h-4 w-4" />
                    <AlertDescription>
                      {linuxIntegrationStatus.session === 'x11'
                        ? t('settings.linuxIntegration.x11Fallback')
                        : t('settings.linuxIntegration.waylandUnsupported')}
                    </AlertDescription>
                  </Alert>
                )}

                {linuxIntegrationStatus?.conflicts.length ? (
                  <Alert variant="destructive">
                    <AlertTriangle className="h-4 w-4" />
                    <AlertDescription>
                      {t('settings.linuxIntegration.conflicts', {
                        conflicts: linuxIntegrationStatus.conflicts.join(', '),
                      })}
                    </AlertDescription>
                  </Alert>
                ) : null}

                {linuxIntegrationError && (
                  <Alert variant="destructive">
                    <AlertTriangle className="h-4 w-4" />
                    <AlertDescription>{linuxIntegrationError}</AlertDescription>
                  </Alert>
                )}

                {linuxIntegrationStatus?.configPath && (
                  <div className="rounded-lg border bg-secondary/50 p-3">
                    <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                      {t('settings.linuxIntegration.configPath')}
                    </p>
                    <p className="mt-1 break-all font-mono text-xs text-foreground">
                      {linuxIntegrationStatus.configPath}
                    </p>
                    <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                      {t('settings.linuxIntegration.backupHelp')}
                    </p>
                  </div>
                )}

                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void handleInstallLinuxIntegration()}
                    disabled={
                      linuxIntegrationBusy ||
                      saveStatus?.state === 'saving' ||
                      !linuxIntegrationStatus?.available ||
                      Boolean(linuxIntegrationStatus?.error) ||
                      Boolean(linuxIntegrationStatus?.conflicts.length)
                    }
                    className="no-drag cursor-pointer"
                  >
                    {linuxIntegrationBusy
                      ? t('settings.linuxIntegration.working')
                      : linuxIntegrationStatus?.installed
                        ? t('settings.linuxIntegration.repair')
                        : t('settings.linuxIntegration.install')}
                  </Button>
                  {linuxIntegrationStatus?.installed && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => void handleRemoveLinuxIntegration()}
                      disabled={linuxIntegrationBusy || !linuxIntegrationStatus.available}
                      className="no-drag cursor-pointer text-muted-foreground"
                    >
                      {t('settings.linuxIntegration.remove')}
                    </Button>
                  )}
                </div>
              </div>
            </SectionCard>
          )}

          {/* 诊断 */}
          <SectionCard
            icon={<ScrollText className="h-[18px] w-[18px]" />}
            title={t('settings.troubleshooting')}
            desc={t('settings.descLogs')}
          >
            <div className="flex items-center justify-between gap-4">
              <p className="max-w-[420px] text-xs leading-relaxed text-muted-foreground">
                {t('settings.logsDescription', {
                  days: LOG_RETENTION_DAYS,
                  size: LOG_FILE_MAX_SIZE_MB,
                })}
              </p>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setLogDialogOpen(true)}
                className="no-drag shrink-0 cursor-pointer"
              >
                <ScrollText className="h-4 w-4" />
                {t('settings.viewLogs')}
              </Button>
            </div>
          </SectionCard>

          {/* 存储位置 */}
          <SectionCard
            icon={<HardDrive className="h-[18px] w-[18px]" />}
            title={t('settings.storage.title')}
            desc={t('settings.storage.desc')}
          >
            <div className="space-y-3">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <p className="text-xs text-muted-foreground">{t('settings.storage.current')}</p>
                  <p className="mt-0.5 break-all font-mono text-xs text-foreground">
                    {storageStatus?.root ?? '…'}
                  </p>
                  {storageStatus ? (
                    <p className="mt-1 text-[11px] text-muted-foreground">
                      {t(storageKindLabelKey(storageStatus.kind))}
                    </p>
                  ) : null}
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleSwitchStorage()}
                  disabled={!storageStatus || storageBusy}
                  className="no-drag shrink-0 cursor-pointer"
                >
                  {storageBusy ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <HardDrive className="h-4 w-4" />
                  )}
                  {storageBusy
                    ? t('settings.storage.pending')
                    : t(storageSwitchLabelKey(storageStatus?.target ?? 'portable'))}
                </Button>
              </div>
              {storageMessage ? (
                <p
                  className={cn(
                    'text-xs leading-relaxed',
                    storageMessage.type === 'error'
                      ? 'text-destructive'
                      : 'text-green-600 dark:text-green-500',
                  )}
                >
                  {storageMessage.text}
                </p>
              ) : null}
            </div>
          </SectionCard>

          <LogViewerDialog open={logDialogOpen} onOpenChange={setLogDialogOpen} />
          <ChangelogDialog open={changelogOpen} onOpenChange={setChangelogOpen} />
        </div>

        {/* 右侧栏 */}
        <aside className="hidden xl:block">
          <div className="sticky top-0 flex flex-col gap-4">
            {/* 自动保存状态 */}
            <div
              className={cn(
                'flex items-center gap-2.5 rounded-xl border px-4 py-3 text-[13px] font-semibold shadow-sm',
                saveStatus?.state === 'error' || saveStatus?.state === 'invalid'
                  ? 'border-destructive/30 bg-destructive/10 text-destructive'
                  : saveStatus?.state === 'saving'
                    ? 'bg-secondary text-muted-foreground'
                    : 'border-green-500/30 bg-green-500/10 text-green-600 dark:text-green-500',
              )}
              data-testid="save-status-card"
            >
              {saveStatus?.state === 'saving' ? (
                <>
                  <span className="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                  {t('settings.autoSave.saving')}
                </>
              ) : saveStatus?.state === 'error' || saveStatus?.state === 'invalid' ? (
                <>
                  <XCircle className="h-[17px] w-[17px]" />
                  {saveStatus.message}
                </>
              ) : (
                <>
                  <CheckCircle2 className="h-[17px] w-[17px]" />
                  {t('settings.autoSave.saved')}
                </>
              )}
            </div>

            {/* 连接状态 */}
            <div className="rounded-xl border bg-card p-4 shadow-sm">
              <p className="mb-1 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                <Activity className="h-3.5 w-3.5" />
                {t('settings.connectionTitle')}
              </p>
              <HealthItem
                on={asrHealthReady}
                title={t('settings.health.asr')}
                status={asrHealthStatus}
              />
              <HealthItem
                on={canTestRefine}
                title={t('settings.health.refine')}
                status={
                  canTestRefine ? t('settings.health.refineOn') : t('settings.health.refineOff')
                }
              />
              <HealthItem
                on={translationActive}
                title={t('settings.health.translation')}
                status={
                  translationActive
                    ? `${translationModes} → ${targetLanguageLabel}`
                    : t('settings.health.translationOff')
                }
              />
            </div>

            {/* 关于 */}
            <div className="rounded-xl border bg-card p-4 shadow-sm">
              <p className="mb-2.5 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                <Info className="h-3.5 w-3.5" />
                {t('settings.about')}
              </p>
              <div className="text-[13px] font-semibold text-foreground">
                {t('settings.version', { version: __APP_VERSION__ })}
              </div>
              {updateInfo?.hasUpdate ? (
                <p className="mt-1 text-xs font-medium text-green-600 dark:text-green-500">
                  {t('settings.hasUpdate', { version: updateInfo.latestVersion })}
                </p>
              ) : updateInfo?.hasUpdate === false && !updateInfo.error ? (
                <p className="mt-1 text-xs text-muted-foreground">{t('settings.noUpdate')}</p>
              ) : updateInfo?.error ? (
                <p className="mt-1 text-xs text-destructive">{t('settings.updateError')}</p>
              ) : (
                <p className="mt-1 text-xs text-muted-foreground">{t('settings.aboutHint')}</p>
              )}
              {updateInfo?.hasUpdate ? (
                <Button
                  size="sm"
                  onClick={handleOpenRelease}
                  className="no-drag mt-3 w-full cursor-pointer"
                >
                  {t('settings.downloadUpdate')}
                </Button>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleCheckUpdate}
                  disabled={checkingUpdate}
                  className="no-drag mt-3 w-full cursor-pointer"
                >
                  <RefreshCw className={cn('h-4 w-4', checkingUpdate && 'animate-spin')} />
                  {checkingUpdate ? t('settings.checkingUpdate') : t('settings.checkUpdate')}
                </Button>
              )}
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setChangelogOpen(true)}
                className="no-drag mt-2 w-full cursor-pointer text-xs text-muted-foreground hover:text-foreground"
              >
                <ScrollText className="mr-1.5 h-3.5 w-3.5" />
                {t('changelog.view')}
              </Button>
            </div>
          </div>
        </aside>
      </div>
    </div>
  )
}

function HealthItem({ on, title, status }: { on: boolean; title: string; status: string }) {
  return (
    <div className="flex items-center gap-2.5 border-b py-2.5 last:border-b-0">
      <span
        className={cn(
          'h-2.5 w-2.5 shrink-0 rounded-full',
          on ? 'bg-green-500 ring-3 ring-green-500/20' : 'bg-muted-foreground/40',
        )}
      />
      <div>
        <div className="text-[12.5px] font-semibold text-foreground">{title}</div>
        <div className="text-[11px] text-muted-foreground">{status}</div>
      </div>
    </div>
  )
}
