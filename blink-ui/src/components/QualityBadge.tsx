import { fmt } from '../lib/format'
import type { LatencyResponse } from '../types'

interface Props {
  latency: LatencyResponse | null
  className?: string
}

type Quality = 'WORLD-CLASS' | 'GOOD' | 'DEGRADED' | 'WARMING UP'

const BADGE_COLORS: Record<Quality, string> = {
  'WORLD-CLASS': 'bg-emerald-500/20 text-emerald-400 ring-emerald-500/30',
  'GOOD':        'bg-blue-500/20 text-blue-400 ring-blue-500/30',
  'DEGRADED':    'bg-red-500/20 text-red-400 ring-red-500/30',
  'WARMING UP':  'bg-slate-500/20 text-slate-400 ring-slate-500/30',
}

function computeQuality(latency: LatencyResponse | null): Quality {
  if (!latency?.signal_age) return 'WARMING UP'
  const { avg_us, p99_us, count } = latency.signal_age
  if ((count ?? 0) < 10) return 'WARMING UP'
  const avg = avg_us ?? Infinity
  const p99 = p99_us ?? Infinity
  if (avg <= 5_000 && p99 <= 120_000) return 'WORLD-CLASS'
  if (avg <= 20_000) return 'GOOD'
  return 'DEGRADED'
}

export default function QualityBadge({ latency, className }: Props) {
  const quality = computeQuality(latency)
  const throughput = latency?.ws_msg_per_sec ?? 0

  return (
    <div className={`flex items-center gap-2 ${className ?? ''}`}>
      <span
        className={`rounded-full px-3 py-1 text-[10px] font-bold uppercase ring-1 ${BADGE_COLORS[quality]}`}
      >
        {quality}
      </span>
      <span className="text-xs font-mono text-slate-400">
        {fmt(throughput, 1)} msg/s
      </span>
    </div>
  )
}
