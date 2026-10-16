import { describe, expect, it } from 'vitest'
import type { OverlayStatus } from '@/shared/types'
import {
  DOCK_COMPACT_PX,
  DOCK_TRANSLATE,
  DRAG_THRESHOLD_PX,
  DOUBLE_CLICK_MAX_MS,
  HOLD_LATCH_MAX_MS,
  SILENCE_AUTO_STOP_MS,
  WATCHDOG_MAX_MS,
  initialOverlayDragState,
  isOverlayEdge,
  isSilenceAutoStopDue,
  onDoubleClick,
  onHoldTimer,
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

/** Begin a fresh gesture from idle and return the resulting state. */
const beginGesture = (
  from: OverlayDragState = initialOverlayDragState(),
  now = 0,
  status: OverlayStatus = 'idle',
  screen: { x: number; y: number } = { x: 100, y: 100 },
): OverlayDragState => {
  const [state] = onPointerDown(downEvent(1, screen), from, now, status)
  return state
}

describe('onPointerDown', () => {
  it('enters pending from idle WITHOUT starting a session (no resize during the gesture)', () => {
    const [state, cmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    expect(state.phase).toBe('pending')
    expect(state.pointerId).toBe(1)
    expect(state.startScreen).toEqual({ x: 100, y: 100 })
    // REGRESSION LOCK: pointerdown must NOT start a session any more. Starting one
    // resizes/re-anchors the overlay window mid-gesture, which broke dragging on
    // real hardware (see overlayDrag.ts module header).
    expect(hasCmd(cmds, 'start-session')).toBe(false)
    expect(hasCmd(cmds, 'capture-pointer')).toBe(true)
    expect(hasCmd(cmds, 'add-listeners')).toBe(true)
    expect(hasCmd(cmds, 'start-watchdog')).toBe(true)
    expect(hasCmd(cmds, 'start-hold-timer')).toBe(true)
    expect(state.holdDeadline).toBe(HOLD_LATCH_MAX_MS)
  })

  it('also begins a gesture on a latched recording so a second tap can stop it', () => {
    const latched: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    const [state] = onPointerDown(downEvent(1), latched, 0, 'recording')
    expect(state.phase).toBe('pending')
    expect(state.startedLatched).toBe(true)
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

  it('is no-op for an active non-latched recording', () => {
    const [state] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'recording')
    expect(state.phase).toBe('idle')
  })

  it('is no-op when already pending or dragging', () => {
    const busy = { ...initialOverlayDragState(), phase: 'pending' as const, pointerId: 1 }
    const [state] = onPointerDown(downEvent(2), busy, 0, 'idle')
    expect(state.pointerId).toBe(1)
  })
})

describe('hold timer resolves pending into PTT', () => {
  it('enters holding and starts a session once the threshold is reached', () => {
    const state = beginGesture()
    const [nextState, cmds] = onHoldTimer(state, HOLD_LATCH_MAX_MS)
    expect(nextState.phase).toBe('holding')
    expect(hasCmd(cmds, 'start-session')).toBe(true)
    expect(hasCmd(cmds, 'clear-watchdog')).toBe(true)
    // Hard invariant: entering PTT must not emit any window-geometry command.
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(false)
    expect(hasCmd(cmds, 'move')).toBe(false)
  })

  it('does nothing before the deadline', () => {
    const state = beginGesture()
    const [nextState, cmds] = onHoldTimer(state, HOLD_LATCH_MAX_MS - 1)
    expect(nextState.phase).toBe('pending')
    expect(cmds).toHaveLength(0)
  })

  it('does nothing once the gesture moved into dragging', () => {
    let state = beginGesture()
    ;[state] = onPointerMove(moveEvent(1, { x: 100 + DRAG_THRESHOLD_PX + 5, y: 100 }), state, 10)
    expect(state.phase).toBe('dragging')
    const [nextState, cmds] = onHoldTimer(state, HOLD_LATCH_MAX_MS)
    expect(nextState.phase).toBe('dragging')
    expect(hasCmd(cmds, 'start-session')).toBe(false)
  })

  it('does not start a session when the gesture began on a latched recording', () => {
    const latched: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    const [state] = onPointerDown(downEvent(1), latched, 0, 'recording')
    const [nextState, cmds] = onHoldTimer(state, HOLD_LATCH_MAX_MS)
    expect(nextState.phase).toBe('pending')
    expect(hasCmd(cmds, 'start-session')).toBe(false)
    expect(nextState.latched).toBe(true)
  })
})

describe('whole-window press surface', () => {
  it('latches a session from a press anywhere on the surface (upper / lower half)', () => {
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
      expect(hasCmd(cmds, 'start-session')).toBe(false)
      const [afterUp, upCmds] = onPointerUp(upEvent(1, { x: 100, y }), state, 50)
      expect(hasCmd(upCmds, 'start-session')).toBe(true)
      expect(hasCmd(upCmds, 'stop-session')).toBe(false)
      expect(afterUp.latched).toBe(true)
    }
  })

  it('keeps lastTap when the second press is rejected while processing, so the reset still fires', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, 50)
    const [afterDown] = onPointerDown(downEvent(2, { x: 100, y: 100 }), state, 60, 'processing')
    expect(afterDown.phase).toBe('idle')
    expect(afterDown.lastTap).not.toBeNull()
    const [, cmds] = onDoubleClick(afterDown, 50 + DOUBLE_CLICK_MAX_MS - 1)
    expect(hasCmd(cmds, 'set-overlay-position')).toBe(true)
  })
})

