interface Props {
  histogram: number[]
  className?: string
}

const LABELS = ['0-1ms', '1-5ms', '5-20ms', '20-100ms', '100ms+']

export default function LatencyHistogram({ histogram, className }: Props) {
  if (!histogram || histogram.length === 0) {
    return (
      <div className={`h-[120px] flex items-center justify-center text-xs text-slate-600 ${className ?? ''}`}>
        Collecting latency samples…
      </div>
    )
  }

  const max = Math.max(1, ...histogram)

  return (
    <div className={`w-full ${className ?? ''}`} style={{ height: 120 }}>
      <div className="flex items-end justify-between gap-2 h-full">
        {LABELS.map((label, i) => {
          const count = histogram[i] ?? 0
          const pct = (count / max) * 100
          return (
            <div key={label} className="flex-1 flex flex-col items-center h-full justify-end">
              <span className="text-[10px] font-mono text-slate-400 mb-1">{count}</span>
              <div className="w-full flex-1 flex items-end">
                <div
                  className="w-full rounded-t bg-blue-500/70"
                  style={{ height: `${pct}%`, minHeight: count > 0 ? 2 : 0 }}
                />
              </div>
              <span className="text-[9px] text-slate-500 mt-1 whitespace-nowrap">{label}</span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
