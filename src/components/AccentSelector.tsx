import { Check } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'
import { ACCENT_OPTIONS, type Accent } from '@/lib/useTheme'

interface AccentSelectorProps {
  value: Accent
  onChange: (accent: Accent) => void
}

export function AccentSelector({ value, onChange }: AccentSelectorProps) {
  const { t } = useTranslation()

  return (
    <div role="radiogroup" aria-label={t('theme.accent.label')} className="flex flex-wrap gap-3">
      {ACCENT_OPTIONS.map((option) => {
        const selected = value === option.id
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-label={t(option.nameKey)}
            onClick={() => onChange(option.id)}
            className={cn(
              'no-drag flex min-w-[96px] max-w-[148px] flex-1 items-center gap-2.5 rounded-xl border bg-card p-2.5 text-left transition-all hover:border-primary/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
              selected && 'border-primary ring-1 ring-primary',
            )}
          >
            <span
              className="grid h-7 w-7 shrink-0 place-items-center rounded-full border shadow-sm"
              style={{
                background: option.swatch,
                borderColor: 'oklch(1 0 0 / 0.25)',
              }}
            >
              {selected ? <Check className="h-4 w-4 text-white drop-shadow-sm" /> : null}
            </span>
            <span className="truncate text-[12.5px] font-medium text-foreground">
              {t(option.nameKey)}
            </span>
          </button>
        )
      })}
    </div>
  )
}
