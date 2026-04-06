import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import { fmt } from '../lib/format'
import { ShieldCheck, ShieldOff, Clock } from 'lucide-react'

export default function FailsafePanel() {
  const { data, error } = usePoll(api.failsafe, 5_000)
  const { data: fillWindow } = usePoll(api.fillWindow, 5_000)

  const isPaperMode = !data || error

  if (isPaperMode) {
    // Paper mode — show fill window data instead
    const fw = fillWindow
    return (
      <div className="card border-slate-700/40">
        <div className="flex items-center justify-between mb-3">
          <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
            Fill Window
          </span>
          <span className="badge badge-neutral">PAPER</span>
        </div>
        {!fw?.available ? (
          <p className="text-slate-600 text-xs text-center py-3">No active fill window</p>
        ) : (
          <div className="space-y-2 text-xs">
            <div className="flex justify-between">
              <span className="text-slate-500">Side</span>
              <span className="font-mono text-slate-300">{fw.side ?? '—'}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Entry</span>
              <span className="font-mono text-slate-300">${fmt(fw.entry_price ?? 0, 4)}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Current</span>
              <span className="font-mono text-slate-300">
                {fw.current_price != null ? `$${fmt(fw.current_price, 4)}` : '—'}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Drift</span>
              <span className={`font-mono font-semibold ${Math.abs(fw.drift_pct ?? 0) > 2 ? 'text-red-400' : 'text-slate-300'}`}>
                {fw.drift_pct != null ? `${fmt(fw.drift_pct, 2)}%` : '—'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-slate-500 flex items-center gap-1"><Clock size={10} /> Countdown</span>
              <span className="font-mono text-amber-400">
                {fw.countdown_secs != null ? `${Math.ceil(fw.countdown_secs)}s` : '—'}
              </span>
            </div>
          </div>
        )}
      </div>
    )
  }

  const {
    confirmation_rate_pct,
    confirmed_fills,
    no_fills,
    stale_orders,
    trigger_count,
    check_count,
  } = data

  const rate = confirmation_rate_pct ?? 0
  const totalOrders = check_count

  const rateColor =
    rate >= 95 ? 'text-emerald-400'
    : rate >= 80 ? 'text-yellow-400'
    : 'text-red-400'

  return (
    <div className="card border-amber-900/40">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Failsafe (Live)
        </span>
        {trigger_count > 0 ? (
          <span className="badge badge-danger flex items-center gap-1">
            <ShieldOff size={10} /> TRIGGERED ×{trigger_count}
          </span>
        ) : (
          <span className="badge badge-ok flex items-center gap-1">
            <ShieldCheck size={10} /> OK
          </span>
        )}
      </div>

      <div className="mb-3">
        <div className="flex justify-between text-xs mb-1">
          <span className="text-slate-500">Confirmation rate</span>
          <span className={`font-mono font-bold ${rateColor}`}>
            {fmt(rate, 1)}%
          </span>
        </div>
        <div className="h-2 bg-surface-600 rounded-full overflow-hidden">
          <div
            className={`h-full rounded-full transition-all duration-500 ${
              rate >= 95 ? 'bg-emerald-500'
              : rate >= 80 ? 'bg-yellow-500'
              : 'bg-red-500'
            }`}
            style={{ width: `${rate}%` }}
          />
        </div>
      </div>

      <div className="grid grid-cols-3 gap-2 text-xs">
        <div className="bg-surface-900 rounded p-2">
          <div className="text-slate-500 mb-0.5">Checks</div>
          <div className="font-mono text-slate-200 font-semibold">{totalOrders}</div>
        </div>
        <div className="bg-surface-900 rounded p-2">
          <div className="text-slate-500 mb-0.5">Confirmed</div>
          <div className="font-mono text-emerald-400 font-semibold">{confirmed_fills}</div>
        </div>
        <div className="bg-surface-900 rounded p-2">
          <div className="text-slate-500 mb-0.5">No-fill/Stale</div>
          <div className={`font-mono font-semibold ${(no_fills + stale_orders) > 0 ? 'text-red-400' : 'text-slate-400'}`}>
            {no_fills + stale_orders}
          </div>
        </div>
      </div>
    </div>
  )
}
