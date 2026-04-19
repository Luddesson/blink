import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import { fmtDuration } from '../lib/format'
import StatusDot from './aurora/StatusDot'
import KeycapHint from './aurora/KeycapHint'

export default function StatusBar() {
  const { data: status } = usePoll(api.status, 5_000)
  const { data: bullpen } = usePoll(api.bullpenHealth, 10_000)

  const uptimeSecs = status?.uptime_secs ?? 0
  const uptime = status ? fmtDuration(uptimeSecs) : '—'
  const wsOk = status?.ws_connected ?? false
  const bpOk = !!(bullpen?.enabled && (bullpen?.consecutive_failures ?? 0) < 3)

  return (
    <footer className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-[color:var(--color-border-subtle)] bg-[color:oklch(0.14_0.013_260/0.7)] px-3 py-1.5 text-[10px] tabular text-[color:var(--color-text-muted)] backdrop-blur-md sm:px-4">
      <StatusPill label="WS" ok={wsOk} />
      <StatusPill label="Bullpen" ok={bpOk} dimmed={!bullpen?.enabled} />
      <span className="hidden text-[color:var(--color-text-dim)] sm:inline">│</span>
      <span>msgs <span className="text-[color:var(--color-text-secondary)] font-mono">{status?.messages_total?.toLocaleString() ?? '—'}</span></span>
      <span>uptime <span className="text-[color:var(--color-text-secondary)] font-mono">{uptime}</span></span>
      <span className="flex w-full items-center gap-3 text-[color:var(--color-text-dim)] sm:ml-auto sm:w-auto">
        <span className="hidden items-center gap-1.5 sm:flex">
          <KeycapHint keys="P" /> pause
        </span>
        <span className="hidden items-center gap-1.5 sm:flex">
          <KeycapHint keys={['⌃', 'K']} /> kill
        </span>
        <span className="font-mono sm:ml-0">Blink v0.2 · Midnight Aurora</span>
      </span>
    </footer>
  )
}

function StatusPill({ label, ok, dimmed }: { label: string; ok: boolean; dimmed?: boolean }) {
  const tone = dimmed ? 'dim' : ok ? 'ok' : 'bad'
  return (
    <span className="flex items-center gap-1.5">
      <StatusDot tone={tone} size="xs" pulse={ok && !dimmed ? 'slow' : 'none'} />
      <span className={dimmed ? 'text-[color:var(--color-text-dim)]' : 'text-[color:var(--color-text-secondary)]'}>{label}</span>
    </span>
  )
}
