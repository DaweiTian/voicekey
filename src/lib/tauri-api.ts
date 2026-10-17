/**
 * Tauri bridge that exposes the same `window.electronAPI` surface as the
 * Electron preload, so existing React components keep working.
 */
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AppConfig,
  ConfigSecretRequest,
  HistoryItem,
  LanguageSnapshot,
  LinuxIntegrationStatus,
  LLMRefineConfig,
  LogEntryPayload,
  LogTailOptions,
  OverlayEdge,
  OverlayState,
  RefineConnectionResult,
  RecordingStartPayload as SharedRecordingStartPayload,
  UpdateInfo,
} from '@/shared/types'
import type { StoredDictationStats } from '@/lib/stats'

export interface RecordingStartPayload extends SharedRecordingStartPayload {
  mode?: string
  microphoneDeviceId?: string
  lowVolumeMode?: boolean
}

export interface AudioChunkPayload {
  sessionId: string
  chunkIndex: number
  isFinal: boolean
  mimeType: string
  buffer: ArrayBuffer | ArrayBufferView
}

export interface StreamingAudioFramePayload {
  sessionId: string
  sequence: number
  sampleRate: number
  buffer: ArrayBuffer
}

export interface StreamingAudioEndPayload {
  sessionId: string
  sequence: number
}

/**
 * Result of `set_config`: the fully persisted (masked) config plus whether the
 * hotkey accelerators were rolled back in the main process because the new ones
 * could not be registered. On rollback the renderer must overwrite its hotkey
 * state with `config.hotkey` and clear the dirty flag.
 */
export interface SetConfigResult {
  config: AppConfig
  hotkeyRolledBack: boolean
}

type Unsub = () => void

function detectPlatform(): string {
  const ua = navigator.userAgent.toLowerCase()
  if (ua.includes('mac')) return 'darwin'
  if (ua.includes('win')) return 'win32'
  return 'linux'
}

function listenAsync<T>(event: string, cb: (payload: T) => void): Unsub {
  let unlisten: UnlistenFn | null = null
  let cancelled = false
  listen<T>(event, (e) => cb(e.payload)).then((fn) => {
    if (cancelled) fn()
    else unlisten = fn
  })
  return () => {
    cancelled = true
    unlisten?.()
  }
}

