import { AnimatePresence, motion } from 'motion/react'
import { useEffect } from 'react'
import { X } from 'lucide-react'

interface Props {
  open: boolean
  onClose: () => void
}

const SHORTCUTS: { keys: string[]; label: string }[] = [
  { keys: ['1'], label: 'Dashboard' },
  { keys: ['2'], label: 'Markets' },
  { keys: ['3'], label: 'History' },
  { keys: ['4'], label: 'Intelligence' },
  { keys: ['5'], label: 'Performance' },
  { keys: ['6'], label: 'Config' },
  { keys: ['7'], label: 'Alpha AI' },
  { keys: ['P'], label: 'Pause / Resume trading' },
  { keys: ['⌘', 'K'], label: 'Kill switch (emergency stop)' },
  { keys: ['⌘', 'P'], label: 'Command palette' },
  { keys: ['?'], label: 'This help sheet' },
  { keys: ['Esc'], label: 'Close modal / sheet' },
]

export default function HelpSheet({ open, onClose }: Props) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.14 }}
          className="fixed inset-0 z-50 flex items-center justify-center p-4"
          onClick={onClose}
        >
          <div className="absolute inset-0 bg-[color:oklch(0.10_0.02_260/0.65)] backdrop-blur-sm" />
          <motion.div
            initial={{ opacity: 0, scale: 0.96, y: 8 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.96, y: 8 }}
            transition={{ duration: 0.2, ease: [0.2, 0, 0, 1] }}
            onClick={(e) => e.stopPropagation()}
            className="relative w-full max-w-md glass rounded-xl p-6"
            style={{
              boxShadow: 'inset 0 0 0 1px oklch(0.72 0.16 285 / 0.28), 0 30px 80px -20px oklch(0.10 0.02 260 / 0.7)',
            }}
          >
            <div className="flex items-center justify-between mb-4">
              <div>
                <h2 className="text-sm font-semibold uppercase tracking-[0.14em] text-[color:var(--color-text-primary)]">
                  Keyboard shortcuts
                </h2>
                <p className="text-[11px] text-[color:var(--color-text-muted)] mt-0.5">
                  Press <kbd className="px-1 py-0.5 rounded border border-[color:var(--color-border-subtle)] text-[10px] font-mono">?</kbd> anytime to open this sheet.
                </p>
              </div>
              <button
                onClick={onClose}
                className="rounded-md p-1 text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)] hover:bg-[color:var(--color-surface-700)] transition-colors"
              >
                <X size={14} />
              </button>
            </div>

            <ul className="space-y-1.5">
              {SHORTCUTS.map((s) => (
                <li key={s.label} className="flex items-center justify-between gap-3 text-xs">
                  <span className="text-[color:var(--color-text-secondary)]">{s.label}</span>
                  <span className="flex items-center gap-1">
                    {s.keys.map((k, i) => (
                      <kbd
                        key={i}
                        className="px-1.5 py-0.5 rounded border border-[color:var(--color-border-subtle)] bg-[color:var(--color-surface-800)] text-[10px] font-mono text-[color:var(--color-text-secondary)]"
                      >
                        {k}
                      </kbd>
                    ))}
                  </span>
                </li>
              ))}
            </ul>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
