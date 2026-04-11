import type { TabId } from '../hooks/useTab'
import Kbd from './shared/Kbd'

const TAB_META: { id: TabId; label: string; key: string }[] = [
  { id: 'dashboard',    label: 'Dashboard',    key: '1' },
  { id: 'markets',      label: 'Markets',      key: '2' },
  { id: 'history',      label: 'History',       key: '3' },
  { id: 'intelligence', label: 'Intelligence', key: '4' },
  { id: 'performance',  label: 'Performance',  key: '5' },
  { id: 'backtest',     label: 'Backtest',     key: '6' },
  { id: 'config',       label: 'Config',       key: '7' },
]

interface Props {
  activeTab: TabId
  onSwitch: (tab: TabId) => void
}

export default function TabBar({ activeTab, onSwitch }: Props) {
  return (
    <nav role="tablist" aria-label="Main navigation" className="flex items-center gap-0.5 px-2 py-1 bg-surface-900 border-b border-slate-800 shrink-0 overflow-x-auto">
      {TAB_META.map((t) => {
        const active = activeTab === t.id
        return (
          <button
            key={t.id}
            onClick={() => onSwitch(t.id)}
            role="tab"
            aria-selected={active}
            aria-label={`Tab ${t.key}: ${t.label}`}
            className={`
              flex items-center gap-1.5 px-3 py-1.5 rounded text-[11px] font-medium
              transition-colors whitespace-nowrap
              ${active
                ? 'bg-slate-800 text-slate-100 border border-slate-700'
                : 'text-slate-500 hover:text-slate-300 hover:bg-slate-800/50 border border-transparent'
              }
            `}
          >
            <Kbd shortcut={t.key} />
            {t.label}
          </button>
        )
      })}
    </nav>
  )
}
