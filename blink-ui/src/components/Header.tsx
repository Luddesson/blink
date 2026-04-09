import { useState, useEffect } from 'react'
import { Activity, AlertTriangle, Zap, TrendingDown } from 'lucide-react'
import { useMode } from '../hooks/useMode'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import LiveConfirmModal from './LiveConfirmModal'
import { PriceFlash } from './ui'
import type { EngineMode } from '../types'

interface HeaderProps {
  wsConnected: boolean
  tradingPaused: boolean
  /** NAV in USDC — shown always in header */
  nav?: number
  /** Unrealized + realized delta for current session */
  navDelta?: number
  navDeltaPct?: number
  /** Number of open positions */
  positionCount?: number
}

export default function Header({
  wsConnected,
  tradingPaused,
  nav,
  navDelta,
  navDeltaPct,
  positionCount,
}: HeaderProps) {
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
  const hasDelta = navDelta !== undefined && navDelta !== 0
  const deltaPositive = (navDelta ?? 0) >= 0

  // WS health: pulsing dot changes color based on connection state
  const wsDotClass = wsConnected ? 'ws-dot-live text-emerald-400' : 'text-red-400'
  const wsLabel = wsConnected ? 'LIVE' : 'DOWN'

  return (
    <>
      <header className="flex items-center gap-3 px-4 py-2 bg-surface-900 border-b border-slate-800/80 sticky top-0 z-40">

        {/* Logo + Mode toggle */}
        <div className="flex items-center gap-3 shrink-0">
          <div className="flex items-center gap-1.5">
            <Zap size={15} className="text-indigo-400" />
            <span className="text-xs font-bold tracking-widest uppercase text-slate-100">Blink</span>
          </div>

          <div className="flex items-center gap-0.5 bg-surface-800 rounded-md p-0.5 border border-slate-800">
            <button
              onClick={() => handleModeClick('paper')}
              className={`flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-bold transition-all ${
                viewMode === 'paper'
                  ? 'bg-indigo-600 text-white shadow'
                  : 'text-slate-500 hover:text-slate-300'
              }`}
            >
              <span className="w-1.5 h-1.5 rounded-full bg-current" />
              PAPER
            </button>
            <button
              onClick={() => handleModeClick('live')}
              disabled={!liveAvailable}
              title={!liveAvailable ? 'Start engine with LIVE_TRADING=true to enable' : undefined}
              className={`flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-bold transition-all ${
                viewMode === 'live'
                  ? 'bg-red-700 text-white shadow'
                  : liveAvailable
                  ? 'text-slate-500 hover:text-red-300'
                  : 'text-slate-700 cursor-not-allowed'
              }`}
            >
              <span className={`w-1.5 h-1.5 rounded-full bg-current ${viewMode === 'live' ? 'live-dot' : ''}`} />
              LIVE
            </button>
          </div>
        </div>

        {/* ── Center: NAV always visible ─────────────────────────── */}
        {nav !== undefined && (
          <div className="flex items-center gap-4 flex-1 justify-center">
            <div className="flex items-baseline gap-2">
              <span className="text-[10px] uppercase tracking-wider text-slate-500">NAV</span>
              <PriceFlash
                value={nav}
                format={(v) => `$${v.toFixed(2)}`}
                className="text-base font-bold text-slate-100"
              />
              {hasDelta && (
                <span className={`text-xs font-mono tabular-nums ${deltaPositive ? 'text-emerald-400' : 'text-red-400'}`}>
                  {deltaPositive ? '+' : ''}${(navDelta ?? 0).toFixed(2)}
                  {navDeltaPct !== undefined && (
                    <span className="ml-1 opacity-70">
                      ({deltaPositive ? '+' : ''}{navDeltaPct.toFixed(2)}%)
                    </span>
                  )}
                </span>
              )}
            </div>

            {positionCount !== undefined && positionCount > 0 && (
              <div className="flex items-center gap-1 text-[10px]">
                <span className="text-slate-500">Positions</span>
                <span className="bg-slate-800 border border-slate-700 rounded px-1.5 py-0.5 font-mono font-semibold text-slate-300">
                  {positionCount}
                </span>
              </div>
            )}
          </div>
        )}

        {/* ── Right: status indicators ───────────────────────────── */}
        <div className="flex items-center gap-3 text-[10px] text-slate-500 shrink-0 ml-auto">
          {tradingPaused && (
            <span className="flex items-center gap-1 text-amber-400 font-semibold">
              <AlertTriangle size={10} /> PAUSED
            </span>
          )}
<<<<<<< Updated upstream
          {metrics?.available && rejections > 0 && (
=======
          {(metrics?.available && rejections > 0) && (
>>>>>>> Stashed changes
            <span
              className={`flex items-center gap-1 font-mono ${rejections > 10 ? 'text-amber-400' : 'text-slate-500'}`}
              title={`Signals rejected last 60s: ${rejections}`}
            >
              <TrendingDown size={10} />
              {rejections}/min
            </span>
          )}
          <span className="flex items-center gap-1.5">
            <Activity size={10} className={`${wsDotClass}`} />
            <span className={wsConnected ? 'text-emerald-400' : 'text-red-400'}>
              WS {wsLabel}
            </span>
          </span>
<<<<<<< Updated upstream
          <span className="text-slate-600 font-mono">{seTime} SE</span>
=======
          <span className="text-slate-600 font-mono tabular-nums">{utcTime}</span>
>>>>>>> Stashed changes
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
