import { cn } from '../../lib/cn'

type ChipVariant = 'default' | 'bull' | 'bear' | 'signal' | 'whale' | 'dim'

interface ChipProps {
  label: string
  count?: number
  active?: boolean
  onClick?: () => void
  variant?: ChipVariant
  className?: string
}

const ACTIVE: Record<ChipVariant, string> = {
  default: 'bg-[color:oklch(0.26_0.022_260/0.8)] text-[color:var(--color-text-primary)] border-[color:var(--color-border-strong)]',
  bull:    'bg-[color:oklch(0.72_0.19_155/0.18)] text-[color:var(--color-bull-300)] border-[color:oklch(0.72_0.19_155/0.45)]',
  bear:    'bg-[color:oklch(0.65_0.24_25/0.18)] text-[color:var(--color-bear-300)] border-[color:oklch(0.65_0.24_25/0.5)]',
  signal:  'bg-[color:oklch(0.68_0.18_230/0.18)] text-[color:var(--color-signal-400)] border-[color:oklch(0.68_0.18_230/0.45)]',
  whale:   'bg-[color:oklch(0.72_0.18_85/0.18)] text-[color:var(--color-whale-400)] border-[color:oklch(0.72_0.18_85/0.45)]',
  dim:     'bg-[color:oklch(0.22_0.018_260/0.5)] text-[color:var(--color-text-muted)] border-[color:var(--color-border-subtle)]',
}

const INACTIVE: Record<ChipVariant, string> = {
  default: 'text-[color:var(--color-text-muted)] border-transparent hover:text-[color:var(--color-text-primary)] hover:bg-[color:oklch(0.22_0.018_260/0.5)]',
  bull:    'text-[color:var(--color-text-muted)] border-transparent hover:text-[color:var(--color-bull-400)] hover:bg-[color:oklch(0.72_0.19_155/0.1)]',
  bear:    'text-[color:var(--color-text-muted)] border-transparent hover:text-[color:var(--color-bear-400)] hover:bg-[color:oklch(0.65_0.24_25/0.1)]',
  signal:  'text-[color:var(--color-text-muted)] border-transparent hover:text-[color:var(--color-signal-400)] hover:bg-[color:oklch(0.68_0.18_230/0.1)]',
  whale:   'text-[color:var(--color-text-muted)] border-transparent hover:text-[color:var(--color-whale-400)] hover:bg-[color:oklch(0.72_0.18_85/0.1)]',
  dim:     'text-[color:var(--color-text-dim)] border-transparent hover:text-[color:var(--color-text-muted)]',
}

export function Chip({ label, count, active = false, onClick, variant = 'default', className }: ChipProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        'inline-flex items-center gap-1.5 px-3 py-1 rounded-full border text-[11px] font-medium whitespace-nowrap transition-all',
        active ? ACTIVE[variant] : INACTIVE[variant],
        className,
      )}
    >
      {label}
      {count !== undefined && (
        <span className={cn('text-[10px] font-mono tabular', active ? 'opacity-85' : 'opacity-55')}>
          {count}
        </span>
      )}
    </button>
  )
}
