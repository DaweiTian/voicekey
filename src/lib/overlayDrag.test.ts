import { describe, expect, it } from 'vitest'
import {
  DRAG_THRESHOLD_PX,
  DOUBLE_CLICK_MAX_MS,
  TAP_MAX_MS,
  WATCHDOG_MAX_MS,
  initialOverlayDragState,
  onDoubleClick,
  onPointerCancel,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onWatchdog,
  onWindowBlur,
  setWinStart,
  type DragCommand,
  type OverlayDragState,
} from './overlayDrag'

const downEvent = (
  pointerId: number,
  screen: { x: number; y: number } = { x: 100, y: 100 },
  button = 0,
) => ({
  button,
  pointerId,
  screenX: screen.x,
  screenY: screen.y,
})

const moveEvent = (pointerId: number, screen: { x: number; y: number } = { x: 100, y: 100 }) => ({
  pointerId,
  screenX: screen.x,
  screenY: screen.y,
})

const upEvent = (pointerId: number, screen: { x: number; y: number } = { x: 100, y: 100 }) => ({
  pointerId,
  screenX: screen.x,
  screenY: screen.y,
})

const cmdTypes = (cmds: DragCommand[]): string[] => cmds.map((c) => c.type)

const hasCmd = (cmds: DragCommand[], type: DragCommand['type']): boolean =>
  cmds.some((c) => c.type === type)

const moveCmdPos = (cmds: DragCommand[]): { x: number; y: number } | null => {
  const cmd = cmds.find(
    (c): c is { type: 'move'; pos: { x: number; y: number } } => c.type === 'move',
  )
  return cmd ? cmd.pos : null
}

describe('onPointerDown', () => {
  it('starts a pending drag / PTT from idle when status is idle', () => {
    const [state, cmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    expect(state.phase).toBe('pending')
    expect(state.pointerId).toBe(1)
    expect(state.startScreen).toEqual({ x: 100, y: 100 })
    expect(hasCmd(cmds, 'start-session')).toBe(true)
    expect(hasCmd(cmds, 'capture-pointer')).toBe(true)
    expect(hasCmd(cmds, 'add-listeners')).toBe(true)
    expect(hasCmd(cmds, 'start-watchdog')).toBe(true)
  })

  it('ignores non-primary buttons', () => {
    const [state] = onPointerDown(
      downEvent(1, { x: 100, y: 100 }, 2),
      initialOverlayDragState(),
      0,
      'idle',
    )
    expect(state.phase).toBe('idle')
  })

  it('is no-op when overlay status is not idle', () => {
    const [state] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'recording')
    expect(state.phase).toBe('idle')
  })

  it('is no-op when already pending or dragging', () => {
    const busy = { ...initialOverlayDragState(), phase: 'pending' as const, pointerId: 1 }
    const [state] = onPointerDown(downEvent(2), busy, 0, 'idle')
    expect(state.pointerId).toBe(1)
  })
})

describe('whole-window press surface', () => {
  it('starts and stops a session from a press anywhere on the surface (upper / lower half)', () => {
    // The idle / drag handlers live on the full-window container, so the press
    // Y coordinate inside the capsule must never decide whether a session starts.
    for (const y of [88, 100, 112]) {
      const [state, cmds] = onPointerDown(
        downEvent(1, { x: 100, y }),
        initialOverlayDragState(),
        0,
        'idle',
      )
      expect(state.phase).toBe('pending')
      expect(hasCmd(cmds, 'start-session')).toBe(true)
      const [, upCmds] = onPointerUp(upEvent(1, { x: 100, y }), state, WATCHDOG_MAX_MS - 1)
      expect(hasCmd(upCmds, 'stop-session')).toBe(true)
    }
  })

  it('keeps lastTap when the second press is rejected while processing, so the reset still fires', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS - 1)
    const [afterDown] = onPointerDown(
      downEvent(2, { x: 100, y: 100 }),
      state,
      TAP_MAX_MS,
      'processing',
    )
    expect(afterDown.lastTap).not.toBeNull()
    const [, cmds] = onDoubleClick(afterDown, DOUBLE_CLICK_MAX_MS - 1)
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(true)
  })
})

describe('pending → hold-to-talk (no drag)', () => {
  it('stops the session on pointerup when movement stayed within threshold', () => {
    const [state] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    const [nextState, cmds] = onPointerUp(
      upEvent(1, { x: 100 + DRAG_THRESHOLD_PX, y: 100 }),
      state,
      TAP_MAX_MS - 1,
    )
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
    expect(hasCmd(cmds, 'release-pointer-capture')).toBe(true)
  })

  it('stops the session even when held longer than tap threshold without moving', () => {
    const [state] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS + 50)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
  })
})

