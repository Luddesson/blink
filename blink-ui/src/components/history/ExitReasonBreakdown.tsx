import { useState } from 'react'
import { fmt, fmtPnl } from '../../lib/format'
import type { ReasonStats } from '../../hooks/useTradeStats'

interface Props {
  byExitReason: Record<string, ReasonStats>
}

export default function ExitReasonBreakdown({ byExitReason }: Props) {
  const [expanded, setExpanded] = useState(false)
  const entries = Object.entries(byExitReason).sort((a, b) => Math.abs(b[1].totalPnl) - Math.abs(a[1].totalPnl))
  const maxAbs = Math.max(...entries.map(([, v]) => Math.abs(v.totalPnl)), 0.01)
  const visibleEntries = entries.slice(0, expanded ? entries.length : 5)
  const hiddenCount = entries.length - 5

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Exit Reason Breakdown
      </span>
      {entries.length === 0 ? (
        <p className="text-xs text-slate-500">No data</p>
      ) : (
        <>
          <div className="space-y-1">
            {visibleEntries.map(([reason, stats]) => {
              const pct = maxAbs > 0 ? (Math.abs(stats.totalPnl) / maxAbs) * 100 : 0
              const positive = stats.totalPnl >= 0
              return (
                <div key={reason} className="relative">
                  <div
                    className={`absolute inset-y-0 left-0 rounded ${positive ? 'bg-emerald-500/30' : 'bg-red-500/30'}`}
                    style={{ width: `${pct}%` }}
                  />
                  <div className="relative flex items-center justify-between px-2 py-1.5">
                    <span className="text-xs text-slate-300 truncate max-w-[40%]">{reason}</span>
                    <div className="flex items-center gap-3">
                      <span className="text-[10px] text-slate-500 font-mono">{stats.count} trades</span>
                      <span className={`text-[10px] font-mono ${stats.winRate >= 50 ? 'text-emerald-400' : 'text-red-400'}`}>
                        {fmt(stats.winRate, 0)}% WR
                      </span>
                      <span className={`text-xs font-mono font-semibold ${positive ? 'text-emerald-400' : 'text-red-400'}`}>
                        ${fmtPnl(stats.totalPnl)}
                      </span>
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
          {entries.length > 5 && (
            <button
              onClick={() => setExpanded(!expanded)}
              className="mt-3 text-xs font-medium text-slate-400 hover:text-slate-300 transition-colors"
            >
              {expanded ? 'Show less ↑' : `Show ${hiddenCount} more ↓`}
            </button>
          )}
        </>
      )}
    </div>
  )
}
