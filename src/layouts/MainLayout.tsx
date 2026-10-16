import { ReactNode, useCallback, useState } from 'react'
import { ChevronLeft, ChevronRight, Home, Settings, History, Sun, Moon } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'
import { useTheme } from '@/lib/useTheme'
import TitleBar from '@/components/TitleBar'
import { ChangelogDialog } from '@/components/ChangelogDialog'

interface MainLayoutProps {
  children: ReactNode
  currentRoute: string
}

const SIDEBAR_COLLAPSED_KEY = 'vk-sidebar-collapsed'

function getInitialCollapsed(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === 'true'
  } catch {
    return false
  }
}

function persistCollapsed(collapsed: boolean) {
  try {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed))
  } catch {
    /* ignore */
  }
}

export default function MainLayout({ children, currentRoute }: MainLayoutProps) {
  const { t } = useTranslation()
  const { theme, setTheme } = useTheme()
  const [collapsed, setCollapsed] = useState(getInitialCollapsed)
  const [changelogOpen, setChangelogOpen] = useState(false)

  const navigate = (path: string) => {
    window.location.hash = path
  }

  const toggleCollapsed = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev
      persistCollapsed(next)
      return next
    })
  }, [])

  const navItems = [
    { path: '/home', label: t('nav.home'), icon: Home },
    { path: '/settings', label: t('nav.settings'), icon: Settings },
    { path: '/history', label: t('nav.history'), icon: History },
  ]

  return (
    <div className="flex h-screen flex-col bg-sidebar">
      {/* 自定义标题栏，覆盖整个窗口顶部 */}
      <TitleBar />

      <div className="flex flex-1 overflow-hidden">
        {/* 左侧边栏 */}
        <aside
          className={cn(
            'relative z-20 flex h-full flex-col bg-sidebar pb-4 transition-[width,padding] duration-200 ease-[cubic-bezier(0.23,1,0.32,1)]',
            collapsed ? 'w-6 overflow-hidden px-0' : 'w-[220px] px-3.5',
          )}
        >
          <div
            className={cn(
              'flex flex-1 flex-col transition-opacity duration-200',
              collapsed && 'invisible opacity-0',
            )}
            aria-hidden={collapsed}
          >
            {/* 导航 */}
            <nav className="flex flex-col gap-1 pt-1">
              {navItems.map((item) => {
                const Icon = item.icon
                const isActive =
                  currentRoute === item.path ||
                  (currentRoute === '/' && item.path === '/home') ||
                  (currentRoute === '' && item.path === '/home')
                return (
                  <button
                    key={item.path}
                    type="button"
                    onClick={() => navigate(item.path)}
                    className={cn(
                      'no-drag relative flex cursor-pointer items-center gap-3 rounded-[10px] px-3 py-2 text-[13.5px] font-medium transition-colors',
                      isActive
                        ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                        : 'text-sidebar-foreground/70 hover:bg-accent hover:text-sidebar-foreground',
                    )}
                  >
                    {isActive && (
                      <span className="absolute -left-3.5 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-primary" />
                    )}
                    <Icon className="h-[18px] w-[18px] opacity-90" />
                    {item.label}
                  </button>
                )
              })}
            </nav>

            <div className="flex-1" />

            {/* 版本 + 主题切换 */}
            <div className="mt-3 flex items-center justify-between px-1">
              <button
                type="button"
                onClick={() => setChangelogOpen(true)}
                title={t('changelog.openTitle')}
                className="no-drag inline-flex items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2 py-0.5 text-[11px] font-medium text-primary transition-colors hover:bg-primary/15"
                aria-label={t('changelog.openTitle')}
              >
                <span className="h-1.5 w-1.5 rounded-full bg-primary" />
                <span className="font-mono">v{__APP_VERSION__}</span>
              </button>
              <div className="no-drag flex items-center gap-0.5 rounded-full border bg-secondary p-0.5">
                <button
                  type="button"
                  onClick={() => setTheme('light')}
                  title={t('theme.light')}
                  className={cn(
                    'grid h-[22px] w-[26px] cursor-pointer place-items-center rounded-full transition-colors',
                    theme === 'light'
                      ? 'bg-card text-primary shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  <Sun className="h-[14px] w-[14px]" />
                </button>
                <button
                  type="button"
                  onClick={() => setTheme('dark')}
                  title={t('theme.dark')}
                  className={cn(
                    'grid h-[22px] w-[26px] cursor-pointer place-items-center rounded-full transition-colors',
                    theme === 'dark'
                      ? 'bg-card text-primary shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  <Moon className="h-[14px] w-[14px]" />
                </button>
              </div>
            </div>
          </div>

          {/* 折叠/展开按钮 - 贴边，位于侧栏右侧中部 */}
          <button
            type="button"
            onClick={toggleCollapsed}
            aria-label={t(collapsed ? 'nav.expandSidebar' : 'nav.collapseSidebar')}
            title={t(collapsed ? 'nav.expandSidebar' : 'nav.collapseSidebar')}
            className="no-drag absolute right-0 top-1/2 z-30 flex h-6 w-6 -translate-y-1/2 translate-x-1/2 cursor-pointer items-center justify-center rounded-full border border-primary/20 bg-primary text-primary-foreground shadow-md transition-transform duration-200 ease-[cubic-bezier(0.23,1,0.32,1)] hover:scale-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
          >
            {collapsed ? (
              <ChevronRight className="h-3.5 w-3.5" strokeWidth={2.5} />
            ) : (
              <ChevronLeft className="h-3.5 w-3.5" strokeWidth={2.5} />
            )}
          </button>

          <ChangelogDialog open={changelogOpen} onOpenChange={setChangelogOpen} />
        </aside>

        {/* 右侧内容 */}
        <main className="flex-1 p-3">
          <div className="h-full overflow-hidden rounded-2xl border bg-background">
            <div className="vk-scroll h-full overflow-auto">{children}</div>
          </div>
        </main>
      </div>
    </div>
  )
}
