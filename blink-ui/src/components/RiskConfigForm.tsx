import type { RiskSummary } from '../types'
import { fmt } from '../lib/format'

interface Props {
  risk: RiskSummary
  className?: string
}

const Row = ({ label, value, valueClass }: { label: string; value: string; valueClass?: string }) => (
  <div className="flex justify-between items-center py-2 border-b border-slate-800">
    <span className="text-xs text-slate-400">{label}</span>
    <span className={`text-xs font-mono tabular-nums text-slate-100 ${valueClass ?? ''}`}>
      {value}
    </span>
  </div>
)

export default function RiskConfigForm({ risk, className }: Props) {
  const maxLossPct = risk.max_daily_loss_pct ?? 0.05

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Risk Parameters
      </span>

      <div>
        <Row label="Max Daily Loss" value={`${fmt(maxLossPct * 100, 1)}%`} />
        <Row
          label="Max Concurrent Positions"
          value={risk.max_concurrent_positions != null ? String(risk.max_concurrent_positions) : '—'}
        />
        <Row
          label="Max Order Size"
          value={risk.max_single_order_usdc != null ? `$${fmt(risk.max_single_order_usdc)}` : '—'}
        />
        <Row
          label="Max Orders/sec"
          value={risk.max_orders_per_second != null ? String(risk.max_orders_per_second) : '—'}
        />
        <Row
          label="VaR Threshold"
          value={risk.var_threshold_pct != null ? `${fmt(risk.var_threshold_pct * 100, 1)}%` : '—'}
        />
        <Row
          label="Trading Enabled"
          value={risk.trading_enabled ? 'YES' : 'NO'}
          valueClass={risk.trading_enabled ? '!text-emerald-400 font-semibold' : '!text-red-400 font-semibold'}
        />
      </div>

      <p className="text-[10px] text-slate-600 mt-3 italic">
        Config editing via .env — live edit coming soon
      </p>
    </div>
  )
}
