import type { ReactNode } from 'react'

type StatDelta = 'bull' | 'bear' | 'neutral'

interface StatProps {
  label: string
  value: ReactNode
  delta?: number
  deltaFormat?: (d: number) => string
  className?: string
}

function deltaClass(d: number): StatDelta {
  if (d > 0) return 'bull'
  if (d < 0) return 'bear'
  return 'neutral'
}

const DELTA_COLOR: Record<StatDelta, string> = {
  bull:    'text-emerald-400',
  bear:    'text-red-400',
  neutral: 'text-slate-500',
}

/**
 * Compact label + value cell with optional delta indicator.
 * Used in stat grids throughout the app.
 */
export function Stat({ label, value, delta, deltaFormat, className = '' }: StatProps) {
  const dir = delta !== undefined ? deltaClass(delta) : undefined

  return (
    <div className={`flex flex-col gap-0.5 ${className}`}>
      <span className="text-[10px] uppercase tracking-wider text-slate-500">{label}</span>
      <div className="flex items-baseline gap-1.5">
        <span className="text-sm font-semibold tabular-nums font-mono text-slate-100">
          {value}
        </span>
        {delta !== undefined && dir && (
          <span className={`text-[11px] tabular-nums font-mono ${DELTA_COLOR[dir]}`}>
            {delta > 0 ? '+' : ''}{deltaFormat ? deltaFormat(delta) : delta.toFixed(2)}
          </span>
        )}
      </div>
    </div>
  )
}
