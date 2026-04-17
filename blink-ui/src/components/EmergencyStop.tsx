import { useState } from 'react'
import { Power } from 'lucide-react'
import { motion, AnimatePresence } from 'motion/react'
import { api } from '../lib/api'
import { cn } from '../lib/cn'

interface Props {
  paused: boolean
  onToggled: (paused: boolean) => void
}

export default function EmergencyStop({ paused, onToggled }: Props) {
  const [clicks, setClicks] = useState(0)
  const [busy, setBusy] = useState(false)

  async function handleClick() {
    if (busy) return
    if (!paused && clicks < 1) {
      setClicks(1)
      setTimeout(() => setClicks(0), 3000)
      return
    }
    setBusy(true)
    try {
      const result = await api.pause(!paused)
      onToggled(result.trading_paused)
      setClicks(0)
    } catch (e) {
      alert(`Failed to toggle pause: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const needsDoubleClick = !paused && clicks === 0
  const confirmMode = !paused && clicks >= 1

  const label = paused ? 'RESUME TRADING' : needsDoubleClick ? 'HALT TRADING' : '⚠ CONFIRM HALT'
  const subtitle = paused
    ? 'Trading is currently HALTED'
    : needsDoubleClick
      ? 'Click once more within 3s to confirm'
      : 'Click to confirm emergency halt'

  return (
    <div className={cn(
      'relative rounded-xl p-4 glass',
      paused
        ? 'shadow-[0_0_0_1px_oklch(0.72_0.19_155/0.3),0_18px_44px_-14px_oklch(0.72_0.19_155/0.35)]'
        : 'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.35),0_18px_44px_-14px_oklch(0.65_0.24_25/0.4)]',
    )}>
      <span className="block mb-3 text-[10px] uppercase tracking-[0.14em] text-[color:var(--color-text-muted)] font-semibold">
        Emergency stop
      </span>

      <motion.button
        onClick={handleClick}
        disabled={busy}
        whileTap={{ scale: 0.97 }}
        className={cn(
          'relative w-full flex items-center justify-center gap-2 py-3.5 rounded-lg font-bold text-sm uppercase tracking-[0.08em] transition-colors overflow-hidden',
          paused && 'text-white',
          needsDoubleClick && 'text-[color:var(--color-bear-300)] border border-[color:oklch(0.65_0.24_25/0.4)]',
          confirmMode && 'text-white',
          busy && 'opacity-60 cursor-wait',
        )}
        style={
          paused
            ? {
                background: 'linear-gradient(135deg, oklch(0.55 0.20 155), oklch(0.72 0.19 155))',
                boxShadow: '0 6px 24px -6px oklch(0.72 0.19 155 / 0.6), inset 0 1px 0 oklch(1 0 0 / 0.15)',
              }
            : needsDoubleClick
              ? {
                  background: 'linear-gradient(135deg, oklch(0.28 0.10 25 / 0.5), oklch(0.35 0.14 25 / 0.5))',
                }
              : {
                  background: 'linear-gradient(135deg, oklch(0.55 0.22 25), oklch(0.65 0.24 25))',
                  boxShadow: '0 6px 24px -6px oklch(0.65 0.24 25 / 0.7), inset 0 1px 0 oklch(1 0 0 / 0.15)',
                }
        }
      >
        {confirmMode && (
          <motion.span
            aria-hidden="true"
            className="absolute inset-0 rounded-lg"
            animate={{
              boxShadow: [
                '0 0 0 0 oklch(0.65 0.24 25 / 0.6)',
                '0 0 0 10px oklch(0.65 0.24 25 / 0)',
              ],
            }}
            transition={{ duration: 1.1, repeat: Infinity, ease: 'easeOut' }}
          />
        )}
        <Power size={15} className="relative" />
        <span className="relative">{label}</span>
      </motion.button>

      <AnimatePresence mode="wait">
        <motion.p
          key={label}
          initial={{ opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -4 }}
          className="text-[11px] text-[color:var(--color-text-muted)] mt-2.5 text-center"
        >
          {subtitle}
        </motion.p>
      </AnimatePresence>
    </div>
  )
}
