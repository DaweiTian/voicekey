import type { OverlayStatus } from '@/shared/types'

/**
 * 纯函数模块：封装 overlay HUD 胶囊的拖拽 / 点按切换 / 按住说话状态机。
 *
 * 设计目标：
 * - 把 pointerdown → pending → dragging / holding → end 的判定逻辑与副作用分离，便于单元测试。
 * - 调用方（HUD.tsx）负责所有 DOM / Tauri 调用；本模块只返回命令描述。
 *
 * 会话语义（0.5.3 重设计，用户拍板）：
 * - **轻点 = 切换（latch）**：待机态一次短按（松开时仍未到 HOLD_LATCH_MAX_MS、也未越过
 *   拖拽阈值）→ 发出 start-session 但**松手不 stop**：胶囊进入持续录音态，说完再点一下
 *   （此时 status==='recording' 且本状态机 latched===true）→ stop-session 正常识别。
 * - **按住 = PTT**：按下后停留到 HOLD_LATCH_MAX_MS（onHoldTimer）且未越过拖拽阈值 →
 *   发 start-session 开始录音，松手 → stop-session。
 * - **拖拽 = 只移窗**：pending 期间位移超过 DRAG_THRESHOLD_PX → dragging，跟随指针移动
 *   窗口；**全程不启动会话**（见下方 REGRESSION WARNING）。
 * - **双击复位保留**：第 1 击 latch 启动会话；第 2 击在 DOUBLE_CLICK_MAX_MS 内、且落点
 *   与前一次轻点相距 ≤ TAP_DISTANCE_PX → cancel-session（静默，无空录音报错），随后浏览器
 *   dblclick 调用 onDoubleClick 复位位置。正好是「点错了撤销」。
 *
 * REGRESSION WARNING（0.5.2 真机缺陷根因，勿回退）：
 * 旧设计在 **pointerdown 就发 start-session**。会话启动会让 HUD 从待机胶囊切到录音胶囊并
 * 触发 `setOverlaySize` → `set_size` + `position_overlay` 重新锚定窗口。这次**在手势进行中**
 * 发生的窗口尺寸 / 位置变化会打断指针手势（Windows/WebView2 上表现为 pointercancel 或
 * 捕获丢失），状态机在到达 8px 拖拽阈值前就被指针取消路径复位成 idle、移除全局监听并
 * cancel-session；于是后续 pointermove 再也进不了 dragging，「拖不动」，同时胶囊闪一下
 * （待机→录音→待机）。单测全绿是因为纯函数从不模拟窗口 resize。**因此 pending 阶段绝不
 * 启动会话、绝不 resize；只有手势歧义消除后（tap / hold）才发 start-session。**
 *
 * latched 会话的存活性不变量：`onPointerCancel` / `onWindowBlur` / `onWatchdog` 的
 * cancel-session **只作用于「指针仍按下」的活跃 pending/holding/dragging 手势**。已松手的
 * latched 会话其 phase 恒为 idle，上述复位路径对 idle 早退，因此点其他应用、窗口失焦或看门狗
 * 都不会误杀 latch 录音。这是在每个 handler 上都必须保持的正确性点。
 *
 * 拖拽期间抑制 setOverlaySize，防止窗口跳回持久化位置；释放后恢复并 flush 测量。
 */

/** 拖拽意图判定阈值（像素）。pending 期间超过此位移视为拖拽，否则视为轻点 / 按住。 */
export const DRAG_THRESHOLD_PX = 8

/** 两次轻点构成双击重置的最大间隔（毫秒）。 */
export const DOUBLE_CLICK_MAX_MS = 300

/**
 * 「轻点（latch 切换）」与「按住（PTT）」的分界时长（毫秒）。按下后在此时刻仍未被松手
 * （也未被拖拽判定接管）→ onHoldTimer 触发，进入 holding 并开始录音。松手早于此时刻 →
 * 视为轻点，latch 启动并保持录音。与 DRAG_THRESHOLD_PX（空间）、DOUBLE_CLICK_MAX_MS
 * （双击间隔）语义相互独立，切勿互相复用。
 */
export const HOLD_LATCH_MAX_MS = 500

/** 轻点判定的最大位移（像素）。必须远小于 DRAG_THRESHOLD_PX，同时用于判定两次轻点
 *  是否落在同一处（双击复位）。 */
export const TAP_DISTANCE_PX = 4

/**
 * 看门狗超时（毫秒）。仅防 pending / dragging 状态因 pointerup 丢失而卡死；
 * holding 与 latched 会话在进入时即清除看门狗，**绝不被看门狗杀掉**。
 */
export const WATCHDOG_MAX_MS = 3000

/** latched 会话静音自动停止的判定时长（毫秒）。 */
export const SILENCE_AUTO_STOP_MS = 5000

/**
 * 静音判定的音频电平阈值（AudioRecorder 上报的 0..1 归一化电平）。低于此值视为静音。
 * 取值保守（偏低）以避免把正常说话误判为静音而提前切断；现场若环境噪声偏高需要上调。
 */