describe('pending → hold-to-talk (PTT)', () => {
  it('stops the session on pointerup when the hold threshold was reached', () => {
    const state = beginGesture()
    const [held] = onHoldTimer(state, HOLD_LATCH_MAX_MS)
    const [nextState, cmds] = onPointerUp(
      upEvent(1, { x: 100, y: 100 }),
      held,
      HOLD_LATCH_MAX_MS + 30,
    )
    expect(nextState.phase).toBe('idle')
    expect(nextState.latched).toBe(false)
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
    expect(hasCmd(cmds, 'release-pointer-capture')).toBe(true)
  })
})

describe('pending → dragging', () => {
  it('enters dragging and never starts or cancels a session when threshold is exceeded', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerMove(
      moveEvent(1, { x: 100 + DRAG_THRESHOLD_PX + 1, y: 100 }),
      state,
      10,
    )
    expect(nextState.phase).toBe('dragging')
    // REGRESSION LOCK: dragging must not touch the session (the old design
    // cancelled a session that pointerdown had wrongly started).
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'start-session')).toBe(false)
    expect(hasCmd(cmds, 'clear-hold-timer')).toBe(true)
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

  it('persists position and keeps latched on drag release', () => {
    let state: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'recording')
    state = setWinStart(state, { x: 1000, y: 500 }, 1.0)
    ;[state] = onPointerMove(moveEvent(1, { x: 110, y: 110 }), state, 10)
    expect(state.phase).toBe('dragging')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 110, y: 110 }), state, 20)
    expect(nextState.phase).toBe('idle')
    // Dragging a latched recording only moves the window; recording continues.
    expect(nextState.latched).toBe(true)
    expect(cmdTypes(cmds)).toContain('flush-move')
    expect(cmdTypes(cmds)).toContain('persist-position')
    expect(cmdTypes(cmds)).toContain('remove-listeners')
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
  })
})

