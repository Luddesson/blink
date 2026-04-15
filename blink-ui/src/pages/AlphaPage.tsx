import { api } from '../lib/api'
import type { AlphaCycleMarket } from '../lib/api'
import { usePoll } from '../hooks/usePoll'

function StatCard({ label, value, sub, highlight }: { label: string; value: string | number; sub?: string; highlight?: 'green' | 'red' }) {
  const valueClass = highlight === 'green'
    ? 'text-emerald-400'
    : highlight === 'red'
      ? 'text-red-400'
      : 'text-slate-100'
  return (
    <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
      <div className="text-[10px] text-slate-500 uppercase tracking-widest mb-1">{label}</div>
      <div className={`text-2xl font-bold tabular-nums ${valueClass}`}>{value}</div>
      {sub && <div className="text-xs text-slate-600 mt-1">{sub}</div>}
    </div>
  )
}

function actionBadge(action: string) {
  switch (action) {
    case 'SUBMITTED':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-emerald-500/20 text-emerald-400 border border-emerald-500/30">✓ SUBMITTED</span>
    case 'LOW_EDGE':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-yellow-500/20 text-yellow-400 border border-yellow-500/30">↓ LOW EDGE</span>
    case 'PASS':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-slate-600/30 text-slate-500 border border-slate-600/30">— PASS</span>
    case 'REJECTED':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-red-500/20 text-red-400 border border-red-500/30">✗ REJECTED</span>
    default:
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-slate-600/30 text-slate-500">{action}</span>
  }
}

function MarketRow({ m }: { m: AlphaCycleMarket }) {
  return (
    <tr className="border-b border-slate-800/50 hover:bg-slate-800/30 transition-colors">
      <td className="py-2 pr-3 text-xs text-slate-300 max-w-[300px] truncate">{m.question}</td>
      <td className="py-2 px-2 text-xs text-slate-400 tabular-nums text-right">{m.yes_price.toFixed(2)}</td>
      <td className="py-2 px-2 text-xs text-cyan-400 tabular-nums text-right">
        {m.llm_probability != null ? m.llm_probability.toFixed(2) : '—'}
      </td>
      <td className="py-2 px-2 text-xs text-slate-400 tabular-nums text-right">
        {m.confidence != null ? (m.confidence * 100).toFixed(0) + '%' : '—'}
      </td>
      <td className="py-2 px-2 text-xs tabular-nums text-right">
        {m.edge_bps != null ? (
          <span className={m.edge_bps >= 150 ? 'text-emerald-400' : 'text-yellow-400'}>
            {m.edge_bps.toFixed(0)}bp
          </span>
        ) : '—'}
      </td>
      <td className="py-2 pl-2 text-right">{actionBadge(m.action)}</td>
    </tr>
  )
}

function timeAgo(iso: string | null): string {
  if (!iso) return 'never'
  const secs = Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  if (secs < 60) return `${secs}s ago`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  return `${Math.floor(secs / 3600)}h ago`
}

