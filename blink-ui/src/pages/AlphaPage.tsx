import { useState } from 'react'
import { motion, AnimatePresence } from 'motion/react'
import { Sparkles, Brain, AlertOctagon, TrendingUp, Target, Clock, Copy, Check } from 'lucide-react'
import { api } from '../lib/api'
import type { AlphaCycleMarket, AlphaSignalRecord, AlphaPosition, AlphaClosedTrade } from '../lib/api'
import { usePoll } from '../hooks/usePoll'
import { Badge } from '../components/ui'
import GlassCard from '../components/aurora/GlassCard'
import NumberFlip from '../components/motion/NumberFlip'
import StatusDot from '../components/aurora/StatusDot'
import { cn } from '../lib/cn'

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation()
    navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }
  return (
    <button
      onClick={handleCopy}
      className="p-1 rounded bg-[color:var(--color-surface-700)/0.3] hover:bg-[color:var(--color-surface-700)/0.6] transition-colors"
    >
      {copied ? <Check size={10} className="text-[color:var(--color-bull-400)]" /> : <Copy size={10} className="text-[color:var(--color-text-dim)]" />}
    </button>
  )
}

function StatCard({
  label,
  value,
  sub,
  highlight,
  icon: Icon,
}: {
  label: string
  value: string | number
  sub?: string
  highlight?: 'green' | 'red'
  icon?: typeof Sparkles
}) {
  const valueClass = highlight === 'green'
    ? 'text-[color:var(--color-bull-400)]'
    : highlight === 'red'
      ? 'text-[color:var(--color-bear-400)]'
      : 'text-[color:var(--color-text-primary)]'
  return (
    <GlassCard padding="md" glow={highlight === 'green' ? 'bull' : highlight === 'red' ? 'bear' : 'none'}>
      <div className="flex items-center justify-between mb-1.5">
        <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em]">
          {label}
        </div>
        {Icon && <Icon size={12} className="text-[color:var(--color-text-dim)]" />}
      </div>
      <div className={cn('text-2xl font-bold tabular font-mono leading-none', valueClass)}>
        {value}
      </div>
      {sub && <div className="text-[11px] text-[color:var(--color-text-muted)] mt-1.5 leading-snug">{sub}</div>}
    </GlassCard>
  )
}

function actionBadge(action: string) {
  if (action.startsWith('rejected:') || action === 'engine_rejected' || action === 'REJECTED')
    return <Badge variant="bear">✗ REJECTED</Badge>
  switch (action) {
    case 'SUBMITTED':
    case 'accepted':
      return <Badge variant="bull">✓ ACCEPTED</Badge>
    case 'opened':
      return <Badge variant="signal">● OPENED</Badge>
    case 'closed':
      return <Badge variant="paper">◆ CLOSED</Badge>
    case 'LOW_EDGE':
      return <Badge variant="warn">↓ LOW EDGE</Badge>
    case 'PASS':
      return <Badge variant="dim">— PASS</Badge>
    default:
      return <Badge variant="dim">{action}</Badge>
  }
}

function pnlColor(pnl: number): string {
  if (pnl > 0) return 'text-[color:var(--color-bull-400)]'
  if (pnl < 0) return 'text-[color:var(--color-bear-400)]'
  return 'text-[color:var(--color-text-muted)]'
}

function formatPnl(pnl: number): string {
  return `${pnl >= 0 ? '+' : ''}$${pnl.toFixed(2)}`
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
}

function confidenceGlow(conf: number): 'aurora' | 'paper' | 'none' {
  if (conf >= 0.75) return 'aurora'
  if (conf >= 0.6) return 'paper'
  return 'none'
}

