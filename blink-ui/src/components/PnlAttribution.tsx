import { useState, useEffect } from 'react'
import type { PnlAttributionResponse } from '../types'
import { fmt } from '../lib/format'

const ENGINE_BASE = 'http://127.0.0.1:7878'

function BarRow({ label, value, maxAbs }: { label: string; value: number; maxAbs: number }) {
  const pct = maxAbs > 0 ? (Math.abs(value) / maxAbs) * 100 : 0
  const positive = value >= 0
  return (
    <div className="relative">
      <div
        className={`absolute inset-y-0 left-0 rounded ${positive ? 'bg-emerald-500/30' : 'bg-red-500/30'}`}
        style={{ width: `${pct}%` }}
      />
      <div className="relative flex items-center justify-between px-2 py-1">
        <span className="text-xs text-slate-300 truncate max-w-[65%]">{label}</span>
        <span className={`text-xs font-mono ${positive ? 'text-emerald-400' : 'text-red-400'}`}>
          {value >= 0 ? '+' : ''}${fmt(value)}
        </span>
      </div>
    </div>
  )
}

function Section({ title, data }: { title: string; data: Record<string, number> }) {
  const entries = Object.entries(data).sort((a, b) => Math.abs(b[1]) - Math.abs(a[1]))
  const maxAbs = Math.max(...entries.map(([, v]) => Math.abs(v)), 1)
  if (entries.length === 0) return null
  return (
    <div className="mb-4">
      <p className="text-[10px] uppercase tracking-widest text-slate-500 mb-1">{title}</p>
      <div className="space-y-1">
        {entries.map(([k, v]) => <BarRow key={k} label={k} value={v} maxAbs={maxAbs} />)}
      </div>
    </div>
  )
}

export default function PnlAttribution({ className }: { className?: string }) {
  const [data, setData] = useState<PnlAttributionResponse | null>(null)
  const [error, setError] = useState(false)

  useEffect(() => {
    let alive = true
    const load = () => {
      fetch(`${ENGINE_BASE}/api/pnl-attribution`)
        .then(r => r.json())
        .then((d: PnlAttributionResponse) => { if (alive) setData(d) })
        .catch(() => { if (alive) setError(true) })
    }
    load()
    const id = setInterval(load, 10_000)
    return () => { alive = false; clearInterval(id) }
  }, [])

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        P&L Attribution
      </span>
      {error && <p className="text-xs text-red-400">Engine unavailable</p>}
      {!data && !error && <p className="text-xs text-slate-500">Loading…</p>}
      {data && !data.available && <p className="text-xs text-slate-500">No closed trades yet</p>}
      {data?.available && (
        <>
          <Section title="By Category" data={data.by_category} />
          <Section title="By Exit Reason" data={data.by_reason} />
          <Section title="By Side" data={data.by_side} />
        </>
      )}
    </div>
  )
}

