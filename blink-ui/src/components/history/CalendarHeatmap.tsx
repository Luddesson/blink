import { useState, useMemo } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { fmt } from '../../lib/format'

interface Props {
  dailyPnl: Map<string, number>
  dailyTrades: Map<string, number>
}

const DAY_LABELS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

function dateKey(y: number, m: number, d: number): string {
  return `${y}-${String(m + 1).padStart(2, '0')}-${String(d).padStart(2, '0')}`
}

function pnlColor(pnl: number, maxAbs: number): string {
  if (maxAbs === 0) return 'bg-slate-800/40'
  const intensity = Math.min(Math.abs(pnl) / maxAbs, 1)
  if (pnl > 0) {
    if (intensity > 0.7) return 'bg-emerald-500/70'
    if (intensity > 0.4) return 'bg-emerald-500/45'
    if (intensity > 0.15) return 'bg-emerald-500/25'
    return 'bg-emerald-500/10'
  }
  if (pnl < 0) {
    if (intensity > 0.7) return 'bg-red-500/70'
    if (intensity > 0.4) return 'bg-red-500/45'
    if (intensity > 0.15) return 'bg-red-500/25'
    return 'bg-red-500/10'
  }
  return 'bg-slate-800/40'
}

export default function CalendarHeatmap({ dailyPnl, dailyTrades }: Props) {
  const [monthOffset, setMonthOffset] = useState(0)

  const { cells, monthLabel, maxAbs, monthPnl, monthTrades } = useMemo(() => {
    const now = new Date()
    const d = new Date(now.getFullYear(), now.getMonth() + monthOffset, 1)
    const year = d.getFullYear()
    const month = d.getMonth()
    const daysInMonth = new Date(year, month + 1, 0).getDate()
    const firstDow = (new Date(year, month, 1).getDay() + 6) % 7 // Mon=0

    let maxAbs = 0
    let monthPnl = 0
    let monthTrades = 0
    const cells: { day: number; key: string; pnl: number; trades: number }[] = []

    for (let i = 0; i < firstDow; i++) {
      cells.push({ day: 0, key: `empty-${i}`, pnl: 0, trades: 0 })
    }
    for (let day = 1; day <= daysInMonth; day++) {
      const key = dateKey(year, month, day)
      const pnl = dailyPnl.get(key) ?? 0
      const trades = dailyTrades.get(key) ?? 0
      cells.push({ day, key, pnl, trades })
      if (Math.abs(pnl) > maxAbs) maxAbs = Math.abs(pnl)
      monthPnl += pnl
      monthTrades += trades
    }

    const monthLabel = d.toLocaleString('en-US', { month: 'long', year: 'numeric' })
    return { year, month, cells, monthLabel, maxAbs, monthPnl, monthTrades }
  }, [monthOffset, dailyPnl, dailyTrades])

  const today = new Date()
  const todayKey = dateKey(today.getFullYear(), today.getMonth(), today.getDate())

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Daily P&L Calendar
        </span>
        <div className="flex items-center gap-3">
          <span className={`text-xs font-mono font-semibold ${monthPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            {monthPnl >= 0 ? '+' : ''}${fmt(monthPnl)} ({monthTrades} trades)
          </span>
          <div className="flex items-center gap-1">
            <button onClick={() => setMonthOffset(o => o - 1)} className="text-slate-500 hover:text-slate-300">
              <ChevronLeft size={14} />
            </button>
            <span className="text-xs text-slate-400 w-[120px] text-center">{monthLabel}</span>
            <button
              onClick={() => setMonthOffset(o => Math.min(0, o + 1))}
              disabled={monthOffset >= 0}
              className="text-slate-500 hover:text-slate-300 disabled:opacity-30"
            >
              <ChevronRight size={14} />
            </button>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-7 gap-1">
        {DAY_LABELS.map(d => (
          <div key={d} className="text-[9px] text-slate-600 text-center font-medium py-0.5">{d}</div>
        ))}
        {cells.map(cell => (
          <div
            key={cell.key}
            className={`
              relative aspect-square rounded-sm flex flex-col items-center justify-center
              ${cell.day === 0 ? '' : pnlColor(cell.pnl, maxAbs)}
              ${cell.key === todayKey ? 'ring-1 ring-cyan-400/60' : ''}
              group
            `}
          >
            {cell.day > 0 && (
              <>
                <span className="text-[10px] text-slate-400 font-mono">{cell.day}</span>
                {cell.trades > 0 && (
                  <span className={`text-[8px] font-mono font-semibold ${cell.pnl >= 0 ? 'text-emerald-300' : 'text-red-300'}`}>
                    {cell.pnl >= 0 ? '+' : ''}{fmt(cell.pnl, 2)}
                  </span>
                )}
                {cell.trades > 0 && (
                  <div className="absolute z-20 bottom-full left-1/2 -translate-x-1/2 mb-1 hidden group-hover:block
                    bg-slate-900 border border-slate-700 rounded px-2 py-1 text-[10px] text-slate-300 whitespace-nowrap shadow-lg">
                    <div className={cell.pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}>
                      P&L: {cell.pnl >= 0 ? '+' : ''}${fmt(cell.pnl)}
                    </div>
                    <div className="text-slate-500">{cell.trades} trade{cell.trades > 1 ? 's' : ''}</div>
                  </div>
                )}
              </>
            )}
          </div>
        ))}
      </div>

      <div className="flex items-center justify-center gap-2 mt-2">
        <span className="text-[9px] text-slate-600">Loss</span>
        <div className="flex gap-0.5">
          <div className="w-3 h-3 rounded-sm bg-red-500/70" />
          <div className="w-3 h-3 rounded-sm bg-red-500/45" />
          <div className="w-3 h-3 rounded-sm bg-red-500/25" />
          <div className="w-3 h-3 rounded-sm bg-slate-800/40" />
          <div className="w-3 h-3 rounded-sm bg-emerald-500/25" />
          <div className="w-3 h-3 rounded-sm bg-emerald-500/45" />
          <div className="w-3 h-3 rounded-sm bg-emerald-500/70" />
        </div>
        <span className="text-[9px] text-slate-600">Profit</span>
      </div>
    </div>
  )
}
