import { useRef, useEffect, useState } from 'react'

interface PriceFlashProps {
  value: number
  format?: (v: number) => string
  className?: string
  /** Suffix appended after formatted value (e.g. "¢" or "%") */
  suffix?: string
}

/**
 * Displays a numeric value and flashes green or red for 280ms
 * whenever the value changes direction.
 */
export function PriceFlash({ value, format, className = '', suffix = '' }: PriceFlashProps) {
  const prev = useRef<number>(value)
  const [flashClass, setFlashClass] = useState('')

  useEffect(() => {
    if (value === prev.current) return
    const cls = value > prev.current ? 'flash-bull' : 'flash-bear'
    prev.current = value
    setFlashClass(cls)
    const t = setTimeout(() => setFlashClass(''), 350)
    return () => clearTimeout(t)
  }, [value])

  const display = format ? format(value) : value.toLocaleString(undefined, { maximumFractionDigits: 4 })

  return (
    <span className={`tabular-nums font-mono ${flashClass} ${className}`}>
      {display}{suffix}
    </span>
  )
}
