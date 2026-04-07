interface Props {
  value: number
  max?: number
  colors?: [string, string, string]
  height?: number
  label?: string
  showPct?: boolean
  className?: string
}

export default function GradientBar({
  value,
  max = 1,
  colors = ['#a6e22e', '#e6db74', '#f92672'],
  height = 6,
  label,
  showPct = true,
  className,
}: Props) {
  const pct = Math.max(0, Math.min(100, (value / max) * 100))
  const gradient = `linear-gradient(90deg, ${colors[0]}, ${colors[1]}, ${colors[2]})`

  return (
    <div className={className}>
      {(label || showPct) && (
        <div className="flex items-center justify-between mb-1 text-[10px] text-slate-500">
          {label && <span>{label}</span>}
          {showPct && <span className="font-mono">{pct.toFixed(1)}%</span>}
        </div>
      )}
      <div
        className="w-full rounded-full bg-slate-800"
        style={{ height: `${height}px` }}
      >
        <div
          className="h-full rounded-full transition-all"
          style={{
            width: `${pct}%`,
            background: gradient,
          }}
        />
      </div>
    </div>
  )
}
