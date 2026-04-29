import type { ReactNode } from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '../../lib/cn'

const badgeVariants = cva(
  'inline-flex items-center gap-1 px-2 py-0.5 rounded-md border text-[10px] font-semibold uppercase tracking-[0.1em]',
  {
    variants: {
      variant: {
        bull:    'bg-[color:var(--color-bull-500)/0.15] text-[color:var(--color-bull-300)] border-[color:var(--color-bull-500)/0.35]',
        bear:    'bg-[color:var(--color-bear-500)/0.15] text-[color:var(--color-bear-300)] border-[color:var(--color-bear-500)/0.4]',
        signal:  'bg-[color:var(--color-signal-500)/0.15] text-[color:var(--color-signal-400)] border-[color:var(--color-signal-500)/0.35]',
        whale:   'bg-[color:var(--color-whale-500)/0.15] text-[color:var(--color-whale-400)] border-[color:var(--color-whale-500)/0.35]',
        neutral: 'bg-[color:var(--color-surface-700)/0.5] text-[color:var(--color-text-secondary)] border-[color:var(--color-border-subtle)]',
        paper:   'bg-[color:var(--color-paper-500)/0.15] text-[color:var(--color-paper-300)] border-[color:var(--color-paper-500)/0.35]',
        live:    'bg-[color:var(--color-live-500)/0.18] text-[color:var(--color-live-300)] border-[color:var(--color-live-500)/0.45]',
        warn:    'bg-[color:var(--color-whale-500)/0.15] text-[color:var(--color-whale-400)] border-[color:var(--color-whale-500)/0.35]',
        ok:      'bg-[color:var(--color-bull-500)/0.15] text-[color:var(--color-bull-300)] border-[color:var(--color-bull-500)/0.35]',
        dim:     'bg-[color:var(--color-surface-950)/0.6] text-[color:var(--color-text-dim)] border-[color:var(--color-border-subtle)]',
        aurora:  'bg-[color:var(--color-aurora-1)/0.12] text-[color:var(--color-aurora-1)] border-[color:var(--color-aurora-1)/0.4]',
      },
    },
    defaultVariants: { variant: 'neutral' },
  }
)

interface BadgeProps extends VariantProps<typeof badgeVariants> {
  children: ReactNode
  dot?: boolean
  className?: string
}

export function Badge({ children, variant, dot = false, className }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ variant }), className)}>
      {dot && (
        <span
          className={cn(
            'w-1.5 h-1.5 rounded-full bg-current',
            variant === 'live' && 'live-dot',
          )}
        />
      )}
      {children}
    </span>
  )
}