describe('pending → dragging', () => {
  it('enters dragging and cancels session when threshold is exceeded', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerMove(
      moveEvent(1, { x: 100 + DRAG_THRESHOLD_PX + 1, y: 100 }),
      state,
      10,
    )
    expect(nextState.phase).toBe('dragging')
    expect(nextState.resizeSuppressed).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
    expect(hasCmd(cmds, 'suppress-resize')).toBe(true)
  })

  it('ignores pointermove from a different pointerId', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState] = onPointerMove(
      moveEvent(2, { x: 100 + DRAG_THRESHOLD_PX + 20, y: 100 }),
      state,
      10,
    )
    expect(nextState.phase).toBe('pending')
  })

  it('computes physical move position once winStart and scaleFactor are known', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    state = setWinStart(state, { x: 1000, y: 500 }, 1.5)
    const [, cmds] = onPointerMove(moveEvent(1, { x: 110, y: 120 }), state, 10)
    expect(moveCmdPos(cmds)).toEqual({ x: 1015, y: 530 })
  })

  it('persists position and allows resize on drag release', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    state = setWinStart(state, { x: 1000, y: 500 }, 1.0)
    ;[state] = onPointerMove(moveEvent(1, { x: 110, y: 110 }), state, 10)
    expect(state.phase).toBe('dragging')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 110, y: 110 }), state, 20)
    expect(nextState.phase).toBe('idle')
    expect(nextState.resizeSuppressed).toBe(false)
    expect(cmdTypes(cmds)).toContain('flush-move')
    expect(cmdTypes(cmds)).toContain('allow-resize')
    expect(cmdTypes(cmds)).toContain('persist-position')
    expect(cmdTypes(cmds)).toContain('remove-listeners')
  })
})

describe('reset paths must always return to idle', () => {
  it('pointerup resets pending to idle and clears watchdog', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerUp(upEvent(1), state, TAP_MAX_MS - 1)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'clear-watchdog')).toBe(true)
    expect(hasCmd(cmds, 'release-pointer-capture')).toBe(true)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
  })

  it('pointercancel resets pending to idle', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, 10)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
  })

  it('pointercancel resets dragging to idle and allows resize', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, 20)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'allow-resize')).toBe(true)
  })

  it('window blur resets any active drag to idle', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    const [nextState, cmds] = onWindowBlur(state)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
    expect(hasCmd(cmds, 'allow-resize')).toBe(true)
  })

  it('watchdog resets stuck dragging state after timeout', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    state = { ...state, watchdogAt: 1000 }
    const [nextState, cmds] = onWatchdog(state, 1001)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
    expect(hasCmd(cmds, 'allow-resize')).toBe(true)
    expect(hasCmd(cmds, 'clear-watchdog')).toBe(true)
  })

  it('watchdog does nothing before deadline', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onWatchdog(state, WATCHDOG_MAX_MS - 1)
    expect(nextState.phase).toBe('pending')
    expect(cmds).toHaveLength(0)
  })
})

describe('tap detection and double-click reset', () => {
  it('lone short click stops the session (records) instead of cancelling it', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS - 1)
    // REGRESSION LOCK: a plain click must reach stop_session (the 0.5.1 build
    // cancelled it here, so no click could ever record anything).
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(nextState.lastTap).toEqual({ point: { x: 100, y: 100 }, time: TAP_MAX_MS - 1 })
  })

  it('cancels only the second click of a double-click, so the reset stays quiet', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS - 1)
    // Second press: the previous tap survives pointerdown so the dblclick reset
    // can still fire even when this press is rejected by Rust.
    ;[state] = onPointerDown(downEvent(2, { x: 100, y: 100 }), state, TAP_MAX_MS, 'idle')
    expect(state.phase).toBe('pending')
    const [, cmds] = onPointerUp(upEvent(2, { x: 100, y: 100 }), state, DOUBLE_CLICK_MAX_MS - 1)
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
  })

  it('treats a distant second click as a fresh recording, not a double-click', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS - 1)
    ;[state] = onPointerDown(downEvent(2, { x: 200, y: 200 }), state, TAP_MAX_MS, 'idle')
    const [, cmds] = onPointerUp(upEvent(2, { x: 200, y: 200 }), state, DOUBLE_CLICK_MAX_MS - 1)
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
  })

  it('subsequent double-click resets overlay position when within time and distance', () => {
    const state: OverlayDragState = {
      ...initialOverlayDragState(),
      lastTap: { point: { x: 100, y: 100 }, time: 0 },
    }
    const [nextState, cmds] = onDoubleClick(state, DOUBLE_CLICK_MAX_MS - 1)
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(true)
    const setPos = cmds.find(
      (c): c is { type: 'set-overlay-position'; x: null; y: null } =>
        c.type === 'set-overlay-position',
    )
    expect(setPos?.x).toBeNull()
    expect(setPos?.y).toBeNull()
    expect(nextState.lastTap).toBeNull()
  })

  it('ignores stale double-click beyond window', () => {
    const state: OverlayDragState = {
      ...initialOverlayDragState(),
      lastTap: { point: { x: 100, y: 100 }, time: 0 },
    }
    const [, cmds] = onDoubleClick(state, DOUBLE_CLICK_MAX_MS + 1)
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(false)
  })

  it('does not reset on double-click when first press was a real hold', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    const [afterUp] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, TAP_MAX_MS + 50)
    expect(afterUp.lastTap).toBeNull()
    const [, cmds] = onDoubleClick(afterUp, TAP_MAX_MS + 100)
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(false)
  })
})

describe('normal hold-to-talk is not swallowed', () => {
  it('allows a normal hold after a double-click reset', () => {
    const state: OverlayDragState = {
      ...initialOverlayDragState(),
      lastTap: { point: { x: 100, y: 100 }, time: 0 },
    }
    const [afterReset] = onDoubleClick(state, DOUBLE_CLICK_MAX_MS - 1)
    const [afterDown, cmds] = onPointerDown(
      downEvent(2, { x: 120, y: 120 }),
      afterReset,
      500,
      'idle',
    )
    expect(afterDown.phase).toBe('pending')
    expect(hasCmd(cmds, 'start-session')).toBe(true)
  })

  it('allows a normal hold when no recent tap exists', () => {
    const [state, cmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    expect(state.phase).toBe('pending')
    expect(hasCmd(cmds, 'start-session')).toBe(true)
  })
})
