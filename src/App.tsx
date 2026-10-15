import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import MainLayout from './layouts/MainLayout'
import HomePage from './pages/HomePage'
import SettingsPage from './pages/SettingsPage'
import HistoryPage from './pages/HistoryPage'
import { AudioRecorder } from './components/AudioRecorder'
import { HUD } from './components/HUD'

type WindowKind = 'main' | 'overlay' | 'background' | 'unknown'

function resolveWindowKind(): WindowKind {
  try {
    const label = getCurrentWindow().label
    if (label === 'main' || label === 'overlay' || label === 'background') {
      return label
    }
  } catch {
    // Not running inside Tauri (browser preview).
  }

  const hash = window.location.hash.slice(1) || ''
  if (hash === '/overlay') return 'overlay'
  if (!hash || hash === '/') return 'background'
  return 'main'
}

function App() {
  const [windowKind] = useState<WindowKind>(() => resolveWindowKind())
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

  if (windowKind === 'overlay') {
    return <HUD />
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
