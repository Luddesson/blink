import { Chip } from './Chip'
import { cn } from '../../lib/cn'

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

export function SubFilterBar({ options, active, onChange, className }: SubFilterBarProps) {
  return (
    <div
      className={cn(
        'flex items-center gap-1.5 px-3 py-1.5 shrink-0 overflow-x-auto',
        'border-b border-[color:var(--color-border-subtle)]',
        'bg-[color:oklch(0.14_0.013_260/0.5)] backdrop-blur-sm',
        className,
      )}
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
