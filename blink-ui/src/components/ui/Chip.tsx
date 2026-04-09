type ChipVariant = 'default' | 'bull' | 'bear' | 'signal' | 'whale' | 'dim'

interface ChipProps {
  label: string
  count?: number
  active?: boolean
  onClick?: () => void
  variant?: ChipVariant
  className?: string
}

const VARIANT_ACTIVE: Record<ChipVariant, string> = {
  default: 'bg-slate-700 text-slate-100 border-slate-500',
  bull:    'bg-emerald-900/60 text-emerald-300 border-emerald-600/60',
  bear:    'bg-red-900/60 text-red-300 border-red-600/60',
  signal:  'bg-blue-900/60 text-blue-300 border-blue-600/60',
  whale:   'bg-amber-900/60 text-amber-300 border-amber-600/60',
  dim:     'bg-slate-800 text-slate-400 border-slate-700',
}

const VARIANT_INACTIVE: Record<ChipVariant, string> = {
  default: 'text-slate-500 border-transparent hover:text-slate-300 hover:bg-slate-800/50',
  bull:    'text-slate-500 border-transparent hover:text-emerald-400 hover:bg-emerald-900/20',
  bear:    'text-slate-500 border-transparent hover:text-red-400 hover:bg-red-900/20',
  signal:  'text-slate-500 border-transparent hover:text-blue-400 hover:bg-blue-900/20',
  whale:   'text-slate-500 border-transparent hover:text-amber-400 hover:bg-amber-900/20',
  dim:     'text-slate-600 border-transparent hover:text-slate-400',
}

/**
 * Filter pill / tag chip. Used in SubFilterBar and inline filter rows.
 */
export function Chip({ label, count, active = false, onClick, variant = 'default', className = '' }: ChipProps) {
  const style = active ? VARIANT_ACTIVE[variant] : VARIANT_INACTIVE[variant]

  return (
    <button
      onClick={onClick}
      className={`
        inline-flex items-center gap-1 px-2.5 py-1 rounded-full border text-[11px] font-medium
        transition-all whitespace-nowrap
        ${style}
        ${className}
      `}
    >
      {label}
      {count !== undefined && (
        <span className={`text-[10px] ${active ? 'opacity-80' : 'opacity-50'}`}>
          {count}
        </span>
      )}
    </button>
  )
}
