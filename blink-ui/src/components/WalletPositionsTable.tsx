import { memo, useMemo } from 'react'
import { WalletCards } from 'lucide-react'
import type { WalletPositionPreview } from '../types'
import { fmt } from '../lib/format'
import { cn } from '../lib/cn'

interface Props {
  positions?: WalletPositionPreview[]
  totalCount?: number
  totalValue?: number
}

function toNumber(value: WalletPositionPreview[keyof WalletPositionPreview]): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return undefined
}

function toText(value: WalletPositionPreview[keyof WalletPositionPreview]): string | undefined {
  return typeof value === 'string' && value.trim().length > 0 ? value : undefined
}

function WalletPositionsTable({ positions = [], totalCount, totalValue }: Props) {
  const count = totalCount ?? positions.length
  const visible = useMemo(() => positions.slice(0, 5), [positions])
  const displayValue = totalValue ?? visible.reduce((sum, p) => sum + (toNumber(p.current_value_usdc) ?? 0), 0)

  if (count <= 0) return null

  return (
    <div className="card-compact overflow-hidden border border-amber-500/25 bg-amber-950/10 backdrop-blur-xl">
      <div className="flex flex-wrap items-center justify-between gap-2 px-2 mb-4 mt-2">
        <div className="flex items-center gap-2.5">
          <div className="p-1.5 rounded-md bg-amber-400/10">
            <WalletCards size={14} className="text-amber-300" />
          </div>
          <span className="text-[11px] font-black uppercase tracking-[0.2em] text-amber-200">
            Wallet Positions
          </span>
          <span className="px-1.5 py-0.5 rounded-full bg-amber-500/15 text-[10px] font-bold tabular font-mono text-amber-200">
            {count}
          </span>
        </div>
        <div className="flex items-center gap-2 text-[10px] uppercase tracking-[0.12em] text-amber-200/80">
          <span>Exchange value</span>
          <span className="font-mono font-semibold text-amber-100">${fmt(displayValue)}</span>
        </div>
      </div>

      <div className="overflow-x-auto">
        <table className="w-full text-left border-collapse">
          <thead>
            <tr className="border-b border-amber-500/15 bg-[color:var(--color-surface-950)/0.24]">
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-amber-200/60 font-bold">Market</th>
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-amber-200/60 font-bold">Outcome</th>
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-amber-200/60 font-bold text-right">Size</th>
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-amber-200/60 font-bold text-right">Value</th>
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-amber-200/60 font-bold text-right">P&L</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-amber-500/10">
            {visible.map((position, index) => {
              const title = toText(position.title) ?? 'Unknown market'
              const outcome = toText(position.outcome) ?? '-'
              const size = toNumber(position.size)
              const value = toNumber(position.current_value_usdc)
              const pnl = toNumber(position.cash_pnl_usdc)

              return (
                <tr key={`${title}-${outcome}-${index}`} className="transition-colors hover:bg-amber-500/5">
                  <td className="py-3 px-3 min-w-[220px] max-w-[360px]">
                    <div className="truncate text-xs font-medium text-[color:var(--color-text-secondary)]" title={title}>
                      {title}
                    </div>
                  </td>
                  <td className="py-3 px-3">
                    <span className="inline-flex max-w-[160px] items-center rounded border border-amber-400/20 bg-amber-400/10 px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wider text-amber-100">
                      <span className="truncate">{outcome}</span>
                    </span>
                  </td>
                  <td className="py-3 px-3 text-right tabular font-mono text-[11px] text-[color:var(--color-text-secondary)]">
                    {size === undefined ? '-' : fmt(size, 3)}
                  </td>
                  <td className="py-3 px-3 text-right tabular font-mono text-[11px] font-semibold text-amber-100">
                    {value === undefined ? '-' : `$${fmt(value)}`}
                  </td>
                  <td className={cn(
                    'py-3 px-3 text-right tabular font-mono text-[11px] font-black',
                    pnl === undefined || Math.abs(pnl) < 0.005
                      ? 'text-[color:var(--color-text-muted)]'
                      : pnl > 0
                        ? 'text-[color:var(--color-bull-400)]'
                        : 'text-[color:var(--color-bear-400)]',
                  )}>
                    {pnl === undefined ? '-' : `${pnl >= 0 ? '+' : ''}$${fmt(pnl)}`}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      {count > visible.length && (
        <div className="border-t border-amber-500/10 px-3 py-2 text-right text-[10px] uppercase tracking-[0.12em] text-amber-200/70">
          +{count - visible.length} more
        </div>
      )}
    </div>
  )
}

export default memo(WalletPositionsTable)
