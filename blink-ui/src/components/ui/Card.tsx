import type { ReactNode } from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '../../lib/cn'

const cardVariants = cva(
  'relative rounded-xl p-3 transition-all glass',
  {
    variants: {
      variant: {
        default: '',
        signal:  'shadow-[0_0_0_1px_oklch(0.68_0.18_230/0.28),0_18px_48px_-14px_oklch(0.68_0.18_230/0.25)]',
        alert:   'shadow-[0_0_0_1px_oklch(0.80_0.17_85/0.3),0_18px_48px_-14px_oklch(0.80_0.17_85/0.25)]',
        whale:   'shadow-[0_0_0_1px_oklch(0.80_0.17_85/0.3),0_18px_48px_-14px_oklch(0.80_0.17_85/0.25)]',
        bear:    'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.3),0_18px_48px_-14px_oklch(0.65_0.24_25/0.3)]',
        bull:    'shadow-[0_0_0_1px_oklch(0.72_0.19_155/0.28),0_18px_48px_-14px_oklch(0.72_0.19_155/0.25)]',
      },
      accent: {
        none: '',
        top:  'before:absolute before:top-0 before:left-4 before:right-4 before:h-px before:bg-gradient-to-r before:from-transparent before:via-[color:var(--color-aurora-1)] before:to-transparent before:opacity-60',
      },
      interactive: {
        none: '',
        hover: 'cursor-pointer hover:scale-[1.004] hover:shadow-[0_0_0_1px_oklch(0.75_0.18_170/0.25),0_22px_52px_-14px_oklch(0.75_0.18_170/0.35)]',
      },
    },
    defaultVariants: {
      variant: 'default',
      accent: 'none',
      interactive: 'none',
    },
  }
)

interface CardProps extends VariantProps<typeof cardVariants> {
  children: ReactNode
  className?: string
  onClick?: () => void
}

export function Card({ children, variant, className, onClick, accent }: CardProps) {
  return (
    <div
      className={cn(
        cardVariants({ variant, accent, interactive: onClick ? 'hover' : 'none' }),
        className,
      )}
      onClick={onClick}
    >
      {children}
    </div>
  )
}

export function CardHeader({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        'text-[10px] font-semibold uppercase tracking-[0.14em] text-[color:var(--color-text-muted)] mb-2',
        className,
      )}
    >
      {children}
    </div>
  )
}
