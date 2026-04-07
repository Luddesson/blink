import { fmt } from '../lib/format'
import type { TwinSnapshot } from '../types'

interface Props {
  mainNav: number
  mainReturn: number
  mainWinRate: number
  mainDrawdown: number
  twin: TwinSnapshot | null
  className?: string
}

function winnerClass(main: number, twin: number, higherIsBetter: boolean): [string, string] {
  if (higherIsBetter) {
    if (main > twin) return ['text-emerald-400', 'text-slate-400']
    if (twin > main) return ['text-slate-400', 'text-emerald-400']
  } else {
    if (main < twin) return ['text-emerald-400', 'text-slate-400']
    if (twin < main) return ['text-slate-400', 'text-emerald-400']
  }
  return ['text-slate-200', 'text-slate-200']
}

export default function TwinComparison({
  mainNav, mainReturn, mainWinRate, mainDrawdown, twin, className,
}: Props) {
  const rows: { label: string; main: string; twinVal: string; mainRaw: number; twinRaw: number; higherBetter: boolean }[] = [
    { label: 'NAV',            main: `$${fmt(mainNav)}`,           twinVal: twin ? `$${fmt(twin.nav)}` : '—',               mainRaw: mainNav,      twinRaw: twin?.nav ?? 0,              higherBetter: true },
    { label: 'Return %',       main: `${fmt(mainReturn)}%`,        twinVal: twin ? `${fmt(twin.nav_return_pct)}%` : '—',     mainRaw: mainReturn,   twinRaw: twin?.nav_return_pct ?? 0,   higherBetter: true },
    { label: 'Win Rate %',     main: `${fmt(mainWinRate)}%`,       twinVal: twin ? `${fmt(twin.win_rate_pct)}%` : '—',       mainRaw: mainWinRate,  twinRaw: twin?.win_rate_pct ?? 0,     higherBetter: true },
    { label: 'Max Drawdown %', main: `${fmt(mainDrawdown)}%`,      twinVal: twin ? `${fmt(twin.max_drawdown_pct)}%` : '—',   mainRaw: mainDrawdown, twinRaw: twin?.max_drawdown_pct ?? 0, higherBetter: false },
    { label: 'Filled Orders',  main: '—',                          twinVal: twin ? `${twin.filled_orders}` : '—',            mainRaw: 0,            twinRaw: twin?.filled_orders ?? 0,    higherBetter: true },
  ]

  return (
    <div className={`card ${className ?? ''}`}>
      {/* Header */}
      <div className="grid grid-cols-3 gap-2 mb-3">
        <div />
        <div className="text-[10px] font-semibold uppercase tracking-widest text-slate-500 text-center">
          Main Engine
        </div>
        <div className="text-[10px] font-semibold uppercase tracking-widest text-slate-500 text-center">
          Blink Twin
          {twin && (
            <span className="ml-1 text-slate-600 normal-case tracking-normal">
              gen {twin.generation}
            </span>
          )}
        </div>
      </div>

      {twin === null ? (
        <div className="grid grid-cols-3 gap-2">
          {rows.map((r) => (
            <div key={r.label} className="contents">
              <span className="text-xs text-slate-400 py-1">{r.label}</span>
              <span className="text-xs font-mono text-slate-200 text-center py-1">{r.main}</span>
              <span className="text-xs font-mono text-slate-600 text-center py-1">Twin not active</span>
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-3 gap-2">
          {rows.map((r) => {
            const [mc, tc] = winnerClass(r.mainRaw, r.twinRaw, r.higherBetter)
            return (
              <div key={r.label} className="contents">
                <span className="text-xs text-slate-400 py-1">{r.label}</span>
                <span className={`text-xs font-mono text-center py-1 ${mc}`}>{r.main}</span>
                <span className={`text-xs font-mono text-center py-1 ${tc}`}>{r.twinVal}</span>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
