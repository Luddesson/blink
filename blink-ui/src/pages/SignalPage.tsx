import { useMemo, useState } from 'react'
import { AlertTriangle, Bell, CheckCircle2, Radio, Settings, ShieldAlert, TrendingUp, Zap } from 'lucide-react'
import { SubFilterBar, type FilterOption } from '../components/ui'
import { api } from '../lib/api'
import { usePoll } from '../hooks/usePoll'
import type { ActivityEntry, BullpenConvergenceSignal, LiveExecution } from '../types'

const FILTERS: FilterOption[] = [
  { id: 'feed',        label: 'Signal Feed' },
  { id: 'convergence', label: 'Convergence' },
  { id: 'alerts',      label: 'Alerts' },
  { id: 'config',      label: 'Config' },
]

type FeedRowTone = 'good' | 'warn' | 'bad' | 'neutral' | 'trade'

interface FeedRow {
  id: string
  timeMs: number
  kind: string
  title: string
  detail: string
  source: string
  tone: FeedRowTone
}

const TONE_CLASS: Record<FeedRowTone, string> = {
  good: 'text-emerald-300 border-emerald-500/25 bg-emerald-500/10',
  warn: 'text-amber-300 border-amber-500/25 bg-amber-500/10',
  bad: 'text-rose-300 border-rose-500/25 bg-rose-500/10',
  neutral: 'text-slate-300 border-slate-700 bg-slate-800/60',
  trade: 'text-cyan-300 border-cyan-500/25 bg-cyan-500/10',
}

export default function SignalPage() {
  const [sub, setSub] = useState('feed')
  const { data: executions, loading: executionsLoading, error: executionsError } = usePoll(
    () => api.liveExecutionsAllResponse('24h', 200),
    10_000,
  )
  const { data: activity, loading: activityLoading, error: activityError } = usePoll(api.activity, 5_000)
  const { data: health } = usePoll(api.bullpenHealth, 10_000)
  const { data: convergence } = usePoll(api.bullpenConvergence, 5_000)

  const feedRows = useMemo(() => {
    const walletRows = (executions?.executions ?? []).map(walletExecutionRow)
    const activityRows = (activity?.entries ?? []).slice(-80).map(activityEntryRow)
    return [...walletRows, ...activityRows]
      .sort((a, b) => b.timeMs - a.timeMs)
      .slice(0, 120)
  }, [activity?.entries, executions?.executions])

  const truthStatus = executions?.reality_status ?? (executionsError ? 'unverified' : 'pending')
  const sourceRows = [
    {
      label: 'Wallet executions',
      value: executionsLoading ? 'loading' : String(executions?.total ?? 0),
      meta: truthStatus,
      tone: executions?.reality_status === 'matched' ? 'good' : executionsError ? 'bad' : 'warn',
    },
    {
      label: 'Engine activity',
      value: activityLoading ? 'loading' : String(activity?.entries?.length ?? 0),
      meta: activityError ? 'unverified' : 'observed',
      tone: activityError ? 'bad' : 'good',
    },
    {
      label: 'Bullpen',
      value: health?.enabled ? 'enabled' : 'disabled',
      meta: health?.last_error ?? health?.status ?? 'ok',
      tone: health?.enabled && !health?.last_error ? 'good' : health?.enabled ? 'warn' : 'neutral',
    },
  ] satisfies Array<{ label: string; value: string; meta: string; tone: FeedRowTone }>

  return (
    <div className="flex-1 flex flex-col overflow-hidden min-h-0">
      <SubFilterBar options={FILTERS} active={sub} onChange={setSub} />

      {sub === 'feed' && (
        <SignalFeedView
          rows={feedRows}
          sourceRows={sourceRows}
          loading={executionsLoading || activityLoading}
          error={executionsError ?? activityError}
        />
      )}
      {sub === 'convergence' && <ConvergenceView signals={convergence?.signals ?? []} enabled={!!convergence?.enabled} status={convergence?.status} />}
      {sub === 'alerts' && <AlertsView enabled={!!health?.enabled} error={health?.last_error ?? null} activeSignals={convergence?.active_signals ?? 0} />}
      {sub === 'config' && <ConfigView enabled={!!health?.enabled} authenticated={!!health?.authenticated} lastError={health?.last_error ?? null} />}
    </div>
  )
}

