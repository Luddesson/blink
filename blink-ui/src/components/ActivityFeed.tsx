import { useEffect, useRef, useState } from 'react'
import type { ActivityEntry } from '../types'
import { fmtNeonTime } from '../lib/format'
import { api } from '../lib/api'

interface Props {
  wsEntries: ActivityEntry[]
}

const LEVEL_COLORS: Record<string, string> = {
  // Frontend lowercase (from WS recent_activity)
  info: 'text-slate-400',
  warn: 'text-yellow-400',
  error: 'text-red-400',
  success: 'text-emerald-400',
  trade: 'text-indigo-300',
  signal: 'text-sky-400',
  // Backend Debug-format values (from /api/activity)
  Engine: 'text-slate-400',
  Signal: 'text-sky-400',
  Fill: 'text-emerald-400',
  Abort: 'text-red-400',
  Skip: 'text-yellow-400',
  Warn: 'text-yellow-400',
}

export default function ActivityFeed({ wsEntries }: Props) {
  const bottomRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const [allEntries, setAllEntries] = useState<ActivityEntry[]>([])

  // Poll /api/activity (100 entries) as supplement to WS 5-entry truncation
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>
    const fetch = () => {
      api.activity()
        .then(({ entries }) => setAllEntries(entries))
        .catch(() => {})
        .finally(() => { timer = setTimeout(fetch, 5_000) })
    }
    fetch()
    return () => clearTimeout(timer)
  }, [])

  // Merge REST entries with latest WS entries (WS wins for newest items)
  const merged = (() => {
    const wsIds = new Set(wsEntries.map(e => `${e.timestamp}|${e.message}`))
    const base = allEntries.filter(e => !wsIds.has(`${e.timestamp}|${e.message}`))
    return [...base, ...wsEntries].sort((a, b) => a.timestamp.localeCompare(b.timestamp))
  })()

  // Only auto-scroll if user is near the bottom (within 80px)
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const distFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    if (distFromBottom <= 80) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [merged.length])

  return (
    <div className="card flex flex-col">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-2 shrink-0">
        Activity Feed
      </span>
      <div className="flex-1 overflow-y-auto max-h-64 space-y-0.5 font-mono text-xs" ref={scrollRef}>
        {merged.length === 0 && (
          <p className="text-slate-600 text-center py-4">No recent activity</p>
        )}
        {merged.map((e, i) => (
          <div key={i} className="flex gap-2 leading-5">
            <span className="text-cyan-400 shrink-0">
              {fmtNeonTime(e.timestamp)}
            </span>
            <span className={`shrink-0 w-14 ${LEVEL_COLORS[e.kind] ?? 'text-slate-400'}`}>
              [{e.kind.toUpperCase()}]
            </span>
            <span className="text-slate-300 break-all">{e.message}</span>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
