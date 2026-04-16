import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import { fmtDuration } from '../lib/format'

export default function StatusBar() {
  const { data: status } = usePoll(api.status, 5_000)
  const { data: bullpen } = usePoll(api.bullpenHealth, 10_000)

  const uptimeSecs = status?.uptime_secs ?? 0
  const uptime = status ? fmtDuration(uptimeSecs) : '—'
  const wsOk = status?.ws_connected ?? false
  const bpOk = bullpen?.enabled && (bullpen?.consecutive_failures ?? 0) < 3

  return (
    <footer className="flex items-center gap-4 px-3 py-1 bg-surface-950 border-t border-slate-800 text-[10px] text-slate-500 shrink-0">
      <StatusPill label="WS" ok={wsOk} />
      <StatusPill label="Bullpen" ok={!!bpOk} dimmed={!bullpen?.enabled} />
      <span className="text-slate-600">│</span>
      <span>msgs: {status?.messages_total?.toLocaleString() ?? '—'}</span>
      <span>uptime: {uptime}</span>
      <span className="ml-auto text-slate-600">
        Blink v0.2.0 · <kbd className="text-slate-500">p</kbd> pause · <kbd className="text-slate-500">Ctrl+K</kbd> kill
      </span>
    </footer>
  )
}

function StatusPill({ label, ok, dimmed }: { label: string; ok: boolean; dimmed?: boolean }) {
  const dot = dimmed
    ? 'bg-slate-600'
    : ok
      ? 'bg-emerald-400'
      : 'bg-red-400 animate-pulse'

  return (
    <span className="flex items-center gap-1">
      <span className={`w-1.5 h-1.5 rounded-full ${dot}`} />
      <span className={dimmed ? 'text-slate-600' : 'text-slate-400'}>{label}</span>
    </span>
  )
}
