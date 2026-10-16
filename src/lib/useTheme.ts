import { useCallback, useLayoutEffect, useRef, useState } from 'react'

export type Theme = 'light' | 'dark'

export const ACCENTS = [
  'gulf-blue',
  'olive-green',
  'lava-orange',
  'sunset-purple',
  'elegant-grey',
  'classic',
] as const

export type Accent = (typeof ACCENTS)[number]

export interface AccentOption {
  id: Accent
  nameKey: string
  swatch: string
}

export const ACCENT_OPTIONS: AccentOption[] = [
  { id: 'gulf-blue', nameKey: 'theme.accent.gulfBlue', swatch: 'oklch(0.62 0.19 245)' },
  { id: 'olive-green', nameKey: 'theme.accent.oliveGreen', swatch: 'oklch(0.66 0.14 135)' },
  { id: 'lava-orange', nameKey: 'theme.accent.lavaOrange', swatch: 'oklch(0.68 0.2 45)' },
  { id: 'sunset-purple', nameKey: 'theme.accent.sunsetPurple', swatch: 'oklch(0.62 0.2 300)' },
  { id: 'elegant-grey', nameKey: 'theme.accent.elegantGrey', swatch: 'oklch(0.72 0.04 250)' },
  { id: 'classic', nameKey: 'theme.accent.classic', swatch: 'oklch(0.755 0.176 63)' },
]

const THEME_STORAGE_KEY = 'vk-theme'
const ACCENT_STORAGE_KEY = 'vk-accent'
const DEFAULT_ACCENT: Accent = 'olive-green'
const TRANSITION_SUPPRESS_CLASS = 'vk-theme-changing'

let transitionFrame = 0
let transitionFallbackTimer = 0

function isAccent(value: unknown): value is Accent {
  return typeof value === 'string' && ACCENTS.includes(value as Accent)
}

function getInitialTheme(): Theme {
  try {
    const stored = localStorage.getItem(THEME_STORAGE_KEY)
    if (stored === 'light' || stored === 'dark') return stored
    if (window.matchMedia('(prefers-color-scheme: dark)').matches) return 'dark'
  } catch {
    /* ignore */
  }
  return 'light'
}

function getInitialAccent(): Accent {
  try {
    const stored = localStorage.getItem(ACCENT_STORAGE_KEY)
    if (isAccent(stored)) return stored
  } catch {
    /* ignore */
  }
  return DEFAULT_ACCENT
}

function persistTheme(theme: Theme) {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, theme)
  } catch {
    /* ignore */
  }
}

function persistAccent(accent: Accent) {
  try {
    localStorage.setItem(ACCENT_STORAGE_KEY, accent)
  } catch {
    /* ignore */
  }
}

function applyTheme(theme: Theme) {
  const root = document.documentElement
  root.classList.toggle('dark', theme === 'dark')
  root.style.colorScheme = theme
}

function applyAccent(accent: Accent) {
  document.documentElement.setAttribute('data-accent', accent)
}

function releaseTransitionSuppression() {
  const root = document.documentElement
  root.classList.remove(TRANSITION_SUPPRESS_CLASS)

  if (transitionFrame) {
    window.cancelAnimationFrame(transitionFrame)
    transitionFrame = 0
  }
  if (transitionFallbackTimer) {
    window.clearTimeout(transitionFallbackTimer)
    transitionFallbackTimer = 0
  }
}

function runWithoutTransitions(action: () => void) {
  const root = document.documentElement
  root.classList.add(TRANSITION_SUPPRESS_CLASS)
  root.getBoundingClientRect()
  action()

  if (transitionFrame) window.cancelAnimationFrame(transitionFrame)
  if (transitionFallbackTimer) window.clearTimeout(transitionFallbackTimer)

  transitionFrame = window.requestAnimationFrame(() => {
    transitionFrame = window.requestAnimationFrame(releaseTransitionSuppression)
  })
  transitionFallbackTimer = window.setTimeout(releaseTransitionSuppression, 140)
}

/** 明暗主题与强调色：持久化到 localStorage 并同步到 <html> 类与 data 属性。 */
export function useTheme(): {
  theme: Theme
  setTheme: (theme: Theme) => void
  toggle: () => void
  accent: Accent
  setAccent: (accent: Accent) => void
} {
  const [theme, setThemeState] = useState<Theme>(getInitialTheme)
  const [accent, setAccentState] = useState<Accent>(getInitialAccent)
  const themeRef = useRef(theme)

  useLayoutEffect(() => {
    themeRef.current = theme
    applyTheme(theme)
    persistTheme(theme)
    applyAccent(accent)
    persistAccent(accent)
  }, [theme, accent])

  const setTheme = useCallback((next: Theme) => {
    if (themeRef.current === next) return
    themeRef.current = next
    runWithoutTransitions(() => applyTheme(next))
    persistTheme(next)
    setThemeState(next)
  }, [])

  const toggle = useCallback(() => {
    setTheme(themeRef.current === 'dark' ? 'light' : 'dark')
  }, [setTheme])

  const setAccent = useCallback(
    (next: Accent) => {
      if (accent === next) return
      runWithoutTransitions(() => applyAccent(next))
      persistAccent(next)
      setAccentState(next)
    },
    [accent],
  )

  return { theme, setTheme, toggle, accent, setAccent }
}
