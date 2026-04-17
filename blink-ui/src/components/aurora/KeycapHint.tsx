import { cn } from '../../lib/cn'

/**
 * Typographic keycap — shows keyboard shortcuts with premium style.
 * Use sparingly — one per action.
 */
export default function KeycapHint({
  keys,
  className,
  tone = 'muted',
}: {
  keys: string | string[]
  className?: string
  tone?: 'muted' | 'aurora'
}) {
  const arr = Array.isArray(keys) ? keys : [keys]
  return (
    <span className={cn('inline-flex items-center gap-1', className)}>
      {arr.map((k, i) => (
        <kbd
          key={i}
          className={cn(
            'inline-flex items-center justify-center min-w-[18px] h-[18px] px-1 rounded-sm text-[10px] font-mono font-medium',
            'border border-b-2',
            tone === 'aurora'
              ? 'bg-[color:oklch(0.75_0.18_170/0.1)] border-[color:oklch(0.75_0.18_170/0.35)] text-[color:var(--color-aurora-1)]'
              : 'bg-[color:oklch(0.22_0.018_260/0.5)] border-[color:var(--color-border-subtle)] text-[color:var(--color-text-muted)]',
          )}
        >
          {k}
        </kbd>
      ))}
    </span>
  )
}