describe('reset paths must always return to idle', () => {
  it('pointerup latches a short tap and clears the watchdog / hold timer', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerUp(upEvent(1), state, 50)
    expect(nextState.phase).toBe('idle')
    expect(nextState.latched).toBe(true)
    expect(hasCmd(cmds, 'clear-watchdog')).toBe(true)
    expect(hasCmd(cmds, 'clear-hold-timer')).toBe(true)
    expect(hasCmd(cmds, 'release-pointer-capture')).toBe(true)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
  })

  it('pointercancel while pending does not cancel a session', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, 10)
    expect(nextState.phase).toBe('idle')
    // pending never started a session, so there is nothing to cancel.
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'start-session')).toBe(false)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
  })

  it('pointercancel while pending keeps a latched recording alive', () => {
    const latched: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    const [state] = onPointerDown(downEvent(1), latched, 0, 'recording')
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, 10)
    expect(nextState.phase).toBe('idle')
    expect(nextState.latched).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
  })

  it('pointercancel while dragging resets to idle (keeps latched)', () => {
    let state: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    ;[state] = onPointerDown(downEvent(1), state, 0, 'recording')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, 20)
    expect(nextState.phase).toBe('idle')
    expect(nextState.latched).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
  })

  it('pointercancel while holding stops the PTT so audio is transcribed, not discarded', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    ;[state] = onHoldTimer(state, HOLD_LATCH_MAX_MS)
    const [nextState, cmds] = onPointerCancel({ pointerId: 1 }, state, HOLD_LATCH_MAX_MS + 10)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
  })

  it('window blur during an active gesture resets to idle without cancelling (latched-exempt)', () => {
    let state: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    ;[state] = onPointerDown(downEvent(1), state, 0, 'recording')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    const [nextState, cmds] = onWindowBlur(state)
    expect(nextState.phase).toBe('idle')
    expect(nextState.latched).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
  })

  it('window blur is a no-op for a latched recording (phase idle)', () => {
    const latched: OverlayDragState = { ...initialOverlayDragState(), latched: true }
    const [nextState, cmds] = onWindowBlur(latched)
    expect(nextState).toBe(latched)
    expect(cmds).toHaveLength(0)
  })

  it('watchdog resets stuck dragging state after timeout without cancelling a session', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    ;[state] = onPointerMove(moveEvent(1, { x: 200, y: 200 }), state, 10)
    state = { ...state, watchdogAt: 1000 }
    const [nextState, cmds] = onWatchdog(state, 1001)
    expect(nextState.phase).toBe('idle')
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'clear-watchdog')).toBe(true)
    expect(hasCmd(cmds, 'remove-listeners')).toBe(true)
  })

  it('watchdog never touches a latched (idle) state', () => {
    const latched: OverlayDragState = {
      ...initialOverlayDragState(),
      latched: true,
      watchdogAt: 0,
    }
    const [nextState, cmds] = onWatchdog(latched, 10_000)
    expect(nextState).toBe(latched)
    expect(cmds).toHaveLength(0)
  })

  it('watchdog does nothing before deadline', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1), state, 0, 'idle')
    const [nextState, cmds] = onWatchdog(state, WATCHDOG_MAX_MS - 1)
    expect(nextState.phase).toBe('pending')
    expect(cmds).toHaveLength(0)
  })
})

describe('tap = toggle (latch) and double-click reset', () => {
  it('a lone short tap latches the session: start-session but no stop-session', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, 50)
    // A tap must keep recording after release (latched), not stop on release.
    expect(hasCmd(cmds, 'start-session')).toBe(true)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
    expect(nextState.latched).toBe(true)
    expect(nextState.lastTap).toEqual({ point: { x: 100, y: 100 }, time: 50 })
  })

  it('a second tap on a latched recording stops it', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, 50)
    expect(state.latched).toBe(true)
    // Second tap, long after the double-click window and far away: a plain stop.
    ;[state] = onPointerDown(downEvent(2, { x: 400, y: 400 }), state, 2000, 'recording')
    expect(state.startedLatched).toBe(true)
    const [nextState, cmds] = onPointerUp(upEvent(2, { x: 400, y: 400 }), state, 2050)
    expect(hasCmd(cmds, 'stop-session')).toBe(true)
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(nextState.latched).toBe(false)
  })

  it('cancels the second click of a double-click (quiet reset) instead of stopping', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    ;[state] = onPointerUp(upEvent(1, { x: 100, y: 100 }), state, 50)
    // Second press close in time and place — the recording is latched, so the
    // press begins a stop gesture that resolves to a double-click cancel.
    ;[state] = onPointerDown(downEvent(2, { x: 100, y: 100 }), state, 150, 'recording')
    expect(state.phase).toBe('pending')
    const [, cmds] = onPointerUp(upEvent(2, { x: 100, y: 100 }), state, 200)
    expect(hasCmd(cmds, 'cancel-session')).toBe(true)
    expect(hasCmd(cmds, 'stop-session')).toBe(false)
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
})