function SignalFeedView({
  rows,
  sourceRows,
  loading,
  error,
}: {
  rows: FeedRow[]
  sourceRows: Array<{ label: string; value: string; meta: string; tone: FeedRowTone }>
  loading: boolean
  error: string | null
}) {
  return (
    <div className="flex-1 grid grid-cols-1 xl:grid-cols-[1fr_320px] gap-2 p-2 overflow-hidden min-h-0">
      <section className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg flex flex-col overflow-hidden">
        <div className="flex items-center justify-between px-3 py-2 border-b border-slate-800">
          <div className="flex items-center gap-2">
            <Radio size={13} className="text-cyan-400" />
            <span className="text-[10px] font-semibold uppercase tracking-wider text-slate-400">
              Verified Signal Feed
            </span>
          </div>
          <TruthPill ok={!error && rows.length > 0} loading={loading} />
        </div>

        <div className="flex-1 overflow-y-auto min-h-0 divide-y divide-slate-800/70">
          {rows.length === 0 && (
            <div className="h-full flex items-center justify-center p-8 text-center">
              <div>
                <Zap size={28} className="text-slate-500 mx-auto mb-3" />
                <p className="text-sm text-slate-300 font-medium">{loading ? 'Loading verified feed' : 'No verified signal events'}</p>
                <p className="text-xs text-slate-500 mt-1">{error ?? 'Wallet executions and engine activity will appear here after verification.'}</p>
              </div>
            </div>
          )}
          {rows.map((row) => (
            <div key={row.id} className="grid grid-cols-[76px_92px_1fr] gap-2 px-3 py-2 text-xs">
              <span className="font-mono text-cyan-400">{formatTime(row.timeMs)}</span>
              <span className={`w-fit rounded border px-1.5 py-0.5 text-[10px] font-semibold uppercase ${TONE_CLASS[row.tone]}`}>
                {row.kind}
              </span>
              <div className="min-w-0">
                <div className="flex items-center justify-between gap-3">
                  <p className="truncate text-slate-200">{row.title}</p>
                  <span className="shrink-0 text-[10px] text-slate-600">{row.source}</span>
                </div>
                <p className="truncate text-[11px] text-slate-500">{row.detail}</p>
              </div>
            </div>
          ))}
        </div>
      </section>

      <aside className="flex flex-col gap-2 overflow-y-auto min-h-0">
        <section className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-3 flex items-center gap-2">
            <ShieldAlert size={11} /> Sources
          </div>
          <div className="space-y-2">
            {sourceRows.map((row) => (
              <div key={row.label} className="flex items-center justify-between gap-2 text-[11px]">
                <span className="text-slate-400">{row.label}</span>
                <span className="font-mono text-slate-200">{row.value}</span>
                <span className={`rounded border px-1.5 py-0.5 text-[10px] ${TONE_CLASS[row.tone]}`}>{row.meta}</span>
              </div>
            ))}
          </div>
        </section>

        <section className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-3 flex items-center gap-2">
            <TrendingUp size={11} /> Last Wallet Execution
          </div>
          <LastExecution rows={rows} />
        </section>
      </aside>
    </div>
  )
}

function LastExecution({ rows }: { rows: FeedRow[] }) {
  const row = rows.find((item) => item.kind === 'wallet')
  if (!row) {
    return <p className="text-xs text-slate-600">No verified wallet execution in the selected window.</p>
  }
  return (
    <div className="space-y-1 text-[11px]">
      <div className="flex justify-between gap-3">
        <span className="text-slate-500">Time</span>
        <span className="font-mono text-cyan-300">{formatTime(row.timeMs)}</span>
      </div>
      <div className="flex justify-between gap-3">
        <span className="text-slate-500">Execution</span>
        <span className="text-right text-slate-200">{row.title}</span>
      </div>
      <p className="text-slate-500">{row.detail}</p>
    </div>
  )
}

