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
      <div className="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Daily P&L Calendar
        </span>
        <div className="flex flex-wrap items-center gap-2 sm:gap-3">
          <span className={`text-xs font-mono font-semibold ${monthPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            {monthPnl >= 0 ? '+' : ''}${fmt(monthPnl)} ({monthTrades} trades)
          </span>
          <div className="flex items-center gap-1">
            <button onClick={() => setMonthOffset(o => o - 1)} className="text-slate-500 hover:text-slate-300">
              <ChevronLeft size={14} />
            </button>
            <span className="w-[120px] text-center text-xs text-slate-400">{monthLabel}</span>
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

      <div className="mx-auto w-full max-w-4xl">
        <div className="grid grid-cols-7 gap-1.5 sm:gap-2">
          {DAY_LABELS.map(d => (
            <div key={d} className="py-1 text-center text-[10px] font-medium text-slate-600 sm:text-xs">
              {d}
            </div>
          ))}
          {cells.map(cell => (
            <div
              key={cell.key}
              className={`
                relative aspect-square min-h-[3.1rem] rounded-md border border-slate-800/40 p-1 sm:min-h-[4.25rem] sm:rounded-lg sm:p-1.5
                flex flex-col items-center justify-center
                ${cell.day === 0 ? '' : pnlColor(cell.pnl, maxAbs)}
                ${cell.key === todayKey ? 'ring-1 ring-cyan-400/60' : ''}
                group
              `}
            >
              {cell.day > 0 && (
                <>
                  <span className="text-[10px] font-mono text-slate-400 sm:text-xs">{cell.day}</span>
                  {cell.trades > 0 && (
                    <span className={`mt-0.5 text-center text-[8px] font-mono font-semibold leading-tight sm:text-[10px] ${cell.pnl >= 0 ? 'text-emerald-300' : 'text-red-300'}`}>
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
      </div>

      <div className="mt-3 flex flex-wrap items-center justify-center gap-2">
        <span className="text-[9px] text-slate-600 sm:text-[10px]">Loss</span>
        <div className="flex gap-1">
          <div className="h-3 w-3 rounded-sm bg-red-500/70 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-red-500/45 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-red-500/25 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-slate-800/40 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-emerald-500/25 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-emerald-500/45 sm:h-3.5 sm:w-3.5" />
          <div className="h-3 w-3 rounded-sm bg-emerald-500/70 sm:h-3.5 sm:w-3.5" />
        </div>
        <span className="text-[9px] text-slate-600 sm:text-[10px]">Profit</span>
      </div>
    </div>
  )
}
