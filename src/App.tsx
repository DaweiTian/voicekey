import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import MainLayout from './layouts/MainLayout'
import HomePage from './pages/HomePage'
import SettingsPage from './pages/SettingsPage'
import HistoryPage from './pages/HistoryPage'
import { AudioRecorder } from './components/AudioRecorder'
import { HUD } from './components/HUD'
import { SessionHUD } from './components/SessionHUD'
import { resolveWindowKind, type WindowKind } from './shared/window-kind'

function readWindowKind(): WindowKind {
  let label: string | null = null
  try {
    label = getCurrentWindow().label
  } catch {
    // Not running inside Tauri (browser preview): hash decides.
  }
  return resolveWindowKind(label, window.location.hash.slice(1))
}

function App() {
  const [windowKind] = useState<WindowKind>(() => readWindowKind())
  const [currentRoute, setCurrentRoute] = useState(() => {
    const hash = window.location.hash.slice(1) || ''
    return hash && hash !== '/' ? hash : '/settings'
  })

  useEffect(() => {
    const handleHashChange = () => {
      const hash = window.location.hash.slice(1) || ''
      if (hash && hash !== '/') {
        setCurrentRoute(hash)
      }
    }
    window.addEventListener('hashchange', handleHashChange)
    return () => window.removeEventListener('hashchange', handleHashChange)
  }, [])

  if (windowKind === 'background') {
    return <AudioRecorder />
  }

  // Standby capsule: draggable / dockable idle + latched launcher (overlay window).
  if (windowKind === 'overlay') {
    return <HUD />
  }

  // Session capsule: fixed bottom-center recording / processing / result
  // visuals (session window, shown & positioned by Rust per session start).
  if (windowKind === 'session') {
    return <SessionHUD />
  }

  const getPage = () => {
    switch (currentRoute) {
      case '/history':
        return <HistoryPage />
      case '/home':
      case '/settings':
      default:
        return currentRoute === '/home' ? <HomePage /> : <SettingsPage />
    }
  }

  // Main window: settings is the operational surface; other routes via hash.
  const route =
    currentRoute === '/home' ? '/home' : currentRoute === '/history' ? '/history' : '/settings'
  return <MainLayout currentRoute={route}>{getPage()}</MainLayout>
}

export default App