export const SILENCE_LEVEL_THRESHOLD = 0.02

export type DragPhase = 'idle' | 'pending' | 'dragging' | 'holding'

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
  /** 本次手势是否起于一个已 latch 的录音（决定一次轻点是「启动 latch」还是「停止识别」）。 */
  startedLatched: boolean
  /**
   * 当前是否存在「已松手、仍在持续录音」的 latch 会话。手势结束后仍为 true，是
   * latched 会话豁免 pointercancel / blur / 看门狗的内在依据（见模块头注释）。
   */
  latched: boolean
  lastTap: { point: ScreenPoint; time: number } | null
  watchdogAt: number | null
  /** onHoldTimer 的到期时刻（pending 期间用于区分轻点与按住）。 */
  holdDeadline: number | null
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
  | { type: 'start-hold-timer'; deadline: number }
  | { type: 'clear-hold-timer' }
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
    startedLatched: false,
    latched: false,
    lastTap: null,
    watchdogAt: null,
    holdDeadline: null,
    resizeSuppressed: false,
  }
}

/** 手势结束：回到 idle，并保留 latched / lastTap（跨手势存在的持久字段）。 */
function endGesture(state: Readonly<OverlayDragState>): OverlayDragState {
  return { ...initialOverlayDragState(), latched: state.latched, lastTap: state.lastTap }
}

/** 手势结束且会话被停止 / 取消：latched 一并清空。 */
function endGestureStopped(): OverlayDragState {
  return initialOverlayDragState()
}

/**
 * latched 静音自动停止的纯判定：仅在 latched 会话、且距最后一次探测到声音
 * 已超过 SILENCE_AUTO_STOP_MS 时为真。
 */
export function isSilenceAutoStopDue(input: {
  latched: boolean
  lastVoiceAt: number
  now: number
}): boolean {
  if (!input.latched) return false
  return input.now - input.lastVoiceAt >= SILENCE_AUTO_STOP_MS
}

function distance(a: ScreenPoint, b: ScreenPoint): number {
  return Math.hypot(a.x - b.x, a.y - b.y)
}

export function onPointerDown(
  event: { button: number; pointerId: number; screenX: number; screenY: number },
  state: Readonly<OverlayDragState>,
  now: number,
  status: OverlayStatus,
): [OverlayDragState, DragCommand[]] {
  if (event.button !== 0 && event.button !== 1) return [state, []]
  if (state.phase !== 'idle') return [state, []]

  // 待机态开始一次新手势；或对已 latch 的录音「再点一下」开始停止手势。
  // 录音 / 处理 / 成功 / 错误态（非 latched）一律不响应。
  const canBegin = status === 'idle' || (status === 'recording' && state.latched)
  if (!canBegin) return [state, []]

  const watchdog = now + WATCHDOG_MAX_MS
  const holdDeadline = now + HOLD_LATCH_MAX_MS
  const nextState: OverlayDragState = {
    ...state,
    phase: 'pending',
    pointerId: event.pointerId,
    startScreen: { x: event.screenX, y: event.screenY },
    startTime: now,
    winStart: null,
    scaleFactor: null,
    startedLatched: state.latched,
    // `lastTap` deliberately survives the next pointerdown: the second press of a
    // double-click may be rejected by Rust ("Processing in progress") while the
    // first click's session is still transcribing, so the dblclick handler must
    // still see the previous tap to perform the reset.
    watchdogAt: watchdog,
    holdDeadline,
    // 手势期间冻结窗口尺寸：pending 不启动会话，本意已无 resize，但显式抑制可防止
    // 任何 React 副作用在手势中途触发 setOverlaySize 而重新锚定窗口、打断指针。
    resizeSuppressed: true,
  }

  return [
    nextState,
    [
      { type: 'capture-pointer', pointerId: event.pointerId },
      { type: 'add-listeners' },
      { type: 'start-watchdog', deadline: watchdog },
      { type: 'start-hold-timer', deadline: holdDeadline },
      { type: 'suppress-resize' },
    ],
  ]
}

/**
 * 按住阈值定时器到期（调用方在 HOLD_LATCH_MAX_MS 后触发）。
 * - pending（指针仍按下、未越拖拽阈值，且本次手势起于待机）→ 进入 holding，开始 PTT 录音。
 * - 本次手势起于 latched 会话 → 不改变会话（已在录音），仅清掉定时器。
 * - 其它 phase（已拖拽 / 已松手）→ 无操作。
 */
