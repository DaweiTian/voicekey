import { useEffect, useState } from 'react'
import { AudioLines, Check, Globe, Mic, Sparkles, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { OverlayProcessingStage, OverlayState } from '@/shared/types'
import { cn } from '../lib/utils'
import { Waveform } from './Waveform'

/**
 * Session capsule (`session` window, label routed in `App.tsx`).
 *
 * 0.5.6 解耦：会话态视觉（录音 / 处理 / 成功 / 错误 + 取消按钮）从 overlay 的
 * `HUD.tsx` 拆出，常驻渲染在**独立的、固定屏幕下方居中的 session 窗口**——由
 * Rust 在每次会话启动时定位并显示（`commands::session_show`，光标所在监视器
 * 工作区底部居中）、回到 idle 时隐藏（`commands::overlay_emit_idle` 唯一收口）。
 * 与可拖拽的待机胶囊彻底解耦：托盘隐藏悬浮胶囊、hold 手势在途都不影响本窗。
 *
 * 事件经 `app.emit` 广播天然送达本窗（`overlay-update` / `audio-level`），
 * 与 overlay 窗各听各的；本组件**只渲染，不开关窗口**（窗口生命周期权威在
 * Rust 侧）。视觉 / 交互与拆分前的 overlay 会话态胶囊基本一致，并补了入场
 * 与态间动效。
 */

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
    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-emerald-400/12 text-emerald-400 ring-1 ring-emerald-400/30 ring-inset">
      <Check className="h-3.5 w-3.5" strokeWidth={3} />
    </span>
  )
}

/** 错误：红色小叉圆章 */
function ErrorBadge() {
  return (
    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-red-500/12 text-red-400 ring-1 ring-red-400/30 ring-inset">
      <X className="h-3.5 w-3.5" strokeWidth={2.5} />
    </span>
  )
}

export function SessionHUD() {
  const { t } = useTranslation()
  const [overlayState, setOverlayState] = useState<OverlayState>({ status: 'idle' })
  const [audioLevel, setAudioLevel] = useState(0)

  useEffect(() => {
    document.documentElement.classList.add('overlay-html')
    return () => {
      document.documentElement.classList.remove('overlay-html')
    }
  }, [])

  useEffect(() => {
    // `app.emit` 广播：与 overlay 窗同一事件源，各窗各听各的状态。
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

  /** 取消会话（cancel 按钮）：静默丢弃；Rust 取消后回 idle 并隐藏本窗。 */
  const handleCancel = () => {
    window.electronAPI.cancelSession().catch(() => {})
  }

  // 待机态本窗不渲染任何内容：session 窗只在会话（或翻译反馈）期间由 Rust 显示，
  // 回 idle 即隐藏——渲染 null 保证隐藏前的最后一帧也是透明空窗，无闪烁。
  if (overlayState.status === 'idle') {
    return null
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
    <div className="pointer-events-auto fixed inset-0 flex items-center justify-center">
      <div className="flex shrink-0">
        <div
          className={cn(
            'pointer-events-auto relative flex h-[52px] w-[268px] items-center gap-2.5 rounded-full',
            'bg-neutral-950/90 px-3 ring-1 ring-white/[0.06]',
            'hud-enter',
          )}
        >
          {/* Left status badge —— key={status} 触发每次状态变化重新挂载并播放 hud-pop */}
          <span key={status} className="inline-flex hud-pop">
            {status === 'recording' && <RecordingBadge audioLevel={audioLevel} />}
            {status === 'processing' && <ProcessingBadge stage={processingStage} />}
            {status === 'success' && <SuccessBadge />}
            {status === 'error' && <ErrorBadge />}
          </span>

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
              className="flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-500 transition-colors duration-150 hover:bg-white/10 hover:text-neutral-200 active:scale-90"
              title={t('hud.cancel')}
              aria-label={t('hud.cancel')}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
