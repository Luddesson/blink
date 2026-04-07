import { fmt } from '../lib/format'

interface Props {
  equityCurve: number[]
  className?: string
}

export default function DrawdownTracker({ equityCurve, className }: Props) {
  const insufficient = equityCurve.length < 2

  let maxDd = 0
  let currentDd = 0
  let hwm = 0

  if (!insufficient) {
    hwm = equityCurve[0]
    for (const v of equityCurve) {
      if (v > hwm) hwm = v
      const dd = hwm > 0 ? ((hwm - v) / hwm) * 100 : 0
      if (dd > maxDd) maxDd = dd
    }
    const last = equityCurve[equityCurve.length - 1]
    currentDd = hwm > 0 ? ((hwm - last) / hwm) * 100 : 0
  }

  function ddColor(dd: number) {
    if (dd > 5) return 'text-red-400'
    if (dd > 2) return 'text-amber-400'
    return 'text-emerald-400'
  }

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Drawdown
      </span>
      {insufficient ? (
        <p className="text-xs text-slate-500">Insufficient data</p>
      ) : (
        <div className="space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-500">Max Drawdown</div>
            <div className={`text-lg font-mono font-semibold ${ddColor(maxDd)}`}>
              -{fmt(maxDd)}%
            </div>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-500">Current DD</div>
            <div className={`text-sm font-mono ${ddColor(currentDd)}`}>
              -{fmt(currentDd)}%
            </div>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-500">High-Water Mark</div>
            <div className="text-sm font-mono text-slate-100">${fmt(hwm)}</div>
          </div>
        </div>
      )}
    </div>
  )
}