export function onHoldTimer(
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase !== 'pending') return [state, []]
  if (state.holdDeadline === null || now < state.holdDeadline) return [state, []]

  if (state.startedLatched) {
    return [{ ...state, holdDeadline: null }, [{ type: 'clear-hold-timer' }]]
  }

  // 进入按住录音：看门狗不再需要（无拖拽在途），清除以免误杀长按会话。
  return [
    { ...state, phase: 'holding', holdDeadline: null, watchdogAt: null },
    [
      { type: 'start-session' },
      { type: 'allow-resize' },
      { type: 'clear-hold-timer' },
      { type: 'clear-watchdog' },
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

    // 越过拖拽阈值：进入 dragging，跟随指针移动窗口。**不启动、也不取消会话**——
    // pending 阶段本就没有会话；若起于 latched 会话，拖拽只移窗、录音继续。
    const nextState: OverlayDragState = {
      ...state,
      phase: 'dragging',
      watchdogAt: deadline,
      holdDeadline: null,
      resizeSuppressed: true,
    }
    const cmds: DragCommand[] = [
      { type: 'suppress-resize' },
      { type: 'start-watchdog', deadline },
      { type: 'clear-hold-timer' },
    ]

    const pos = computeDragPosition(state, event)
    if (pos) {
      cmds.push({ type: 'move', pos })
    }

    return [nextState, cmds]
  }

  if (state.phase === 'holding') {
    // 已进入按住录音：录音期间不再跟随拖拽，避免窗口乱动打断说话。
    return [state, []]
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
  cmds.push({ type: 'remove-listeners' }, { type: 'clear-watchdog' }, { type: 'clear-hold-timer' })

  if (state.phase === 'dragging') {
    // 拖拽结束：落位、持久化、恢复 resize。会话状态（latched / lastTap）原样保留。
    cmds.push({ type: 'flush-move' }, { type: 'persist-position' }, { type: 'allow-resize' })
    return [endGesture(state), cmds]
  }

  if (state.phase === 'holding') {
    // 按住录音松开：正常停止并识别。
    cmds.push({ type: 'stop-session' }, { type: 'allow-resize' })
    return [endGestureStopped(), cmds]
  }

  // pending：本次手势结束。先判双击（第 2 击复位，静默取消），再看是否停 latch，否则轻点 latch。
  cmds.push({ type: 'allow-resize' })

  const prevTap = state.lastTap
  const isDoubleTap =
    prevTap !== null &&
    now - prevTap.time <= DOUBLE_CLICK_MAX_MS &&
    distance(prevTap.point, { x: event.screenX, y: event.screenY }) <= TAP_DISTANCE_PX

  if (isDoubleTap) {
    // 双击复位：静默取消第 1 击刚 latch 起来的会话，随后 onDoubleClick 复位位置。
    // 保留 lastTap 供 onDoubleClick 判定（不可在此清空）。
    cmds.push({ type: 'cancel-session' })
    const next = endGestureStopped()
    next.lastTap = prevTap
    return [next, cmds]
  }

  if (state.startedLatched) {
    // 在已 latch 的录音上再点一下：停止并识别。
    cmds.push({ type: 'stop-session' })
    return [endGestureStopped(), cmds]
  }

  // 轻点：latch 启动，**松手不 stop**——录音持续，直到用户再点一下。
  cmds.push({ type: 'start-session' })
  const next = endGestureStopped()
  next.latched = true
  if (state.startScreen) {
    next.lastTap = { point: state.startScreen, time: now }
  }
  return [next, cmds]
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
  cmds.push({ type: 'remove-listeners' }, { type: 'clear-watchdog' }, { type: 'clear-hold-timer' })

  if (state.phase === 'holding') {
    // 指针被系统取消时正在按住录音：保存已录到的音频而不是丢弃（安全优先）。
    cmds.push({ type: 'stop-session' })
  }
  // pending（尚未启动会话）与 dragging 都无需 cancel——latched 会话必须原样保留。
  cmds.push({ type: 'allow-resize' })
  return [endGesture(state), cmds]
}

export function onWindowBlur(state: Readonly<OverlayDragState>): [OverlayDragState, DragCommand[]] {
  // 仅作用于活跃手势；latched 会话（phase==='idle'）在此早退，不被失焦误杀。
  if (state.phase === 'idle') return [state, []]
  return onPointerCancel({ pointerId: state.pointerId ?? -1 }, state, 0)
}

export function onWatchdog(
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (state.phase === 'idle' || state.watchdogAt === null) return [state, []]
  if (now < state.watchdogAt) return [state, []]
  // 看门狗只防 pending / dragging 卡死；holding 已在进入时清除看门狗，这里再兜一层。
  if (state.phase === 'holding') return [state, []]

  const cmds: DragCommand[] = []
  if (state.pointerId !== null) {
    cmds.push({ type: 'release-pointer-capture', pointerId: state.pointerId })
  }
  cmds.push({ type: 'remove-listeners' }, { type: 'clear-watchdog' }, { type: 'clear-hold-timer' })
  cmds.push({ type: 'allow-resize' })
  return [endGesture(state), cmds]
}

export function onDoubleClick(
  state: Readonly<OverlayDragState>,
  now: number,
): [OverlayDragState, DragCommand[]] {
  if (!state.lastTap) return [state, []]
  if (now - state.lastTap.time > DOUBLE_CLICK_MAX_MS) return [state, []]

  return [{ ...state, lastTap: null }, [{ type: 'set-overlay-position', x: null, y: null }]]
}
