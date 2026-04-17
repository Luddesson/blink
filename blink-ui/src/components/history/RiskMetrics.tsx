import { fmt } from '../../lib/format'
import type { TradeStats } from '../../hooks/useTradeStats'

type Props = Pick<TradeStats, 'maxDrawdown' | 'maxDrawdownPct' | 'sharpeRatio' | 'sortinoRatio' | 'calmarRatio' | 'profitFactor'>

function metricColor(value: number, thresholds: [number, number]): string {
  if (value >= thresholds[1]) return 'text-emerald-400'
  if (value >= thresholds[0]) return 'text-amber-400'
  return 'text-red-400'
}

export default function RiskMetrics(props: Props) {
  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Risk Metrics
      </span>
      <div className="grid grid-cols-3 gap-4">
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Max Drawdown</div>
          <div className={`text-sm font-mono font-semibold ${props.maxDrawdownPct > 5 ? 'text-red-400' : props.maxDrawdownPct > 2 ? 'text-amber-400' : 'text-emerald-400'}`}>
            -{fmt(props.maxDrawdownPct)}%
          </div>
          <div className="text-[10px] text-slate-600 font-mono">${fmt(props.maxDrawdown)}</div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Sharpe Ratio</div>
          <div className={`text-sm font-mono font-semibold ${metricColor(props.sharpeRatio, [0, 1])}`}>
            {fmt(props.sharpeRatio, 2)}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Sortino Ratio</div>
          <div className={`text-sm font-mono font-semibold ${metricColor(props.sortinoRatio, [0, 2])}`}>
            {fmt(props.sortinoRatio, 2)}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Calmar Ratio</div>
          <div className={`text-sm font-mono font-semibold ${metricColor(props.calmarRatio, [0, 1])}`}>
            {fmt(props.calmarRatio, 2)}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Profit Factor</div>
          <div className={`text-sm font-mono font-semibold ${metricColor(props.profitFactor, [1, 1.5])}`}>
            {props.profitFactor === Infinity ? '∞' : fmt(props.profitFactor, 2)}
          </div>
        </div>
      </div>
    </div>
  )
}
