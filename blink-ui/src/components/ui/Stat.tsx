import type { ReactNode } from 'react'
import { cn } from '../../lib/cn'

type StatDelta = 'bull' | 'bear' | 'neutral'

interface StatProps {
  label: string
  value: ReactNode
  delta?: number
  deltaFormat?: (d: number) => string
  className?: string
}

function deltaDir(d: number): StatDelta {
  if (d > 0) return 'bull'
  if (d < 0) return 'bear'
  return 'neutral'
}

const DELTA_COLOR: Record<StatDelta, string> = {
  bull:    'text-[color:var(--color-bull-400)]',
  bear:    'text-[color:var(--color-bear-400)]',
  neutral: 'text-[color:var(--color-text-muted)]',
}

export function Stat({ label, value, delta, deltaFormat, className }: StatProps) {
  const dir = delta !== undefined ? deltaDir(delta) : undefined

  return (
    <div className={cn('flex flex-col gap-0.5', className)}>
      <span className="text-[10px] uppercase tracking-[0.12em] text-[color:var(--color-text-muted)]">
        {label}
      </span>
      <div className="flex items-baseline gap-1.5">
        <span className="text-sm font-semibold tabular font-mono text-[color:var(--color-text-primary)]">
          {value}
        </span>
        {delta !== undefined && dir && (
          <span className={cn('text-[11px] tabular font-mono', DELTA_COLOR[dir])}>
            {delta > 0 ? '+' : ''}{deltaFormat ? deltaFormat(delta) : delta.toFixed(2)}
          </span>
        )}
      </div>
    </div>
  )
}
