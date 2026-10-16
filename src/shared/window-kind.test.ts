import { describe, expect, it } from 'vitest'
import { resolveWindowKind } from './window-kind'

describe('resolveWindowKind', () => {
  it('routes every known Tauri window label to its surface', () => {
    expect(resolveWindowKind('main', '')).toBe('main')
    expect(resolveWindowKind('overlay', '')).toBe('overlay')
    // 0.5.6 decoupling: the session capsule window routes to SessionHUD.
    expect(resolveWindowKind('session', '')).toBe('session')
    expect(resolveWindowKind('background', '')).toBe('background')
  })

  it('lets a recognized label win over the hash', () => {
    expect(resolveWindowKind('session', '/overlay')).toBe('session')
    expect(resolveWindowKind('overlay', '/session')).toBe('overlay')
    expect(resolveWindowKind('main', '/history')).toBe('main')
  })

  it('falls back to the hash outside Tauri (browser preview)', () => {
    expect(resolveWindowKind(null, '/overlay')).toBe('overlay')
    expect(resolveWindowKind(null, '/session')).toBe('session')
    expect(resolveWindowKind(null, '/')).toBe('background')
    expect(resolveWindowKind(null, '')).toBe('background')
    expect(resolveWindowKind(null, '/history')).toBe('main')
  })

  it('treats an unknown label as "no label" (hash decides)', () => {
    expect(resolveWindowKind('mystery-window', '/session')).toBe('session')
    expect(resolveWindowKind('mystery-window', '')).toBe('background')
  })
})
