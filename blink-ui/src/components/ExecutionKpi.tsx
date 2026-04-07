import { fmt } from '../lib/format'
import type { MetricsResponse } from '../types'

interface Props {
  portfolio: {
    fill_rate_pct: number
    reject_rate_pct?: number
    avg_slippage_bps?: number
  } | null
  metrics: MetricsResponse | null
  className?: string
}

function color(value: number, thresholds: [number, number], invert = false): string {
  const [lo, hi] = thresholds
  const isGood = invert ? value > hi : value < lo
  const isMid = invert ? value > lo : value < hi
  if (isGood) return 'text-emerald-400'
  if (isMid) return 'text-amber-400'
  return 'text-red-400'
}

export default function ExecutionKpi({ portfolio, metrics, className }: Props) {
  const fillRate = portfolio?.fill_rate_pct ?? 0
  const rejectRate = portfolio?.reject_rate_pct ?? 0
  const slippage = portfolio?.avg_slippage_bps ?? 0
  const rejections60s = metrics?.signals_rejected_last_60s ?? 0

  return (
    <div className={`grid grid-cols-2 gap-3 ${className ?? ''}`}>
      <div className="bg-surface-900 rounded-lg px-3 py-2">
        <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Fill Rate</div>
        <div className={`font-mono font-bold text-lg ${color(fillRate, [60, 80], true)}`}>
          {fmt(fillRate, 1)}%
        </div>
      </div>

      <div className="bg-surface-900 rounded-lg px-3 py-2">
        <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Reject Rate</div>
        <div className={`font-mono font-bold text-lg ${color(rejectRate, [10, 20])}`}>
          {fmt(rejectRate, 1)}%
        </div>
      </div>

      <div className="bg-surface-900 rounded-lg px-3 py-2">
        <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Avg Slippage</div>
        <div className={`font-mono font-bold text-lg ${color(slippage, [5, 15])}`}>
          {fmt(slippage, 1)} bps
        </div>
      </div>

      <div className="bg-surface-900 rounded-lg px-3 py-2">
        <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Rejections/60s</div>
        <div className="font-mono font-bold text-lg text-slate-200">
          {rejections60s}
        </div>
      </div>
    </div>
  )
}