export default function AlphaPage() {
  const { data, loading, error } = usePoll(api.alpha, 5_000)

  if (loading) {
    return (
      <div className="flex-1 flex items-center justify-center text-slate-600 text-sm">
        Loading alpha status…
      </div>
    )
  }

  if (error || !data) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center">
          <div className="text-red-500 text-sm mb-2">Cannot reach engine</div>
          <div className="text-slate-600 text-xs">Make sure the engine is running on port 3030</div>
        </div>
      </div>
    )
  }

  if (!data.enabled) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center space-y-3">
          <span className="px-3 py-1 rounded bg-yellow-500/20 text-yellow-400 border border-yellow-500/30 text-xs font-bold">
            DISABLED
          </span>
          <div className="text-slate-400 text-sm">Alpha sidecar is not active</div>
          <div className="text-slate-600 text-xs max-w-xs">
            {data.reason ?? 'Set ALPHA_ENABLED=true in .env and restart the engine'}
          </div>
        </div>
      </div>
    )
  }

  const pnlTotal = data.realized_pnl_usdc + data.unrealized_pnl_usdc
  const pnlHighlight: 'green' | 'red' | undefined = pnlTotal > 0 ? 'green' : pnlTotal < 0 ? 'red' : undefined
  const topRejectReasons = Object.entries(data.reject_reasons)
    .sort(([, a], [, b]) => b - a)
    .slice(0, 6)
  const hasCycles = (data.cycles_completed ?? 0) > 0
  const markets = data.last_cycle_top_markets ?? []

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-5">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-base font-bold text-slate-100">Alpha Sidecar</h1>
          <p className="text-xs text-slate-500 mt-0.5">AI-generated signals — CLOB enrichment + Kelly sizing</p>
        </div>
        <div className="flex items-center gap-3">
          {hasCycles && (
            <span className="text-[10px] text-slate-500 tabular-nums">
              Cycle {data.cycles_completed} · {timeAgo(data.last_cycle_at)}
            </span>
          )}
          <span className={`px-2 py-0.5 rounded text-[10px] font-bold border ${
            hasCycles
              ? 'bg-emerald-500/20 text-emerald-400 border-emerald-500/30'
              : 'bg-yellow-500/20 text-yellow-400 border-yellow-500/30'
          }`}>
            {hasCycles ? '● ACTIVE' : '○ WAITING'}
          </span>
        </div>
      </div>

      {/* Cycle funnel */}
      {hasCycles && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">Last Cycle Summary</div>
          <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
            <div>
              <div className="text-[10px] text-slate-600 mb-0.5">Scanned</div>
              <div className="text-lg font-bold text-slate-300 tabular-nums">{data.last_cycle_markets_scanned}</div>
            </div>
            <div>
              <div className="text-[10px] text-slate-600 mb-0.5">Analyzed</div>
              <div className="text-lg font-bold text-cyan-400 tabular-nums">{data.last_cycle_markets_analyzed}</div>
            </div>
            <div>
              <div className="text-[10px] text-slate-600 mb-0.5">Signals</div>
              <div className="text-lg font-bold text-yellow-400 tabular-nums">{data.last_cycle_signals_generated}</div>
            </div>
            <div>
              <div className="text-[10px] text-slate-600 mb-0.5">Submitted</div>
              <div className="text-lg font-bold text-emerald-400 tabular-nums">{data.last_cycle_signals_submitted}</div>
            </div>
            <div>
              <div className="text-[10px] text-slate-600 mb-0.5">Duration</div>
              <div className="text-lg font-bold text-slate-400 tabular-nums">{data.last_cycle_duration_secs?.toFixed(1)}s</div>
            </div>
          </div>
        </div>
      )}

      {/* Stats row */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatCard label="Signals Received" value={data.signals_received} />
        <StatCard
          label="Accepted"
          value={data.signals_accepted}
          sub={`${data.accept_rate_pct.toFixed(1)}% accept rate`}
          highlight={data.signals_accepted > 0 ? 'green' : undefined}
        />
        <StatCard label="Rejected" value={data.signals_rejected} />
        <StatCard
          label="AI P&L"
          value={`${pnlTotal >= 0 ? '+' : ''}$${pnlTotal.toFixed(2)}`}
          sub={`$${data.realized_pnl_usdc.toFixed(2)} real · $${data.unrealized_pnl_usdc.toFixed(2)} open`}
          highlight={pnlHighlight}
        />
      </div>

      {/* Market analysis table */}
      {markets.length > 0 && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">
            Market Analysis — Last Cycle ({markets.length} markets)
          </div>
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b border-slate-700">
                  <th className="text-left text-[10px] text-slate-600 uppercase tracking-wider pb-2 pr-3">Market</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 px-2">Price</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 px-2">LLM Est.</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 px-2">Conf.</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 px-2">Edge</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 pl-2">Action</th>
                </tr>
              </thead>
              <tbody>
                {markets.map((m, i) => <MarketRow key={i} m={m} />)}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Position summary */}
      <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
        <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">Position Summary</div>
        <div className="grid grid-cols-3 gap-4">
          <div>
            <div className="text-[10px] text-slate-600 mb-1">Positions Opened</div>
            <div className="text-xl font-bold text-slate-100">{data.positions_opened}</div>
          </div>
          <div>
            <div className="text-[10px] text-slate-600 mb-1">Positions Closed</div>
            <div className="text-xl font-bold text-slate-100">{data.positions_closed}</div>
          </div>
          <div>
            <div className="text-[10px] text-slate-600 mb-1">Total AI P&L</div>
            <div className={`text-xl font-bold ${pnlTotal >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
              {pnlTotal >= 0 ? '+' : ''}${pnlTotal.toFixed(2)}
            </div>
          </div>
        </div>
      </div>

      {/* Reject reasons */}
      {topRejectReasons.length > 0 && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">
            Top Rejection Reasons
          </div>
          <div className="space-y-2.5">
            {topRejectReasons.map(([reason, count]) => {
              const pct = data.signals_rejected > 0
                ? (count / data.signals_rejected) * 100
                : 0
              return (
                <div key={reason} className="flex items-center gap-3">
                  <div className="text-xs text-slate-400 w-44 shrink-0 truncate">{reason}</div>
                  <div className="flex-1 bg-slate-800 rounded-full h-1.5 overflow-hidden">
                    <div
                      className="h-full bg-red-500/60 rounded-full transition-all"
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                  <div className="text-xs text-slate-500 tabular-nums w-8 text-right">{count}</div>
                </div>
              )
            })}
          </div>
        </div>
      )}

      {/* How it works */}
      <div className="bg-surface-900/60 border border-slate-800 rounded-lg p-4 text-xs text-slate-600">
        <div className="font-semibold text-slate-500 mb-2">How Alpha signals work</div>
        <ol className="list-decimal ml-4 space-y-1">
          <li>Scanner filters Polymarket markets by volume ($5k–$50M) and blocks esports/gaming categories</li>
          <li>CLOB client fetches live orderbook — best bid/ask, spread, depth, and 1h price drift</li>
          <li>GPT-4o-mini analyzes each market with full CLOB context and produces a YES/NO probability estimate</li>
          <li>Confidence is spread-calibrated: tight spread ({"<"}50bps) → −20%, wide spread ({">"}200bps) → +10%</li>
          <li>Kelly criterion sizes the position: quarter-Kelly × $100 bankroll → typical $1–$6 per trade</li>
          <li>Engine applies a second independent risk layer (circuit breaker, position cap, daily loss) before execution</li>
        </ol>
      </div>
    </div>
  )
}
