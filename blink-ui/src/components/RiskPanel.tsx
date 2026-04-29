import { ShieldCheck, ShieldOff, Activity } from 'lucide-react'
import type { RiskSummary } from '../types'
import { fmt, fmtPnl, pnlClass } from '../lib/format'
import GradientBar from './shared/GradientBar'
import { useMode } from '../hooks/useMode'

interface Props {
  risk: RiskSummary
  volBps?: number
  equityCurve?: number[]
  currentNav?: number
}

export default function RiskPanel({ risk, volBps, equityCurve, currentNav }: Props) {
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  // Derive daily loss limit from percentage (applied to typical NAV of ~$250)
  const pnl = risk.daily_pnl ?? 0
  const maxLossPct = risk.max_daily_loss_pct ?? 0.05
  const cbTripped = risk.circuit_breaker_tripped ?? risk.circuit_breaker ?? false

  // Session drawdown: first equity curve sample vs current NAV
  const sessionStartNav = equityCurve && equityCurve.length > 0 ? equityCurve[0] : undefined
  const nav = currentNav ?? 0
  const sessionDrawdownPct = sessionStartNav && sessionStartNav > 0 && nav < sessionStartNav
    ? (sessionStartNav - nav) / sessionStartNav * 100
    : 0

  // Volatility regime label
  const volLabel = volBps === undefined ? null
    : volBps > 1600 ? { text: 'EXTREME', cls: 'text-red-400' }
    : volBps > 800  ? { text: 'HIGH', cls: 'text-orange-400' }
    : volBps > 400  ? { text: 'MED', cls: 'text-amber-400' }
    : { text: 'LOW', cls: 'text-emerald-400' }

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Risk
        </span>
        {cbTripped ? (
          <span className="badge badge-danger flex items-center gap-1">
            <ShieldOff size={10} /> CB TRIPPED
          </span>
        ) : (
          <span className="badge badge-ok flex items-center gap-1">
            <ShieldCheck size={10} /> CB OK
          </span>
        )}
      </div>

      <div className="space-y-3">
        {/* Volatility regime */}
        {volLabel && (
          <div className="flex justify-between text-xs items-center">
            <span className="text-slate-500 flex items-center gap-1"><Activity size={10} /> Volatility</span>
            <span className={`font-mono font-semibold ${volLabel.cls}`}>
              {volLabel.text} <span className="text-slate-500 font-normal">{fmt(volBps!, 0)}bps</span>
            </span>
          </div>
        )}

        {/* Session drawdown gauge — info only, no pause/size warnings */}
        {sessionDrawdownPct > 0 && (
          <div>
            <div className="flex justify-between text-xs mb-1">
              <span className="text-slate-500">Session DD</span>
              <span className="font-mono text-slate-300">
                -{fmt(sessionDrawdownPct, 1)}%
              </span>
            </div>
            <GradientBar
              value={Math.min(1, sessionDrawdownPct / 10)}
              label="Drawdown"
              height={6}
              className="mt-1"
            />
          </div>
        )}

        {/* Risk P&L + progress bar toward daily loss limit */}
        <div>
          <div className="flex justify-between text-xs mb-1">
            <span className="text-slate-500">{isLive ? 'Risk P&L' : 'Daily P&L'}</span>
            <span className={`font-mono font-semibold ${pnlClass(pnl)}`}>
              {fmtPnl(pnl)} USDC
            </span>
          </div>
          {/* Risk gauge: gradient bar showing % of daily loss budget consumed */}
        {maxLossPct > 0 && (() => {
          const limitUSDC = 250 * maxLossPct
          const usedPct = Math.max(0, Math.min(1, Math.abs(pnl < 0 ? pnl / limitUSDC : 0)))
          return (
            <GradientBar
              value={usedPct}
              label="Loss budget"
              height={6}
              className="mt-1"
            />
          )
        })()}
        </div>

        <div className="flex justify-between text-xs">
          <span className="text-slate-500">Max daily loss</span>
          <span className="font-mono text-slate-300">{fmt(maxLossPct * 100, 1)}%</span>
        </div>
        {risk.max_concurrent_positions != null && (
          <div className="flex justify-between text-xs">
            <span className="text-slate-500">Max concurrent</span>
            <span className="font-mono text-slate-300">{risk.max_concurrent_positions}</span>
          </div>
        )}
        {risk.max_single_order_usdc != null && (
          <div className="flex justify-between text-xs">
            <span className="text-slate-500">Max order size</span>
            <span className="font-mono text-slate-300">${fmt(risk.max_single_order_usdc)}</span>
          </div>
        )}
        <div className="flex justify-between text-xs">
          <span className="text-slate-500">Trading enabled</span>
          <span className={`font-mono font-semibold ${risk.trading_enabled ? 'text-emerald-400' : 'text-red-400'}`}>
            {risk.trading_enabled ? 'YES' : 'NO'}
          </span>
        </div>
      </div>
    </div>
  )
}
