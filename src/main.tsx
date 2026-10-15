import React from 'react'
import ReactDOM from 'react-dom/client'
import { SessionNotifications } from '@/components/SessionNotifications'
import { Toaster } from '@/components/ui/sonner'
import { initRendererLogger } from '@/lib/logger'
import { installTauriApi, tauriApi } from '@/lib/tauri-api'
import App from './App.tsx'
import i18n, { initI18n } from './i18n'
import './index.css'

installTauriApi()
initRendererLogger()

const root = ReactDOM.createRoot(document.getElementById('root')!)

initI18n()
  .catch((error) => {
    console.error('[i18n] Failed to initialize, falling back to defaults.', error)
  })
  .finally(() => {
    root.render(
      <React.StrictMode>
        <App />
        <SessionNotifications />
        <Toaster />
      </React.StrictMode>,
    )

    tauriApi.onAppLanguageChanged((snapshot) => {
      const resolved = (snapshot as { resolved?: string }).resolved
      if (resolved) {
        void i18n.changeLanguage(resolved)
        document.documentElement.lang = resolved
      }
    })
  })
