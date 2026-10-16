import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { AudioLines, Check, Globe, Mic, Sparkles, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { OverlayProcessingStage, OverlayState, OverlayStatus } from '@/shared/types'
import { cn } from '../lib/utils'
import { Waveform } from './Waveform'

/** 窗口缩放到胶囊内容尺寸时的内边距（每侧 2px）。 */
const OVERLAY_PADDING_PX = 2

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
   * 按住说话的「按下」所有权标记：只有本胶囊 pointerdown 成功发起的会话才由
   * 本胶囊 pointerup 结束。pointerup 会从任何子元素冒泡上来，若无此标记，
   * 取消按钮的松开或他人会话的松开会被误当成「松开说话结束」。
   */
  const holdActiveRef = useRef(false)
  /** 与 overlayState 同步，但可在事件回调里同步读取（避免闭包过期）。 */
  const statusRef = useRef<OverlayStatus>('idle')
  /**
   * 内容测量容器 ref：容器自身无 transform，其 rect 恒等于胶囊布局尺寸，
   * 不受入场 / 按压缩放动画影响，供窗口缩放（setOverlaySize）使用。
   */
  const measureRef = useRef<HTMLDivElement | null>(null)
  /** 测量用 rAF：同一帧内多次重渲染只测一次。 */
  const measureRafRef = useRef<number | null>(null)
  /** 上次已下发的窗口尺寸，尺寸未变不重复 invoke（录音电平帧高频重渲染）。 */
  const lastOverlaySizeRef = useRef<{ width: number; height: number } | null>(null)

  useEffect(() => {
    document.documentElement.classList.add('overlay-html')
    rafIdRef.current = requestAnimationFrame(() => setIsVisible(true))
    return () => {
      if (rafIdRef.current !== null) {
        cancelAnimationFrame(rafIdRef.current)
      }
      if (measureRafRef.current !== null) {
        cancelAnimationFrame(measureRafRef.current)
      }
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
    })

    return () => {
      removeOverlayUpdateListener?.()
      removeAudioLevelListener?.()
    }
  }, [])

  useEffect(() => {
    statusRef.current = overlayState.status
    // 窗口已缩到内容大小（见 scheduleOverlayResize），整窗即胶囊、无死区，所有
    // 状态保持可点击（待机 = 按住说话，录音 / 处理 = 取消按钮）。不做 hover 穿透：
    // Tauri `set_ignore_cursor_events` 只有 bool、无 Electron 式 forward 转发，一旦
    // 穿透 mouseenter 不再可达，胶囊（含取消按钮）会永久点不到。
    window.electronAPI.setIgnoreMouseEvents(false)
  }, [overlayState.status])

  /**
   * 状态 / 文案变化后测量胶囊实际内容尺寸，把 overlay 窗口缩到内容大小
   * （每侧 2px 内边距；Rust `set_overlay_size` 钳制尺寸合法性并保持底部居中）。
   * 在 rAF 中取 `getBoundingClientRect` 等布局落定；测量容器自身无 transform，
   * 故 rect 不受入场 / 按压缩放动画干扰。尺寸不变则去重、不重复 invoke。
   */
  const scheduleOverlayResize = useCallback(() => {
    if (measureRafRef.current !== null) return
    measureRafRef.current = requestAnimationFrame(() => {
      measureRafRef.current = null
      const el = measureRef.current
      if (!el) return
      const rect = el.getBoundingClientRect()
      const width = Math.ceil(rect.width) + OVERLAY_PADDING_PX * 2
      const height = Math.ceil(rect.height) + OVERLAY_PADDING_PX * 2
      if (width <= 0 || height <= 0) return
      const last = lastOverlaySizeRef.current
      if (last && last.width === width && last.height === height) return
      lastOverlaySizeRef.current = { width, height }
      window.electronAPI.setOverlaySize(width, height).catch(() => {
        // 下发失败：清掉去重缓存，下次渲染重试
        lastOverlaySizeRef.current = null
      })
    })
  }, [])

  // 每次渲染后测量并按需缩放：覆盖状态切换与文案长度变化（如 success 预览变长）。
  useEffect(() => {
    scheduleOverlayResize()
  })

  /** pointerdown：按住说话（左键 / 中键）。语义与 PTT 心智一致，绝不做切换。 */
  const handleHoldStart = (event: ReactPointerEvent) => {
    if (event.button !== 0 && event.button !== 1) return
    // 仅待机态由胶囊发起会话；录音 / 处理态的按下交给各自按钮（如取消），
    // 防止「点取消 → 误开始」。
    if (statusRef.current !== 'idle') return
    if (holdActiveRef.current) return
    holdActiveRef.current = true
    window.electronAPI.startSession().catch(() => {
      holdActiveRef.current = false
    })
  }

  /** pointerup / pointercancel / pointerleave：只结束本胶囊发起的会话。 */
  const handleHoldEnd = () => {
    if (!holdActiveRef.current) return
    holdActiveRef.current = false
    window.electronAPI.stopSession().catch(() => {})
  }

  const handleCancel = () => {
    window.electronAPI.cancelSession()
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
      className="pointer-events-none fixed inset-0 flex items-center justify-center"
      onPointerUp={handleHoldEnd}
      onPointerCancel={handleHoldEnd}
    >
      {/* 测量容器：无自身 transform，rect 恒等于胶囊布局尺寸，供窗口缩放 */}
      <div ref={measureRef} className="flex shrink-0">
        {status === 'idle' ? (
          // 待机态：小尺寸低调胶囊，点击 = 按住说话（pointerdown/up 语义，不做切换）
          <button
            type="button"
            className={cn(
              'pointer-events-auto flex h-9 select-none items-center gap-2 rounded-full',
              'border border-white/10 bg-neutral-950/70 px-3.5 backdrop-blur-xl touch-none',
              'shadow-[0_12px_32px_-12px_rgba(0,0,0,0.65)]',
              'transition-[opacity,transform,filter] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)] active:scale-95',
              isVisible
                ? 'translate-y-0 scale-100 opacity-100 blur-0'
                : 'translate-y-2 scale-95 opacity-0 blur-sm',
            )}
            onPointerDown={handleHoldStart}
            onPointerUp={handleHoldEnd}
            onPointerCancel={handleHoldEnd}
            onPointerLeave={handleHoldEnd}
            onContextMenu={(event) => event.preventDefault()}
            title={t('hud.holdToTalk')}
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
              'border border-white/10 bg-neutral-950/85 px-3 backdrop-blur-xl',
              'shadow-[0_12px_32px_-12px_rgba(0,0,0,0.65),0_4px_10px_-4px_rgba(0,0,0,0.4)]',
              'transition-[opacity,transform,filter] duration-300 ease-[cubic-bezier(0.23,1,0.32,1)]',
              isVisible
                ? 'translate-y-0 scale-100 opacity-100 blur-0'
                : 'translate-y-2 scale-95 opacity-0 blur-sm',
            )}
            onPointerUp={handleHoldEnd}
            onPointerCancel={handleHoldEnd}
            onPointerLeave={handleHoldEnd}
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
