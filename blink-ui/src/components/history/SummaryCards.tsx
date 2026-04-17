import { fmt, fmtPnl, fmtDuration } from '../../lib/format'
import StatGrid from '../shared/StatGrid'
import type { TradeStats } from '../../hooks/useTradeStats'

type Props = Pick<TradeStats,
  'totalTrades' | 'winRate' | 'totalPnl' | 'netPnl' | 'avgPnl' | 'medianPnl' |
  'avgRiskReward' | 'profitFactor' | 'expectancy' | 'avgDuration' | 'medianDuration' |
  'totalFees' | 'avgSlippage'
>

export default function SummaryCards(props: Props) {
  const pnlColor = props.totalPnl >= 0 ? 'text-emerald-400' : 'text-red-400'
  const wrColor = props.winRate >= 50 ? 'text-emerald-400' : props.winRate >= 40 ? 'text-amber-400' : 'text-red-400'

  const stats = [
    { label: 'Total P&L', value: `$${fmtPnl(props.totalPnl)}`, deltaColor: pnlColor },
    { label: 'Net P&L (after fees)', value: `$${fmtPnl(props.netPnl)}`, deltaColor: props.netPnl >= 0 ? 'text-emerald-400' : 'text-red-400' },
    { label: 'Win Rate', value: `${fmt(props.winRate, 1)}%`, deltaColor: wrColor },
    { label: 'Avg R/R', value: props.avgRiskReward === Infinity ? '∞' : fmt(props.avgRiskReward, 2), delta: 'ratio' },
    { label: 'Profit Factor', value: props.profitFactor === Infinity ? '∞' : fmt(props.profitFactor, 2) },
    { label: 'Expectancy', value: `$${fmtPnl(props.expectancy)}`, deltaColor: props.expectancy >= 0 ? 'text-emerald-400' : 'text-red-400' },
    { label: 'Total Trades', value: String(props.totalTrades) },
    { label: 'Avg Duration', value: fmtDuration(props.avgDuration) },
    { label: 'Median Duration', value: fmtDuration(Math.round(props.medianDuration)) },
    { label: 'Avg P&L', value: `$${fmtPnl(props.avgPnl)}`, deltaColor: props.avgPnl >= 0 ? 'text-emerald-400' : 'text-red-400' },
    { label: 'Median P&L', value: `$${fmtPnl(props.medianPnl)}`, deltaColor: props.medianPnl >= 0 ? 'text-emerald-400' : 'text-red-400' },
    { label: 'Total Fees', value: `$${fmt(props.totalFees)}` },
    { label: 'Avg Slippage', value: `${fmt(props.avgSlippage, 1)} bps` },
  ]

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Performance Summary
      </span>
      <StatGrid stats={stats} columns={5} />
    </div>
  )
}
