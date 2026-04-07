import { memo } from 'react'

interface Props {
  bestBid: number | null
  bestAsk: number | null
  spreadBps: number | null
}

function SpreadIndicator({ bestBid, bestAsk, spreadBps }: Props) {
  if (bestBid == null || bestAsk == null || spreadBps == null) {
    return (
      <span className="text-slate-500 text-[11px]">No spread data</span>
    )
  }

  const color =
    spreadBps < 50
      ? 'text-emerald-400'
      : spreadBps <= 200
        ? 'text-yellow-400'
        : 'text-red-400'

  return (
    <span className={`inline-flex items-center gap-1.5 text-[11px] tabular-nums ${color}`}>
      <span className="text-emerald-400">
        BID <span className="text-slate-200">${bestBid.toFixed(3)}</span>
      </span>
      <span className="text-slate-500">←</span>
      <span className={color}>{spreadBps} bps</span>
      <span className="text-slate-500">→</span>
      <span className="text-red-400">
        ASK <span className="text-slate-200">${bestAsk.toFixed(3)}</span>
      </span>
    </span>
  )
}

export default memo(SpreadIndicator)