export const tauriApi = {
  platform: detectPlatform(),

  // config
  getConfig: () => invoke<AppConfig>('get_config'),
  setConfig: (config: Partial<AppConfig>) => invoke<SetConfigResult>('set_config', { config }),
  getConfigSecret: (request: ConfigSecretRequest) =>
    invoke<string>('get_config_secret', { request }),
  testRefineConnection: (config: LLMRefineConfig) =>
    invoke<RefineConnectionResult>('test_refine_connection', { config }),
  connectTokenDance: () => invoke<{ ok: boolean; message?: string }>('connect_token_dance'),
  getAppLanguage: () => invoke<LanguageSnapshot>('get_app_language'),
  onAppLanguageChanged: (cb: (snapshot: LanguageSnapshot) => void): Unsub =>
    listenAsync('app-language-changed', cb),
  getLinuxIntegrationStatus: () => invoke<LinuxIntegrationStatus>('get_linux_integration_status'),
  installLinuxIntegration: () => invoke<LinuxIntegrationStatus>('install_linux_integration'),
  removeLinuxIntegration: () => invoke<LinuxIntegrationStatus>('remove_linux_integration'),

  // session
  startSession: () => invoke<void>('start_session'),
  stopSession: () => invoke<string>('stop_session'),
  getSessionStatus: () => invoke<string>('get_session_status'),
  cancelSession: () => invoke<void>('cancel_session'),

  // history
  getHistory: () => invoke<HistoryItem[]>('get_history'),
  clearHistory: () => invoke<void>('clear_history'),
  deleteHistoryItem: (id: string) => invoke<boolean>('delete_history_item', { id }),

  // lifetime stats (independent store; history deletion/pruning never resets it)
  getStats: () => invoke<StoredDictationStats>('get_stats'),

  // hotkeys (registration is handled in Rust)
  registerHotkey: async () => true,
  unregisterHotkey: async () => undefined,
  // While the settings hotkey recorder is capturing, Rust ignores PTT/translate
  // global hotkey presses so recorded combos cannot trigger actions.
  setHotkeyCaptureActive: (active: boolean) =>
    invoke<void>('set_hotkey_capture_active', { active }),

  // events
  onSessionStatus: (cb: (status: string) => void): Unsub => listenAsync('session-status', cb),
  onTranscription: (cb: (text: string) => void): Unsub => listenAsync('transcription', cb),
  // Rust emits `{ message }` on the `error` channel (see commands.rs emit_session_error),
  // so unwrap the envelope here and keep the Electron-era string surface intact.
  onError: (cb: (error: string) => void): Unsub =>
    listenAsync<string | { message?: string }>('error', (payload) => {
      cb(typeof payload === 'string' ? payload : (payload?.message ?? ''))
    }),

  onStartRecording: (cb: (payload: RecordingStartPayload) => void): Unsub =>
    listenAsync('start-recording', cb),
  onStopRecording: (cb: () => void): Unsub => listenAsync('stop-recording', () => cb()),
  sendAudioChunk: (payload: AudioChunkPayload) => {
    void invoke('send_audio_chunk', {
      payload: {
        sessionId: payload.sessionId,
        chunkIndex: payload.chunkIndex,
        isFinal: payload.isFinal,
        mimeType: payload.mimeType,
        buffer: Array.from(
          new Uint8Array(
            payload.buffer instanceof ArrayBuffer
              ? payload.buffer
              : ((payload.buffer as ArrayBufferView).buffer as ArrayBuffer),
          ),
        ),
      },
    })
  },
  sendStreamingAudioFrame: (payload: StreamingAudioFramePayload) => {
    void invoke('send_streaming_audio_frame', {
      payload: {
        sessionId: payload.sessionId,
        sequence: payload.sequence,
        sampleRate: payload.sampleRate,
        buffer: Array.from(new Uint8Array(payload.buffer)),
      },
    })
  },
  sendStreamingAudioEnd: (payload: StreamingAudioEndPayload) => {
    void invoke('send_streaming_audio_end', { payload })
  },
  sendError: (error: string) => {
    void invoke('send_error', { error })
  },
  sendAudioLevel: (level: number) => {
    void invoke('send_audio_level', { level })
  },

  onOverlayUpdate: (cb: (state: OverlayState) => void): Unsub => listenAsync('overlay-update', cb),
  onAudioLevel: (cb: (level: number) => void): Unsub => listenAsync('audio-level', cb),

  setIgnoreMouseEvents: (ignore: boolean) => {
    void invoke('set_ignore_mouse_events', { ignore })
  },

  // Overlay 窗口按内容尺寸缩放：渲染层测量胶囊实际尺寸后调用，Rust 侧
  // `set_overlay_size` 负责钳制尺寸合法性并保持窗口底部居中（W1 点击死区修复）。
  setOverlaySize: (width: number, height: number) =>
    invoke<void>('set_overlay_size', { width, height }),

  // Overlay 窗口位置持久化：渲染层在拖拽释放后把物理像素坐标发给 Rust，
  // Rust 负责贴边吸附、工作区钳制、持久化；传 null, null 表示重置为底部居中。
  // 返回值 = 本次落位是否「吸附贴边」：`'left' | 'right' | 'top' | 'bottom'` 表示吸附到了
  // 该工作区边（docked，HUD 待机胶囊渲染紧凑形态），`null` 表示超磁性阈值仅钳制落位 /
  // 重置分支（保持全形）。旧调用忽略返回值也不受影响（向后兼容）。
  setOverlayPosition: (
    x: number | null,
    y: number | null,
    // 可选：可见胶囊的实际尺寸（逻辑 px，DOM offsetWidth/offsetHeight）。Rust 据此做
    // 「胶囊贴边」吸附；缺省（或 malformed）回退按窗口外框吸附的旧行为。
    capsule?: { width: number; height: number },
  ) => invoke<OverlayEdge | null>('set_overlay_position', { x, y, capsule: capsule ?? null }),

  // updates
  checkForUpdates: () => invoke<UpdateInfo>('check_for_updates'),
  getUpdateStatus: () => invoke<UpdateInfo | null>('get_update_status'),
  getAppVersion: () => invoke<string>('get_app_version'),
  openExternal: (url: string) => invoke('open_external', { url }),

  // logs
  getLogTail: (options?: LogTailOptions) => invoke<string>('get_log_tail', { options }),
  openLogFolder: () => invoke('open_log_folder'),
  log: (entry: LogEntryPayload) => {
    void invoke('log', { entry })
  },

  // extras used by Tauri build
  showSettingsWindow: () => invoke('show_settings_window'),
  refineText: (text: string) => invoke<string>('refine_text', { text }),
  translateText: (text: string, targetLanguage: string) =>
    invoke<string>('translate_text', { text, targetLanguage }),
}

export type ElectronAPI = typeof tauriApi

export function installTauriApi(): void {
  ;(window as unknown as { electronAPI: ElectronAPI }).electronAPI = tauriApi
}
