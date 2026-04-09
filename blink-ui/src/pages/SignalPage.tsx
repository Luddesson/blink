import { SubFilterBar, type FilterOption } from '../components/ui'
import { useState } from 'react'
import { Zap, Eye, TrendingUp, Bell } from 'lucide-react'

const FILTERS: FilterOption[] = [
  { id: 'feed',        label: 'Signal Feed' },
  { id: 'convergence', label: 'Convergence' },
  { id: 'alerts',      label: 'Alerts' },
  { id: 'config',      label: 'Config' },
]

export default function SignalPage() {
  const [sub, setSub] = useState('feed')

  return (
    <div className="flex-1 flex flex-col overflow-hidden min-h-0">
      <SubFilterBar options={FILTERS} active={sub} onChange={setSub} />

      {sub === 'feed' && <SignalFeedPlaceholder />}
      {sub === 'convergence' && <ConvergencePlaceholder />}
      {sub === 'alerts' && <AlertsPlaceholder />}
      {sub === 'config' && <ConfigPlaceholder />}
    </div>
  )
}

function SignalFeedPlaceholder() {
  return (
    <div className="flex-1 flex flex-col overflow-hidden min-h-0">
      {/* Split layout: feed left, monitor right */}
      <div className="flex-1 grid grid-cols-1 xl:grid-cols-[1fr_320px] gap-2 p-2 overflow-hidden min-h-0">

        {/* Signal stream */}
        <div className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg flex flex-col overflow-hidden">
          <div className="flex items-center justify-between px-3 py-2 border-b border-slate-800">
            <div className="flex items-center gap-2">
              <Zap size={13} className="text-blue-400" />
              <span className="text-[10px] font-semibold uppercase tracking-wider text-slate-400">
                Signal Stream
              </span>
            </div>
            <div className="flex items-center gap-1.5">
              <span className="w-1.5 h-1.5 rounded-full bg-blue-400 ws-dot-live" />
              <span className="text-[10px] text-slate-500">Live</span>
            </div>
          </div>

          <div className="flex-1 flex flex-col items-center justify-center gap-3 text-center p-8">
            <div className="w-10 h-10 rounded-full bg-blue-900/30 border border-blue-800/40 flex items-center justify-center">
              <Zap size={18} className="text-blue-400" />
            </div>
            <div>
              <p className="text-sm text-slate-300 font-medium">Signal Stream</p>
              <p className="text-xs text-slate-500 mt-1 max-w-xs">
                Merged feed combining RN1 wallet activity, smart money whale trades,
                and engine execution signals — all in one timeline.
              </p>
            </div>
            <div className="grid grid-cols-3 gap-3 mt-2">
              <div className="text-center p-2 rounded bg-surface-700/50 border border-slate-800">
                <div className="text-[10px] text-red-400 font-semibold uppercase mb-0.5">RN1</div>
                <div className="text-[10px] text-slate-500">Wallet activity</div>
              </div>
              <div className="text-center p-2 rounded bg-surface-700/50 border border-slate-800">
                <div className="text-[10px] text-amber-400 font-semibold uppercase mb-0.5">🐋 Whale</div>
                <div className="text-[10px] text-slate-500">Smart money</div>
              </div>
              <div className="text-center p-2 rounded bg-surface-700/50 border border-slate-800">
                <div className="text-[10px] text-blue-400 font-semibold uppercase mb-0.5">⚙ Engine</div>
                <div className="text-[10px] text-slate-500">Fills & stops</div>
              </div>
            </div>
            <p className="text-[10px] text-slate-600 mt-1">
              Requires backend <code className="text-slate-500">/api/bullpen/feed</code> endpoint
            </p>
          </div>
        </div>

        {/* Right panel: attribution preview */}
        <div className="flex flex-col gap-2 overflow-y-auto min-h-0">
          <div className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3">
            <div className="text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-3 flex items-center gap-2">
              <Eye size={11} /> Attribution
            </div>
            <div className="space-y-2">
              {[
                { lens: 'sports',   trades: 14, wr: '78.6%', pnl: '+$4.20', color: 'text-emerald-400' },
                { lens: 'traders',  trades: 22, wr: '81.8%', pnl: '+$6.40', color: 'text-emerald-400' },
                { lens: 'crypto',   trades:  8, wr: '62.5%', pnl: '+$2.10', color: 'text-emerald-400' },
                { lens: 'no signal',trades:  6, wr: '33.3%', pnl: '-$2.40', color: 'text-red-400' },
              ].map((row) => (
                <div key={row.lens} className="flex items-center justify-between text-[11px]">
                  <span className="text-slate-400 capitalize">{row.lens}</span>
                  <span className="text-slate-600">{row.trades} trades</span>
                  <span className="text-slate-400">{row.wr}</span>
                  <span className={`font-mono ${row.color}`}>{row.pnl}</span>
                </div>
              ))}
            </div>
            <p className="text-[10px] text-slate-600 mt-2">Sample data — live attribution requires signal tracking</p>
          </div>

          <div className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3">
            <div className="text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-3 flex items-center gap-2">
              <TrendingUp size={11} /> Edge Summary
            </div>
            <div className="space-y-1.5 text-[11px]">
              <div className="flex justify-between">
                <span className="text-slate-500">Signal-confirmed trades</span>
                <span className="text-emerald-400 font-mono">+3.4% edge</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Unconfirmed trades</span>
                <span className="text-red-400 font-mono">-3.0% edge</span>
              </div>
              <div className="flex justify-between border-t border-slate-800 pt-1.5 mt-1.5">
                <span className="text-slate-400 font-semibold">Net signal alpha</span>
                <span className="text-emerald-400 font-semibold font-mono">+2.8%</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function ConvergencePlaceholder() {
  return (
    <div className="flex-1 flex items-center justify-center p-8 text-center">
      <div className="max-w-sm">
        <Bell size={32} className="text-blue-400 mx-auto mb-3" />
        <p className="text-sm text-slate-300 font-medium mb-1">Convergence Monitor</p>
        <p className="text-xs text-slate-500">
          Track when multiple intelligence lenses simultaneously signal the same market.
          Highest-conviction opportunities surface here.
        </p>
      </div>
    </div>
  )
}

function AlertsPlaceholder() {
  return (
    <div className="flex-1 flex items-center justify-center p-8 text-center">
      <div className="max-w-sm">
        <Bell size={32} className="text-amber-400 mx-auto mb-3" />
        <p className="text-sm text-slate-300 font-medium mb-1">Custom Alerts</p>
        <p className="text-xs text-slate-500">
          Set thresholds for whale size, viability score, and convergence strength
          to get notified when high-quality signals appear.
        </p>
      </div>
    </div>
  )
}

function ConfigPlaceholder() {
  return (
    <div className="flex-1 flex items-center justify-center p-8 text-center">
      <div className="max-w-sm">
        <p className="text-sm text-slate-300 font-medium mb-1">Signal Configuration</p>
        <p className="text-xs text-slate-500">
          Configure which signal sources are enabled, minimum whale size filter,
          and how signals influence the execution engine.
        </p>
      </div>
    </div>
  )
}
