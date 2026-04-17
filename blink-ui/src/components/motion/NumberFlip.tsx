import { useEffect, useRef, useState } from 'react'
import { motion, useMotionValue, useSpring, useTransform } from 'motion/react'
import { cn } from '../../lib/cn'

/**
 * NumberFlip — smoothly animates a numeric value with a subtle color flash
 * on direction change. Uses a spring for organic feel.
 */
interface Props {
  value: number
  format?: (v: number) => string
  className?: string
  /** Duration of direction-flash in ms */
  flashMs?: number
  /** Spring stiffness (default 140 — crisp but smooth) */
  stiffness?: number
  damping?: number
}

export default function NumberFlip({
  value,
  format = (v) => v.toFixed(2),
  className,
  flashMs = 340,
  stiffness = 140,
  damping = 20,
}: Props) {
  const mv = useMotionValue(value)
  const spring = useSpring(mv, { stiffness, damping })
  const display = useTransform(spring, (latest) => format(latest))

  const prevRef = useRef(value)
  const [flash, setFlash] = useState<'bull' | 'bear' | null>(null)
  const tRef = useRef<number | null>(null)

  useEffect(() => {
    mv.set(value)
    const prev = prevRef.current
    if (prev !== value && Number.isFinite(prev) && Number.isFinite(value)) {
      const dir = value > prev ? 'bull' : 'bear'
      setFlash(dir)
      if (tRef.current) window.clearTimeout(tRef.current)
      tRef.current = window.setTimeout(() => setFlash(null), flashMs) as unknown as number
    }
    prevRef.current = value
    return () => {
      if (tRef.current) window.clearTimeout(tRef.current)
    }
  }, [value, mv, flashMs])

  return (
    <motion.span
      className={cn(
        'tabular font-mono inline-block transition-colors',
        flash === 'bull' && 'text-[color:var(--color-bull-400)]',
        flash === 'bear' && 'text-[color:var(--color-bear-400)]',
        className,
      )}
    >
      <motion.span>{display}</motion.span>
    </motion.span>
  )
}
