import { useState } from 'react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import type { ProjectInventoryItem, ProjectInventoryStatus } from '../types'

const STATUS_ORDER: readonly ProjectInventoryStatus[] = [
  'active-runtime',
  'active-ops',
  'compiled-not-wired',
  'archived-or-legacy',
  'unknown-needs-review',
]

function statusTone(status: ProjectInventoryStatus): string {
  switch (status) {
    case 'active-runtime':
      return 'border-[color:oklch(0.70_0.16_145/0.5)] bg-[color:oklch(0.28_0.08_145/0.28)] text-[color:oklch(0.84_0.12_145)]'
    case 'active-ops':
      return 'border-[color:oklch(0.74_0.13_250/0.45)] bg-[color:oklch(0.28_0.06_250/0.25)] text-[color:oklch(0.88_0.08_250)]'
    case 'compiled-not-wired':
      return 'border-[color:oklch(0.75_0.18_80/0.45)] bg-[color:oklch(0.30_0.09_80/0.24)] text-[color:oklch(0.90_0.10_80)]'
    case 'archived-or-legacy':
      return 'border-[color:oklch(0.65_0.05_260/0.35)] bg-[color:oklch(0.24_0.02_260/0.30)] text-[color:var(--color-text-muted)]'
    default:
      return 'border-[color:oklch(0.78_0.15_30/0.45)] bg-[color:oklch(0.30_0.10_30/0.22)] text-[color:oklch(0.90_0.09_30)]'
  }
}

function normalize(value: string): string {
  return value.toLowerCase().trim()
}

function includesQuery(item: ProjectInventoryItem, query: string): boolean {
  if (!query) return true
  const q = normalize(query)
  if (normalize(item.name).includes(q) || normalize(item.area).includes(q) || normalize(item.status).includes(q)) {
    return true
  }
  return item.evidence.some((ev) => normalize(ev.path).includes(q) || normalize(ev.note).includes(q))
}

