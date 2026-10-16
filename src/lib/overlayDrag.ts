import type { OverlayStatus } from '@/shared/types'

/**
 * 纯函数模块：封装 overlay HUD 胶囊的拖拽 / 按住说话状态机。
 *
 * 设计目标：
 * - 把 pointerdown → pending → dragging → end 的判定逻辑与副作用分离，便于单元测试。
 * - 调用方（HUD.tsx）负责所有 DOM / Tauri 调用；本模块只返回命令描述。
 *
 * 主要语义：
 * - 待机态 pointerdown 同时启动 PTT 会话并进入 pending，位移未超过阈值时视为按住说话。
 * - 位移超过 DRAG_THRESHOLD_PX 则进入 dragging，取消会话并开始跟随指针移动窗口。
 * - pointerup 时若仍在 pending 且时间/位移都很小（几乎未移动），视为一次 tap：取消会话
 *   （避免 ~0 音频报错），并记录 lastTap；快速连续两次 tap 触发 onDoubleClick 重置位置。
 *   位移在拖拽阈值内但明显移动过的正常松开仍走 stop-session。
 * - 拖拽期间抑制 setOverlaySize，防止窗口跳回持久化位置；释放后恢复并 flush 测量。
 */

/** 拖拽意图判定阈值（像素）。超过此位移视为拖拽，否则视为按住说话。 */
export const DRAG_THRESHOLD_PX = 8

/** 两次 tap 构成双击重置的最大间隔（毫秒）。 */
export const DOUBLE_CLICK_MAX_MS = 300

/** tap 判定的最大按住时长（毫秒）。 */
export const TAP_MAX_MS = 200

/** tap 判定的最大位移（像素）。必须远小于 DRAG_THRESHOLD_PX，避免正常松开被误判为 tap。 */
export const TAP_DISTANCE_PX = 4

/**
 * 看门狗超时（毫秒）。拖拽或 pending 状态下若超过此时间未收到任何指针事件，
 * 强制复位到 idle，防止 pointerup 丢失导致状态卡死。
 */
export const WATCHDOG_MAX_MS = 3000

export type DragPhase = 'idle' | 'pending' | 'dragging'

export interface ScreenPoint {
  x: number
  y: number
}

export interface PhysicalPoint {
  x: number
  y: number
}

export interface OverlayDragState {
  phase: DragPhase
  pointerId: number | null
  startScreen: ScreenPoint | null
  startTime: number
  winStart: PhysicalPoint | null
  scaleFactor: number | null
  lastTap: { point: ScreenPoint; time: number } | null
  watchdogAt: number | null
  resizeSuppressed: boolean
}

export type DragCommand =
  | { type: 'start-session' }
  | { type: 'stop-session' }
  | { type: 'cancel-session' }
  | { type: 'set-overlay-position'; x: number | null; y: number | null }
  | { type: 'capture-pointer'; pointerId: number }
  | { type: 'release-pointer-capture'; pointerId: number }
  | { type: 'add-listeners' }
  | { type: 'remove-listeners' }
  | { type: 'start-watchdog'; deadline: number }
  | { type: 'clear-watchdog' }
  | { type: 'suppress-resize' }
  | { type: 'allow-resize' }
  | { type: 'move'; pos: PhysicalPoint }
  | { type: 'flush-move' }
  | { type: 'persist-position' }

export function initialOverlayDragState(): OverlayDragState {
  return {
    phase: 'idle',
    pointerId: null,
    startScreen: null,
    startTime: 0,
    winStart: null,
    scaleFactor: null,
    lastTap: null,
    watchdogAt: null,
    resizeSuppressed: false,
  }
}

function distance(a: ScreenPoint, b: ScreenPoint): number {
  return Math.hypot(a.x - b.x, a.y - b.y)
}

function isTap(state: Readonly<OverlayDragState>, endScreen: ScreenPoint, now: number): boolean {
  if (!state.startScreen) return false
  if (now - state.startTime > TAP_MAX_MS) return false
  return distance(state.startScreen, endScreen) <= TAP_DISTANCE_PX
}

export function onPointerDown(
  event: { button: number; pointerId: number; screenX: number; screenY: number },
  state: Readonly<OverlayDragState>,
  now: number,
  status: OverlayStatus,
): [OverlayDragState, DragCommand[]] {
  if (event.button !== 0 && event.button !== 1) return [state, []]
  if (status !== 'idle') return [state, []]
  if (state.phase !== 'idle') return [state, []]

  const deadline = now + WATCHDOG_MAX_MS
  const nextState: OverlayDragState = {
    ...state,
    phase: 'pending',
    pointerId: event.pointerId,
    startScreen: { x: event.screenX, y: event.screenY },
    startTime: now,
    winStart: null,
    scaleFactor: null,
    lastTap: null,
    watchdogAt: deadline,
    resizeSuppressed: false,
  }

  return [
    nextState,
    [
      { type: 'start-session' },
      { type: 'capture-pointer', pointerId: event.pointerId },
      { type: 'add-listeners' },
      { type: 'start-watchdog', deadline },
    ],
  ]
}

