interface OddsBarProps {
  /** Yes probability 0–100 */
  yesPct: number
  yesLabel?: string
  noLabel?: string
  size?: 'sm' | 'md' | 'lg'
  showLabels?: boolean
  className?: string
}

const HEIGHT = { sm: 'h-1', md: 'h-1.5', lg: 'h-2' } as const

/**
 * Dual-color probability bar — green for Yes, red for No.
 * Optionally shows probability percentages as labels.
 */
export function OddsBar({
  yesPct,
  yesLabel = 'Yes',
  noLabel = 'No',
  size = 'sm',
  showLabels = false,
  className = '',
}: OddsBarProps) {
  const clampedYes = Math.max(0, Math.min(100, yesPct))
  const noPct = 100 - clampedYes

  return (
    <div className={`w-full ${className}`}>
      {showLabels && (
        <div className="flex justify-between text-[10px] font-mono mb-0.5">
          <span className="text-emerald-400">{yesLabel} {clampedYes.toFixed(0)}%</span>
          <span className="text-red-400">{noPct.toFixed(0)}% {noLabel}</span>
        </div>
      )}
      <div className={`flex w-full rounded-full overflow-hidden ${HEIGHT[size]}`}>
        <div
          className="bg-emerald-500 transition-all duration-500"
          style={{ width: `${clampedYes}%` }}
        />
        <div
          className="bg-red-500 flex-1 transition-all duration-500"
        />
      </div>
    </div>
  )
}
