import { forwardRef, type HTMLAttributes } from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '../../lib/cn'

const glassCardVariants = cva(
  'relative rounded-xl transition-all',
  {
    variants: {
      tone: {
        default: 'glass',
        subtle: 'glass-subtle',
        solid: 'bg-[color:var(--color-surface-800)] border border-[color:var(--color-border-subtle)]',
      },
      glow: {
        none: '',
        aurora:
          'shadow-[0_0_0_1px_oklch(0.75_0.18_170/0.18),0_20px_60px_-20px_oklch(0.75_0.18_170/0.25)]',
        paper:
          'shadow-[0_0_0_1px_oklch(0.65_0.22_285/0.2),0_20px_60px_-20px_oklch(0.65_0.22_285/0.3)]',
        live:
          'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.28),0_20px_60px_-20px_oklch(0.65_0.24_25/0.35)]',
        bull:
          'shadow-[0_0_0_1px_oklch(0.72_0.19_155/0.22),0_20px_60px_-20px_oklch(0.72_0.19_155/0.28)]',
        bear:
          'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.22),0_20px_60px_-20px_oklch(0.65_0.24_25/0.28)]',
      },
      padding: {
        none: 'p-0',
        sm: 'p-3',
        md: 'p-4',
        lg: 'p-5',
        xl: 'p-6',
      },
    },
    defaultVariants: {
      tone: 'default',
      glow: 'none',
      padding: 'md',
    },
  }
)

export interface GlassCardProps
  extends HTMLAttributes<HTMLDivElement>,
    VariantProps<typeof glassCardVariants> {}

const GlassCard = forwardRef<HTMLDivElement, GlassCardProps>(
  ({ className, tone, glow, padding, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(glassCardVariants({ tone, glow, padding }), className)}
      {...props}
    />
  )
)
GlassCard.displayName = 'GlassCard'

export default GlassCard
export { glassCardVariants }
