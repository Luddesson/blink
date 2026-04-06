import { useMemo } from 'react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import { fmt } from '../lib/format'

const BUCKET_LABELS = ['0–10µs', '10–50µs', '50–100µs', '100–500µs', '500ms–1ms', '1ms+']

export default function LatencyPanel() {
  const { data } = usePoll(api.latency, 5_000)

  const signalAge = data?.signal_age
  const avgUs = signalAge?.avg_us ?? 0
  const p99Us = signalAge?.p99_us ?? 0
  const p999Us = signalAge?.p999_us ?? 0
  const avgSecs = avgUs / 1_000_000

  const ageColor =
    avgSecs < 5 ? '#34d399' : avgSecs < 15 ? '#fbbf24' : '#f87171'

  const buckets: number[] = useMemo(
    () => signalAge?.histogram ?? [0, 0, 0, 0, 0, 0],
    [signalAge]
  )
  const maxBucket = Math.max(1, ...buckets)

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 block mb-3">
        Latency
      </span>

      <div className="grid grid-cols-2 gap-3 mb-4">
        <div className="bg-surface-900 rounded-lg px-3 py-2">
          <div className="text-xs text-slate-500 mb-0.5">Sig Age (avg)</div>
          <div className="font-mono font-bold text-lg" style={{ color: ageColor }}>
            {avgUs > 0 ? `${fmt(avgUs / 1000, 0)}ms` : '—'}
          </div>
        </div>
        <div className="bg-surface-900 rounded-lg px-3 py-2">
          <div className="text-xs text-slate-500 mb-0.5">WS Msgs/sec</div>
          <div className="font-mono font-bold text-lg text-slate-200">
            {fmt(data?.ws_msg_per_sec ?? 0, 1)}
          </div>
        </div>
        <div className="bg-surface-900 rounded-lg px-3 py-2">
          <div className="text-xs text-slate-500 mb-0.5">p99</div>
          <div className="font-mono font-bold text-slate-300">
            {p99Us > 0 ? `${fmt(p99Us / 1000, 0)}ms` : '—'}
          </div>
        </div>
        <div className="bg-surface-900 rounded-lg px-3 py-2">
          <div className="text-xs text-slate-500 mb-0.5">p999</div>
          <div className="font-mono font-bold text-amber-400">
            {p999Us > 0 ? `${fmt(p999Us / 1000, 0)}ms` : '—'}
          </div>
        </div>
      </div>

      {/* Histogram */}
      <div className="text-xs text-slate-500 mb-1.5">Signal age distribution</div>
      <div className="space-y-1">
        {BUCKET_LABELS.map((label, i) => (
          <div key={label} className="flex items-center gap-2">
            <span className="text-xs text-slate-600 w-20 shrink-0">{label}</span>
            <div className="flex-1 bg-surface-900 rounded-full h-2 overflow-hidden">
              <div
                className="h-2 rounded-full bg-indigo-500"
                style={{ width: `${(buckets[i] / maxBucket) * 100}%` }}
              />
            </div>
            <span className="text-xs font-mono text-slate-500 w-8 text-right">{buckets[i]}</span>
          </div>
        ))}
      </div>
      <div className="text-xs text-slate-600 mt-2">Samples: {signalAge?.count ?? 0}</div>
    </div>
  )
}
