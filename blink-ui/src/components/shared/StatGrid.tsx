interface StatItem {
  label: string
  value: string | number
  delta?: string
  deltaColor?: string
}

interface Props {
  stats: StatItem[]
  columns?: number
  className?: string
}

export default function StatGrid({ stats, columns = 3, className }: Props) {
  return (
    <div
      className={`grid gap-4 ${className ?? ''}`}
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {stats.map((s) => (
        <div key={s.label}>
          <span className="block text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">
            {s.label}
          </span>
          <span className="flex items-baseline gap-1.5">
            <span className="text-sm text-slate-100 font-medium font-mono">{s.value}</span>
            {s.delta && (
              <span className={`text-[10px] font-mono ${s.deltaColor ?? 'text-slate-400'}`}>
                {s.delta}
              </span>
            )}
          </span>
        </div>
      ))}
    </div>
  )
}
