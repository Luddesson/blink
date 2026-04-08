import { useState, useEffect } from 'react'
import type { RiskSummary } from '../types'
import { fmt } from '../lib/format'
import { api } from '../lib/api'

interface Props {
  risk: RiskSummary
  className?: string
}

export default function RiskConfigForm({ risk, className }: Props) {
  const [editing, setEditing] = useState(false)
  const [saving, setSaving] = useState(false)
  const [draft, setDraft] = useState({
    max_daily_loss_pct: (risk.max_daily_loss_pct ?? 0.05) * 100,
    max_concurrent_positions: risk.max_concurrent_positions ?? 5,
    max_single_order_usdc: risk.max_single_order_usdc ?? 20,
    max_orders_per_second: risk.max_orders_per_second ?? 3,
    var_threshold_pct: (risk.var_threshold_pct ?? 0.05) * 100,
  })

  useEffect(() => {
    if (!editing) {
      setDraft({
        max_daily_loss_pct: (risk.max_daily_loss_pct ?? 0.05) * 100,
        max_concurrent_positions: risk.max_concurrent_positions ?? 5,
        max_single_order_usdc: risk.max_single_order_usdc ?? 20,
        max_orders_per_second: risk.max_orders_per_second ?? 3,
        var_threshold_pct: (risk.var_threshold_pct ?? 0.05) * 100,
      })
    }
  }, [risk, editing])

  const handleSave = async () => {
    setSaving(true)
    try {
      await api.updateConfig({
        max_daily_loss_pct: draft.max_daily_loss_pct / 100,
        max_concurrent_positions: draft.max_concurrent_positions,
        max_single_order_usdc: draft.max_single_order_usdc,
        max_orders_per_second: draft.max_orders_per_second,
        var_threshold_pct: draft.var_threshold_pct / 100,
      })
      setEditing(false)
    } catch (e) {
      alert(`Save failed: ${e}`)
    } finally {
      setSaving(false)
    }
  }

  const Row = ({ label, field, suffix, step }: { label: string; field: keyof typeof draft; suffix: string; step?: number }) => (
    <div className="flex justify-between items-center py-2 border-b border-slate-800">
      <span className="text-xs text-slate-400">{label}</span>
      {editing ? (
        <div className="flex items-center gap-1">
          <input
            type="number"
            value={draft[field]}
            step={step ?? 1}
            onChange={(e) => setDraft({ ...draft, [field]: parseFloat(e.target.value) || 0 })}
            className="w-20 bg-slate-800 border border-slate-600 rounded px-2 py-0.5 text-xs font-mono text-slate-100 text-right focus:outline-none focus:border-cyan-500"
          />
          <span className="text-[10px] text-slate-500">{suffix}</span>
        </div>
      ) : (
        <span className="text-xs font-mono tabular-nums text-slate-100">
          {typeof draft[field] === 'number' && suffix === '%'
            ? `${fmt(draft[field] as number, 1)}%`
            : suffix === '$'
              ? `$${fmt(draft[field] as number)}`
              : String(draft[field])}
        </span>
      )}
    </div>
  )

  return (
    <div className={`card ${className ?? ''}`}>
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Risk Parameters
        </span>
        {!editing ? (
          <button
            onClick={() => setEditing(true)}
            className="text-[10px] px-2 py-0.5 rounded bg-slate-700 text-slate-300 hover:bg-slate-600"
          >
            Edit
          </button>
        ) : (
          <div className="flex gap-1">
            <button
              onClick={() => setEditing(false)}
              className="text-[10px] px-2 py-0.5 rounded bg-slate-700 text-slate-400 hover:bg-slate-600"
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="text-[10px] px-2 py-0.5 rounded bg-cyan-800 text-cyan-200 hover:bg-cyan-700 disabled:opacity-40"
            >
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        )}
      </div>

      <div>
        <Row label="Max Daily Loss" field="max_daily_loss_pct" suffix="%" step={0.5} />
        <Row label="Max Concurrent Positions" field="max_concurrent_positions" suffix="" />
        <Row label="Max Order Size" field="max_single_order_usdc" suffix="$" step={1} />
        <Row label="Max Orders/sec" field="max_orders_per_second" suffix="" />
        <Row label="VaR Threshold" field="var_threshold_pct" suffix="%" step={0.5} />
        <div className="flex justify-between items-center py-2 border-b border-slate-800">
          <span className="text-xs text-slate-400">Trading Enabled</span>
          <span className={`text-xs font-mono font-semibold ${risk.trading_enabled ? 'text-emerald-400' : 'text-red-400'}`}>
            {risk.trading_enabled ? 'YES' : 'NO'}
          </span>
        </div>
      </div>

      {!editing && (
        <p className="text-[10px] text-slate-600 mt-3 italic">
          Click Edit to modify parameters at runtime
        </p>
      )}
    </div>
  )
}