function SignalCard({ s, expanded, onToggle }: { s: AlphaSignalRecord; expanded: boolean; onToggle: () => void }) {
  const ts = new Date(s.timestamp)
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  const hasPnl = s.realized_pnl != null
  const glow = confidenceGlow(s.confidence)

  return (
    <motion.div layout initial={{ opacity: 0, scale: 0.98 }} animate={{ opacity: 1, scale: 1 }}>
      <GlassCard
        padding="sm"
        glow={glow}
        onClick={onToggle}
        className="cursor-pointer transition-all hover:ring-1 hover:ring-[color:var(--color-aurora-1)/0.3]"
      >
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <span className={cn(
              'text-[10px] font-black uppercase tracking-[0.2em] px-1.5 py-0.5 rounded',
              s.side === 'Buy'
                ? 'bg-[color:var(--color-bull-500)/0.1] text-[color:var(--color-bull-400)]'
                : 'bg-[color:var(--color-bear-500)/0.1] text-[color:var(--color-bear-400)]',
            )}>
              {s.side === 'Buy' ? '▲ Long' : '▼ Short'}
            </span>
            {actionBadge(s.status)}
            {hasPnl && (
              <span className={cn('text-xs font-bold tabular font-mono', pnlColor(s.realized_pnl!))}>
                {formatPnl(s.realized_pnl!)}
              </span>
            )}
          </div>
          <span className="text-[10px] text-[color:var(--color-text-dim)] tabular font-mono">{timeStr}</span>
        </div>

        <div className="text-xs text-[color:var(--color-text-secondary)] font-medium truncate mb-2 group flex items-center gap-1.5">
          {s.market_question || s.token_id.slice(0, 16) + '…'}
          <CopyButton text={s.token_id} />
        </div>

        <div className="grid grid-cols-2 gap-y-1.5 gap-x-3 text-[10px] text-[color:var(--color-text-muted)] tabular font-mono">
          <div className="flex items-center gap-1.5">
            <Target size={10} className="text-[color:var(--color-aurora-3)]" />
            <span>Conf <span className="text-[color:var(--color-text-primary)]">{(s.confidence * 100).toFixed(0)}%</span></span>
          </div>
          <div className="text-right">
            Price <span className="text-[color:var(--color-text-primary)]">{s.recommended_price.toFixed(3)}</span>
          </div>
          <div>
            Size <span className="text-[color:var(--color-text-primary)]">${s.recommended_size_usdc.toFixed(2)}</span>
          </div>
          <div className="text-right">
            {s.unrealized_pnl != null && (
              <span className={cn('font-bold', pnlColor(s.unrealized_pnl))}>uPnL {formatPnl(s.unrealized_pnl)}</span>
            )}
          </div>
        </div>

        <AnimatePresence>
          {expanded && s.reasoning && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              className="mt-3 pt-3 border-t border-[color:var(--color-border-subtle)] overflow-hidden"
            >
              <div className="text-[10px] text-[color:var(--color-aurora-1)] uppercase tracking-[0.14em] mb-1.5 flex items-center gap-1">
                <Brain size={10} /> AI reasoning
              </div>
              <p className="text-xs text-[color:var(--color-text-secondary)] leading-relaxed italic">{s.reasoning}</p>
            </motion.div>
          )}
        </AnimatePresence>
      </GlassCard>
    </motion.div>
  )
}

