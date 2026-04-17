import { useState } from 'react';
import { useAlpha } from '../hooks/useApi';
import type { AlphaCycleMarket, AlphaPosition } from '../hooks/useApi';

function Badge({ children, variant = 'neutral' }: { children: React.ReactNode; variant?: 'green' | 'red' | 'yellow' | 'neutral' | 'blue' }) {
  const colors = {
    green:   'bg-emerald-500/20 text-emerald-400 border-emerald-500/30',
    red:     'bg-red-500/20 text-red-400 border-red-500/30',
    yellow:  'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
    blue:    'bg-blue-500/20 text-blue-400 border-blue-500/30',
    neutral: 'bg-gray-800 text-gray-400 border-gray-700',
  } satisfies Record<string, string>;
  return (
    <span className={`px-2 py-0.5 rounded text-[10px] font-bold border ${colors[variant]}`}>
      {children}
    </span>
  );
}

function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="bg-gray-900 border border-gray-800 rounded-lg p-4">
      <div className="text-[10px] text-gray-500 uppercase tracking-widest mb-1">{label}</div>
      <div className="text-2xl font-bold text-white tabular-nums">{value}</div>
      {sub && <div className="text-xs text-gray-600 mt-1">{sub}</div>}
    </div>
  );
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${(secs / 3600).toFixed(1)}h`;
}

function ReasoningChainViewer({ market }: { market: AlphaCycleMarket }) {
  const [expanded, setExpanded] = useState(false);
  const chain = market.reasoning_chain;

  return (
    <div className="bg-gray-900/60 border border-gray-800 rounded-lg p-3">
      <div className="flex items-center justify-between gap-2">
        <div className="flex-1 min-w-0">
          <div className="text-xs text-white truncate">{market.question}</div>
          <div className="flex items-center gap-2 mt-1">
            <Badge variant={market.action === 'SUBMITTED' ? 'green' : market.action === 'LOW_EDGE' ? 'yellow' : 'neutral'}>
              {market.action}
            </Badge>
            {market.side && <Badge variant={market.side === 'BUY' ? 'green' : 'red'}>{market.side}</Badge>}
            {chain?.category && <Badge variant="blue">{chain.category}</Badge>}
            {market.edge_bps != null && (
              <span className="text-[10px] text-gray-500">{market.edge_bps.toFixed(0)}bps edge</span>
            )}
          </div>
        </div>
        <div className="text-right shrink-0">
          <div className="text-xs text-gray-400">
            {market.yes_price.toFixed(2)} → {market.llm_probability?.toFixed(2) ?? '?'}
          </div>
          {market.confidence != null && (
            <div className="text-[10px] text-gray-600">{(market.confidence * 100).toFixed(0)}% conf</div>
          )}
        </div>
        {chain && (
          <button onClick={() => setExpanded(!expanded)} className="text-gray-600 hover:text-gray-400 text-xs ml-2">
            {expanded ? '▼' : '▶'}
          </button>
        )}
      </div>

      {expanded && chain && (
        <div className="mt-3 space-y-2 border-t border-gray-800 pt-2">
          {/* Probability flow */}
          <div className="flex items-center gap-2 text-[10px]">
            <span className="text-gray-600">Call 1:</span>
            <span className="text-white">{chain.call1_probability?.toFixed(3)}</span>
            <span className="text-gray-700">→</span>
            <span className="text-gray-600">Call 2:</span>
            <span className="text-white">{chain.call2_probability?.toFixed(3)}</span>
            <span className="text-gray-700">→</span>
            <span className="text-gray-600">Final:</span>
            <span className="text-emerald-400 font-bold">{chain.final_probability?.toFixed(3)}</span>
            {chain.combination_method && (
              <span className="text-gray-700 ml-1">({chain.combination_method})</span>
            )}
          </div>

          {/* Base rate */}
          {chain.base_rate && (
            <div className="text-[10px]">
              <span className="text-gray-600">Base rate: </span>
              <span className="text-gray-400">{chain.base_rate}</span>
            </div>
          )}

          {/* Evidence */}
          {chain.evidence_for && chain.evidence_for.length > 0 && (
            <div className="text-[10px]">
              <span className="text-emerald-600">Evidence FOR:</span>
              <ul className="ml-3 mt-0.5 space-y-0.5">
                {chain.evidence_for.map((e, i) => (
                  <li key={i} className="text-gray-400">+ {e}</li>
                ))}
              </ul>
            </div>
          )}
          {chain.evidence_against && chain.evidence_against.length > 0 && (
            <div className="text-[10px]">
              <span className="text-red-600">Evidence AGAINST:</span>
              <ul className="ml-3 mt-0.5 space-y-0.5">
                {chain.evidence_against.map((e, i) => (
                  <li key={i} className="text-gray-400">- {e}</li>
                ))}
              </ul>
            </div>
          )}

          {/* Reasoning */}
          {chain.call1_reasoning && (
            <div className="text-[10px]">
              <span className="text-gray-600">Bayesian: </span>
              <span className="text-gray-500">{chain.call1_reasoning}</span>
            </div>
          )}

          {/* Critique */}
          {chain.call2_critique && (
            <div className="text-[10px]">
              <span className="text-yellow-600">Critique: </span>
              <span className="text-gray-500">{chain.call2_critique}</span>
            </div>
          )}

          {/* Cognitive biases */}
          {chain.cognitive_biases && chain.cognitive_biases.length > 0 && (
            <div className="text-[10px]">
              <span className="text-orange-600">Biases detected: </span>
              <span className="text-gray-500">{chain.cognitive_biases.join(', ')}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function PositionCard({ pos }: { pos: AlphaPosition }) {
  const pnlColor = pos.unrealized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400';
  return (
    <div className="bg-gray-900/60 border border-gray-800 rounded-lg p-3">
      <div className="flex items-center justify-between">
        <div className="flex-1 min-w-0">
          <div className="text-xs text-white truncate">{pos.market_title}</div>
          <div className="flex items-center gap-2 mt-1">
            <Badge variant={pos.side === 'YES' ? 'green' : 'red'}>{pos.side}</Badge>
            <span className="text-[10px] text-gray-500">${pos.usdc_spent.toFixed(2)} invested</span>
            <span className="text-[10px] text-gray-600">{formatDuration(pos.duration_secs)}</span>
          </div>
        </div>
        <div className="text-right">
          <div className={`text-sm font-bold ${pnlColor}`}>
            {pos.unrealized_pnl >= 0 ? '+' : ''}${pos.unrealized_pnl.toFixed(2)}
          </div>
          <div className="text-[10px] text-gray-600">
            {pos.entry_price.toFixed(3)} → {pos.current_price.toFixed(3)}
          </div>
        </div>
      </div>
    </div>
  );
}

type SubTab = 'overview' | 'reasoning' | 'positions' | 'history';

export default function Alpha() {
  const { data, error } = useAlpha(5000);
  const [subTab, setSubTab] = useState<SubTab>('overview');

  if (!data && !error) {
    return (
      <div className="flex items-center justify-center h-48 text-gray-600 text-sm">
        Loading alpha status…
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-center">
          <div className="text-red-500 text-sm mb-2">Cannot reach engine</div>
          <div className="text-gray-600 text-xs">Make sure the engine is running on port 7878</div>
        </div>
      </div>
    );
  }

  if (!data.enabled) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-center">
          <Badge variant="yellow">DISABLED</Badge>
          <div className="text-gray-400 text-sm mt-3">Alpha sidecar is not active</div>
          <div className="text-gray-600 text-xs mt-1">{data.reason ?? 'Set ALPHA_ENABLED=true in .env and restart the engine'}</div>
        </div>
      </div>
    );
  }

  const pnlTotal = data.realized_pnl_usdc + data.unrealized_pnl_usdc;
  const pnlColor = pnlTotal >= 0 ? 'text-emerald-400' : 'text-red-400';
  const perf = data.performance;
  const topRejectReasons = Object.entries(data.reject_reasons)
    .sort(([, a], [, b]) => b - a)
    .slice(0, 6);

  const subTabs: { id: SubTab; label: string }[] = [
    { id: 'overview', label: 'Overview' },
    { id: 'reasoning', label: `Reasoning (${data.last_cycle_top_markets?.length ?? 0})` },
    { id: 'positions', label: `Positions (${data.ai_positions?.length ?? 0})` },
    { id: 'history', label: `History (${data.signal_history?.length ?? 0})` },
  ];

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-lg font-bold text-white">Alpha Intelligence</h1>
          <p className="text-xs text-gray-500 mt-0.5">
            {data.cycles_completed ?? 0} cycles · Last: {data.last_cycle_at ? new Date(data.last_cycle_at).toLocaleTimeString() : 'never'}
            {data.last_cycle_duration_secs != null && ` (${data.last_cycle_duration_secs.toFixed(1)}s)`}
          </p>
        </div>
        <Badge variant="green">● ACTIVE</Badge>
      </div>

      {/* Stats row */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        <StatCard
          label="Signals"
          value={data.signals_received}
          sub={`${data.signals_accepted} accepted · ${data.signals_rejected} rejected`}
        />
        <StatCard
          label="AI P&L"
          value={`${pnlTotal >= 0 ? '+' : ''}$${pnlTotal.toFixed(2)}`}
          sub={`$${data.realized_pnl_usdc.toFixed(2)} real · $${data.unrealized_pnl_usdc.toFixed(2)} open`}
        />
        <StatCard
          label="Win Rate"
          value={perf ? `${perf.win_rate_pct.toFixed(0)}%` : '—'}
          sub={perf ? `${perf.win_count}W / ${perf.loss_count}L` : undefined}
        />
        <StatCard
          label="Avg P&L/Trade"
          value={perf?.avg_pnl_per_trade != null ? `$${perf.avg_pnl_per_trade.toFixed(2)}` : '—'}
          sub={perf ? `Best: $${perf.best_trade_pnl.toFixed(2)} · Worst: $${perf.worst_trade_pnl.toFixed(2)}` : undefined}
        />
        <StatCard
          label="Markets Scanned"
          value={data.last_cycle_markets_scanned ?? 0}
          sub={`${data.last_cycle_markets_analyzed ?? 0} analyzed · ${data.last_cycle_signals_submitted ?? 0} submitted`}
        />
      </div>

      {/* Sub-tabs */}
      <div className="flex gap-1 border-b border-gray-800 pb-1">
        {subTabs.map(tab => (
          <button
            key={tab.id}
            onClick={() => setSubTab(tab.id)}
            className={`px-3 py-1.5 text-xs rounded-t transition-colors ${
              subTab === tab.id
                ? 'bg-gray-800 text-white border-b-2 border-emerald-500'
                : 'text-gray-500 hover:text-gray-300'
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Sub-tab content */}
      {subTab === 'overview' && (
        <div className="space-y-4">
          {/* Reject reasons */}
          {topRejectReasons.length > 0 && (
            <div className="bg-gray-900 border border-gray-800 rounded-lg p-4">
              <div className="text-xs text-gray-500 uppercase tracking-widest mb-3">Top Rejection Reasons</div>
              <div className="space-y-2">
                {topRejectReasons.map(([reason, count]) => {
                  const pct = data.signals_rejected > 0 ? (count / data.signals_rejected) * 100 : 0;
                  return (
                    <div key={reason} className="flex items-center gap-3">
                      <div className="text-xs text-gray-400 w-40 shrink-0 truncate">{reason}</div>
                      <div className="flex-1 bg-gray-800 rounded-full h-1.5 overflow-hidden">
                        <div className="h-full bg-red-500/60 rounded-full" style={{ width: `${pct}%` }} />
                      </div>
                      <div className="text-xs text-gray-500 tabular-nums w-10 text-right">{count}</div>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Pipeline info */}
          <div className="bg-gray-900/50 border border-gray-800 rounded-lg p-4 text-xs text-gray-600">
            <div className="font-semibold text-gray-500 mb-1">AI Intelligence Pipeline</div>
            <ol className="list-decimal ml-4 space-y-1">
              <li>Scanner discovers markets by volume + inefficiency score</li>
              <li>CLOB enrichment: orderbook depth, spread, 1h price change</li>
              <li><span className="text-blue-400">RAG news</span>: Tavily API fetches real-time news context</li>
              <li><span className="text-emerald-400">Reasoning chain</span>: Deep Analysis → Devil&apos;s Advocate (2 LLM calls)</li>
              <li><span className="text-yellow-400">Multi-model consensus</span>: 3x self-consistency for high-edge signals</li>
              <li>Calibration adjusts confidence by spread + historical accuracy</li>
              <li>Kelly criterion sizing → engine risk layer → execution</li>
              <li><span className="text-purple-400">Self-improvement</span>: auto-tuner adjusts per-category thresholds</li>
            </ol>
          </div>
        </div>
      )}

      {subTab === 'reasoning' && (
        <div className="space-y-2">
          {data.last_cycle_top_markets && data.last_cycle_top_markets.length > 0 ? (
            data.last_cycle_top_markets.map((market, i) => (
              <ReasoningChainViewer key={`${market.token_id}-${i}`} market={market} />
            ))
          ) : (
            <div className="text-gray-600 text-sm text-center py-8">
              No reasoning data yet — waiting for next analysis cycle
            </div>
          )}
        </div>
      )}

      {subTab === 'positions' && (
        <div className="space-y-4">
          {/* Live positions */}
          {data.ai_positions && data.ai_positions.length > 0 ? (
            <div className="space-y-2">
              <div className="text-xs text-gray-500 uppercase tracking-widest">Open Positions</div>
              {data.ai_positions.map(pos => (
                <PositionCard key={pos.id} pos={pos} />
              ))}
            </div>
          ) : (
            <div className="text-gray-600 text-sm text-center py-4">No open AI positions</div>
          )}

          {/* Closed trades */}
          {data.ai_closed_trades && data.ai_closed_trades.length > 0 && (
            <div>
              <div className="text-xs text-gray-500 uppercase tracking-widest mb-2">Recent Closed Trades</div>
              <div className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="text-gray-600 border-b border-gray-800">
                      <th className="text-left py-1 px-2">Market</th>
                      <th className="text-right py-1 px-2">Entry</th>
                      <th className="text-right py-1 px-2">Exit</th>
                      <th className="text-right py-1 px-2">P&L</th>
                      <th className="text-right py-1 px-2">Reason</th>
                      <th className="text-right py-1 px-2">Duration</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.ai_closed_trades.map((t, i) => (
                      <tr key={i} className="border-b border-gray-800/50">
                        <td className="py-1 px-2 text-gray-400 truncate max-w-48">{t.market_title}</td>
                        <td className="py-1 px-2 text-right text-gray-400">{t.entry_price.toFixed(3)}</td>
                        <td className="py-1 px-2 text-right text-gray-400">{t.exit_price.toFixed(3)}</td>
                        <td className={`py-1 px-2 text-right font-bold ${t.realized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                          {t.realized_pnl >= 0 ? '+' : ''}${t.realized_pnl.toFixed(2)}
                        </td>
                        <td className="py-1 px-2 text-right text-gray-600">{t.reason}</td>
                        <td className="py-1 px-2 text-right text-gray-600">{formatDuration(t.duration_secs)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      )}

      {subTab === 'history' && (
        <div>
          {data.signal_history && data.signal_history.length > 0 ? (
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-gray-600 border-b border-gray-800">
                    <th className="text-left py-1 px-2">Time</th>
                    <th className="text-left py-1 px-2">Market</th>
                    <th className="text-center py-1 px-2">Side</th>
                    <th className="text-right py-1 px-2">Conf</th>
                    <th className="text-right py-1 px-2">Price</th>
                    <th className="text-right py-1 px-2">Size</th>
                    <th className="text-center py-1 px-2">Status</th>
                    <th className="text-right py-1 px-2">P&L</th>
                  </tr>
                </thead>
                <tbody>
                  {data.signal_history.map((sig, i) => {
                    const statusColor = sig.status === 'opened' ? 'text-emerald-400'
                      : sig.status === 'closed' ? 'text-blue-400'
                      : sig.status.includes('rejected') ? 'text-red-400'
                      : 'text-gray-400';
                    return (
                      <tr key={`${sig.analysis_id}-${i}`} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                        <td className="py-1 px-2 text-gray-600 whitespace-nowrap">
                          {new Date(sig.timestamp).toLocaleTimeString()}
                        </td>
                        <td className="py-1 px-2 text-gray-400 truncate max-w-40">{sig.market_question}</td>
                        <td className="py-1 px-2 text-center">
                          <Badge variant={sig.side === 'BUY' ? 'green' : 'red'}>{sig.side}</Badge>
                        </td>
                        <td className="py-1 px-2 text-right text-gray-400">{(sig.confidence * 100).toFixed(0)}%</td>
                        <td className="py-1 px-2 text-right text-gray-400">{sig.recommended_price.toFixed(3)}</td>
                        <td className="py-1 px-2 text-right text-gray-400">${sig.recommended_size_usdc.toFixed(2)}</td>
                        <td className={`py-1 px-2 text-center ${statusColor}`}>{sig.status}</td>
                        <td className="py-1 px-2 text-right">
                          {sig.realized_pnl != null && (
                            <span className={sig.realized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}>
                              {sig.realized_pnl >= 0 ? '+' : ''}${sig.realized_pnl.toFixed(2)}
                            </span>
                          )}
                          {sig.unrealized_pnl != null && sig.realized_pnl == null && (
                            <span className={`${sig.unrealized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'} opacity-60`}>
                              {sig.unrealized_pnl >= 0 ? '+' : ''}${sig.unrealized_pnl.toFixed(2)}
                            </span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="text-gray-600 text-sm text-center py-8">
              No signal history yet
            </div>
          )}
        </div>
      )}
    </div>
  );
}
