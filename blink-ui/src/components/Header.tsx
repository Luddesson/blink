import { useState, useEffect } from 'react'
import { Activity, AlertTriangle, TrendingDown } from 'lucide-react'
import { motion, AnimatePresence } from 'motion/react'
import { useMode } from '../hooks/useMode'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import LiveConfirmModal from './LiveConfirmModal'
import { PriceFlash } from './ui'
import StatusDot from './aurora/StatusDot'
import { cn } from '../lib/cn'
import type { EngineMode } from '../types'

interface HeaderProps {
  wsConnected: boolean
  tradingPaused: boolean
  nav?: number
  navDelta?: number
  navDeltaPct?: number
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
  const isLive = viewMode === 'live'

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
  const seTimeCompact = seTime.slice(0, 5)

  return (
    <>
      <header
        className={cn(
          'relative z-40 grid shrink-0 grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 px-3 py-2.5 sm:flex sm:flex-wrap sm:items-start sm:gap-x-4 sm:px-5',
          'border-b border-[color:var(--color-border-subtle)]',
          'backdrop-blur-xl',
          isLive
            ? 'bg-[color:var(--color-surface-950)/0.75] shadow-[inset_0_-1px_0_0_var(--color-live-500)/0.15]'
            : 'bg-[color:var(--color-surface-950)/0.7] shadow-[inset_0_-1px_0_0_var(--color-paper-500)/0.12]',
        )}
      >
        {/* Left: Brand + Mode */}
        <div className="col-start-1 row-start-1 flex min-w-0 flex-col items-start gap-2 sm:flex-1 sm:flex-row sm:items-center sm:gap-4">
          <div className="flex min-w-0 items-center gap-2">
            <motion.div
              initial={{ rotate: -90, opacity: 0 }}
              animate={{ rotate: 0, opacity: 1 }}
              transition={{ type: 'spring', stiffness: 200, damping: 18 }}
              className="relative w-6 h-6 flex items-center justify-center"
            >
              <div
                className="absolute inset-0 rounded-md blur-md opacity-80"
                style={{
                  background:
                    'conic-gradient(from 180deg, var(--color-aurora-2), var(--color-aurora-1), var(--color-aurora-3), var(--color-aurora-2))',
                }}
              />
              <svg
                viewBox="0 0 24 24"
                className="relative w-[18px] h-[18px] text-white"
                fill="currentColor"
              >
                <path d="M13 2 L3 14 h8 l-1 8 l10-12 h-8 z" />
              </svg>
            </motion.div>
            <span className="truncate serif-accent text-[17px] text-[color:var(--color-text-primary)] tracking-tight">
              Blink
            </span>
          </div>

          <div className="relative flex w-full items-center rounded-lg border border-[color:var(--color-border-subtle)] bg-[color:var(--color-surface-900)/0.6] p-0.5 sm:w-auto">
            <ModeToggleBtn
              active={!isLive}
              onClick={() => handleModeClick('paper')}
              tone="paper"
              label="Paper"
            />
            <ModeToggleBtn
              active={isLive}
              onClick={() => handleModeClick('live')}
              tone="live"
              label="Live"
              disabled={!liveAvailable}
              disabledHint="Set LIVE_TRADING=true in engine to enable"
            />
          </div>
        </div>

        {/* Center: NAV */}
        {nav !== undefined && (
          <div className="col-span-2 row-start-2 flex min-w-0 flex-col items-start gap-2 border-t border-[color:var(--color-border-subtle)] pt-2 sm:order-none sm:flex-1 sm:basis-auto sm:flex-row sm:items-center sm:justify-center sm:gap-6 sm:border-t-0 sm:pt-0">
            <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
              <span className="text-[10px] uppercase tracking-[0.14em] text-[color:var(--color-text-muted)]">
                NAV
              </span>
              <PriceFlash
                value={nav}
                format={(v) => `$${v.toFixed(2)}`}
                className="text-lg font-bold tabular font-mono text-[color:var(--color-text-primary)]"
              />
              <AnimatePresence>
                {hasDelta && (
                  <motion.span
                    key={deltaPositive ? 'up' : 'down'}
                    initial={{ opacity: 0, y: -3 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: 3 }}
                    className={cn(
                      'text-xs tabular font-mono font-semibold',
                      deltaPositive ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-bear-400)]',
                    )}
                  >
                    {deltaPositive ? '▲' : '▼'}
                    {'\u00A0'}
                    {deltaPositive ? '+' : ''}${(navDelta ?? 0).toFixed(2)}
                    {navDeltaPct !== undefined && (
                      <span className="ml-1 opacity-70">
                        ({deltaPositive ? '+' : ''}{navDeltaPct.toFixed(2)}%)
                      </span>
                    )}
                  </motion.span>
                )}
              </AnimatePresence>
            </div>

            {positionCount !== undefined && positionCount > 0 && (
              <div className="flex items-center gap-1.5 text-[10px] uppercase tracking-[0.12em]">
                <span className="text-[color:var(--color-text-muted)]">Positions</span>
                <span className="px-2 py-0.5 rounded-md font-mono font-semibold text-[11px] bg-[color:var(--color-surface-700)/0.5] border border-[color:var(--color-border-subtle)] text-[color:var(--color-text-primary)]">
                  {positionCount}
                </span>
              </div>
            )}
          </div>
        )}

        {/* Right: status */}
        <div className="col-start-2 row-start-1 flex shrink-0 flex-wrap items-center justify-end gap-x-2 gap-y-1 text-[10px] sm:ml-auto sm:gap-x-3 sm:text-[11px]">
          <AnimatePresence>
            {tradingPaused && (
              <motion.span
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.9 }}
                className="flex items-center gap-1 font-semibold text-[color:var(--color-whale-400)]"
              >
                <AlertTriangle size={11} /> PAUSED
              </motion.span>
            )}
          </AnimatePresence>
          {metrics?.available && rejections > 0 && (
            <span
              className={cn(
                'hidden items-center gap-1 font-mono sm:flex',
                rejections > 10
                  ? 'text-[color:var(--color-whale-400)]'
                  : 'text-[color:var(--color-text-muted)]',
              )}
              title={`Signals rejected last 60s: ${rejections}`}
            >
              <TrendingDown size={11} />
              {rejections}/min
            </span>
          )}
          <span className="flex items-center gap-1.5">
            <StatusDot tone={wsConnected ? 'ok' : 'bad'} size="sm" pulse={wsConnected ? 'slow' : 'fast'} />
            <Activity size={11} className={wsConnected ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-bear-400)]'} />
            <span className={wsConnected ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-bear-400)]'}>
              {wsConnected ? 'LIVE' : 'DOWN'}
            </span>
          </span>
          <span className="font-mono tabular text-[color:var(--color-text-dim)] sm:hidden">
            {seTimeCompact}
          </span>
          <span className="hidden text-[color:var(--color-text-dim)] font-mono tabular sm:inline">
            {seTime}
          </span>
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

