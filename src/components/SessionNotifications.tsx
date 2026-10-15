import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** Only the main window should surface capture feedback; the overlay HUD speaks for itself. */
function isMainWindow(): boolean {
  try {
    return getCurrentWindow().label === 'main'
  } catch {
    // Not running inside Tauri (browser preview): treat as the main window.
    return true
  }
}

/** Statuses the overlay already communicates; skip them so toasts do not become noise. */
const QUIET_SESSION_STATUSES = new Set(['recording', 'processing', 'idle', 'ready', ''])

/**
 * Lightweight capture feedback for the main window: surfaces capture errors and the finished
 * transcription, and skips noisy recording/processing session-status transitions.
 */
export function SessionNotifications() {
  const { t } = useTranslation()
  const lastStatusRef = useRef<string | null>(null)

  useEffect(() => {
    if (!isMainWindow()) return

    const api = window.electronAPI
    const removeError = api.onError((message) => {
      toast.error(t('notification.errorTitle'), {
        description: message?.trim() || t('notification.errorFallback'),
      })
    })

    const removeTranscription = api.onTranscription((text) => {
      const transcript = text?.trim()
      if (!transcript) return
      toast.success(t('notification.transcriptionTitle'), { description: transcript })
    })

    const removeSessionStatus = api.onSessionStatus((status) => {
      if (status === lastStatusRef.current) return
      lastStatusRef.current = status
      if (QUIET_SESSION_STATUSES.has(status)) return
      toast(t('notification.sessionStatusTitle'), { description: status })
    })

    return () => {
      removeError()
      removeTranscription()
      removeSessionStatus()
    }
  }, [t])

  return null
}
