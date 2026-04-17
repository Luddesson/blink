import type { ClosedTrade } from '../../types'
import { fmtPnl, fmtDuration, fmtNeonTime, pnlClass } from '../../lib/format'
import MarketLink from '../MarketLink'

interface Props {
  top5Wins: ClosedTrade[]
  top5Losses: ClosedTrade[]
}

function TradeRow({ trade }: { trade: ClosedTrade }) {
  return (
    <tr className="border-b border-surface-700/50">
      <td className="py-1 pr-2 text-xs text-slate-300 max-w-[120px] truncate">
        <MarketLink tokenId={trade.token_id} label={trade.market_title ?? trade.token_id} />
      </td>
      <td className={`py-1 pr-2 text-xs font-mono text-right font-semibold ${pnlClass(trade.realized_pnl)}`}>
        ${fmtPnl(trade.realized_pnl)}
      </td>
      <td className="py-1 pr-2 text-[10px] font-mono text-slate-500 text-right">
        {fmtDuration(trade.duration_secs)}
      </td>
      <td className="py-1 text-[10px] font-mono text-cyan-400 text-right">
        {trade.closed_at ? fmtNeonTime(trade.closed_at) : '—'}
      </td>
    </tr>
  )
}

function TradeTable({ title, trades, emptyMsg }: { title: string; trades: ClosedTrade[]; emptyMsg: string }) {
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-slate-500 mb-1">{title}</p>
      {trades.length === 0 ? (
        <p className="text-xs text-slate-600">{emptyMsg}</p>
      ) : (
        <table className="w-full text-[11px]">
          <thead>
            <tr className="text-slate-600 border-b border-surface-600">
              <th className="text-left pb-1 font-normal">Market</th>
              <th className="text-right pb-1 font-normal">P&L</th>
              <th className="text-right pb-1 font-normal">Duration</th>
              <th className="text-right pb-1 font-normal">Closed</th>
            </tr>
          </thead>
          <tbody>
            {trades.map((t, i) => <TradeRow key={i} trade={t} />)}
          </tbody>
        </table>
      )}
    </div>
  )
}

export default function BiggestTrades({ top5Wins, top5Losses }: Props) {
  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Biggest Trades
      </span>
      <div className="grid grid-cols-2 gap-4">
        <TradeTable title="Top 5 Wins" trades={top5Wins} emptyMsg="No winning trades" />
        <TradeTable title="Top 5 Losses" trades={top5Losses} emptyMsg="No losing trades" />
      </div>
    </div>
  )
}
