import { cn } from '../../lib/cn'

interface OddsBarProps {
  yesPct: number
  yesLabel?: string
  noLabel?: string
  size?: 'sm' | 'md' | 'lg'
  showLabels?: boolean
  className?: string
}

const HEIGHT = { sm: 'h-1', md: 'h-1.5', lg: 'h-2' } as const

export function OddsBar({
  yesPct,
  yesLabel = 'Yes',
  noLabel = 'No',
  size = 'sm',
  showLabels = false,
  className,
}: OddsBarProps) {
  const clampedYes = Math.max(0, Math.min(100, yesPct))
  const noPct = 100 - clampedYes

  return (
    <div className={cn('w-full', className)}>
      {showLabels && (
        <div className="flex justify-between text-[10px] font-mono tabular mb-0.5">
          <span className="text-[color:var(--color-bull-400)]">{yesLabel} {clampedYes.toFixed(0)}%</span>
          <span className="text-[color:var(--color-bear-400)]">{noPct.toFixed(0)}% {noLabel}</span>
        </div>
      )}
      <div className={cn('relative flex w-full rounded-full overflow-hidden', HEIGHT[size])}>
        <div
          className="transition-[width] duration-500 ease-out"
          style={{
            width: `${clampedYes}%`,
            background:
              'linear-gradient(90deg, oklch(0.72 0.19 155 / 0.9), oklch(0.78 0.18 155))',
            boxShadow: 'inset 0 0 8px oklch(0.78 0.18 155 / 0.4)',
          }}
        />
        <div
          className="flex-1"
          style={{
            background:
              'linear-gradient(90deg, oklch(0.65 0.24 25 / 0.85), oklch(0.72 0.22 25))',
            boxShadow: 'inset 0 0 8px oklch(0.65 0.24 25 / 0.4)',
          }}
        />
      </div>
    </div>
  )
}
