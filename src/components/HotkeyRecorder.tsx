import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'
import {
  buildAccelerator,
  formatHotkey,
  hasNonModifierKey,
  isModifierOnly,
  normalizeKey,
} from '@/lib/hotkey-utils'

interface HotkeyRecorderProps {
  /** 当前快捷键值（Electron Accelerator 格式）*/
  value: string
  /** 值变化回调 */
  onChange: (value: string) => void
  /** 标签文本 */
  label: string
  /** 描述文本 */
  description?: string
  /** 是否禁用 */
  disabled?: boolean
  /** 是否显示错误状态 */
  hasError?: boolean
  /** 错误信息 */
  errorMessage?: string
  /** 是否允许单修饰键（仅 PTT 按住说话支持，如单独的 Alt） */
  allowModifierOnly?: boolean
}

export function HotkeyRecorder({
  value,
  onChange,
  label,
  description,
  disabled = false,
  hasError = false,
  errorMessage,
  allowModifierOnly = false,
}: HotkeyRecorderProps) {
  const { t } = useTranslation()
  const [isRecording, setIsRecording] = useState(false)
  const [pressedKeys, setPressedKeys] = useState<Set<string>>(new Set())
  // 使用 Ref 来保持最新的按键状态，解决 useEffect 依赖导致的监听器反复重绑问题
  const pressedKeysRef = useRef<Set<string>>(new Set())
  const recorderRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!isRecording || disabled) return

    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()

      if (e.key === 'Escape') {
        setIsRecording(false)
        pressedKeysRef.current.clear()
        setPressedKeys(new Set())
        return
      }

      const key = normalizeKey(e)
      if (key) {
        pressedKeysRef.current.add(key)
        setPressedKeys(new Set(pressedKeysRef.current))
      }
    }

    const handleKeyUp = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()

      const token = normalizeKey(e)
      // 未识别 / 被丢弃的键（如 AltGr 释放时补发的合成 Control keyup）：
      // 忽略即可，等待真实按键的 keyup 或 Esc / 失焦收尾
      if (!token) return

      if (isModifierOnly(token)) {
        if (pressedKeysRef.current.size === 1 && !hasNonModifierKey(pressedKeysRef.current)) {
          if (!allowModifierOnly) {
            toast.error(t('hotkey.toast.invalid'), {
              description: t('hotkey.validation.modifierOnly'),
            })
            setIsRecording(false)
            pressedKeysRef.current.clear()
            setPressedKeys(new Set())
            return
          }

          // 提交按键集合里的 token（而不是 keyup 归一值）：AltGr 松开时可能先收到
          // 合成 Control 的 keyup，集合中的 'AltRight' 才是用户实际按下的键
          const soleKey = [...pressedKeysRef.current][0]
          onChange(soleKey ?? token)
          setIsRecording(false)
          pressedKeysRef.current.clear()
          setPressedKeys(new Set())
          return
        }
      }

      if (pressedKeysRef.current.size > 0) {
        const keysArray = [...pressedKeysRef.current]
        const mainKeyCount = keysArray.filter((k) => !isModifierOnly(k)).length

        if (mainKeyCount > 1) {
          toast.error(t('hotkey.toast.invalid'), {
            description: t('hotkey.validation.multiple'),
          })
          setIsRecording(false)
          pressedKeysRef.current.clear()
          setPressedKeys(new Set())
          return
        }

        const accelerator = buildAccelerator(pressedKeysRef.current)
        if (accelerator) {
          onChange(accelerator)
        }
        setIsRecording(false)
        pressedKeysRef.current.clear()
        setPressedKeys(new Set())
      }
    }

    const handleClickOutside = (e: MouseEvent) => {
      const target = e.target as HTMLElement
      if (recorderRef.current && !recorderRef.current.contains(target)) {
        setIsRecording(false)
        pressedKeysRef.current.clear()
        setPressedKeys(new Set())
      }
    }

    const handleBlur = () => {
      setIsRecording(false)
      pressedKeysRef.current.clear()
      setPressedKeys(new Set())
    }

    window.addEventListener('keydown', handleKeyDown, true)
    window.addEventListener('keyup', handleKeyUp, true)
    window.addEventListener('mousedown', handleClickOutside)
    window.addEventListener('blur', handleBlur)

    return () => {
      window.removeEventListener('keydown', handleKeyDown, true)
      window.removeEventListener('keyup', handleKeyUp, true)
      window.removeEventListener('mousedown', handleClickOutside)
      window.removeEventListener('blur', handleBlur)
    }
  }, [allowModifierOnly, disabled, isRecording, onChange, t])

  // 录制期间通知 Rust 屏蔽全局快捷键触发（PTT / 翻译），否则录入组合键时会误启动录音。
  // effect cleanup 覆盖所有退出路径（提交、Esc、点击外部、失焦、卸载），保证 flag 不会卡死。
  useEffect(() => {
    if (!isRecording || disabled) return
    void window.electronAPI?.setHotkeyCaptureActive(true)?.catch(() => undefined)
    return () => {
      void window.electronAPI?.setHotkeyCaptureActive(false)?.catch(() => undefined)
    }
  }, [disabled, isRecording])

  const startRecording = useCallback(() => {
    if (!disabled) {
      setIsRecording(true)
      pressedKeysRef.current.clear()
      setPressedKeys(new Set())
    }
  }, [disabled])

  const displayValue = isRecording
    ? pressedKeys.size > 0
      ? formatHotkey(buildAccelerator(pressedKeys), t('hotkey.notSet'))
      : t('hotkey.recordingHint')
    : formatHotkey(value, t('hotkey.notSet'))

  return (
    <div ref={recorderRef} className="hotkey-recorder space-y-2">
      {label && <label className="text-sm font-medium text-foreground">{label}</label>}

      <div className="flex items-center gap-2">
        <div
          onClick={startRecording}
          className={cn(
            'flex-1 px-4 py-2.5 rounded-lg border cursor-pointer transition-[background-color,border-color,box-shadow,color]',
            'bg-background text-foreground font-mono text-sm',
            'select-none',
            isRecording
              ? 'border-primary ring-2 ring-primary/30 animate-pulse'
              : 'border-border hover:border-primary/50',
            hasError && 'border-destructive',
            disabled && 'opacity-50 cursor-not-allowed hover:border-border',
          )}
        >
          <span className={isRecording && pressedKeys.size === 0 ? 'text-muted-foreground' : ''}>
            {displayValue}
          </span>
        </div>

        <button
          type="button"
          onClick={startRecording}
          disabled={disabled || isRecording}
          className={cn(
            'px-4 py-2.5 rounded-lg text-sm font-medium transition-colors',
            'bg-secondary text-secondary-foreground hover:bg-secondary/80',
            'focus:outline-none focus:ring-2 focus:ring-primary/50',
            (disabled || isRecording) && 'opacity-50 cursor-not-allowed',
          )}
        >
          {isRecording ? t('hotkey.recording') : t('hotkey.record')}
        </button>
      </div>

      {description && !hasError && <p className="text-xs text-muted-foreground">{description}</p>}

      {hasError && errorMessage && <p className="text-xs text-destructive">{errorMessage}</p>}
    </div>
  )
}
