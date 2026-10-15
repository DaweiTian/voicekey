import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Minus, Square, Maximize2, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'

type TauriWindow = ReturnType<typeof getCurrentWindow>

function getCurrentWindowSafe(): TauriWindow | null {
  try {
    return getCurrentWindow()
  } catch {
    return null
  }
}

/** 可重新着色的品牌 V 标记。 */
function VMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden="true">
      <path
        d="M5.4 5 L12 18.4 L18.6 5"
        stroke="currentColor"
        strokeWidth={4.4}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

export default function TitleBar() {
  const { t } = useTranslation()
  const [isMaximized, setIsMaximized] = useState(false)
  const isMac = window.electronAPI.platform === 'darwin'
  // Lazily resolved once: reading a ref during render is not allowed.
  const [win] = useState<TauriWindow | null>(getCurrentWindowSafe)

  useEffect(() => {
    if (!win) return

    let mounted = true
    const update = async () => {
      try {
        const maximized = await win.isMaximized()
        if (mounted) setIsMaximized(maximized)
      } catch {
        // Ignore in browser preview.
      }
    }
    void update()

    const onResize = () => void update()
    window.addEventListener('resize', onResize)
    return () => {
      mounted = false
      window.removeEventListener('resize', onResize)
    }
  }, [win])

  const handleMinimize = async () => {
    try {
      await win?.minimize()
    } catch {
      /* ignore */
    }
  }

  const handleToggleMaximize = async () => {
    try {
      await win?.toggleMaximize()
      setIsMaximized((prev) => !prev)
    } catch {
      /* ignore */
    }
  }

  const handleClose = async () => {
    try {
      await win?.close()
    } catch {
      /* ignore */
    }
  }

  const handleDragDoubleClick = () => void handleToggleMaximize()

  return (
    <header
      className={cn(
        'flex h-9 shrink-0 select-none items-stretch bg-sidebar text-sidebar-foreground',
        isMac && 'pl-[72px]',
      )}
      aria-label={t('titleBar.label')}
    >
      {/* 品牌区 - 与侧栏 Logo 呼应 */}
      <div className="no-drag flex items-center gap-2 px-3">
        <span className="vk-brand-gradient grid h-5 w-5 place-items-center rounded-[6px] text-white shadow-sm">
          <VMark className="h-3 w-3" />
        </span>
        <span className="font-display text-[13px] font-semibold tracking-tight">
          {t('app.name')}
        </span>
      </div>

      {/* 拖拽区 - Tauri v2 要求元素本身带 data-tauri-drag-region */}
      <div
        className="flex flex-1 items-center"
        data-tauri-drag-region
        onDoubleClick={handleDragDoubleClick}
        aria-hidden="true"
      />

      {/* 窗口控制按钮 */}
      <div className="no-drag flex items-stretch">
        <button
          type="button"
          onClick={handleMinimize}
          title={t('titleBar.minimize')}
          aria-label={t('titleBar.minimize')}
          className="inline-flex h-full w-[46px] cursor-pointer items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/60"
        >
          <Minus className="h-4 w-4" strokeWidth={1.8} />
        </button>
        <button
          type="button"
          onClick={handleToggleMaximize}
          title={isMaximized ? t('titleBar.restore') : t('titleBar.maximize')}
          aria-label={isMaximized ? t('titleBar.restore') : t('titleBar.maximize')}
          className="inline-flex h-full w-[46px] cursor-pointer items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/60"
        >
          {isMaximized ? (
            <Square className="h-3.5 w-3.5" strokeWidth={1.8} />
          ) : (
            <Maximize2 className="h-3.5 w-3.5" strokeWidth={1.8} />
          )}
        </button>
        <button
          type="button"
          onClick={handleClose}
          title={t('titleBar.close')}
          aria-label={t('titleBar.close')}
          className="inline-flex h-full w-[46px] cursor-pointer items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-destructive-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/60"
        >
          <X className="h-4 w-4" strokeWidth={1.8} />
        </button>
      </div>
    </header>
  )
}
