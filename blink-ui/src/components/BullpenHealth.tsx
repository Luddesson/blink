import type { BullpenHealthResponse } from '../types'
import { fmt } from '../lib/format'

interface Props { health: BullpenHealthResponse | null }

export default function BullpenHealth({ health }: Props) {
  if (!health || !health.enabled) {
    return (
      <div className="card opacity-50">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Bullpen
        </span>
        <p className="text-[11px] text-slate-600 mt-1">Disabled</p>
      </div>
    )
  }

  const ok = health.authenticated && (health.consecutive_failures ?? 0) < 3
  const statusColor = ok ? 'text-emerald-400' : 'text-red-400'
  const statusDot = ok ? 'bg-emerald-400' : 'bg-red-400'

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Bullpen
        </span>
        <span className={`flex items-center gap-1 text-[10px] font-medium ${statusColor}`}>
          <span className={`w-1.5 h-1.5 rounded-full ${statusDot}`} />
          {ok ? 'Connected' : 'Degraded'}
        </span>
      </div>

      <div className="space-y-1 text-[11px] text-slate-400">
        <div className="flex justify-between">
          <span>Latency</span>
          <span className="text-slate-200">{fmt(health.avg_latency_ms ?? 0, 0)}ms</span>
        </div>
        <div className="flex justify-between">
          <span>Calls</span>
          <span className="text-slate-200">{health.total_calls ?? 0}</span>
        </div>
        {(health.consecutive_failures ?? 0) > 0 && (
          <div className="flex justify-between">
            <span>Failures</span>
            <span className="text-red-400">{health.consecutive_failures}</span>
          </div>
        )}
        {health.last_error && (
          <p className="text-red-400/80 truncate" title={health.last_error}>
            {health.last_error}
          </p>
        )}
      </div>
    </div>
  )
}
