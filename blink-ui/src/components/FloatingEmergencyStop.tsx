import { useState, useEffect } from 'react'
import { motion, AnimatePresence } from 'motion/react'
import { Power, X } from 'lucide-react'
import EmergencyStop from './EmergencyStop'
import { cn } from '../lib/cn'

interface Props {
  paused: boolean
  onToggled: (paused: boolean) => void
  isLive: boolean
}

/**
 * FloatingEmergencyStop — fixed bottom-right FAB that expands into the full
 * EmergencyStop panel. Always reachable from every tab; styled louder in live mode.
 */
export default function FloatingEmergencyStop({ paused, onToggled, isLive }: Props) {
  const [open, setOpen] = useState(false)

  // Keyboard shortcut: Escape collapses the panel when open
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  return (
    <div className="fixed bottom-16 right-4 z-40">
      <AnimatePresence mode="wait">
        {open ? (
          <motion.div
            key="panel"
            initial={{ opacity: 0, y: 12, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.96 }}
            transition={{ duration: 0.18, ease: [0.2, 0, 0, 1] }}
            className="relative w-[260px]"
          >
            <button
              onClick={() => setOpen(false)}
              className="absolute -top-2 -right-2 z-10 rounded-full p-1 bg-[color:var(--color-surface-800)] border border-[color:var(--color-border-subtle)] text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)] transition-colors"
              title="Collapse (Esc)"
            >
              <X size={12} />
            </button>
            <EmergencyStop paused={paused} onToggled={onToggled} />
          </motion.div>
        ) : (
          <motion.button
            key="fab"
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            whileTap={{ scale: 0.94 }}
            onClick={() => setOpen(true)}
            title={paused ? 'Trading halted — click to review' : 'Emergency stop'}
            className={cn(
              'relative flex items-center gap-2 pl-3 pr-4 py-2.5 rounded-full font-semibold text-[11px] uppercase tracking-[0.12em] text-white overflow-hidden glass',
              'border',
              paused
                ? 'border-[color:oklch(0.72_0.19_155/0.5)]'
                : isLive
                  ? 'border-[color:oklch(0.65_0.24_25/0.55)]'
                  : 'border-[color:var(--color-border-strong)]',
            )}
            style={{
              background: paused
                ? 'linear-gradient(135deg, oklch(0.48 0.18 155 / 0.85), oklch(0.62 0.19 155 / 0.85))'
                : isLive
                  ? 'linear-gradient(135deg, oklch(0.50 0.22 25 / 0.85), oklch(0.62 0.24 25 / 0.85))'
                  : 'linear-gradient(135deg, oklch(0.28 0.06 260 / 0.75), oklch(0.22 0.04 260 / 0.75))',
              boxShadow: paused
                ? '0 10px 30px -10px oklch(0.72 0.19 155 / 0.5)'
                : isLive
                  ? '0 10px 30px -10px oklch(0.65 0.24 25 / 0.6)'
                  : '0 10px 30px -10px oklch(0 0 0 / 0.5)',
            }}
          >
            {isLive && !paused && (
              <motion.span
                aria-hidden="true"
                className="absolute inset-0 rounded-full pointer-events-none"
                animate={{
                  boxShadow: [
                    '0 0 0 0 oklch(0.65 0.24 25 / 0.45)',
                    '0 0 0 12px oklch(0.65 0.24 25 / 0)',
                  ],
                }}
                transition={{ duration: 1.6, repeat: Infinity, ease: 'easeOut' }}
              />
            )}
            <Power size={13} className="relative" />
            <span className="relative">{paused ? 'Halted' : 'Halt'}</span>
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  )
}
