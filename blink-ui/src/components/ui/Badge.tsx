import type { ReactNode } from 'react'

type BadgeVariant = 'bull' | 'bear' | 'signal' | 'whale' | 'neutral' | 'paper' | 'live' | 'warn' | 'ok' | 'dim'

interface BadgeProps {
  children: ReactNode
  variant?: BadgeVariant
  dot?: boolean
  className?: string
}

const STYLES: Record<BadgeVariant, string> = {
  bull:    'bg-emerald-900/60 text-emerald-300 border-emerald-700/40',
  bear:    'bg-red-900/60 text-red-300 border-red-700/40',
  signal:  'bg-blue-900/60 text-blue-300 border-blue-700/40',
  whale:   'bg-amber-900/60 text-amber-300 border-amber-700/40',
  neutral: 'bg-slate-800 text-slate-400 border-slate-700',
  paper:   'bg-indigo-900/60 text-indigo-300 border-indigo-700/40',
  live:    'bg-red-900/60 text-red-300 border-red-700/40',
  warn:    'bg-amber-900/60 text-amber-300 border-amber-700/40',
  ok:      'bg-emerald-900/60 text-emerald-300 border-emerald-700/40',
  dim:     'bg-slate-900 text-slate-500 border-slate-800',
}

/**
 * Inline badge/tag with semantic color variants.
 */
export function Badge({ children, variant = 'neutral', dot = false, className = '' }: BadgeProps) {
  return (
    <span
      className={`
        inline-flex items-center gap-1 px-1.5 py-0.5 rounded
        border text-[10px] font-semibold uppercase tracking-wide
        ${STYLES[variant]}
        ${className}
      `}
    >
      {dot && (
        <span className={`w-1.5 h-1.5 rounded-full bg-current ${variant === 'live' ? 'live-dot' : ''}`} />
      )}
      {children}
    </span>
  )
}