function AiPositionsPanel({ positions, closedTrades }: { positions: AlphaPosition[]; closedTrades: AlphaClosedTrade[] }) {
  return (
    <GlassCard padding="md">
      <div className="flex items-center justify-between mb-4">
        <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em]">
          AI positions
        </div>
        {positions.length > 0 && (
          <Badge variant="bull" dot>{positions.length} active</Badge>
        )}
      </div>

      {positions.length === 0 && closedTrades.length === 0 && (
        <div className="flex flex-col items-center justify-center py-8 text-center">
          <Clock size={20} className="text-[color:var(--color-text-dim)] mb-2 opacity-20" />
          <div className="text-xs text-[color:var(--color-text-dim)]">No active trades in this cycle</div>
        </div>
      )}

      {positions.length > 0 && (
        <div className="space-y-3 mb-5">
          {positions.map(pos => {
            const title = pos.market_title?.replace('[ALPHA] ', '') ?? pos.token_id.slice(0, 16)
            return (
              <div key={pos.id} className="group relative flex items-center justify-between py-2 border-b border-[color:var(--color-border-subtle)] last:border-0 hover:bg-[color:var(--color-surface-800)/0.2] transition-colors rounded-sm -mx-1 px-1">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-1.5">
                    <div className="text-xs text-[color:var(--color-text-secondary)] truncate font-medium">{title}</div>
                    <CopyButton text={pos.token_id} />
                  </div>
                  <div className="text-[10px] text-[color:var(--color-text-muted)] tabular font-mono mt-0.5">
                    <span className={pos.side === 'Buy' ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-bear-400)]'}>{pos.side}</span>
                    <span className="mx-1">·</span>
                    {pos.entry_price.toFixed(3)} → {pos.current_price.toFixed(3)}
                    <span className="mx-1">·</span>
                    ${pos.usdc_spent.toFixed(1)}
                    <span className="mx-1">·</span>
                    {formatDuration(pos.duration_secs)}
                  </div>
                </div>
                <div className={cn('text-sm font-bold tabular font-mono ml-3 text-right', pnlColor(pos.unrealized_pnl))}>
                  {formatPnl(pos.unrealized_pnl)}
                  <div className="text-[10px] font-medium opacity-80">{pos.unrealized_pnl_pct.toFixed(1)}%</div>
                </div>
              </div>
            )
          })}
        </div>
      )}

      {closedTrades.length > 0 && (
        <div className="pt-3 border-t border-[color:var(--color-border-subtle)]">
          <div className="text-[10px] text-[color:var(--color-text-dim)] uppercase tracking-[0.14em] mb-2.5">
            Recent closures
          </div>
          <div className="space-y-2">
            {closedTrades.slice(0, 5).map((t, i) => (
              <div key={i} className="flex items-center justify-between text-[10px]">
                <div className="flex items-center gap-2 flex-1 min-w-0">
                  <span className="text-[color:var(--color-text-muted)] truncate">
                    {t.market_title?.replace('[ALPHA] ', '') ?? t.token_id.slice(0, 12)}
                  </span>
                  <span className="text-[color:var(--color-text-dim)] text-[9px] px-1 rounded bg-[color:var(--color-surface-700)/0.4]">{t.reason}</span>
                </div>
                <span className={cn('font-bold tabular font-mono ml-2', pnlColor(t.realized_pnl))}>
                  {formatPnl(t.realized_pnl)}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </GlassCard>
  )
}

function MarketRow({ m, expanded, onToggle }: { m: AlphaCycleMarket; expanded: boolean; onToggle: () => void }) {
  return (
    <>
      <tr
        className="border-b border-[color:var(--color-border-subtle)] hover:bg-[color:var(--color-surface-800)/0.4] transition-colors cursor-pointer group"
        onClick={onToggle}
      >
        <td className="py-2 pr-3 text-xs text-[color:var(--color-text-secondary)] max-w-[250px] truncate group-hover:text-[color:var(--color-text-primary)] transition-colors">{m.question}</td>
        <td className="py-2 px-2 text-xs text-[color:var(--color-text-secondary)] tabular font-mono text-right">{m.yes_price.toFixed(2)}</td>
        <td className="py-2 px-2 text-xs text-[color:var(--color-aurora-3)] tabular font-mono text-right font-semibold">
          {m.llm_probability != null ? m.llm_probability.toFixed(2) : '—'}
        </td>
        <td className="py-2 px-2 text-xs text-[color:var(--color-text-muted)] tabular font-mono text-right">
          {m.confidence != null ? (m.confidence * 100).toFixed(0) + '%' : '—'}
        </td>
        <td className="py-2 px-2 text-xs tabular font-mono text-right">
          {m.edge_bps != null ? (
            <span className={m.edge_bps >= 150 ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-whale-400)]'}>
              {m.edge_bps.toFixed(0)}bp
            </span>
          ) : '—'}
        </td>
        <td className="py-2 px-2 text-xs tabular font-mono text-right text-[color:var(--color-text-muted)]">
          {m.spread_pct != null ? `${(m.spread_pct * 100).toFixed(1)}%` : '—'}
        </td>
        <td className="py-2 pl-2 text-right">{actionBadge(m.action)}</td>
      </tr>
      <AnimatePresence initial={false}>
        {expanded && (m.reasoning || m.reasoning_chain) && (
          <tr>
            <td colSpan={7} className="p-0">
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
              >
                <div className="mx-2 my-2 p-3 rounded-lg glass-subtle space-y-3">
                  {m.reasoning_chain && (
                    <div className="space-y-2">
                      <div className="flex items-center gap-2 mb-1">
                        <div className="text-[10px] text-[color:var(--color-aurora-3)] uppercase tracking-[0.14em] font-bold flex items-center gap-1">
                          <Brain size={10} /> Reasoning chain
                        </div>
                        {m.reasoning_chain.category && (
                          <Badge variant="aurora">{m.reasoning_chain.category}</Badge>
                        )}
                        {m.reasoning_chain.combination_method && (
                          <span className="text-[10px] text-[color:var(--color-text-dim)]">{m.reasoning_chain.combination_method}</span>
                        )}
                      </div>

                        <div className="flex flex-wrap items-center gap-3 text-[10px] tabular font-mono">
                        <span className="text-[color:var(--color-text-muted)]">Call 1:</span>
                        <span className="text-[color:var(--color-aurora-3)] font-bold">
                          {m.reasoning_chain.call1_probability != null ? (m.reasoning_chain.call1_probability * 100).toFixed(1) + '%' : '—'}
                        </span>
                        <span className="text-[color:var(--color-text-dim)]">→</span>
                        <span className="text-[color:var(--color-text-muted)]">Devil's advocate:</span>
                        <span className="text-[color:var(--color-whale-400)] font-bold">
                          {m.reasoning_chain.call2_probability != null ? (m.reasoning_chain.call2_probability * 100).toFixed(1) + '%' : '—'}
                        </span>
                        <span className="text-[color:var(--color-text-dim)]">→</span>
                        <span className="text-[color:var(--color-text-muted)]">Final:</span>
                        <span className="text-[color:var(--color-bull-400)] font-bold">
                          {m.reasoning_chain.final_probability != null ? (m.reasoning_chain.final_probability * 100).toFixed(1) + '%' : '—'}
                        </span>
                      </div>

                      {m.reasoning_chain.base_rate && (
                        <div>
                          <div className="text-[10px] text-[color:var(--color-paper-300)] uppercase tracking-[0.14em] mb-0.5">Base rate</div>
                          <p className="text-xs text-[color:var(--color-text-secondary)] leading-relaxed">{m.reasoning_chain.base_rate}</p>
                        </div>
                      )}

                      {(m.reasoning_chain.evidence_for?.length > 0 || m.reasoning_chain.evidence_against?.length > 0) && (
                        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                          {m.reasoning_chain.evidence_for?.length > 0 && (
                            <div>
                              <div className="text-[10px] text-[color:var(--color-bull-400)] uppercase tracking-[0.14em] mb-1">Evidence for</div>
                              <ul className="space-y-0.5">
                                {m.reasoning_chain.evidence_for.map((e, i) => (
                                  <li key={i} className="text-[11px] text-[color:oklch(0.72_0.19_155/0.85)] leading-snug">• {e}</li>
                                ))}
                              </ul>
                            </div>
                          )}
                          {m.reasoning_chain.evidence_against?.length > 0 && (
                            <div>
                              <div className="text-[10px] text-[color:var(--color-bear-400)] uppercase tracking-[0.14em] mb-1">Evidence against</div>
                              <ul className="space-y-0.5">
                                {m.reasoning_chain.evidence_against.map((e, i) => (
                                  <li key={i} className="text-[11px] text-[color:oklch(0.72_0.22_25/0.85)] leading-snug">• {e}</li>
                                ))}
                              </ul>
                            </div>
                          )}
                        </div>
                      )}

                      {m.reasoning_chain.call1_reasoning && (
                        <div>
                          <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] mb-0.5">Bayesian analysis</div>
                          <p className="text-xs text-[color:var(--color-text-secondary)] leading-relaxed">{m.reasoning_chain.call1_reasoning}</p>
                        </div>
                      )}

                      {m.reasoning_chain.call2_critique && (
                        <div>
                          <div className="text-[10px] text-[color:var(--color-whale-400)] uppercase tracking-[0.14em] mb-0.5">Devil's advocate</div>
                          <p className="text-xs text-[color:oklch(0.85_0.14_85/0.8)] leading-relaxed">{m.reasoning_chain.call2_critique}</p>
                        </div>
                      )}

                      {m.reasoning_chain.cognitive_biases?.length > 0 && (
                        <div>
                          <div className="text-[10px] text-[color:var(--color-whale-400)] uppercase tracking-[0.14em] mb-1">Cognitive biases detected</div>
                          <div className="flex flex-wrap gap-1">
                            {m.reasoning_chain.cognitive_biases.map((b, i) => (
                              <Badge key={i} variant="warn">{b}</Badge>
                            ))}
                          </div>
                        </div>
                      )}
                    </div>
                  )}

                  {!m.reasoning_chain && m.reasoning && (
                    <>
                      <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] mb-1">AI reasoning</div>
                      <p className="text-xs text-[color:var(--color-text-secondary)] leading-relaxed">{m.reasoning}</p>
                    </>
                  )}

                  <div className="flex flex-wrap gap-4 text-[10px] text-[color:var(--color-text-muted)] tabular font-mono">
                    {m.bid_depth_usdc != null && <span>Bid depth ${m.bid_depth_usdc.toFixed(0)}</span>}
                    {m.ask_depth_usdc != null && <span>Ask depth ${m.ask_depth_usdc.toFixed(0)}</span>}
                    {m.price_change_1h != null && (
                      <span className={m.price_change_1h > 0 ? 'text-[color:var(--color-bull-400)]' : m.price_change_1h < 0 ? 'text-[color:var(--color-bear-400)]' : ''}>
                        1h: {m.price_change_1h > 0 ? '+' : ''}{(m.price_change_1h * 100).toFixed(1)}%
                      </span>
                    )}
                    {m.recommended_size_usdc != null && <span>Size ${m.recommended_size_usdc.toFixed(2)}</span>}
                  </div>
                </div>
              </motion.div>
            </td>
          </tr>
        )}
      </AnimatePresence>
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
      <div className="flex-1 flex items-center justify-center text-[color:var(--color-text-dim)] text-sm">
        <motion.div
          animate={{ opacity: [0.3, 0.8, 0.3] }}
          transition={{ duration: 1.5, repeat: Infinity }}
          className="flex items-center gap-2"
        >
          <Sparkles size={14} className="text-[color:var(--color-aurora-1)]" />
          Loading alpha status…
        </motion.div>
      </div>
    )
  }

  if (error || !data) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <GlassCard padding="lg" glow="bear" className="text-center max-w-sm">
          <AlertOctagon size={22} className="text-[color:var(--color-bear-400)] mx-auto mb-2" />
          <div className="text-[color:var(--color-bear-300)] text-sm mb-1.5 font-semibold">Cannot reach engine</div>
          <div className="text-[color:var(--color-text-muted)] text-xs">Make sure the engine is running on port 3030</div>
        </GlassCard>
      </div>
    )
  }

  if (!data.enabled) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <GlassCard padding="xl" glow="paper" className="text-center max-w-sm space-y-3">
          <Badge variant="warn">DISABLED</Badge>
          <div className="text-[color:var(--color-text-secondary)] text-sm">Alpha sidecar is not active</div>
          <div className="text-[color:var(--color-text-muted)] text-xs max-w-xs mx-auto">
            {data.reason ?? 'Set ALPHA_ENABLED=true in .env and restart the engine'}
          </div>
        </GlassCard>
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
    <div className="flex-1 overflow-y-auto p-3 space-y-4 sm:p-5 sm:space-y-5">
      {/* Hero */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between"
      >
        <div className="flex items-start gap-3 sm:items-center">
          <div className="relative w-10 h-10 flex items-center justify-center rounded-xl"
            style={{
              background: 'linear-gradient(135deg, oklch(0.25 0.03 260 / 0.8), oklch(0.20 0.018 260 / 0.6))',
              boxShadow: '0 0 0 1px oklch(0.75 0.18 170 / 0.35), 0 0 24px -4px oklch(0.75 0.18 170 / 0.5)',
            }}
          >
            <Sparkles size={18} className="text-[color:var(--color-aurora-1)]" />
          </div>
          <div>
            <h1 className="serif-accent text-2xl text-[color:var(--color-text-primary)] leading-none">Alpha AI</h1>
            <p className="text-xs text-[color:var(--color-text-muted)] mt-1">
              Autonomous AI signals · CLOB analysis · Kelly sizing · self-tracking
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {hasCycles && (
            <span className="text-[10px] text-[color:var(--color-text-muted)] tabular font-mono flex items-center gap-1">
              <Clock size={10} /> Cycle {data.cycles_completed} · {timeAgo(data.last_cycle_at)}
            </span>
          )}
          <Badge variant={hasCycles ? 'bull' : 'warn'} dot>
            {hasCycles ? 'ACTIVE' : 'WAITING'}
          </Badge>
        </div>
      </motion.div>

      {/* Stats row */}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-6">
        <StatCard label="Signals" value={data.signals_received} sub={`${data.signals_accepted} accepted`} icon={Sparkles} />
        <StatCard
          label="Positions"
          value={data.positions_opened}
          sub={`${data.positions_closed} closed · ${aiPositions.length} open`}
          highlight={aiPositions.length > 0 ? 'green' : undefined}
          icon={TrendingUp}
        />
        <GlassCard padding="md" glow={pnlHighlight === 'green' ? 'bull' : pnlHighlight === 'red' ? 'bear' : 'none'}>
          <div className="flex items-center justify-between mb-1.5">
            <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em]">AI P&amp;L</div>
            <TrendingUp size={12} className="text-[color:var(--color-text-dim)]" />
          </div>
          <NumberFlip
            value={pnlTotal}
            format={(v) => `${v >= 0 ? '+' : '−'}$${Math.abs(v).toFixed(2)}`}
            className={cn(
              'text-2xl font-bold leading-none',
              pnlHighlight === 'green' ? 'text-[color:var(--color-bull-400)]' : pnlHighlight === 'red' ? 'text-[color:var(--color-bear-400)]' : 'text-[color:var(--color-text-primary)]',
            )}
          />
          <div className="text-[11px] text-[color:var(--color-text-muted)] mt-1.5">
            ${data.realized_pnl_usdc.toFixed(2)} real · ${data.unrealized_pnl_usdc.toFixed(2)} open
          </div>
        </GlassCard>
        <StatCard
          label="Win rate"
          value={perf ? `${perf.win_rate_pct.toFixed(0)}%` : '—'}
          sub={perf ? `${perf.win_count}W / ${perf.loss_count}L` : undefined}
          highlight={perf && perf.win_rate_pct > 50 ? 'green' : perf && perf.win_rate_pct < 50 ? 'red' : undefined}
          icon={Target}
        />
        <StatCard
          label="Avg trade"
          value={perf ? formatPnl(perf.avg_pnl_per_trade) : '—'}
          sub={perf ? `Best ${formatPnl(perf.best_trade_pnl)} · Worst ${formatPnl(perf.worst_trade_pnl)}` : undefined}
        />
        <StatCard
          label="Cycles"
          value={data.cycles_completed}
          sub={hasCycles ? `${data.last_cycle_duration_secs?.toFixed(0)}s last` : undefined}
          icon={Clock}
        />
      </div>

      {/* Two-column */}
      <div className="grid grid-cols-1 lg:grid-cols-5 gap-4">
        <div className="lg:col-span-3 space-y-3">
          <div className="flex items-center justify-between">
            <div className="text-[11px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] flex items-center gap-2">
              <StatusDot tone="aurora" size="xs" pulse="slow" />
              Signal feed
              {signalHistory.length > 0 && (
                <span className="text-[color:var(--color-text-dim)] ml-1">({signalHistory.length})</span>
              )}
            </div>
          </div>
          {signalHistory.length === 0 ? (
            <GlassCard padding="lg" className="text-center text-xs text-[color:var(--color-text-dim)]">
              No signals yet — waiting for sidecar to submit…
            </GlassCard>
          ) : (
            <div className="space-y-2 max-h-[70vh] overflow-y-auto pr-1 lg:max-h-[560px]">
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

        <div className="lg:col-span-2">
          <AiPositionsPanel positions={aiPositions} closedTrades={closedTrades} />
        </div>
      </div>

      {/* Cycle funnel */}
      {hasCycles && (
        <GlassCard padding="md">
          <div className="text-[11px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] mb-3">
            Last cycle pipeline
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {[
              { label: 'Scanned', value: data.last_cycle_markets_scanned, color: 'text-[color:var(--color-text-secondary)]' },
              { label: 'Analyzed', value: data.last_cycle_markets_analyzed, color: 'text-[color:var(--color-aurora-3)]' },
              { label: 'Signals', value: data.last_cycle_signals_generated, color: 'text-[color:var(--color-whale-400)]' },
              { label: 'Submitted', value: data.last_cycle_signals_submitted, color: 'text-[color:var(--color-bull-400)]' },
            ].map((step, i) => (
              <div key={step.label} className="flex items-center">
                {i > 0 && <span className="text-[color:var(--color-text-dim)] mx-2">→</span>}
                <div className="text-center">
                  <div className={cn('text-lg font-bold tabular font-mono', step.color)}>{step.value}</div>
                  <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-wider">{step.label}</div>
                </div>
              </div>
            ))}
            <span className="text-[color:var(--color-text-dim)] mx-2">·</span>
            <div className="text-center">
              <div className="text-lg font-bold tabular font-mono text-[color:var(--color-text-secondary)]">
                {data.last_cycle_duration_secs?.toFixed(1)}s
              </div>
              <div className="text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-wider">Duration</div>
            </div>
          </div>
        </GlassCard>
      )}

      {/* Market table */}
      {markets.length > 0 && (
        <GlassCard padding="md">
          <div className="text-[11px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] mb-3 flex items-center gap-2">
            Market analysis · last cycle
            <span className="text-[color:var(--color-text-dim)] normal-case font-normal tracking-normal">
              ({markets.length} markets — click row to expand)
            </span>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b border-[color:var(--color-border-strong)]">
                  <th className="text-left text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 pr-3 font-semibold">Market</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 px-2 font-semibold">Price</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 px-2 font-semibold">LLM est.</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 px-2 font-semibold">Conf.</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 px-2 font-semibold">Edge</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 px-2 font-semibold">Spread</th>
                  <th className="text-right text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.1em] pb-2 pl-2 font-semibold">Action</th>
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
        </GlassCard>
      )}

      {/* Reject reasons */}
      {topRejectReasons.length > 0 && (
        <GlassCard padding="md">
          <div className="text-[11px] text-[color:var(--color-text-muted)] uppercase tracking-[0.14em] mb-3">
            Rejection analysis
          </div>
          <div className="space-y-2.5">
            {topRejectReasons.map(([reason, count]) => {
              const pct = data.signals_rejected > 0
                ? (count / data.signals_rejected) * 100
                : 0
              return (
                <div key={reason} className="flex flex-wrap items-center gap-3 sm:flex-nowrap">
                  <div className="w-full shrink-0 truncate text-xs text-[color:var(--color-text-secondary)] sm:w-44">{reason}</div>
                  <div className="flex-1 h-1.5 rounded-full overflow-hidden bg-[color:oklch(0.22_0.018_260/0.5)]">
                    <motion.div
                      initial={{ width: 0 }}
                      animate={{ width: `${pct}%` }}
                      transition={{ duration: 0.6, ease: 'easeOut' }}
                      className="h-full rounded-full"
                      style={{
                        background: 'linear-gradient(90deg, oklch(0.55 0.22 25 / 0.8), oklch(0.72 0.22 25))',
                        boxShadow: 'inset 0 0 6px oklch(0.72 0.22 25 / 0.4)',
                      }}
                    />
                  </div>
                  <div className="text-xs text-[color:var(--color-text-muted)] tabular font-mono w-8 text-right">{count}</div>
                </div>
              )
            })}
          </div>
        </GlassCard>
      )}

      {/* How it works */}
      <GlassCard padding="md" tone="subtle" className="text-xs text-[color:var(--color-text-muted)]">
        <div className="serif-accent text-[13px] text-[color:var(--color-text-primary)] mb-2">How Alpha AI works</div>
        <ol className="list-decimal ml-4 space-y-1 leading-relaxed">
          <li>Scanner filters Polymarket markets by volume ($5k–$50M) and blocks esports/gaming categories</li>
          <li>CLOB client fetches live orderbook — best bid/ask, spread, depth, and 1h price drift</li>
          <li><span className="text-[color:var(--color-aurora-3)]">Deep analysis call</span> — category-specific Bayesian reasoning with base rates and evidence weighing</li>
          <li><span className="text-[color:var(--color-whale-400)]">Devil's advocate call</span> — adversarial critique: missed evidence, cognitive biases, revised probability</li>
          <li>Final estimate = 70% deep analysis + 30% devil's advocate (penalty if disagreement {">"}15pp)</li>
          <li>Confidence is spread-calibrated: tight spread ({"<"}50bps) → −20%, wide spread ({">"}200bps) → +10%</li>
          <li>Kelly criterion sizes the position: quarter-Kelly × $100 bankroll → typical $1–$6 per trade</li>
          <li>Engine applies a second independent risk layer (circuit breaker, position cap, daily loss) before execution</li>
          <li>Every prediction is stored in memory — outcomes tracked, Brier scores computed, calibration updated</li>
        </ol>
      </GlassCard>
    </div>
  )
}
