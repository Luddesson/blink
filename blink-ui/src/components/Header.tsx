import { useState, useEffect } from 'react'
import { Activity, AlertTriangle, Zap, TrendingDown } from 'lucide-react'
import { useMode } from '../hooks/useMode'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import LiveConfirmModal from './LiveConfirmModal'
import type { EngineMode } from '../types'

interface HeaderProps {
  wsConnected: boolean
  tradingPaused: boolean
}

export default function Header({ wsConnected, tradingPaused }: HeaderProps) {
  const { viewMode, setViewMode, liveAvailable } = useMode()
  const [showConfirm, setShowConfirm] = useState(false)
  const fmtSE = () =>
    new Date().toLocaleTimeString('sv-SE', {
      timeZone: 'Europe/Stockholm',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
  const [seTime, setSeTime] = useState(fmtSE)
  const { data: metrics } = usePoll(api.metrics, 10_000)

  useEffect(() => {
    const id = setInterval(() => setSeTime(fmtSE()), 1000)
    return () => clearInterval(id)
  }, [])

  function handleModeClick(mode: EngineMode) {
    if (mode === viewMode) return
    if (mode === 'live') {
      if (!liveAvailable) return
      setShowConfirm(true)
    } else {
      setViewMode('paper')
    }
  }

  const rejections = metrics?.signals_rejected_last_60s ?? 0

  return (
    <>
      <header className="flex items-center justify-between px-5 py-3 bg-surface-800 border-b border-surface-600 sticky top-0 z-40">
        {/* Logo */}
        <div className="flex items-center gap-2">
          <Zap size={18} className="text-indigo-400" />
          <span className="text-sm font-semibold tracking-widest uppercase text-slate-100">
            Blink Engine
          </span>
        </div>

        {/* Mode toggle */}
        <div className="flex items-center gap-1 bg-surface-900 rounded-lg p-1 border border-surface-600">
          <button
            onClick={() => handleModeClick('paper')}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded text-xs font-semibold transition-all ${
              viewMode === 'paper'
                ? 'bg-indigo-600 text-white shadow'
                : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            <span className="w-1.5 h-1.5 rounded-full bg-current" />
            PAPER
          </button>
          <button
            onClick={() => handleModeClick('live')}
            disabled={!liveAvailable}
            title={!liveAvailable ? 'Start engine with LIVE_TRADING=true to enable' : undefined}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded text-xs font-semibold transition-all ${
              viewMode === 'live'
                ? 'bg-red-700 text-white shadow'
                : liveAvailable
                ? 'text-slate-400 hover:text-red-300'
                : 'text-slate-600 cursor-not-allowed'
            }`}
          >
            <span
              className={`w-1.5 h-1.5 rounded-full bg-current ${
                viewMode === 'live' ? 'live-dot' : ''
              }`}
            />
            LIVE
            {!liveAvailable && (
              <span className="text-xs text-slate-600 font-normal">(offline)</span>
            )}
          </button>
        </div>

        {/* Status bar */}
        <div className="flex items-center gap-4 text-xs text-slate-500">
          {tradingPaused && (
            <span className="badge badge-warn flex items-center gap-1">
              <AlertTriangle size={10} /> PAUSED
            </span>
          )}
          {metrics?.available && rejections > 0 && (
            <span
              className={`flex items-center gap-1 font-mono ${rejections > 10 ? 'text-amber-400' : 'text-slate-500'}`}
              title={`Signals rejected last 60s: ${rejections}`}
            >
              <TrendingDown size={11} />
              {rejections}/min rejected
            </span>
          )}
          <span className="flex items-center gap-1.5">
            <Activity size={11} className={wsConnected ? 'text-emerald-400' : 'text-red-400'} />
            <span className={wsConnected ? 'text-emerald-400' : 'text-red-400'}>
              {wsConnected ? 'WS LIVE' : 'WS DOWN'}
            </span>
          </span>
          <span className="text-slate-600 font-mono">{seTime} SE</span>
        </div>
      </header>

      {showConfirm && (
        <LiveConfirmModal
          onConfirm={() => {
            setShowConfirm(false)
            setViewMode('live')
          }}
          onCancel={() => setShowConfirm(false)}
        />
      )}
    </>
  )
}

