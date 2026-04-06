import { createContext, useContext, useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import type { EngineMode } from '../types'
import { api } from '../lib/api'

interface ModeContextValue {
  engineMode: EngineMode        // what the backend is actually running
  viewMode: EngineMode          // what the UI is currently showing
  setViewMode: (m: EngineMode) => void
  liveAvailable: boolean
}

const ModeContext = createContext<ModeContextValue>({
  engineMode: 'paper',
  viewMode: 'paper',
  setViewMode: () => {},
  liveAvailable: false,
})

const STORAGE_KEY = 'blink_view_mode'

export function ModeProvider({ children }: { children: ReactNode }) {
  const [engineMode, setEngineMode] = useState<EngineMode>('paper')
  const [liveAvailable, setLiveAvailable] = useState(false)
  const [viewMode, setViewModeState] = useState<EngineMode>(() => {
    const saved = localStorage.getItem(STORAGE_KEY) as EngineMode | null
    return saved ?? 'paper'
  })

  useEffect(() => {
    const poll = () => {
      api.mode()
        .then(({ mode, live_active }) => {
          setEngineMode(mode)
          setLiveAvailable(live_active)
          if (!live_active && viewMode === 'live') {
            setViewModeState('paper')
            localStorage.setItem(STORAGE_KEY, 'paper')
          }
        })
        .catch(() => {})
    }
    poll()
    const id = setInterval(poll, 30_000)
    return () => clearInterval(id)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewMode])

  const setViewMode = (m: EngineMode) => {
    setViewModeState(m)
    localStorage.setItem(STORAGE_KEY, m)
  }

  return (
    <ModeContext.Provider value={{ engineMode, viewMode, setViewMode, liveAvailable }}>
      {children}
    </ModeContext.Provider>
  )
}

export function useMode() {
  return useContext(ModeContext)
}
