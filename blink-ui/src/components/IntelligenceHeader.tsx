import type { BullpenHealthResponse, BullpenDiscoveryResponse } from '../types'
import { fmt } from '../lib/format'

interface Props {
  health: BullpenHealthResponse | null
  discovery: BullpenDiscoveryResponse | null
}

export default function IntelligenceHeader({ health, discovery }: Props) {
  if (!health || !health.enabled) {
    return (
      <div className="bg-slate-900/80 border-b border-slate-800 px-4 py-2">
        <span className="text-xs text-slate-500">
          Bullpen Disabled — configure <code className="text-slate-400">BULLPEN_ENABLED=true</code>
        </span>
      </div>
    )
  }

  const ok = health.authenticated && (health.consecutive_failures ?? 0) < 3
  const dotColor = ok ? 'bg-emerald-400' : 'bg-red-400'

  return (
    <div className="bg-slate-900/80 border-b border-slate-800 px-4 py-2 flex items-center gap-4 text-[11px]">
      {/* Status + title */}
      <div className="flex items-center gap-2">
        <span className={`w-2 h-2 rounded-full ${dotColor} shrink-0`} />
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-300">
          Bullpen Intelligence
        </span>
      </div>

      {/* Divider */}
      <span className="w-px h-4 bg-slate-700" />

      {/* Latency badge */}
      <span className="px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 tabular-nums">
        {fmt(health.avg_latency_ms ?? 0, 0)}ms
      </span>

      {/* Stats */}
      <span className="text-slate-500">
        <span className="text-slate-300 tabular-nums">{health.total_calls ?? 0}</span> calls
      </span>

      <span className="text-slate-500">
        <span className="text-slate-300 tabular-nums">{discovery?.total_markets ?? 0}</span> markets
      </span>

      <span className="text-slate-500">
        scan <span className="text-slate-300 tabular-nums">#{discovery?.scan_count ?? 0}</span>
      </span>

      {/* Failures (if any) */}
      {(health.consecutive_failures ?? 0) > 0 && (
        <span className="ml-auto text-red-400">
          {health.consecutive_failures} consecutive failures
        </span>
      )}

      {health.last_error && (
        <span
          className="text-red-400/70 truncate max-w-[200px] ml-auto"
          title={health.last_error}
        >
          {health.last_error}
        </span>
      )}
    </div>
  )
}