function ModeToggleBtn({
  active,
  onClick,
  tone,
  label,
  disabled,
  disabledHint,
}: {
  active: boolean
  onClick: () => void
  tone: 'paper' | 'live'
  label: string
  disabled?: boolean
  disabledHint?: string
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={disabled ? disabledHint : undefined}
      className={cn(
        'relative flex flex-1 items-center justify-center gap-1.5 px-2.5 py-1 rounded-md text-[10px] font-bold uppercase tracking-[0.1em] transition-colors sm:flex-none sm:px-3',
        active && tone === 'paper' && 'text-white',
        active && tone === 'live' && 'text-white',
        !active && !disabled && 'text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)]',
        disabled && 'text-[color:var(--color-text-dim)] cursor-not-allowed',
      )}
    >
      {active && (
        <motion.span
          layoutId="mode-pill"
          transition={{ type: 'spring', stiffness: 380, damping: 30 }}
          className={cn(
            'absolute inset-0 rounded-md',
            tone === 'paper'
              ? 'bg-gradient-to-br from-[color:var(--color-paper-500)] to-[color:var(--color-paper-600)]'
              : 'bg-gradient-to-br from-[color:var(--color-live-500)] to-[color:var(--color-live-danger)]',
          )}
          style={{
            boxShadow:
              tone === 'paper'
                ? '0 4px 14px -2px oklch(0.65 0.22 285 / 0.5), inset 0 1px 0 oklch(1 0 0 / 0.15)'
                : '0 4px 14px -2px oklch(0.65 0.24 25 / 0.5), inset 0 1px 0 oklch(1 0 0 / 0.15)',
          }}
        />
      )}
      <span className="relative flex items-center gap-1.5">
        <span
          className={cn(
            'w-1.5 h-1.5 rounded-full',
            active ? 'bg-white' : tone === 'live' ? 'bg-[color:var(--color-live-danger)]' : 'bg-[color:var(--color-paper-400)]',
            active && tone === 'live' && 'live-dot',
          )}
        />
        {label}
      </span>
    </button>
  )
}
