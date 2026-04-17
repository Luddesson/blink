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
    <footer className="flex items-center gap-4 px-4 py-1.5 shrink-0 text-[10px] tabular border-t border-[color:var(--color-border-subtle)] bg-[color:oklch(0.14_0.013_260/0.7)] backdrop-blur-md text-[color:var(--color-text-muted)]">
      <StatusPill label="WS" ok={wsOk} />
      <StatusPill label="Bullpen" ok={bpOk} dimmed={!bullpen?.enabled} />
      <span className="text-[color:var(--color-text-dim)]">│</span>
      <span>msgs <span className="text-[color:var(--color-text-secondary)] font-mono">{status?.messages_total?.toLocaleString() ?? '—'}</span></span>
      <span>uptime <span className="text-[color:var(--color-text-secondary)] font-mono">{uptime}</span></span>
      <span className="ml-auto flex items-center gap-3 text-[color:var(--color-text-dim)]">
        <span className="flex items-center gap-1.5">
          <KeycapHint keys="P" /> pause
        </span>
        <span className="flex items-center gap-1.5">
          <KeycapHint keys={['⌃', 'K']} /> kill
        </span>
        <span className="font-mono">Blink v0.2 · Midnight Aurora</span>
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