describe('silence auto-stop helper', () => {
  it('is due only for a latched recording quiet for the whole window', () => {
    expect(isSilenceAutoStopDue({ latched: true, lastVoiceAt: 0, now: SILENCE_AUTO_STOP_MS })).toBe(
      true,
    )
    expect(
      isSilenceAutoStopDue({ latched: true, lastVoiceAt: 0, now: SILENCE_AUTO_STOP_MS - 1 }),
    ).toBe(false)
    expect(
      isSilenceAutoStopDue({ latched: false, lastVoiceAt: 0, now: SILENCE_AUTO_STOP_MS * 10 }),
    ).toBe(false)
  })
})

describe('hold-to-talk still begins with a gesture, not on pointerdown', () => {
  it('a hold after a double-click reset only resolves into PTT via the hold timer', () => {
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
    // No session on pointerdown any more; it begins at the hold threshold.
    expect(hasCmd(cmds, 'start-session')).toBe(false)
    const [held, holdCmds] = onHoldTimer(afterDown, 500 + HOLD_LATCH_MAX_MS)
    expect(held.phase).toBe('holding')
    expect(hasCmd(holdCmds, 'start-session')).toBe(true)
  })
})

// HARD INVARIANT (0.5.3): the state machine must never emit a command that changes
// window geometry, because an in-gesture window resize/re-anchor is what broke drag
// (0.5.2) and hold-to-talk (0.5.3) on real hardware. The overlay window is a fixed
// 280x64 and the capsule only swaps DOM branches; nothing here may resize or move
// the window during a gesture or a live session. The only legitimately
// geometry-moving commands are 'move'/'flush-move'/'persist-position' (self-drag,
// live) and 'set-overlay-position' (double-click reset, only after a gesture ends).
describe('window geometry invariant', () => {
  // Every command that is NOT a window-geometry change.
  const NON_GEOMETRY_COMMANDS = new Set<string>([
    'start-session',
    'stop-session',
    'cancel-session',
    'capture-pointer',
    'release-pointer-capture',
    'add-listeners',
    'remove-listeners',
    'start-watchdog',
    'clear-watchdog',
    'start-hold-timer',
    'clear-hold-timer',
  ])
  // Self-drag position updates are allowed; they must never appear in a PTT gesture.
  const DRAG_ONLY_COMMANDS = new Set<string>(['move', 'flush-move', 'persist-position'])

  it('emits no geometry command while entering PTT (holding)', () => {
    const [down, downCmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    const [, holdCmds] = onHoldTimer(down, HOLD_LATCH_MAX_MS)
    for (const type of cmdTypes([...downCmds, ...holdCmds])) {
      expect(NON_GEOMETRY_COMMANDS.has(type)).toBe(true)
    }
  })

  it('a full hold gesture never resizes or re-anchors the window', () => {
    const [down, downCmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    const [held, holdCmds] = onHoldTimer(down, HOLD_LATCH_MAX_MS)
    const [, upCmds] = onPointerUp(upEvent(1), held, HOLD_LATCH_MAX_MS + 2000)
    for (const type of cmdTypes([...downCmds, ...holdCmds, ...upCmds])) {
      expect(NON_GEOMETRY_COMMANDS.has(type)).toBe(true)
    }
  })

  it('a full drag gesture only moves the window, never resizes it', () => {
    const [down, downCmds] = onPointerDown(downEvent(1), initialOverlayDragState(), 0, 'idle')
    const [dragging, moveCmds] = onPointerMove(
      moveEvent(1, { x: 100 + DRAG_THRESHOLD_PX + 30, y: 130 }),
      down,
      10,
    )
    const [, upCmds] = onPointerUp(upEvent(1, { x: 140, y: 130 }), dragging, 20)
    for (const type of cmdTypes([...downCmds, ...moveCmds, ...upCmds])) {
      expect(NON_GEOMETRY_COMMANDS.has(type) || DRAG_ONLY_COMMANDS.has(type)).toBe(true)
    }
  })
})

// docked 紧凑形态的输入收敛：`setOverlayPosition` 返回值（拖拽 persist / 双击复位）与
// 启动时读到的配置 `app.overlayDocked` 都必须经 isOverlayEdge 判定。invoke 边界的数据
// 一律按不可信处理——只有 Rust 约定的四个工作区边是 true，其余（含 null / 脏值）一律
// false，保证未知数据永远渲染带文字的全形胶囊。
describe('isOverlayEdge (docked compact capsule gate)', () => {
  it('accepts the four work-area edges Rust can report', () => {
    for (const edge of ['left', 'right', 'top', 'bottom']) {
      expect(isOverlayEdge(edge)).toBe(true)
    }
  })

  it('treats null, absent and malformed values as not docked', () => {
    expect(isOverlayEdge(null)).toBe(false)
    expect(isOverlayEdge(undefined)).toBe(false)
    expect(isOverlayEdge('')).toBe(false)
    expect(isOverlayEdge('LEFT')).toBe(false)
    expect(isOverlayEdge('center')).toBe(false)
    expect(isOverlayEdge(0)).toBe(false)
    expect(isOverlayEdge({ edge: 'left' })).toBe(false)
  })
})

// 贴边紧凑胶囊的几何推演（用户拍板：贴边后麦克风图标必须**完整**在屏内、屏幕边缘刚好
// 托住图标，不能被切半）。坐标系取**窗口中线**为 0——0.5.4 中线落点保证吸附后该中线恰在
// 工作区（屏幕）边上，故屏内可见半区 = 窗口一半：左右 280/2 = 140px 充裕，上下 64/2 =
// 32px（正是紧凑胶囊 32px 的高度上限）。位移 = 边长/2 = 16px 把胶囊外缘压到中线上，
// 于是整只胶囊（图标 chip 居中、内缩 6px）完全落在屏内。
describe('docked compact capsule flush placement', () => {
  const EDGES = ['left', 'right', 'top', 'bottom'] as const
  /** overlay 常驻窗口（逻辑 px，零 resize 不变量）。 */
  const WINDOW = { w: 280, h: 64 }
  /** 图标 chip：`h-5 w-5`。 */
  const ICON = 20
  /** 贴齐位移绝对值 = 紧凑胶囊边长 / 2。 */
  const OFFSET = DOCK_COMPACT_PX / 2

  /** 解析 `-?translate-[xy]-N`（Tailwind spacing 格 = 4px）为轴向 + 有符号位移。 */
  const parseTranslate = (cls: string): { axis: 'x' | 'y'; px: number } => {
    const m = /^(-?)translate-([xy])-(\d+)$/.exec(cls)
    if (!m) throw new Error(`unparsable dock translate class: ${cls}`)
    return { axis: m[2] as 'x' | 'y', px: (m[1] === '-' ? -1 : 1) * Number(m[3]) * 4 }
  }

  it('offsets each edge by exactly half the capsule on the matching axis, pointing in-screen', () => {
    expect(DOCK_COMPACT_PX).toBe(32)
    for (const edge of EDGES) {
      const { axis, px } = parseTranslate(DOCK_TRANSLATE[edge])
      // 轴向：左右吸附动 x、上下吸附动 y。
      expect(axis).toBe(edge === 'left' || edge === 'right' ? 'x' : 'y')
      // 大小：恰好 边长/2 = 16px（外缘压中线 = 屏幕边）。
      expect(Math.abs(px)).toBe(OFFSET)
      // 符号：一律指向屏内（left/top 为正方向，right/bottom 为负方向）。
      expect(px).toBe(edge === 'left' || edge === 'top' ? OFFSET : -OFFSET)
    }
  })

  it('keeps the whole capsule and its icon chip inside the visible half-window on all four edges', () => {
    for (const edge of EDGES) {
      const { px } = parseTranslate(DOCK_TRANSLATE[edge])
      const vertical = edge === 'top' || edge === 'bottom'
      const axisWindow = vertical ? WINDOW.h : WINDOW.w
      const half = axisWindow / 2
      // 贴边前胶囊居中于中线，贴边后区间为 [px − 16, px + 16]。
      const outer = px - OFFSET
      const inner = px + OFFSET
      // 屏内可见半区（相对中线）：left/top 内侧为正向，right/bottom 为负向。
      const [visLo, visHi] = px > 0 ? [0, half] : [-half, 0]
      expect(outer).toBeGreaterThanOrEqual(visLo)
      expect(inner).toBeLessThanOrEqual(visHi)
      // 图标 chip（20px 居中）距屏幕边的余量 = 16 − 10 = 6px：即使 DPI 舍入 / 浏览器
      // 次像素布局让胶囊整体外移 ≤2 物理像素，图标仍 100% 在屏内（≥4px 余量）。
      const iconMargin = Math.abs(px) - ICON / 2
      expect(iconMargin).toBe(6)
      expect(iconMargin).toBeGreaterThanOrEqual(4)
      // 另一轴：胶囊居中 [−16, +16] 必须落在窗口半幅内（不被窗口裁切）。
      const otherAxis = vertical ? WINDOW.w : WINDOW.h
      expect(DOCK_COMPACT_PX).toBeLessThanOrEqual(otherAxis)
      // 上下贴边时贴齐端同时压在窗口内缘（32 = 64/2）：内容永不越出窗口矩形。
      expect(inner).toBeLessThanOrEqual(half)
      expect(outer).toBeGreaterThanOrEqual(-half)
    }
  })
})

// 0.5.7「拖过头也吸附」契约的渲染层一半：拖拽期间必须**原样**发出未钳制的目标坐标——
// 包括已经溢出屏幕边外的坐标（负值 / 超大值）。贴边判定（任一边接触或溢出都吸附）与
// 工作区钳制全部在 Rust `set_overlay_position` 里做；渲染层若先钳回屏内，Rust 永远看不到
// 溢出态，溢出吸附就无从成立。松手一律经 flush-move + persist-position 把落点交还 Rust 判定。
describe('drag emits raw unclamped positions (0.5.7 overflow snap contract)', () => {
  it('passes overflowed coordinates past the right edge through untouched', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    state = setWinStart(state, { x: 1800, y: 500 }, 1.0)
    // 越过拖拽阈值进入 dragging。
    ;[state] = onPointerMove(moveEvent(1, { x: 120, y: 100 }), state, 10)
    expect(state.phase).toBe('dragging')
    // 再拖 300px：目标 x = 1800 + 320 = 2120，远超任何屏幕右边——原样发出、零钳制。
    const [, cmds] = onPointerMove(moveEvent(1, { x: 420, y: 100 }), state, 20)
    expect(moveCmdPos(cmds)).toEqual({ x: 2120, y: 500 })
  })

  it('passes negative coordinates (past the left edge) through unchanged', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 400, y: 100 }), state, 0, 'idle')
    state = setWinStart(state, { x: 300, y: 200 }, 1.0)
    // 越过拖拽阈值进入 dragging。
    ;[state] = onPointerMove(moveEvent(1, { x: 380, y: 100 }), state, 10)
    expect(state.phase).toBe('dragging')
    // 目标 x = 300 + (−200 − 400) = −300（窗口已在屏外）——负坐标不得被钳成 0。
    const [, cmds] = onPointerMove(moveEvent(1, { x: -200, y: 100 }), state, 20)
    expect(moveCmdPos(cmds)).toEqual({ x: -300, y: 200 })
  })

  it('releases an overflowed drop through persist-position for Rust to judge', () => {
    let state = initialOverlayDragState()
    ;[state] = onPointerDown(downEvent(1, { x: 100, y: 100 }), state, 0, 'idle')
    state = setWinStart(state, { x: 1800, y: 500 }, 1.0)
    ;[state] = onPointerMove(moveEvent(1, { x: 420, y: 100 }), state, 10)
    expect(state.phase).toBe('dragging')
    const [nextState, cmds] = onPointerUp(upEvent(1, { x: 420, y: 100 }), state, 20)
    expect(nextState.phase).toBe('idle')
    // 溢出落点同样走 persist-position：吸附/钳制在 Rust，渲染层不自行处置。
    expect(cmdTypes(cmds)).toContain('persist-position')
    expect(cmdTypes(cmds)).toContain('flush-move')
    expect(hasCmd(cmds, 'cancel-session')).toBe(false)
    expect(hasCmd(cmds, 'start-session')).toBe(false)
  })
})
