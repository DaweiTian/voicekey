import type { ElectronAPI } from './lib/tauri-api'

declare global {
  interface Window {
    electronAPI: ElectronAPI
    __APP_VERSION__?: string
  }
}

export {}
