import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '../../lib/cn'

const dotVariants = cva('inline-block rounded-full relative', {
  variants: {
    tone: {
      ok: 'bg-[color:var(--color-bull-400)] shadow-[0_0_8px_oklch(0.78_0.18_155/0.8)]',
      warn: 'bg-[color:var(--color-whale-400)] shadow-[0_0_8px_oklch(0.80_0.17_85/0.8)]',
      bad: 'bg-[color:var(--color-bear-500)] shadow-[0_0_8px_oklch(0.65_0.24_25/0.8)]',
      dim: 'bg-[color:var(--color-surface-600)]',
      live: 'bg-[color:var(--color-live-danger)]',
      aurora: 'bg-[color:var(--color-aurora-1)] shadow-[0_0_10px_oklch(0.75_0.18_170/0.8)]',
    },
    size: {
      xs: 'w-1.5 h-1.5',
      sm: 'w-2 h-2',
      md: 'w-2.5 h-2.5',
      lg: 'w-3 h-3',
    },
    pulse: {
      none: '',
      slow: 'animate-[aurora-pulse_2.4s_ease-in-out_infinite]',
      fast: 'animate-[aurora-pulse_0.9s_ease-in-out_infinite]',
      ring: 'live-dot',
    },
  },
  defaultVariants: {
    tone: 'ok',
    size: 'sm',
    pulse: 'slow',
  },
})

export interface StatusDotProps extends VariantProps<typeof dotVariants> {
  className?: string
  label?: string
}

export default function StatusDot({ tone, size, pulse, className, label }: StatusDotProps) {
  return (
    <span
      aria-label={label}
      role={label ? 'status' : undefined}
      className={cn(dotVariants({ tone, size, pulse }), className)}
    />
  )
}
