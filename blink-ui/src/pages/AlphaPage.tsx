import { useState } from 'react'
import { api } from '../lib/api'
import type { AlphaCycleMarket, AlphaSignalRecord, AlphaPosition, AlphaClosedTrade } from '../lib/api'
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
  if (action.startsWith('rejected:') || action === 'engine_rejected')
    return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-red-500/20 text-red-400 border border-red-500/30">✗ REJECTED</span>
  switch (action) {
    case 'SUBMITTED':
    case 'accepted':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-emerald-500/20 text-emerald-400 border border-emerald-500/30">✓ ACCEPTED</span>
    case 'opened':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-blue-500/20 text-blue-400 border border-blue-500/30">● OPENED</span>
    case 'closed':
      return <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-purple-500/20 text-purple-400 border border-purple-500/30">◆ CLOSED</span>
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

function pnlColor(pnl: number): string {
  if (pnl > 0) return 'text-emerald-400'
  if (pnl < 0) return 'text-red-400'
  return 'text-slate-400'
}

function formatPnl(pnl: number): string {
  return `${pnl >= 0 ? '+' : ''}$${pnl.toFixed(2)}`
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
}

// ─── Signal Feed Card ─────────────────────────────────────────────

function SignalCard({ s, expanded, onToggle }: { s: AlphaSignalRecord; expanded: boolean; onToggle: () => void }) {
  const ts = new Date(s.timestamp)
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  const hasPnl = s.realized_pnl != null

  return (
    <div
      className="bg-surface-900 border border-slate-800 rounded-lg p-3 cursor-pointer hover:border-slate-700 transition-colors"
      onClick={onToggle}
    >
      <div className="flex items-center justify-between mb-1.5">
        <div className="flex items-center gap-2">
          <span className={`text-xs font-bold ${s.side === 'Buy' ? 'text-emerald-400' : 'text-red-400'}`}>
            {s.side === 'Buy' ? '▲ BUY' : '▼ SELL'}
          </span>
          {actionBadge(s.status)}
          {hasPnl && (
            <span className={`text-xs font-bold tabular-nums ${pnlColor(s.realized_pnl!)}`}>
              {formatPnl(s.realized_pnl!)}
            </span>
          )}
        </div>
        <span className="text-[10px] text-slate-600 tabular-nums">{timeStr}</span>
      </div>

      <div className="text-xs text-slate-300 truncate mb-1">
        {s.market_question || s.token_id.slice(0, 16) + '…'}
      </div>

      <div className="flex items-center gap-3 text-[10px] text-slate-500 tabular-nums">
        <span>Conf: {(s.confidence * 100).toFixed(0)}%</span>
        <span>Price: {s.recommended_price.toFixed(3)}</span>
        <span>Size: ${s.recommended_size_usdc.toFixed(2)}</span>
        {s.entry_price != null && <span>Entry: {s.entry_price.toFixed(3)}</span>}
        {s.unrealized_pnl != null && (
          <span className={pnlColor(s.unrealized_pnl)}>uPnL: {formatPnl(s.unrealized_pnl)}</span>
        )}
      </div>

      {expanded && s.reasoning && (
        <div className="mt-2 pt-2 border-t border-slate-800">
          <div className="text-[10px] text-slate-600 uppercase tracking-widest mb-1">AI Reasoning</div>
          <p className="text-xs text-slate-400 leading-relaxed whitespace-pre-wrap">{s.reasoning}</p>
        </div>
      )}
    </div>
  )
}

// ─── AI Positions Panel ───────────────────────────────────────────

function AiPositionsPanel({ positions, closedTrades }: { positions: AlphaPosition[]; closedTrades: AlphaClosedTrade[] }) {
  return (
    <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
      <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">
        AI Positions {positions.length > 0 && <span className="text-emerald-400">({positions.length} open)</span>}
      </div>

      {positions.length === 0 && closedTrades.length === 0 && (
        <div className="text-xs text-slate-600 text-center py-4">No AI positions yet</div>
      )}

      {positions.length > 0 && (
        <div className="space-y-2 mb-4">
          {positions.map(pos => {
            const title = pos.market_title?.replace('[ALPHA] ', '') ?? pos.token_id.slice(0, 16)
            return (
              <div key={pos.id} className="flex items-center justify-between py-1.5 border-b border-slate-800/50">
                <div className="flex-1 min-w-0">
                  <div className="text-xs text-slate-300 truncate">{title}</div>
                  <div className="text-[10px] text-slate-600">
                    {pos.side} @ {pos.entry_price.toFixed(3)} → {pos.current_price.toFixed(3)} · ${pos.usdc_spent.toFixed(2)} · {formatDuration(pos.duration_secs)}
                  </div>
                </div>
                <div className={`text-sm font-bold tabular-nums ml-3 ${pnlColor(pos.unrealized_pnl)}`}>
                  {formatPnl(pos.unrealized_pnl)}
                  <div className="text-[10px] text-slate-600 text-right">{pos.unrealized_pnl_pct.toFixed(1)}%</div>
                </div>
              </div>
            )
          })}
        </div>
      )}

      {closedTrades.length > 0 && (
        <>
          <div className="text-[10px] text-slate-600 uppercase tracking-widest mb-2">
            Recent Closed ({closedTrades.length})
          </div>
          <div className="space-y-1">
            {closedTrades.slice(0, 5).map((t, i) => (
              <div key={i} className="flex items-center justify-between py-1 text-[10px]">
                <span className="text-slate-500 truncate max-w-[200px]">
                  {t.market_title?.replace('[ALPHA] ', '') ?? t.token_id.slice(0, 12)}
                </span>
                <div className="flex items-center gap-2">
                  <span className="text-slate-600">{t.reason}</span>
                  <span className={`font-bold tabular-nums ${pnlColor(t.realized_pnl)}`}>
                    {formatPnl(t.realized_pnl)}
                  </span>
                </div>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  )
}

// ─── Enhanced Market Row ──────────────────────────────────────────

function MarketRow({ m, expanded, onToggle }: { m: AlphaCycleMarket; expanded: boolean; onToggle: () => void }) {
  return (
    <>
      <tr
        className="border-b border-slate-800/50 hover:bg-slate-800/30 transition-colors cursor-pointer"
        onClick={onToggle}
      >
        <td className="py-2 pr-3 text-xs text-slate-300 max-w-[250px] truncate">{m.question}</td>
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
        <td className="py-2 px-2 text-xs tabular-nums text-right">
          {m.spread_pct != null ? `${(m.spread_pct * 100).toFixed(1)}%` : '—'}
        </td>
        <td className="py-2 pl-2 text-right">{actionBadge(m.action)}</td>
      </tr>
      {expanded && (m.reasoning || m.reasoning_chain) && (
        <tr className="border-b border-slate-800/50">
          <td colSpan={7} className="py-2 px-3">
            <div className="bg-slate-900/50 rounded p-2 space-y-2">
              {/* Reasoning chain (Phase 2) */}
              {m.reasoning_chain && (
                <div className="space-y-1.5">
                  <div className="flex items-center gap-2 mb-1">
                    <div className="text-[10px] text-cyan-500/80 uppercase tracking-widest font-bold">Reasoning Chain</div>
                    {m.reasoning_chain.category && (
                      <span className="px-1.5 py-0.5 rounded text-[10px] bg-cyan-500/10 text-cyan-500 border border-cyan-500/20">
                        {m.reasoning_chain.category}
                      </span>
                    )}
                    {m.reasoning_chain.combination_method && (
                      <span className="text-[10px] text-slate-600">{m.reasoning_chain.combination_method}</span>
                    )}
                  </div>

                  {/* Probability comparison */}
                  <div className="flex items-center gap-3 text-[10px] tabular-nums">
                    <span className="text-slate-500">Call 1:</span>
                    <span className="text-cyan-400 font-bold">
                      {m.reasoning_chain.call1_probability != null ? (m.reasoning_chain.call1_probability * 100).toFixed(1) + '%' : '—'}
                    </span>
                    <span className="text-slate-700">→</span>
                    <span className="text-slate-500">Devil's Advocate:</span>
                    <span className="text-amber-400 font-bold">
                      {m.reasoning_chain.call2_probability != null ? (m.reasoning_chain.call2_probability * 100).toFixed(1) + '%' : '—'}
                    </span>
                    <span className="text-slate-700">→</span>
                    <span className="text-slate-500">Final:</span>
                    <span className="text-emerald-400 font-bold">
                      {m.reasoning_chain.final_probability != null ? (m.reasoning_chain.final_probability * 100).toFixed(1) + '%' : '—'}
                    </span>
                  </div>

                  {/* Base rate */}
                  {m.reasoning_chain.base_rate && (
                    <div>
                      <div className="text-[10px] text-violet-500/70 uppercase tracking-widest mb-0.5">Base Rate</div>
                      <p className="text-xs text-slate-400 leading-relaxed">{m.reasoning_chain.base_rate}</p>
                    </div>
                  )}

                  {/* Evidence for / against */}
                  {(m.reasoning_chain.evidence_for?.length > 0 || m.reasoning_chain.evidence_against?.length > 0) && (
                    <div className="grid grid-cols-2 gap-2">
                      {m.reasoning_chain.evidence_for?.length > 0 && (
                        <div>
                          <div className="text-[10px] text-emerald-500/70 uppercase tracking-widest mb-0.5">Evidence For</div>
                          <ul className="space-y-0.5">
                            {m.reasoning_chain.evidence_for.map((e, i) => (
                              <li key={i} className="text-[11px] text-emerald-400/60 leading-tight">• {e}</li>
                            ))}
                          </ul>
                        </div>
                      )}
                      {m.reasoning_chain.evidence_against?.length > 0 && (
                        <div>
                          <div className="text-[10px] text-rose-500/70 uppercase tracking-widest mb-0.5">Evidence Against</div>
                          <ul className="space-y-0.5">
                            {m.reasoning_chain.evidence_against.map((e, i) => (
                              <li key={i} className="text-[11px] text-rose-400/60 leading-tight">• {e}</li>
                            ))}
                          </ul>
                        </div>
                      )}
                    </div>
                  )}

                  {/* Call 1 reasoning */}
                  {m.reasoning_chain.call1_reasoning && (
                    <div>
                      <div className="text-[10px] text-slate-600 uppercase tracking-widest mb-0.5">Bayesian Analysis</div>
                      <p className="text-xs text-slate-400 leading-relaxed">{m.reasoning_chain.call1_reasoning}</p>
                    </div>
                  )}

                  {/* Devil's advocate critique */}
                  {m.reasoning_chain.call2_critique && (
                    <div>
                      <div className="text-[10px] text-amber-500/70 uppercase tracking-widest mb-0.5">Devil's Advocate</div>
                      <p className="text-xs text-amber-400/70 leading-relaxed">{m.reasoning_chain.call2_critique}</p>
                    </div>
                  )}

                  {/* Cognitive biases detected */}
                  {m.reasoning_chain.cognitive_biases?.length > 0 && (
                    <div>
                      <div className="text-[10px] text-orange-500/70 uppercase tracking-widest mb-0.5">Cognitive Biases Detected</div>
                      <div className="flex flex-wrap gap-1">
                        {m.reasoning_chain.cognitive_biases.map((b, i) => (
                          <span key={i} className="px-1.5 py-0.5 rounded text-[10px] bg-orange-500/10 text-orange-400 border border-orange-500/20">
                            {b}
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              )}

              {/* Fallback: plain reasoning (v1 or when chain not available) */}
              {!m.reasoning_chain && m.reasoning && (
                <>
                  <div className="text-[10px] text-slate-600 uppercase tracking-widest mb-1">AI Reasoning</div>
                  <p className="text-xs text-slate-400 leading-relaxed">{m.reasoning}</p>
                </>
              )}

              <div className="flex gap-4 mt-2 text-[10px] text-slate-600 tabular-nums">
                {m.bid_depth_usdc != null && <span>Bid depth: ${m.bid_depth_usdc.toFixed(0)}</span>}
                {m.ask_depth_usdc != null && <span>Ask depth: ${m.ask_depth_usdc.toFixed(0)}</span>}
                {m.price_change_1h != null && (
                  <span className={m.price_change_1h > 0 ? 'text-emerald-500' : m.price_change_1h < 0 ? 'text-red-500' : ''}>
                    1h: {m.price_change_1h > 0 ? '+' : ''}{(m.price_change_1h * 100).toFixed(1)}%
                  </span>
                )}
                {m.recommended_size_usdc != null && <span>Size: ${m.recommended_size_usdc.toFixed(2)}</span>}
              </div>
            </div>
          </td>
        </tr>
      )}
    </>
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
  const [expandedSignal, setExpandedSignal] = useState<string | null>(null)
  const [expandedMarket, setExpandedMarket] = useState<number | null>(null)

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
  const signalHistory = data.signal_history ?? []
  const aiPositions = data.ai_positions ?? []
  const closedTrades = data.ai_closed_trades ?? []
  const perf = data.performance

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-5">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-base font-bold text-slate-100">Alpha AI</h1>
          <p className="text-xs text-slate-500 mt-0.5">Autonomous AI signals — CLOB analysis + Kelly sizing + self-tracking</p>
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

      {/* Stats row — 6 cards */}
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
        <StatCard label="Signals" value={data.signals_received} sub={`${data.signals_accepted} accepted`} />
        <StatCard
          label="Positions"
          value={data.positions_opened}
          sub={`${data.positions_closed} closed · ${aiPositions.length} open`}
          highlight={aiPositions.length > 0 ? 'green' : undefined}
        />
        <StatCard
          label="AI P&L"
          value={formatPnl(pnlTotal)}
          sub={`$${data.realized_pnl_usdc.toFixed(2)} real · $${data.unrealized_pnl_usdc.toFixed(2)} open`}
          highlight={pnlHighlight}
        />
        <StatCard
          label="Win Rate"
          value={perf ? `${perf.win_rate_pct.toFixed(0)}%` : '—'}
          sub={perf ? `${perf.win_count}W / ${perf.loss_count}L` : undefined}
          highlight={perf && perf.win_rate_pct > 50 ? 'green' : perf && perf.win_rate_pct < 50 ? 'red' : undefined}
        />
        <StatCard
          label="Avg Trade"
          value={perf ? formatPnl(perf.avg_pnl_per_trade) : '—'}
          sub={perf ? `Best: ${formatPnl(perf.best_trade_pnl)} · Worst: ${formatPnl(perf.worst_trade_pnl)}` : undefined}
        />
        <StatCard
          label="Cycles"
          value={data.cycles_completed}
          sub={hasCycles ? `${data.last_cycle_duration_secs?.toFixed(0)}s last` : undefined}
        />
      </div>

      {/* Two-column layout: Signal Feed + AI Positions */}
      <div className="grid grid-cols-1 lg:grid-cols-5 gap-4">
        {/* Signal Feed — 3 columns */}
        <div className="lg:col-span-3 space-y-3">
          <div className="text-xs text-slate-500 uppercase tracking-widest">
            Signal Feed {signalHistory.length > 0 && <span className="text-slate-600">({signalHistory.length})</span>}
          </div>
          {signalHistory.length === 0 ? (
            <div className="bg-surface-900 border border-slate-800 rounded-lg p-6 text-center text-xs text-slate-600">
              No signals yet — waiting for sidecar to submit…
            </div>
          ) : (
            <div className="space-y-2 max-h-[500px] overflow-y-auto pr-1">
              {[...signalHistory].reverse().map(s => (
                <SignalCard
                  key={s.analysis_id}
                  s={s}
                  expanded={expandedSignal === s.analysis_id}
                  onToggle={() => setExpandedSignal(expandedSignal === s.analysis_id ? null : s.analysis_id)}
                />
              ))}
            </div>
          )}
        </div>

        {/* AI Positions — 2 columns */}
        <div className="lg:col-span-2">
          <AiPositionsPanel positions={aiPositions} closedTrades={closedTrades} />
        </div>
      </div>

      {/* Cycle funnel */}
      {hasCycles && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">Last Cycle Pipeline</div>
          <div className="flex items-center gap-2">
            {[
              { label: 'Scanned', value: data.last_cycle_markets_scanned, color: 'text-slate-300' },
              { label: 'Analyzed', value: data.last_cycle_markets_analyzed, color: 'text-cyan-400' },
              { label: 'Signals', value: data.last_cycle_signals_generated, color: 'text-yellow-400' },
              { label: 'Submitted', value: data.last_cycle_signals_submitted, color: 'text-emerald-400' },
            ].map((step, i) => (
              <div key={step.label} className="flex items-center">
                {i > 0 && <span className="text-slate-700 mx-2">→</span>}
                <div className="text-center">
                  <div className={`text-lg font-bold tabular-nums ${step.color}`}>{step.value}</div>
                  <div className="text-[10px] text-slate-600">{step.label}</div>
                </div>
              </div>
            ))}
            <span className="text-slate-700 mx-2">·</span>
            <div className="text-center">
              <div className="text-lg font-bold tabular-nums text-slate-400">{data.last_cycle_duration_secs?.toFixed(1)}s</div>
              <div className="text-[10px] text-slate-600">Duration</div>
            </div>
          </div>
        </div>
      )}

      {/* Market analysis table */}
      {markets.length > 0 && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">
            Market Analysis — Last Cycle ({markets.length} markets)
            <span className="text-slate-700 ml-2">click row to expand reasoning</span>
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
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 px-2">Spread</th>
                  <th className="text-right text-[10px] text-slate-600 uppercase tracking-wider pb-2 pl-2">Action</th>
                </tr>
              </thead>
              <tbody>
                {markets.map((m, i) => (
                  <MarketRow
                    key={i}
                    m={m}
                    expanded={expandedMarket === i}
                    onToggle={() => setExpandedMarket(expandedMarket === i ? null : i)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Reject reasons */}
      {topRejectReasons.length > 0 && (
        <div className="bg-surface-900 border border-slate-800 rounded-lg p-4">
          <div className="text-xs text-slate-500 uppercase tracking-widest mb-3">
            Rejection Analysis
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
        <div className="font-semibold text-slate-500 mb-2">How Alpha AI works</div>
        <ol className="list-decimal ml-4 space-y-1">
          <li>Scanner filters Polymarket markets by volume ($5k–$50M) and blocks esports/gaming categories</li>
          <li>CLOB client fetches live orderbook — best bid/ask, spread, depth, and 1h price drift</li>
          <li><span className="text-cyan-500">Deep Analysis call</span> — category-specific Bayesian reasoning with base rates and evidence weighing</li>
          <li><span className="text-amber-500">Devil's Advocate call</span> — adversarial critique: missed evidence, cognitive biases, revised probability</li>
          <li>Final estimate = 70% deep analysis + 30% devil's advocate (penalty if disagreement {">"}15pp)</li>
          <li>Confidence is spread-calibrated: tight spread ({"<"}50bps) → −20%, wide spread ({">"}200bps) → +10%</li>
          <li>Kelly criterion sizes the position: quarter-Kelly × $100 bankroll → typical $1–$6 per trade</li>
          <li>Engine applies a second independent risk layer (circuit breaker, position cap, daily loss) before execution</li>
          <li>Every prediction is stored in memory — outcomes tracked, Brier scores computed, calibration updated</li>
        </ol>
      </div>
    </div>
  )
}
