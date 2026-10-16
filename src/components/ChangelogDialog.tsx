import { useTranslation } from 'react-i18next'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { parseChangelog } from '@/lib/changelog'
import changelog from '../../CHANGELOG.md?raw'

const CHANGELOG_ENTRIES = parseChangelog(changelog)

interface ChangelogDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function ChangelogDialog({ open, onOpenChange }: ChangelogDialogProps) {
  const { t } = useTranslation()

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="no-drag max-h-[85vh] sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t('changelog.title')}</DialogTitle>
          <DialogDescription>{t('changelog.description')}</DialogDescription>
        </DialogHeader>
        <div className="vk-scroll -mr-2 max-h-[60vh] overflow-auto pr-2">
          {CHANGELOG_ENTRIES.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted-foreground">{t('changelog.empty')}</p>
          ) : (
            <div className="flex flex-col gap-4">
              {CHANGELOG_ENTRIES.map((entry) => (
                <article key={entry.version} className="rounded-xl border bg-card p-4 shadow-sm">
                  <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                    <h3 className="text-[15px] font-semibold text-foreground">v{entry.version}</h3>
                    {entry.date ? (
                      <span className="text-xs tabular-nums text-muted-foreground">
                        {entry.date}
                      </span>
                    ) : null}
                  </div>
                  <div className="whitespace-pre-wrap text-[13px] leading-relaxed text-muted-foreground">
                    {entry.body}
                  </div>
                </article>
              ))}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
