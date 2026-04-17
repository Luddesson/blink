import { Chip } from '../ui/Chip'
import type { TradeFilters } from '../../hooks/useTradeStats'

interface Props {
  filters: TradeFilters
  onChange: (f: TradeFilters) => void
  totalTrades: number
}

type DatePreset = '1d' | '7d' | '30d' | 'all'

function getDateRange(preset: DatePreset): [Date, Date] | undefined {
  if (preset === 'all') return undefined
  const now = new Date()
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 23, 59, 59, 999)
  const start = new Date(end)
  if (preset === '1d') start.setDate(start.getDate() - 1)
  else if (preset === '7d') start.setDate(start.getDate() - 7)
  else if (preset === '30d') start.setDate(start.getDate() - 30)
  start.setHours(0, 0, 0, 0)
  return [start, end]
}

function getActivePreset(range?: [Date, Date]): DatePreset {
  if (!range) return 'all'
  const diffMs = range[1].getTime() - range[0].getTime()
  const diffDays = Math.round(diffMs / 86400000)
  if (diffDays <= 1) return '1d'
  if (diffDays <= 7) return '7d'
  if (diffDays <= 30) return '30d'
  return 'all'
}

export default function FilterBar({ filters, onChange, totalTrades }: Props) {
  const datePreset = getActivePreset(filters.dateRange)

  return (
    <div className="flex items-center gap-4 px-3 py-2 bg-surface-950 border-b border-slate-800/60 rounded-lg">
      <div className="flex items-center gap-1">
        <span className="text-[10px] uppercase tracking-wider text-slate-600 mr-1">Period</span>
        {(['1d', '7d', '30d', 'all'] as DatePreset[]).map(p => (
          <Chip
            key={p}
            label={p === 'all' ? 'All' : p.toUpperCase()}
            active={datePreset === p}
            onClick={() => onChange({ ...filters, dateRange: getDateRange(p) })}
          />
        ))}
      </div>

      <div className="w-px h-5 bg-slate-800" />

      <div className="flex items-center gap-1">
        <span className="text-[10px] uppercase tracking-wider text-slate-600 mr-1">Source</span>
        {(['all', 'rn1', 'alpha'] as const).map(s => (
          <Chip
            key={s}
            label={s === 'all' ? 'All' : s.toUpperCase()}
            active={(filters.signalSource ?? 'all') === s}
            onClick={() => onChange({ ...filters, signalSource: s })}
            variant={s === 'alpha' ? 'signal' : s === 'rn1' ? 'whale' : 'default'}
          />
        ))}
      </div>

      <div className="w-px h-5 bg-slate-800" />

      <div className="flex items-center gap-1">
        <span className="text-[10px] uppercase tracking-wider text-slate-600 mr-1">Side</span>
        {(['all', 'buy', 'sell'] as const).map(s => (
          <Chip
            key={s}
            label={s === 'all' ? 'All' : s.toUpperCase()}
            active={(filters.side ?? 'all') === s}
            onClick={() => onChange({ ...filters, side: s })}
            variant={s === 'buy' ? 'bull' : s === 'sell' ? 'bear' : 'default'}
          />
        ))}
      </div>

      <div className="flex-1" />
      <span className="text-[10px] text-slate-600 font-mono">{totalTrades} trades</span>
    </div>
  )
}
