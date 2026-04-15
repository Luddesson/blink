import { api } from '../lib/api'
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

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-5">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-base font-bold text-slate-100">Alpha Sidecar</h1>
          <p className="text-xs text-slate-500 mt-0.5">AI-generated signals — CLOB enrichment + Kelly sizing</p>
        </div>
        <span className="px-2 py-0.5 rounded bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 text-[10px] font-bold">
          ● ACTIVE
        </span>
      </div>

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
          <li>Scanner filters Polymarket markets by volume ($5k–$500k) and blocks esports/gaming categories</li>
          <li>CLOB client fetches live orderbook — best bid/ask, spread, depth, and 1h price drift</li>
          <li>Grok-3 LLM analyses each market with full CLOB context and produces a YES/NO signal</li>
          <li>Confidence is spread-calibrated: tight spread ({"<"}50bps) → −20%, wide spread ({">"}200bps) → +10%</li>
          <li>Kelly criterion sizes the position: quarter-Kelly × $100 bankroll → typical $1–$6 per trade</li>
          <li>Engine applies a second independent risk layer (circuit breaker, position cap, daily loss) before execution</li>
        </ol>
        <div className="mt-3 font-mono text-[10px] text-slate-700 break-all">
          curl localhost:7878/rpc -d '{`{"jsonrpc":"2.0","id":"1","method":"alpha_status","params":{}}`}'
        </div>
      </div>
    </div>
  )
}