function ConvergenceView({
  signals,
  enabled,
  status,
}: {
  signals: BullpenConvergenceSignal[]
  enabled: boolean
  status?: string
}) {
  return (
    <div className="flex-1 overflow-y-auto p-2">
      <section className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg overflow-hidden">
        <div className="flex items-center justify-between px-3 py-2 border-b border-slate-800">
          <div className="flex items-center gap-2 text-[10px] font-semibold uppercase tracking-wider text-slate-400">
            <Bell size={12} className="text-cyan-400" /> Convergence
          </div>
          <span className={`rounded border px-2 py-0.5 text-[10px] ${enabled ? TONE_CLASS.warn : TONE_CLASS.neutral}`}>
            {status ?? (enabled ? 'enabled' : 'disabled')}
          </span>
        </div>

        {signals.length === 0 ? (
          <div className="p-8 text-center">
            <Bell size={28} className="text-slate-500 mx-auto mb-3" />
            <p className="text-sm text-slate-300 font-medium">No verified convergence signals</p>
            <p className="text-xs text-slate-500 mt-1">Current Bullpen status: {status ?? (enabled ? 'enabled' : 'disabled')}</p>
          </div>
        ) : (
          <div className="divide-y divide-slate-800/70">
            {signals.map((signal, idx) => (
              <div key={`${signal.market_title ?? 'market'}-${idx}`} className="grid grid-cols-[1fr_92px_92px_92px] gap-3 px-3 py-2 text-xs">
                <span className="truncate text-slate-200">{signal.market_title ?? 'Untitled market'}</span>
                <span className="font-mono text-slate-300">{signal.wallet_count} wallets</span>
                <span className="font-mono text-cyan-300">{signal.convergence_score.toFixed(2)}</span>
                <span className="font-mono text-slate-400">{formatUsd(signal.total_usd)}</span>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  )
}

function AlertsView({ enabled, error, activeSignals }: { enabled: boolean; error: string | null; activeSignals: number }) {
  const ok = enabled && !error
  return (
    <div className="flex-1 flex items-center justify-center p-8 text-center">
      <div className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-6 w-full max-w-sm">
        {ok ? <CheckCircle2 size={30} className="text-emerald-400 mx-auto mb-3" /> : <AlertTriangle size={30} className="text-amber-400 mx-auto mb-3" />}
        <p className="text-sm text-slate-300 font-medium">{ok ? 'Alert source connected' : 'Alert source unavailable'}</p>
        <div className="mt-4 grid grid-cols-2 gap-2 text-xs">
          <Metric label="Enabled" value={enabled ? 'yes' : 'no'} />
          <Metric label="Active" value={String(activeSignals)} />
        </div>
        {error && <p className="text-xs text-amber-300 mt-3 break-all">{error}</p>}
      </div>
    </div>
  )
}

function ConfigView({ enabled, authenticated, lastError }: { enabled: boolean; authenticated: boolean; lastError: string | null }) {
  return (
    <div className="flex-1 flex items-center justify-center p-8">
      <div className="bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-6 w-full max-w-sm">
        <div className="flex items-center gap-2 text-[10px] font-semibold uppercase tracking-wider text-slate-500 mb-4">
          <Settings size={12} /> Runtime Status
        </div>
        <div className="grid grid-cols-1 gap-2 text-xs">
          <Metric label="Bullpen enabled" value={enabled ? 'yes' : 'no'} />
          <Metric label="Authenticated" value={authenticated ? 'yes' : 'no'} />
          <Metric label="Last error" value={lastError ?? 'none'} />
        </div>
      </div>
    </div>
  )
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded border border-slate-800 bg-slate-900/40 px-2 py-1.5">
      <span className="text-slate-500">{label}</span>
      <span className="font-mono text-slate-200 truncate">{value}</span>
    </div>
  )
}

function TruthPill({ ok, loading }: { ok: boolean; loading: boolean }) {
  const tone = ok ? 'good' : loading ? 'warn' : 'neutral'
  return (
    <span className={`rounded border px-2 py-0.5 text-[10px] font-semibold uppercase ${TONE_CLASS[tone]}`}>
      {ok ? 'verified' : loading ? 'checking' : 'empty'}
    </span>
  )
}

function walletExecutionRow(execution: LiveExecution): FeedRow {
  const title = `${execution.side.toUpperCase()} ${formatShares(execution.shares)} @ ${formatPrice(execution.price)}`
  const market = [execution.market_title, execution.market_outcome].filter(Boolean).join(' / ')
  return {
    id: `wallet-${execution.transaction_hash ?? execution.token_id}-${execution.timestamp}`,
    timeMs: execution.timestamp * 1000,
    kind: 'wallet',
    title,
    detail: market || shortToken(execution.token_id),
    source: execution.source,
    tone: 'trade',
  }
}

function activityEntryRow(entry: ActivityEntry): FeedRow {
  const kind = entry.kind.toLowerCase()
  return {
    id: `activity-${entry.timestamp}-${entry.kind}-${entry.message}`,
    timeMs: Date.parse(entry.timestamp) || 0,
    kind,
    title: entry.message,
    detail: entry.timestamp,
    source: 'engine_activity',
    tone: activityTone(kind),
  }
}

function activityTone(kind: string): FeedRowTone {
  if (kind === 'fill' || kind === 'success') return 'good'
  if (kind === 'abort' || kind === 'error') return 'bad'
  if (kind === 'warn' || kind === 'skip') return 'warn'
  if (kind === 'signal') return 'trade'
  return 'neutral'
}

function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '--:--:--'
  return new Date(ms).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

function formatShares(value: number): string {
  return Number.isFinite(value) ? value.toFixed(4).replace(/\.?0+$/, '') : '0'
}

function formatPrice(value: number): string {
  return Number.isFinite(value) ? value.toFixed(3) : '0.000'
}

function formatUsd(value: number): string {
  return Number.isFinite(value) ? `$${value.toFixed(2)}` : '$0.00'
}

function shortToken(tokenId: string): string {
  if (tokenId.length <= 16) return tokenId
  return `${tokenId.slice(0, 8)}...${tokenId.slice(-6)}`
}
