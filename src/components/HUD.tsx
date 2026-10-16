import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { getCurrentWindow, type Window as TauriWindow } from '@tauri-apps/api/window'
import { PhysicalPosition } from '@tauri-apps/api/dpi'
import { AudioLines, Check, Globe, Mic, Sparkles, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { OverlayProcessingStage, OverlayState, OverlayStatus } from '@/shared/types'
import { cn } from '../lib/utils'
import {
  initialOverlayDragState,
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
  type OverlayDragState,
  type PhysicalPoint,
} from '../lib/overlayDrag'
import { Waveform } from './Waveform'

/**
 * 各处理阶段的小面积语义色：只作用于转圈描边与阶段图标，
 * 胶囊底色在所有状态下保持统一的深色玻璃。
 */
const STAGE_META: Record<
  OverlayProcessingStage,
  {
    icon: typeof AudioLines
    iconColor: string
    spinnerTop: string
    spinnerBottom: string
  }
> = {
  transcribing: {
    icon: AudioLines,
    iconColor: 'text-indigo-300',
    spinnerTop: 'border-t-indigo-400',
    spinnerBottom: 'border-b-indigo-500/25',
  },
  refining: {
    icon: Sparkles,
    iconColor: 'text-violet-300',
    spinnerTop: 'border-t-violet-400',
    spinnerBottom: 'border-b-violet-500/25',
  },
  translating: {
    icon: Globe,
    iconColor: 'text-cyan-300',
    spinnerTop: 'border-t-cyan-400',
    spinnerBottom: 'border-b-cyan-500/25',
  },
}

function getProcessingTitle(
  stage: OverlayProcessingStage | undefined,
  t: (key: string) => string,
): string {
  switch (stage) {
    case 'transcribing':
      return t('hud.transcribing')
    case 'refining':
      return t('hud.refining')
    case 'translating':
      return t('hud.translating')
    default:
      return t('hud.thinking')
  }
}

/** 录音：琥珀色麦克风圆章，随音量呼吸缩放 + 外圈涟漪 */
function RecordingBadge({ audioLevel }: { audioLevel: number }) {
  const scale = 1 + Math.min(Math.max(audioLevel, 0), 1) * 0.14
  return (
    <span className="relative flex h-7 w-7 shrink-0 items-center justify-center">
      <span className="absolute inset-0 animate-ping rounded-full bg-primary/25 [animation-duration:2s]" />
      <span
        className="relative flex h-7 w-7 items-center justify-center rounded-full bg-primary text-primary-foreground"
        style={{
          transform: `scale(${scale})`,
          boxShadow: '0 0 14px -4px color-mix(in oklch, var(--primary) 75%, transparent)',
        }}
      >
        <Mic className="h-3.5 w-3.5" strokeWidth={2.4} />
      </span>
    </span>
  )
}

/** 处理中：细环转圈（阶段色）包住阶段小图标 */
function ProcessingBadge({ stage }: { stage: OverlayProcessingStage | undefined }) {
  const meta = stage ? STAGE_META[stage] : null
  const Icon = meta?.icon ?? AudioLines
  return (
    <span className="relative flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-white/10 bg-white/5">
      <span
        className={cn(
          'absolute inset-[2px] animate-spin rounded-full border-2 border-l-transparent border-r-transparent [animation-duration:1.1s]',
          meta
            ? [meta.spinnerTop, meta.spinnerBottom]
            : 'border-t-indigo-400 border-b-indigo-500/25',
        )}
      />
      <Icon className={cn('h-3 w-3', meta?.iconColor ?? 'text-indigo-300')} />
    </span>
  )
}

/** 成功：绿色小勾圆章 */
function SuccessBadge() {
  return (
    <span className="hud-pop flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-emerald-400/12 text-emerald-400 ring-1 ring-emerald-400/30 ring-inset">
      <Check className="h-3.5 w-3.5" strokeWidth={3} />
    </span>
  )
}

/** 错误：红色小叉圆章 */
function ErrorBadge() {
  return (
    <span className="hud-pop flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-red-500/12 text-red-400 ring-1 ring-red-400/30 ring-inset">
      <X className="h-3.5 w-3.5" strokeWidth={2.5} />
    </span>
  )
}

export function HUD() {
  const { t } = useTranslation()
  const [overlayState, setOverlayState] = useState<OverlayState>({ status: 'idle' })
  const [audioLevel, setAudioLevel] = useState(0)
  const [isVisible, setIsVisible] = useState(false)

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
    if (nextState.latched && !wasLatched) {
      lastVoiceAtRef.current = Date.now()
    }
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

  useEffect(() => {
    const removeOverlayUpdateListener = window.electronAPI.onOverlayUpdate(
      (state: OverlayState) => {
        setOverlayState(state)
      },
    )

    const removeAudioLevelListener = window.electronAPI.onAudioLevel((level: number) => {
      setAudioLevel(level)
      // 只要有可闻声音就刷新「最后说话时刻」；静音自动停止据此判定。
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
    // 窗口常驻 tauri.conf.json 的 280×64（任何状态都装得下），不再随状态缩放——这是
    // 「手势 / 会话期间窗口几何不可变」硬不变量的实现基础（见 overlayDrag.ts 模块头
    // REGRESSION WARNING）。整窗即胶囊热区、无死区，所有状态保持可点击（待机 = 点按切换 /
    // 按住说话 / 拖拽，录音 / 处理 = 取消按钮）。不做 hover 穿透：Tauri
    // `set_ignore_cursor_events` 只有 bool、无 Electron 式 forward 转发，一旦穿透
    // mouseenter 不再可达，胶囊（含取消按钮）会永久点不到。
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
   * 把当前窗口物理坐标持久化到 Rust（贴边吸附 / 工作区钳制由 Rust 完成）。
   *
   * 随坐标带上**可见胶囊**的实际尺寸（逻辑 px）：吸附数学在 Rust，而胶囊在 280×64 窗口内
   * 居中——不带尺寸时 Rust 只能按窗口外框贴边，待机胶囊会离屏幕边 ~(280-104)/2 ≈ 88px。
   * 测量时机是「拖拽松手之后」（persist-position 命令），只读 offsetWidth/offsetHeight
   * 一次性读取布局尺寸，绝不触发 resize / 重绘回路，与手势期几何不变量无冲突。
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
      await window.electronAPI.setOverlayPosition(pos.x, pos.y, capsule)
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
          window.electronAPI.setOverlayPosition(cmd.x, cmd.y).catch(() => {})
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

  const { status, message, processingStage, transcript = '' } = overlayState
  const transcriptCharacters = Array.from(transcript)
  const unstableTailLength =
    status === 'recording' || status === 'processing' ? Math.min(6, transcriptCharacters.length) : 0
  const stableTranscript = transcriptCharacters
    .slice(0, transcriptCharacters.length - unstableTailLength)
    .join('')
  const unstableTranscript = transcriptCharacters.slice(-unstableTailLength).join('')

  // 取消按钮只在会话进行中（录音 / 处理）有意义；成功与错误态把宽度让给文案。
  const showCancel = status === 'recording' || status === 'processing'
  const textClass = 'min-w-0 flex-1 truncate text-[13px] leading-4 text-neutral-100'

  return (
    <div
      ref={rootRef}
      className="pointer-events-auto fixed inset-0 flex items-center justify-center"
      onPointerDown={handleIdlePointerDown}
      onDoubleClick={handleIdleDoubleClick}
      onContextMenu={(event) => event.preventDefault()}
    >
      {/* 布局容器：窗口常驻 280×64，胶囊居中于窗口（不再随状态缩放窗口）。
          同时是 persist-position 的胶囊尺寸测量盒（shrink-wrap = 胶囊实际尺寸）。 */}
      <div ref={capsuleBoxRef} className="flex shrink-0">
        {status === 'idle' ? (
          // 待机态：小尺寸低调胶囊。仅作视觉主体，pointerdown / dblclick 等交互
          // 统一挂在整窗容器上（见 handleIdlePointerDown 注释）。不设 title，
          // 避免悬停弹出原生 tooltip（aria-label 保留可访问性）。
          <button
            type="button"
            className={cn(
              'pointer-events-auto flex h-9 select-none items-center gap-2 rounded-full',
              'bg-neutral-950/85 px-3.5 ring-1 ring-white/[0.06] touch-none',
              'transition-[opacity,transform,filter] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)] active:scale-95',
              isVisible
                ? 'translate-y-0 scale-100 opacity-100 blur-0'
                : 'translate-y-2 scale-95 opacity-0 blur-sm',
            )}
            aria-label={t('hud.holdToTalk')}
          >
            <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-white/10 text-neutral-300">
              <Mic className="h-3 w-3" />
            </span>
            <span className="text-xs font-medium text-neutral-200">{t('hud.holdToTalk')}</span>
          </button>
        ) : (
          <div
            className={cn(
              'pointer-events-auto relative flex h-[52px] w-[268px] items-center gap-2.5 rounded-full',
              'bg-neutral-950/90 px-3 ring-1 ring-white/[0.06]',
              'transition-[opacity,transform,filter] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)]',
              isVisible
                ? 'translate-y-0 scale-100 opacity-100 blur-0'
                : 'translate-y-2 scale-95 opacity-0 blur-sm',
            )}
          >
            {/* Left status badge */}
            {status === 'recording' && <RecordingBadge audioLevel={audioLevel} />}
            {status === 'processing' && <ProcessingBadge stage={processingStage} />}
            {status === 'success' && <SuccessBadge />}
            {status === 'error' && <ErrorBadge />}

            {/* Single-line status text */}
            {status === 'recording' &&
              (transcript ? (
                /* 流式识别：实时文本单行截断，尾部弱化 + 呼吸光标 */
                <p className={cn(textClass, 'font-normal')} title={transcript}>
                  <span>{stableTranscript}</span>
                  <span className="text-neutral-500">{unstableTranscript}</span>
                  <span className="hud-caret ml-[2px] inline-block h-[13px] w-[2px] translate-y-[2px] rounded-full bg-primary" />
                </p>
              ) : (
                /* 无流式文本：状态词 + 迷你波形 */
                <div className="flex min-w-0 flex-1 items-center justify-between gap-2">
                  <span className="truncate text-[13px] font-medium text-neutral-200">
                    {t('hud.listening')}
                  </span>
                  <Waveform
                    audioLevel={audioLevel}
                    barCount={7}
                    className="h-4 w-[26px] shrink-0 gap-[2px]"
                    barClassName="w-[2px] bg-linear-to-t from-primary/40 to-primary"
                  />
                </div>
              ))}

            {status === 'processing' &&
              (transcript ? (
                /* 流式识别增量：与录音分支同款单行截断 + 尾部弱化 + 光标 */
                <p className={cn(textClass, 'font-normal')} title={transcript}>
                  <span>{stableTranscript}</span>
                  <span className="text-neutral-500">{unstableTranscript}</span>
                  <span className="hud-caret ml-[2px] inline-block h-[13px] w-[2px] translate-y-[2px] rounded-full bg-primary" />
                </p>
              ) : (
                <p
                  className={cn(textClass, 'font-medium')}
                  title={getProcessingTitle(processingStage, t)}
                >
                  {getProcessingTitle(processingStage, t)}
                </p>
              ))}

            {status === 'success' && (
              <p className={textClass} title={transcript || undefined}>
                {transcript || t('hud.injected')}
              </p>
            )}

            {status === 'error' && (
              <p className={cn(textClass, 'text-neutral-200')} title={message}>
                {message || t('hud.errorFallback')}
              </p>
            )}

            {/* Cancel button */}
            {showCancel && (
              <button
                type="button"
                onClick={handleCancel}
                /* 与按住说话互斥：取消按钮的按下 / 松开不得触发 start / stop */
                onPointerDown={(event) => event.stopPropagation()}
                onPointerUp={(event) => event.stopPropagation()}
                onPointerCancel={(event) => event.stopPropagation()}
                className="flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-500 transition-colors duration-150 hover:bg-white/10 hover:text-neutral-200 active:scale-90"
                title={t('hud.cancel')}
                aria-label={t('hud.cancel')}
              >
                <X className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