export function setWinStart(
  state: Readonly<OverlayDragState>,
  winStart: PhysicalPoint,
  scaleFactor: number,
): OverlayDragState {
  return { ...state, winStart, scaleFactor }
}

function computeDragPosition(
  state: Readonly<OverlayDragState>,
  event: { screenX: number; screenY: number },
): PhysicalPoint | null {
  if (!state.startScreen || state.winStart === null || state.scaleFactor === null) return null
  const x = state.winStart.x + Math.round((event.screenX - state.startScreen.x) * state.scaleFactor)
  const y = state.winStart.y + Math.round((event.screenY - state.startScreen.y) * state.scaleFactor)
  return { x, y }
}

export function onPointerMove(
  event: { pointerId: number; screenX: number; screenY: number },
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle') return [state, []]
  if (state.pointerId !== null && event.pointerId !== state.pointerId) return [state, []]

  const deadline = now + WATCHDOG_MAX_MS

  if (state.phase === 'pending') {
    if (!state.startScreen) return [state, []]
    if (distance(state.startScreen, { x: event.screenX, y: event.screenY }) <= DRAG_THRESHOLD_PX) {
      return [{ ...state, watchdogAt: deadline }, [{ type: 'start-watchdog', deadline }]]
    }

    const nextState: OverlayDragState = {
      ...state,
      phase: 'dragging',
      watchdogAt: deadline,
      resizeSuppressed: true,
    }
    const cmds: DragCommand[] = [
      { type: 'cancel-session' },
      { type: 'suppress-resize' },
      { type: 'start-watchdog', deadline },
    ]

    const pos = computeDragPosition(state, event)
    if (pos) {
      cmds.push({ type: 'move', pos })
    }

    return [nextState, cmds]
  }

  const pos = computeDragPosition(state, event)
  if (!pos) {
    return [{ ...state, watchdogAt: deadline }, [{ type: 'start-watchdog', deadline }]]
  }

  return [
    { ...state, watchdogAt: deadline },
    [
      { type: 'move', pos },
      { type: 'start-watchdog', deadline },
    ],
  ]
}

export function onPointerUp(
  event: { pointerId: number; screenX: number; screenY: number },
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle') return [state, []]
  if (state.pointerId !== null && event.pointerId !== state.pointerId) return [state, []]

  const cmds: DragCommand[] = []
  if (state.pointerId !== null) {
    cmds.push({ type: 'release-pointer-capture', pointerId: state.pointerId })
  }
  cmds.push({ type: 'remove-listeners' }, { type: 'clear-watchdog' })

  if (state.phase === 'dragging') {
    cmds.push({ type: 'flush-move' }, { type: 'allow-resize' }, { type: 'persist-position' })
    return [initialOverlayDragState(), cmds]
  }

  if (isTap(state, { x: event.screenX, y: event.screenY }, now)) {
    cmds.push({ type: 'cancel-session' })
    const nextState = initialOverlayDragState()
    if (state.startScreen) {
      nextState.lastTap = { point: state.startScreen, time: now }
    }
    return [nextState, cmds]
  }

  cmds.push({ type: 'stop-session' })
  return [initialOverlayDragState(), cmds]
}

export function onPointerCancel(
  event: { pointerId: number },
  state: Readonly<OverlayDragState>,
  _now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle') return [state, []]
  if (state.pointerId !== null && event.pointerId !== state.pointerId) return [state, []]

  const cmds: DragCommand[] = []
  if (state.pointerId !== null) {
    cmds.push({ type: 'release-pointer-capture', pointerId: state.pointerId })
  }
  cmds.push({ type: 'cancel-session' }, { type: 'remove-listeners' }, { type: 'clear-watchdog' })
  if (state.phase === 'dragging') {
    cmds.push({ type: 'allow-resize' })
  }
  return [initialOverlayDragState(), cmds]
}

export function onWindowBlur(state: Readonly<OverlayDragState>): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle') return [state, []]

  const cmds: DragCommand[] = []
  if (state.pointerId !== null) {
    cmds.push({ type: 'release-pointer-capture', pointerId: state.pointerId })
  }
  cmds.push({ type: 'cancel-session' }, { type: 'remove-listeners' }, { type: 'clear-watchdog' })
  if (state.phase === 'dragging') {
    cmds.push({ type: 'allow-resize' })
  }
  return [initialOverlayDragState(), cmds]
}

export function onWatchdog(
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle' || state.watchdogAt === null) return [state, []]
  if (now < state.watchdogAt) return [state, []]

  const cmds: DragCommand[] = []
  if (state.pointerId !== null) {
    cmds.push({ type: 'release-pointer-capture', pointerId: state.pointerId })
  }
  cmds.push({ type: 'cancel-session' }, { type: 'remove-listeners' }, { type: 'clear-watchdog' })
  if (state.phase === 'dragging') {
    cmds.push({ type: 'allow-resize' })
  }
  return [initialOverlayDragState(), cmds]
}

export function onDoubleClick(
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (!state.lastTap) return [state, []]
  if (now - state.lastTap.time > DOUBLE_CLICK_MAX_MS) return [state, []]

  return [{ ...state, lastTap: null }, [{ type: 'set-overlay-position', x: null, y: null }]]
}
