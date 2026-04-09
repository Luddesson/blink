import type { ReactNode } from 'react'

type CardVariant = 'default' | 'signal' | 'alert' | 'whale' | 'bear'

interface CardProps {
  children: ReactNode
  variant?: CardVariant
  className?: string
  onClick?: () => void
  /** Show subtle top accent border */
  accent?: boolean
}

const VARIANT_TOP: Record<CardVariant, string> = {
  default: '',
  signal:  'border-t-blue-500/50',
  alert:   'border-t-amber-500/50',
  whale:   'border-t-amber-400/60',
  bear:    'border-t-red-500/50',
}

/**
 * Base card with optional accent top border and click interaction.
 */
export function Card({ children, variant = 'default', className = '', onClick, accent }: CardProps) {
  const accentClass = accent && variant !== 'default' ? `border-t-2 ${VARIANT_TOP[variant]}` : ''
  const cursor = onClick ? 'cursor-pointer hover:bg-slate-800/70' : ''

  return (
    <div
      className={`
        bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3
        transition-colors
        ${accentClass}
        ${cursor}
        ${className}
      `}
      onClick={onClick}
    >
      {children}
    </div>
  )
}

/** Compact section header used inside cards */
export function CardHeader({ children, className = '' }: { children: ReactNode; className?: string }) {
  return (
    <div className={`text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-2 ${className}`}>
      {children}
    </div>
  )
}
