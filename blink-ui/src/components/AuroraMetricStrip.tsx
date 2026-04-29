import { motion } from 'motion/react'
import { TrendingUp, TrendingDown, Wallet, Activity, Target, Coins } from 'lucide-react'
import NumberFlip from './motion/NumberFlip'
import { cn } from '../lib/cn'
import { fmt } from '../lib/format'
import { useMode } from '../hooks/useMode'

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
  walletPositionValue?: number
  walletPositionBasis?: number
  walletPositions?: number
  walletTruthVerified?: boolean
  walletNavVerified?: boolean
  realityStatus?: string
}

const TONE_RING: Record<NonNullable<Metric['tone']>, string> = {
  neutral: 'var(--color-surface-600)/0.3',
  bull: 'var(--color-bull-500)/0.3',
  bear: 'var(--color-bear-500)/0.3',
  iris: 'var(--color-aurora-2)/0.3',
}

const TONE_GLOW: Record<NonNullable<Metric['tone']>, string> = {
  neutral: '0 8px 32px -12px var(--color-surface-950)/0.5',
  bull: '0 8px 32px -12px var(--color-bull-500)/0.2',
  bear: '0 8px 32px -12px var(--color-bear-500)/0.2',
  iris: '0 8px 32px -12px var(--color-aurora-2)/0.2',
}

function MetricCard({ label, value, format, icon, tone = 'neutral', subtitle, index }: Metric & { index: number }) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ delay: index * 0.05, duration: 0.3 }}
      className={cn(
        'relative rounded-xl px-4 py-3.5 glass flex flex-col gap-2 min-w-0 border',
      )}
      style={{
        borderColor: `rgba(255, 255, 255, 0.05)`,
        boxShadow: `inset 0 0 0 1px ${TONE_RING[tone]}, ${TONE_GLOW[tone]}`,
      }}
    >
      <div className="flex items-center justify-between">
        <span className="text-[10px] uppercase tracking-[0.18em] text-[color:var(--color-text-primary)] font-black opacity-90">
          {label}
        </span>
        <span
          className="opacity-100"
          style={{
            color: tone === 'bull'
              ? 'var(--color-bull-400)'
              : tone === 'bear'
                ? 'var(--color-bear-400)'
                : tone === 'iris'
                  ? 'var(--color-aurora-2)'
                  : 'var(--color-aurora-1)',
          }}
        >
          {icon}
        </span>
      </div>
      <NumberFlip
        value={value}
        format={format}
        className="text-xl font-black text-white tabular drop-shadow-sm"
      />
      {subtitle && (
        <span className="text-[9px] text-[color:var(--color-text-secondary)] font-bold uppercase tracking-widest opacity-80">
          {subtitle}
        </span>
      )}
    </motion.div>
  )
}

export default function AuroraMetricStrip({
  nav,
  realized,
  unrealized,
  fees,
  winRate,
  closedTrades,
  walletPositionValue = 0,
  walletPositionBasis = 0,
  walletPositions = 0,
  walletTruthVerified = false,
  walletNavVerified = walletTruthVerified,
  realityStatus,
}: Props) {
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  const netPnl = realized + unrealized - fees

  const metrics: Metric[] = isLive ? [
    {
      label: 'Wallet NAV',
      value: nav,
      format: (v) => walletNavVerified ? `$${fmt(v, 2)}` : 'unverified',
      icon: <Wallet size={14} />,
      tone: walletNavVerified ? 'iris' : 'neutral',
      subtitle: realityStatus ?? 'unverified',
    },
    {
      label: 'Wallet P&L',
      value: walletTruthVerified ? unrealized : 0,
      format: (v) => `${v >= 0 ? '+' : ''}$${fmt(v, 2)}`,
      icon: unrealized >= 0 ? <TrendingUp size={14} /> : <TrendingDown size={14} />,
      tone: unrealized >= 0 ? 'bull' : 'bear',
      subtitle: 'open positions',
    },
    {
      label: 'Position Value',
      value: walletTruthVerified ? walletPositionValue : 0,
      format: (v) => `$${fmt(v, 2)}`,
      icon: <Activity size={14} />,
      tone: 'iris',
    },
    {
      label: 'Position Basis',
      value: walletTruthVerified ? walletPositionBasis : 0,
      format: (v) => `$${fmt(v, 2)}`,
      icon: <Coins size={14} />,
      tone: 'neutral',
    },
    {
      label: 'Positions',
      value: walletTruthVerified ? walletPositions : 0,
      format: (v) => fmt(v, 0),
      icon: <Target size={14} />,
      tone: walletTruthVerified ? 'bull' : 'neutral',
    },
  ] : [
    {
      label: 'NAV',
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
      label: 'uPnL',
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
      label: 'Win Rate',
      value: winRate,
      format: (v) => `${fmt(v, 1)}%`,
      icon: <Target size={14} />,
      tone: winRate >= 55 ? 'bull' : winRate >= 45 ? 'neutral' : 'bear',
      subtitle: `${closedTrades} trades`,
    },
  ]

  return (
    <div className="flex flex-col gap-3">
      {metrics.map((m, i) => (
        <MetricCard key={m.label} {...m} index={i} />
      ))}
    </div>
  )
}
