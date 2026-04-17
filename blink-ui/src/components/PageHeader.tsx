import { motion } from 'motion/react'
import { cn } from '../lib/cn'

interface Props {
  title: string
  subtitle?: string
  icon?: React.ReactNode
  right?: React.ReactNode
  className?: string
}

/**
 * PageHeader — slim aurora-tinted heading for page-level pages (non-dashboard).
 * Uses animated fade-in + subtle iris underline.
 */
export default function PageHeader({ title, subtitle, icon, right, className }: Props) {
  return (
    <motion.div
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.24, ease: [0.2, 0, 0, 1] }}
      className={cn(
        'flex items-center gap-3 px-3 py-2.5 rounded-lg glass-subtle',
        className,
      )}
      style={{
        boxShadow: 'inset 0 0 0 1px oklch(0.62 0.02 260 / 0.18), inset 0 -1px 0 oklch(0.72 0.16 285 / 0.22)',
      }}
    >
      {icon && (
        <span className="text-[color:oklch(0.78_0.14_285)] opacity-80 shrink-0">
          {icon}
        </span>
      )}
      <div className="flex-1 min-w-0">
        <h2 className="text-[11px] font-semibold uppercase tracking-[0.14em] text-[color:var(--color-text-primary)] truncate">
          {title}
        </h2>
        {subtitle && (
          <p className="text-[10px] text-[color:var(--color-text-muted)] mt-0.5 truncate">
            {subtitle}
          </p>
        )}
      </div>
      {right && <div className="shrink-0">{right}</div>}
    </motion.div>
  )
}
