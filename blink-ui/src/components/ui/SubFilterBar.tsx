import { Chip } from './Chip'

export interface FilterOption {
  id: string
  label: string
  count?: number
}

interface SubFilterBarProps {
  options: FilterOption[]
  active: string
  onChange: (id: string) => void
  className?: string
}

/**
 * Horizontal scrollable row of filter chips.
 * Used as a per-tab second navigation layer.
 */
export function SubFilterBar({ options, active, onChange, className = '' }: SubFilterBarProps) {
  return (
    <div
      className={`
        flex items-center gap-1.5 px-3 py-1.5
        bg-surface-950 border-b border-slate-800/60
        overflow-x-auto scrollbar-none shrink-0
        ${className}
      `}
    >
      {options.map((opt) => (
        <Chip
          key={opt.id}
          label={opt.label}
          count={opt.count}
          active={active === opt.id}
          onClick={() => onChange(opt.id)}
        />
      ))}
    </div>
  )
}
