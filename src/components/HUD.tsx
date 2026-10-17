import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { getCurrentWindow, type Window as TauriWindow } from '@tauri-apps/api/window'
import { PhysicalPosition } from '@tauri-apps/api/dpi'
import { Mic } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { OverlayEdge, OverlayState, OverlayStatus } from '@/shared/types'
import { cn } from '../lib/utils'
import {
  DOCK_TRANSLATE,
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
  SILENCE_LEVEL_THRESHOLD,
  type DragCommand,
  type DragPhase,
  type OverlayDragState,
  type PhysicalPoint,
} from '../lib/overlayDrag'

export function HUD() {
  const { t } = useTranslation()
  const [overlayState, setOverlayState] = useState<OverlayState>({ status: 'idle' })
  const [isVisible, setIsVisible] = useState(false)
  /** 手势阶段：用于给待机胶囊提供显式按压反馈（pending / holding 时收缩）。 */
  const [gesturePhase, setGesturePhase] = useState<DragPhase>('idle')
  /**
   * 贴边吸附（docked）的**边**：`null` = 未吸附（待机胶囊全形：图标 + `holdToTalk`），
   * `'left' | 'right' | 'top' | 'bottom'` = 已吸附（紧凑形态：仅图标，且按边贴齐屏幕
   * 中线，整只落在屏内）。两个来源：
   * ① 挂载时读持久化配置 `app.overlayDocked`（重启后仍保持贴边形态）；
   * ② 每次 `setOverlayPosition` 的返回值（拖拽松手 persist / 双击复位）。
   * 只改胶囊自身 DOM 形态与 transform——窗口常驻 280×64，绝不触碰窗口几何。
   */
  const [dockEdge, setDockEdge] = useState<OverlayEdge | null>(null)
  /** 紧凑形态开关（待机胶囊渲染用）。 */
  const docked = dockEdge !== null

  // RAF cleanup ref
  const rafIdRef = useRef<number | null>(null)
  /**
   * 会话「松手后是否仍在录音」标记（latch 语义）。true = 一次轻点启动的持续录音，
   * 需要通过「再点一下」或静音自动停止来结束；此时 pointercancel / blur 均不得杀死它。
   * 由拖拽状态机的 latched 字段驱动（见 applyDragResult）。
   */
  const latchedRef = useRef(false)
  /** 轻点 latch 起算 / 最后一次探测到声音的时刻，供静音自动停止判定使用。 */
  const lastVoiceAtRef = useRef(0)
  /**
   * 按住说话的所有权标记：只有本胶囊（经状态机命令）成功发起的会话才由本胶囊结束时停止。
   * 现在的 stop/cancel 全部经状态机命令执行，不再是 React 冒泡处理，故标记仅作对称保护。
   */
  const holdActiveRef = useRef(false)
  /** 与 overlayState 同步，但可在事件回调里同步读取（避免闭包过期）。 */
  const statusRef = useRef<OverlayStatus>('idle')
  /**
   * 根容器 ref：与 pointerdown 的 `setPointerCapture` 同一个元素，供 releasePointerCapture
   * 对称释放。窗口尺寸常驻不变（见下），已无「内容测量容器」（无 resize 测量回路）。
   */
  const rootRef = useRef<HTMLDivElement | null>(null)
  /**
   * 胶囊布局盒 ref（包住当前状态胶囊的 shrink-wrap 容器）。仅在拖拽松手时做**一次性只读**
   * offsetWidth/offsetHeight 测量，把可见胶囊实际尺寸带给 Rust 做「胶囊贴边」吸附；
   * 不参与任何 resize / 重绘回路，也不在手势中触发几何变化。
   */
  const capsuleBoxRef = useRef<HTMLDivElement | null>(null)

  /** 当前 Tauri overlay 窗口句柄（缓存以避免重复创建）。 */
  const windowRef = useRef<TauriWindow | null>(null)

  /** 拖拽状态机纯函数状态。 */
  const dragRef = useRef<OverlayDragState>(initialOverlayDragState())
  /** 拖拽移动节流：是否已有待处理的 rAF。 */
  const dragMovePendingRef = useRef(false)
  /** 拖拽移动节流：待下发的最新坐标。 */
  const dragPendingPosRef = useRef<PhysicalPoint | null>(null)

  /** 当前挂载的全局 pointermove / pointerup / pointercancel / blur 监听器。 */
  const currentMoveHandlerRef = useRef<((event: PointerEvent) => void) | undefined>(undefined)
  const currentUpHandlerRef = useRef<((event: PointerEvent) => void) | undefined>(undefined)
  const currentCancelHandlerRef = useRef<((event: PointerEvent) => void) | undefined>(undefined)
  const currentBlurHandlerRef = useRef<(() => void) | undefined>(undefined)

  /** 看门狗定时器：拖拽 / pending 状态下 N 秒无事件则强制复位。 */
  const watchdogTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  /** 按住阈值定时器：按下 HOLD_LATCH_MAX_MS 后仍未松手 / 未拖拽则进入 PTT。 */
  const holdTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  /** 命令执行器 ref：在渲染期间保持最新，供全局事件处理器读取。 */
  const executeDragCommandsRef = useRef<(cmds: DragCommand[]) => void>(() => {})

  /**
   * 应用状态机的输出：同步 dragRef / latchedRef、在 latch 起点重置静音计时，再执行命令。
   * 所有状态机入口（pointerdown / 全局 move/up/cancel/blur / 看门狗 / 按住定时器）都经此，
   * 保证 latchedRef 与 dragRef 永远一致——这是「latched 会话豁免误杀」的正确性前提。
   */
  const applyDragResult = (nextState: OverlayDragState, cmds: DragCommand[]) => {
    const wasLatched = dragRef.current.latched
    dragRef.current = nextState
    latchedRef.current = nextState.latched
    // pending/holding 阶段让待机胶囊显式收缩，提供跟手的按压反馈；
    // dragging 时立即释放按压态，避免拖拽过程中仍显示按下。
    setGesturePhase(nextState.phase)
    if (nextState.latched && !wasLatched) {
      lastVoiceAtRef.current = Date.now()
    }
    // 拖拽一经判定（进入 dragging）立即还原全形：给「已拖出吸附」的即时视觉反馈，
    // 也保证松手 persist 测到的是全形胶囊尺寸（与 Rust 吸附口径一致）。只切胶囊 DOM
    // 分支与贴边位移（transform 归零自然过渡回居中）、不改窗口几何（常驻 280×64），
    // 不触发 setOverlaySize 类调用，与手势期几何不变量无冲突；松手 persist 的返回值
    // 随后重新定夺 dockEdge（贴回边 → 再收缩 + 再贴齐）。
    if (nextState.phase === 'dragging') setDockEdge(null)
    executeDragCommandsRef.current(cmds)
  }

  const getOverlayWindow = useCallback((): TauriWindow => {
    if (!windowRef.current) {
      windowRef.current = getCurrentWindow()
    }
    return windowRef.current
  }, [])

  useEffect(() => {
    document.documentElement.classList.add('overlay-html')
    rafIdRef.current = requestAnimationFrame(() => setIsVisible(true))
    return () => {
      if (rafIdRef.current !== null) {
        cancelAnimationFrame(rafIdRef.current)
      }
      removeDragListeners()
      clearWatchdog()
      clearHoldTimer()
      document.documentElement.classList.remove('overlay-html')
    }
  }, [])

  /**
   * 启动时恢复贴边状态：读 `app.overlayDocked`（Rust 在每次 persist / 复位时写入），
   * 让重启后的胶囊直接以紧凑形态出现，而不是先展开再等下一次拖拽。读取是纯异步的
   * 一次状态赋值，失败静默保持默认全形；形态过渡只发生在胶囊自身（窗口几何不变，
   * Rust 侧 `position_overlay_persisted` 同时按原坐标复位窗口）。
   */
  useEffect(() => {
    let cancelled = false
    window.electronAPI
      .getConfig()
      .then((config) => {
        if (!cancelled) {
          const stored = config.app.overlayDocked
          setDockEdge(isOverlayEdge(stored) ? stored : null)
        }
      })
      .catch(() => {
        // 配置读取失败不阻塞 HUD：保持全形，下一次拖拽 persist 会纠正
      })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    const removeOverlayUpdateListener = window.electronAPI.onOverlayUpdate(
      (state: OverlayState) => {
        setOverlayState(state)
      },
    )

    const removeAudioLevelListener = window.electronAPI.onAudioLevel((level: number) => {
      // 0.5.6 起波形 / 呼吸圆章移至 session 窗（SessionHUD）：本窗不再渲染电平，
      // 只保留「只要有可闻声音就刷新最后说话时刻」——静音自动停止据此判定。
      if (level >= SILENCE_LEVEL_THRESHOLD) {
        lastVoiceAtRef.current = Date.now()
      }
    })

    return () => {
      removeOverlayUpdateListener?.()
      removeAudioLevelListener?.()
    }
  }, [])

  useEffect(() => {
    statusRef.current = overlayState.status
    // 会话一旦离开录音态（正常停止 / 被 Rust 侧结束），latch 标记必须复位，否则后续轻点
    // 会被误当成「停止一个已不存在的 latch 会话」而永远无效。仅在无手势在途时清除：
    // 若用户正按下（pending），让手势自己解析（startedLatched 反映手势起点，提前清掉会
    // 让松手被误判成一次新的 latch 启动）。
    if (overlayState.status !== 'recording' && dragRef.current.phase === 'idle') {
      latchedRef.current = false
      dragRef.current = { ...dragRef.current, latched: false, startedLatched: false }
    }
    // 窗口常驻 tauri.conf.json 的 280×64，不随状态缩放——「手势 / 会话期间窗口几何
    // 不可变」硬不变量的实现基础（见 overlayDrag.ts 模块头 REGRESSION WARNING）。
    // 0.5.6 解耦后本窗只渲染待机胶囊（点按切换 / 按住说话 / 拖拽），会话态大胶囊
    // 移至独立 session 窗（SessionHUD）；整窗仍是胶囊热区、无死区、始终可点击。
    // 不做 hover 穿透：Tauri `set_ignore_cursor_events` 只有 bool、无 Electron 式
    // forward 转发，一旦穿透 mouseenter 不再可达，胶囊会永久点不到。
    window.electronAPI.setIgnoreMouseEvents(false)
  }, [overlayState.status])

  // latched 会话的静音自动停止：仅对 latch（已松手持续录音）生效，PTT 按住不受影响。
  useEffect(() => {
    const id = setInterval(() => {
      if (
        !isSilenceAutoStopDue({
          latched: latchedRef.current,
          lastVoiceAt: lastVoiceAtRef.current,
          now: Date.now(),
        })
      ) {
        return
      }
      latchedRef.current = false
      dragRef.current = { ...dragRef.current, latched: false, startedLatched: false }
      holdActiveRef.current = false
      window.electronAPI.stopSession().catch(() => {})
    }, 1000)
    return () => clearInterval(id)
  }, [])

  /**
   * 启动会话（start-session 命令）。轻点 latch 与按住 PTT 都经此；会话结束由 stop/cancel 负责。
   * 状态机已保证只会在待机（或已 latch 会话的「停止」手势）语境下发出该命令。
   */
  const handleHoldStart = () => {
    if (holdActiveRef.current) return
    holdActiveRef.current = true
    window.electronAPI.startSession().catch(() => {
      holdActiveRef.current = false
    })
  }

  /** 停止会话并进入识别（stop-session 命令）。 */
  const handleHoldEnd = () => {
    if (!holdActiveRef.current) return
    holdActiveRef.current = false
    window.electronAPI.stopSession().catch(() => {})
  }

  /** 取消会话（cancel-session 命令 / 取消按钮）：静默丢弃，同时清掉 latch 标记。 */
  const handleCancel = () => {
    holdActiveRef.current = false
    latchedRef.current = false
    dragRef.current = { ...dragRef.current, latched: false, startedLatched: false }
    window.electronAPI.cancelSession()
  }

  /** 释放 pointer capture；失败时静默忽略（window 监听器仍可兜底）。 */
  const releasePointerCapture = (pointerId: number) => {
    try {
      rootRef.current?.releasePointerCapture(pointerId)
    } catch {
      // ignore
    }
  }

  /** 挂载全局指针 / blur 监听器。 */
  const addDragListeners = () => {
    removeDragListeners()

    const move = (event: PointerEvent) => {
      const [nextState, cmds] = onPointerMove(
        { pointerId: event.pointerId, screenX: event.screenX, screenY: event.screenY },
        dragRef.current,
        Date.now(),
      )
      applyDragResult(nextState, cmds)
    }
    const up = (event: PointerEvent) => {
      const [nextState, cmds] = onPointerUp(
        { pointerId: event.pointerId, screenX: event.screenX, screenY: event.screenY },
        dragRef.current,
        Date.now(),
      )
      applyDragResult(nextState, cmds)
    }
    const cancel = (event: PointerEvent) => {
      const [nextState, cmds] = onPointerCancel(
        { pointerId: event.pointerId },
        dragRef.current,
        Date.now(),
      )
      applyDragResult(nextState, cmds)
    }
    const blur = () => {
      const [nextState, cmds] = onWindowBlur(dragRef.current)
      applyDragResult(nextState, cmds)
    }

    currentMoveHandlerRef.current = move
    currentUpHandlerRef.current = up
    currentCancelHandlerRef.current = cancel
    currentBlurHandlerRef.current = blur

    window.addEventListener('pointermove', move, true)
    window.addEventListener('pointerup', up, true)
    window.addEventListener('pointercancel', cancel, true)
    window.addEventListener('blur', blur, true)
  }

  /** 卸载全局指针 / blur 监听器。 */
  const removeDragListeners = () => {
    if (currentMoveHandlerRef.current) {
      window.removeEventListener('pointermove', currentMoveHandlerRef.current, true)
      currentMoveHandlerRef.current = undefined
    }
    if (currentUpHandlerRef.current) {
      window.removeEventListener('pointerup', currentUpHandlerRef.current, true)
      currentUpHandlerRef.current = undefined
    }
    if (currentCancelHandlerRef.current) {
      window.removeEventListener('pointercancel', currentCancelHandlerRef.current, true)
      currentCancelHandlerRef.current = undefined
    }
    if (currentBlurHandlerRef.current) {
      window.removeEventListener('blur', currentBlurHandlerRef.current, true)
      currentBlurHandlerRef.current = undefined
    }
  }

  /**
   * 把当前窗口物理坐标持久化到 Rust（贴边吸附 / 工作区钳制由 Rust 完成），并用返回值
   * 更新 docked：`'left' | 'right' | 'top' | 'bottom'` = 本次落位吸附贴边（胶囊收缩成
   * 紧凑形态），`null` = 超磁性阈值仅钳制落位（保持全形）。
   *
   * 随坐标带上**可见胶囊**的实际尺寸（逻辑 px）：吸附数学在 Rust，而胶囊在 280×64 窗口内
   * 居中——不带尺寸时 Rust 只能按窗口外框贴边，待机胶囊会离屏幕边 ~(280-104)/2 ≈ 88px。
   * 测量时机是「拖拽松手之后」（persist-position 命令），只读 offsetWidth/offsetHeight
   * 一次性读取布局尺寸，绝不触发 resize / 重绘回路，与手势期几何不变量无冲突；此时手势
   * 已结束、胶囊早已还原全形，量到的正是全形尺寸。
   */
  const persistDragPosition = useCallback(async () => {
    try {
      const box = capsuleBoxRef.current
      const capsule =
        box && box.offsetWidth > 0 && box.offsetHeight > 0
          ? { width: box.offsetWidth, height: box.offsetHeight }
          : undefined
      const win = getOverlayWindow()
      const pos = await win.outerPosition()
      const edge = await window.electronAPI.setOverlayPosition(pos.x, pos.y, capsule)
      setDockEdge(isOverlayEdge(edge) ? edge : null)
    } catch {
      // 持久化失败不阻塞 UI，下次拖拽会重试
    }
  }, [getOverlayWindow])

  /** 拖拽移动节流 flush：在下一帧把窗口移动到最新坐标。 */
  const flushDragMove = useCallback(() => {
    dragMovePendingRef.current = false
    const pos = dragPendingPosRef.current
    dragPendingPosRef.current = null
    if (!pos) return
    const win = getOverlayWindow()
    win.setPosition(new PhysicalPosition(pos.x, pos.y)).catch(() => {})
  }, [getOverlayWindow])

  /** 收到新的拖拽目标坐标时，rAF 节流下发窗口移动。 */
  const scheduleDragMove = useCallback(
    (pos: PhysicalPoint) => {
      dragPendingPosRef.current = pos
      if (!dragMovePendingRef.current) {
        dragMovePendingRef.current = true
        requestAnimationFrame(flushDragMove)
      }
    },
    [flushDragMove],
  )

  /** 启动 / 重置看门狗：拖拽 / pending 状态无事件 N 秒后强制复位。 */
  const startWatchdog = (deadline: number) => {
    clearWatchdog()
    const delay = Math.max(0, deadline - Date.now())
    watchdogTimeoutRef.current = setTimeout(() => {
      const [nextState, cmds] = onWatchdog(dragRef.current, Date.now())
      applyDragResult(nextState, cmds)
    }, delay)
  }

  /** 清除看门狗定时器。 */
  const clearWatchdog = () => {
    if (watchdogTimeoutRef.current !== null) {
      clearTimeout(watchdogTimeoutRef.current)
      watchdogTimeoutRef.current = null
    }
  }

  /**
   * 启动按住阈值定时器：按下满 HOLD_LATCH_MAX_MS 仍未松手 / 未拖拽 → 状态机进入 holding
   * 并开始 PTT 录音（onHoldTimer）。拖拽或松手都会由状态机清除它。
   */
  const startHoldTimer = (deadline: number) => {
    clearHoldTimer()
    const delay = Math.max(0, deadline - Date.now())
    holdTimerRef.current = setTimeout(() => {
      holdTimerRef.current = null
      const [nextState, cmds] = onHoldTimer(dragRef.current, Date.now())
      applyDragResult(nextState, cmds)
    }, delay)
  }

  /** 清除按住阈值定时器。 */
  const clearHoldTimer = () => {
    if (holdTimerRef.current !== null) {
      clearTimeout(holdTimerRef.current)
      holdTimerRef.current = null
    }
  }

  /** 执行拖拽状态机返回的命令。 */
  const executeDragCommands = (cmds: DragCommand[]) => {
    for (const cmd of cmds) {
      switch (cmd.type) {
        case 'start-session':
          handleHoldStart()
          break
        case 'stop-session':
          handleHoldEnd()
          break
        case 'cancel-session':
          handleCancel()
          break
        case 'set-overlay-position':
          // 双击复位（x/y 均为 null）：Rust 返回 null → docked=false、胶囊回到全形。
          // 统一按返回值消费而不是直接写 false，未来若该命令携带其它坐标仍正确。
          window.electronAPI
            .setOverlayPosition(cmd.x, cmd.y)
            .then((edge) => setDockEdge(isOverlayEdge(edge) ? edge : null))
            .catch(() => {})
          break
        case 'release-pointer-capture':
          releasePointerCapture(cmd.pointerId)
          break
        case 'add-listeners':
          addDragListeners()
          break
        case 'remove-listeners':
          removeDragListeners()
          break
        case 'start-watchdog':
          startWatchdog(cmd.deadline)
          break
        case 'clear-watchdog':
          clearWatchdog()
          break
        case 'start-hold-timer':
          startHoldTimer(cmd.deadline)
          break
        case 'clear-hold-timer':
          clearHoldTimer()
          break
        case 'move':
          scheduleDragMove(cmd.pos)
          break
        case 'flush-move':
          flushDragMove()
          break
        case 'persist-position':
          persistDragPosition()
          break
      }
    }
  }
  executeDragCommandsRef.current = executeDragCommands

  /**
   * 待机胶囊 pointerdown：开始一次指针手势（**不立即开录**）。
   *
   * 监听挂在**整个 overlay 窗口**的容器上（而不是只挂在胶囊 `<button>` 上）：待机胶囊比
   * 窗口小，胶囊四周有不可见的窗口空白，且 `<button>` 的圆角本身也会让四角不参与命中；
   * 只挂胶囊会让落在胶囊边缘/上下的按下「完全无反应」（0.5.1 回归缺陷现象之一）。整窗即热区后，
   * 点在窗口内任意位置都能开始手势。
   *
   * 手势结果由状态机裁决：轻点 → 松手时 latch 启动并保持；右键 / 中键同左键；位移超过
   * 阈值 → 拖拽移窗（绝不启动会话）；停留过阈值 → 按住 PTT。窗口尺寸在整个手势 / 会话期间
   * 恒定（不 resize / 不重锚定），故手势绝不会被自身的几何变化打断。
   */
  const handleIdlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    const [nextState, cmds] = onPointerDown(
      {
        button: event.button,
        pointerId: event.pointerId,
        screenX: event.screenX,
        screenY: event.screenY,
      },
      dragRef.current,
      Date.now(),
      statusRef.current,
    )

    if (nextState.pointerId !== null) {
      try {
        event.currentTarget.setPointerCapture(nextState.pointerId)
      } catch {
        // setPointerCapture 在某些平台 / 元素上可能失败；window 监听器兜底
      }
    }

    applyDragResult(nextState, cmds)

    // 异步解析窗口起始物理坐标与缩放因子，供拖拽移动计算使用。
    if (nextState.phase === 'pending') {
      const win = getOverlayWindow()
      Promise.all([win.outerPosition().then((pos) => ({ x: pos.x, y: pos.y })), win.scaleFactor()])
        .then(([pos, factor]) => {
          dragRef.current = setWinStart(dragRef.current, pos, factor)
        })
        .catch(() => {})
    }
  }

  /**
   * 双击复位：重置到默认底部居中位置。
   *
   * 「第 1 击 latch、第 2 击静默取消并复位」的链路里，dblclick 触发时 overlay 仍可能处于
   * recording（第 2 击的 cancel-session 尚未回传），因此**不能**用 status 作门禁。真正的
   * 判据是 `onDoubleClick` 对 lastTap 的「距上次轻点 ≤ DOUBLE_CLICK_MAX_MS」检查——lastTap
   * 只由轻点写入，PTT 按住 / 处理中不会留下新鲜 lastTap，故不会被误复位。
   */
  const handleIdleDoubleClick = () => {
    const [nextState, cmds] = onDoubleClick(dragRef.current, Date.now())
    applyDragResult(nextState, cmds)
  }

  return (
    <div
      ref={rootRef}
      className="pointer-events-auto fixed inset-0 flex items-center justify-center"
      onPointerDown={handleIdlePointerDown}
      onDoubleClick={handleIdleDoubleClick}
      onContextMenu={(event) => event.preventDefault()}
    >
      {/* 布局容器：窗口常驻 280×64，待机胶囊居中于窗口。同时是 persist-position
          的胶囊尺寸测量盒（shrink-wrap = 胶囊实际尺寸）。0.5.6 解耦后本窗恒渲染
          待机胶囊——hold 手势在途时也如此（大胶囊在下方居中的 session 窗，本胶囊
          在手指下原位不动，两窗同显）。 */}
      <div ref={capsuleBoxRef} className="flex shrink-0">
        {/* 待机胶囊：小尺寸低调形态。仅作视觉主体，pointerdown / dblclick 等交互
            统一挂在整窗容器上（见 handleIdlePointerDown 注释）。不设 title，
            避免悬停弹出原生 tooltip（aria-label 保留可访问性）。 */}
        <button
          type="button"
          className={cn(
            'pointer-events-auto flex select-none items-center rounded-full',
            'bg-neutral-950/85 ring-1 ring-white/[0.06] touch-none',
            // 形态过渡：`width: auto` 无法被 CSS 过渡插值，故宽度由可插值的
            // padding / gap / 文字 max-width 连续驱动（≈104px ↔ 32px 平滑收放），
            // 高度是显式类值（h-9 → h-8）可直接插值；v4 的位移走独立 `translate`
            // 属性（不是 `transform`），必须显式列入 transition 才平滑贴齐 / 归零。
            'transition-[opacity,transform,translate,filter,padding,gap,max-width,height] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)]',
            // 紧凑 32×32（= 上/下吸附的可见半窗高 64/2，故上下贴边整只屏内）；
            // 全形 h-9 ≈104×36，形态与原实现一致。
            docked ? 'h-8 max-w-[32px] px-1.5 gap-0' : 'h-9 max-w-[240px] px-3.5 gap-2',
            isVisible
              ? 'translate-y-0 scale-100 opacity-100 blur-0'
              : 'translate-y-2 scale-95 opacity-0 blur-sm',
            // pending / holding 阶段显式收缩，提供跟手的按压反馈；dragging 已释放。
            (gesturePhase === 'pending' || gesturePhase === 'holding') && 'scale-95',
            // 贴边贴齐（±16px = 紧凑边长/2，把胶囊外缘正好压到窗口中线 = 屏幕边，
            // 整只胶囊含图标 chip 完全落在屏内）。必须排在入场 translate **之后**：
            // twMerge 保留最后写法，贴边位移优先于入场位移（二者同轴时）。
            dockEdge ? DOCK_TRANSLATE[dockEdge] : '',
          )}
          aria-label={t('hud.holdToTalk')}
        >
          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-white/10 text-neutral-300">
            <Mic className="h-3 w-3" />
          </span>
          {/* 贴边（docked）时文字收宽为 0 并淡出——仅视觉隐藏，随过渡平滑收放；
                可访问名称始终由按钮 `aria-label` 提供，读屏仍能读到这段文本。 */}
          <span
            className={cn(
              // min-w-0：flex item 的 `min-width: auto`（内容最小宽）会顶住 max-w-0，
              // 显式清零才能让文字真正收到 0 宽；overflow-hidden 使其成为滚动容器、
              // 不参与可见裁剪溢出。
              'min-w-0 overflow-hidden whitespace-nowrap text-xs font-medium text-neutral-200',
              'transition-[max-width,opacity] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)]',
              docked ? 'max-w-0 opacity-0' : 'max-w-[160px] opacity-100',
            )}
          >
            {t('hud.holdToTalk')}
          </span>
        </button>
      </div>
    </div>
  )
}
