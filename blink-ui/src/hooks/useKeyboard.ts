import { useEffect, useCallback } from 'react'

interface KeyboardActions {
  onTabSwitch: (index: number) => void
  onPause?: () => void
  onKill?: () => void
  onPalette?: () => void
  onHelp?: () => void
}

export function useKeyboard({ onTabSwitch, onPause, onKill, onPalette, onHelp }: KeyboardActions) {
  const handler = useCallback((e: KeyboardEvent) => {
    // Don't capture when typing in inputs
    const tag = (e.target as HTMLElement)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return

    // Command palette: Cmd/Ctrl + P
    if (e.key === 'p' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault()
      onPalette?.()
      return
    }

    // Help sheet: ?
    if (e.key === '?' && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault()
      onHelp?.()
      return
    }

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

    // Kill switch: Ctrl+K
    if (e.key === 'k' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault()
      onKill?.()
      return
    }
  }, [onTabSwitch, onPause, onKill, onPalette, onHelp])

  useEffect(() => {
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [handler])
}