export default function ProjectInventoryPage() {
  const { data } = usePoll(api.projectInventory, 30_000)
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState<'all' | ProjectInventoryStatus>('all')
  const [area, setArea] = useState('all')

  const items = data?.items ?? []
  const summary = data?.summary
  const areas = Array.from(new Set(items.map((item) => item.area))).sort()
  const filteredItems = items.filter((item) => {
    if (status !== 'all' && item.status !== status) return false
    if (area !== 'all' && item.area !== area) return false
    return includesQuery(item, query)
  })

  if (!data?.available) {
    return (
      <div className="flex-1 p-3 overflow-y-auto">
        <section className="rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-elevated)] p-4 shadow-[0_12px_40px_rgba(0,0,0,0.2)]">
          <h2 className="text-sm font-semibold text-[color:var(--color-text-primary)]">Project Inventory unavailable</h2>
          <p className="mt-2 text-xs text-[color:var(--color-text-muted)]">{data?.error ?? 'Inventory payload missing.'}</p>
          <p className="mt-3 text-xs text-[color:var(--color-text-secondary)]">
            Generate with <code className="font-mono">{data?.generate_command ?? '.\\scripts\\generate-project-inventory.ps1'}</code>
          </p>
        </section>
      </div>
    )
  }

  return (
    <div className="flex-1 p-3 overflow-y-auto">
      <section className="grid grid-cols-1 gap-2 md:grid-cols-3">
        <div className="rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] p-3">
          <p className="text-[11px] text-[color:var(--color-text-muted)]">Total inventory items</p>
          <p className="mt-1 text-xl font-semibold text-[color:var(--color-text-primary)]">{summary?.totalItems ?? items.length}</p>
        </div>
        <div className="rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] p-3">
          <p className="text-[11px] text-[color:var(--color-text-muted)]">Visible after filters</p>
          <p className="mt-1 text-xl font-semibold text-[color:var(--color-text-primary)]">{filteredItems.length}</p>
        </div>
        <div className="rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] p-3">
          <p className="text-[11px] text-[color:var(--color-text-muted)]">Generated</p>
          <p className="mt-1 text-xs font-medium text-[color:var(--color-text-secondary)]">
            {data.generatedAt ? new Date(data.generatedAt).toLocaleString() : 'unknown'}
          </p>
        </div>
      </section>

      <section className="mt-3 rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-elevated)] p-3">
        <div className="flex flex-wrap items-center gap-2">
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search name, area, status, path or evidence notes"
            className="min-w-[250px] flex-1 rounded-md border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] px-3 py-2 text-xs text-[color:var(--color-text-primary)] outline-none focus:border-[color:var(--color-bull-500)]"
          />
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value as 'all' | ProjectInventoryStatus)}
            className="rounded-md border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] px-2 py-2 text-xs text-[color:var(--color-text-primary)]"
          >
            <option value="all">All statuses</option>
            {STATUS_ORDER.map((entry) => (
              <option key={entry} value={entry}>{entry}</option>
            ))}
          </select>
          <select
            value={area}
            onChange={(event) => setArea(event.target.value)}
            className="rounded-md border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] px-2 py-2 text-xs text-[color:var(--color-text-primary)]"
          >
            <option value="all">All areas</option>
            {areas.map((entry) => (
              <option key={entry} value={entry}>{entry}</option>
            ))}
          </select>

          <button
            onClick={async () => {
              try {
                const res = await api.projectInventory()
                const blob = new Blob([JSON.stringify(res, null, 2)], { type: 'application/json' })
                const url = URL.createObjectURL(blob)
                const a = document.createElement('a')
                a.href = url
                const ts = res.generatedAt ? new Date(res.generatedAt).toISOString().slice(0,19).replace(/:/g,'') : 'snapshot'
                a.download = `project-inventory-${ts}.json`
                document.body.appendChild(a)
                a.click()
                a.remove()
                URL.revokeObjectURL(url)
              } catch (err) {
                // @ts-ignore
                alert('Failed to download project inventory: ' + (err?.message ?? String(err)))
                console.error(err)
              }
            }}
            className="rounded-md bg-[color:var(--color-bull-600)] px-3 py-2 text-xs font-medium text-white"
          >
            Download JSON
          </button>
        </div>

        <div className="mt-3 overflow-x-auto rounded-md border border-[color:var(--color-border-subtle)]">
          <table className="min-w-full text-xs">
            <thead className="bg-[color:var(--color-surface-subtle)] text-[color:var(--color-text-muted)]">
              <tr>
                <th className="px-3 py-2 text-left font-medium">Area</th>
                <th className="px-3 py-2 text-left font-medium">Name</th>
                <th className="px-3 py-2 text-left font-medium">Status</th>
                <th className="px-3 py-2 text-left font-medium">Confidence</th>
                <th className="px-3 py-2 text-left font-medium">Evidence</th>
                <th className="px-3 py-2 text-left font-medium">Recommendation</th>
              </tr>
            </thead>
            <tbody>
              {filteredItems.map((item) => (
                <tr key={item.id} className="border-t border-[color:var(--color-border-subtle)] align-top">
                  <td className="px-3 py-2 text-[color:var(--color-text-secondary)]">{item.area}</td>
                  <td className="px-3 py-2 text-[color:var(--color-text-primary)]">{item.name}</td>
                  <td className="px-3 py-2">
                    <span className={`inline-flex rounded-full border px-2 py-0.5 text-[10px] font-semibold ${statusTone(item.status)}`}>
                      {item.status}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-[color:var(--color-text-secondary)]">{item.confidence}</td>
                  <td className="px-3 py-2 text-[color:var(--color-text-muted)]">
                    {item.evidence.length === 0 ? (
                      <span>none</span>
                    ) : (
                      <div className="space-y-1">
                        {item.evidence.slice(0, 2).map((ev) => (
                          <div key={`${item.id}-${ev.path}-${ev.line ?? 'n'}`} className="font-mono text-[10px]">
                            {ev.path}{ev.line ? `:${ev.line}` : ''} — {ev.note}
                          </div>
                        ))}
                        {item.evidence.length > 2 && (
                          <div className="text-[10px] text-[color:var(--color-text-muted)]">+{item.evidence.length - 2} more</div>
                        )}
                      </div>
                    )}
                  </td>
                  <td className="px-3 py-2 text-[color:var(--color-text-secondary)]">{item.recommendation}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  )
}

