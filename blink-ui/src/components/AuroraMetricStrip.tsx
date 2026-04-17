import { motion } from 'motion/react'
import { TrendingUp, TrendingDown, Wallet, Activity, Target, Coins } from 'lucide-react'
import NumberFlip from './motion/NumberFlip'
import { cn } from '../lib/cn'
import { fmt } from '../lib/format'

interface Metric {
  label: string
  value: number
  format: (v: number) => string
  icon: React.ReactNode
  tone?: 'neutral' | 'bull' | 'bear' | 'iris'
  subtitle?: string
}

interface Props {
  nav: number
  realized: number
  unrealized: number
  fees: number
  winRate: number
  closedTrades: number
}

const TONE_RING: Record<NonNullable<Metric['tone']>, string> = {
  neutral: 'oklch(0.62 0.02 260 / 0.25)',
  bull: 'oklch(0.72 0.18 152 / 0.35)',
  bear: 'oklch(0.65 0.24 25 / 0.35)',
  iris: 'oklch(0.72 0.16 285 / 0.38)',
}

const TONE_GLOW: Record<NonNullable<Metric['tone']>, string> = {
  neutral: '0 14px 34px -16px oklch(0.62 0.02 260 / 0.3)',
  bull: '0 14px 34px -14px oklch(0.72 0.18 152 / 0.45)',
  bear: '0 14px 34px -14px oklch(0.65 0.24 25 / 0.45)',
  iris: '0 14px 34px -14px oklch(0.72 0.16 285 / 0.45)',
}

function MetricCard({ label, value, format, icon, tone = 'neutral', subtitle, index }: Metric & { index: number }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: index * 0.04, duration: 0.24, ease: [0.2, 0, 0, 1] }}
      className={cn(
        'relative rounded-xl px-3.5 py-3 glass flex flex-col gap-1 min-w-0',
      )}
      style={{
        boxShadow: `inset 0 0 0 1px ${TONE_RING[tone]}, ${TONE_GLOW[tone]}`,
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-[10px] uppercase tracking-[0.14em] text-[color:var(--color-text-muted)] font-semibold truncate">
          {label}
        </span>
        <span
          className="opacity-70"
          style={{
            color: tone === 'bull'
              ? 'var(--color-bull-400)'
              : tone === 'bear'
                ? 'var(--color-bear-400)'
                : tone === 'iris'
                  ? 'oklch(0.78 0.14 285)'
                  : 'var(--color-text-muted)',
          }}
        >
          {icon}
        </span>
      </div>
      <NumberFlip
        value={value}
        format={format}
        className="text-lg font-semibold text-[color:var(--color-text-primary)]"
      />
      {subtitle && (
        <span className="text-[10px] text-[color:var(--color-text-muted)] font-mono truncate">
          {subtitle}
        </span>
      )}
    </motion.div>
  )
}

/**
 * AuroraMetricStrip — compact row of glass metric cards with NumberFlip animation.
 * Sits above the equity chart as the Dashboard hero.
 */
export default function AuroraMetricStrip({ nav, realized, unrealized, fees, winRate, closedTrades }: Props) {
  const netPnl = realized + unrealized - fees

  const metrics: Metric[] = [
    {
      label: 'Net asset value',
      value: nav,
      format: (v) => `$${fmt(v, 2)}`,
      icon: <Wallet size={14} />,
      tone: 'iris',
    },
    {
      label: 'Net P&L',
      value: netPnl,
      format: (v) => `${v >= 0 ? '+' : ''}$${fmt(v, 2)}`,
      icon: netPnl >= 0 ? <TrendingUp size={14} /> : <TrendingDown size={14} />,
      tone: netPnl >= 0 ? 'bull' : 'bear',
    },
    {
      label: 'Unrealized',
      value: unrealized,
      format: (v) => `${v >= 0 ? '+' : ''}$${fmt(v, 2)}`,
      icon: <Activity size={14} />,
      tone: unrealized >= 0 ? 'bull' : 'bear',
    },
    {
      label: 'Fees',
      value: fees,
      format: (v) => `$${fmt(v, 2)}`,
      icon: <Coins size={14} />,
      tone: 'neutral',
    },
    {
      label: 'Win rate',
      value: winRate,
      format: (v) => `${fmt(v, 1)}%`,
      icon: <Target size={14} />,
      tone: winRate >= 55 ? 'bull' : winRate >= 45 ? 'neutral' : 'bear',
      subtitle: `${closedTrades} closed`,
    },
  ]

  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
      {metrics.map((m, i) => (
        <MetricCard key={m.label} {...m} index={i} />
      ))}
    </div>
  )
}
