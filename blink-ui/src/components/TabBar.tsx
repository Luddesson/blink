import { motion } from 'motion/react'
import { Sparkles, LineChart, History, Radar, TrendingUp, Settings, LayoutDashboard } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { TabId } from '../hooks/useTab'
import KeycapHint from './aurora/KeycapHint'
import { cn } from '../lib/cn'

const TAB_META: { id: TabId; label: string; key: string; icon: LucideIcon; accent?: boolean }[] = [
  { id: 'dashboard',    label: 'Dashboard', key: '1', icon: LayoutDashboard },
  { id: 'markets',      label: 'Markets',   key: '2', icon: LineChart },
  { id: 'history',      label: 'History',   key: '3', icon: History },
  { id: 'intelligence', label: 'Bullpen',   key: '4', icon: Radar },
  { id: 'performance',  label: 'Perf',      key: '5', icon: TrendingUp },
  { id: 'config',       label: 'Config',    key: '6', icon: Settings },
  { id: 'alpha',        label: 'Alpha AI',  key: '7', icon: Sparkles, accent: true },
]

interface Props {
  activeTab: TabId
  onSwitch: (tab: TabId) => void
}

export default function TabBar({ activeTab, onSwitch }: Props) {
  return (
    <nav
      role="tablist"
      aria-label="Main navigation"
      className={cn(
        'relative flex items-center gap-1 px-2 py-1.5 shrink-0 overflow-x-auto overscroll-x-contain [scrollbar-width:none] [-ms-overflow-style:none] sm:px-3',
        'border-b border-[color:var(--color-border-subtle)]',
        'bg-[color:oklch(0.14_0.013_260/0.55)] backdrop-blur-lg',
      )}
    >
      {TAB_META.map((t) => {
        const active = activeTab === t.id
        const Icon = t.icon
        return (
          <button
            key={t.id}
            onClick={() => onSwitch(t.id)}
            role="tab"
            aria-selected={active}
            aria-label={`Tab ${t.key}: ${t.label}`}
            className={cn(
              'relative snap-start flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-[11px] font-medium whitespace-nowrap transition-colors sm:px-3',
              active
                ? t.accent
                  ? 'text-[color:var(--color-aurora-1)]'
                  : 'text-[color:var(--color-text-primary)]'
                : t.accent
                  ? 'text-[color:oklch(0.75_0.18_170/0.6)] hover:text-[color:var(--color-aurora-1)]'
                  : 'text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)]',
            )}
          >
            {active && (
              <motion.span
                layoutId="tab-active"
                transition={{ type: 'spring', stiffness: 380, damping: 30 }}
                className={cn(
                  'absolute inset-0 rounded-md border',
                  t.accent
                    ? 'bg-[color:oklch(0.75_0.18_170/0.1)] border-[color:oklch(0.75_0.18_170/0.35)]'
                    : 'bg-[color:oklch(0.26_0.022_260/0.5)] border-[color:var(--color-border-subtle)]',
                )}
                style={{
                  boxShadow: t.accent
                    ? 'inset 0 1px 0 oklch(1 0 0 / 0.04), 0 0 18px -4px oklch(0.75 0.18 170 / 0.3)'
                    : 'inset 0 1px 0 oklch(1 0 0 / 0.04)',
                }}
              />
            )}
            <span className="relative flex items-center gap-1.5">
              <Icon size={12} strokeWidth={2} />
              <KeycapHint keys={t.key} className="hidden sm:inline-flex" tone={t.accent && active ? 'aurora' : 'muted'} />
              {t.label}
              {t.accent && active && (
                <span
                  className="w-1 h-1 rounded-full bg-[color:var(--color-aurora-1)] shadow-[0_0_6px_oklch(0.75_0.18_170/0.9)]"
                />
              )}
            </span>
          </button>
        )
      })}
    </nav>
  )
}
