import { useEffect, useCallback } from 'react'

interface KeyboardActions {
  onTabSwitch: (index: number) => void
  onPause?: () => void
  onKill?: () => void
}

export function useKeyboard({ onTabSwitch, onPause, onKill }: KeyboardActions) {
  const handler = useCallback((e: KeyboardEvent) => {
    // Don't capture when typing in inputs
    const tag = (e.target as HTMLElement)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return

    // Tab switching: 1-8
    if (e.key >= '1' && e.key <= '8' && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault()
      onTabSwitch(parseInt(e.key) - 1)
      return
    }

    // Pause: p
    if (e.key === 'p' && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault()
      onPause?.()
      return
    }

    // Kill switch: k or Ctrl+K
    if (e.key === 'k' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault()
      onKill?.()
      return
    }
  }, [onTabSwitch, onPause, onKill])

  useEffect(() => {
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [handler])
}
