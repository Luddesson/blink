import { TrendingUp, TrendingDown } from 'lucide-react'
import {
  Area,
  AreaChart,
  ResponsiveContainer,
  Tooltip,
  YAxis,
} from 'recharts'
import { fmt, fmtPct, pnlClass } from '../lib/format'
import { useMode } from '../hooks/useMode'

interface Props {
  nav: number
  navDelta: number
  navDeltaPct: number
  equityCurve: number[]
  equityTimestamps: number[]
}

export default function NavCard({ nav, navDelta, navDeltaPct, equityCurve, equityTimestamps }: Props) {
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  const positive = navDelta >= 0

  const strokeColor = isLive
    ? positive ? '#f59e0b' : '#ef4444'
    : positive ? '#818cf8' : '#6366f1'

  const chartData = equityCurve.map((v, i) => ({
    t: equityTimestamps[i] ?? i,
    v,
  }))

  return (
    <div className={`card ${isLive ? 'border-amber-900/60' : ''}`}>
      <div className="flex items-start justify-between mb-1">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          NAV
        </span>
        <span className={`badge ${isLive ? 'badge-live' : 'badge-paper'}`}>
          {isLive ? 'LIVE' : 'PAPER'}
        </span>
      </div>

      <div className="flex items-baseline gap-3 my-2">
        <span className={`text-3xl font-bold font-mono ${isLive ? 'text-amber-300' : 'text-slate-100'}`}>
          ${fmt(nav)}
        </span>
        <span className={`text-sm font-mono flex items-center gap-1 ${pnlClass(navDelta)}`}>
          {positive ? <TrendingUp size={13} /> : <TrendingDown size={13} />}
          {fmtPct(navDeltaPct)}
        </span>
      </div>

      {/* Sparkline */}
      {chartData.length > 1 && (
        <div className="h-16 -mx-1 mt-2">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="navGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={strokeColor} stopOpacity={0.25} />
                  <stop offset="100%" stopColor={strokeColor} stopOpacity={0} />
                </linearGradient>
              </defs>
              <YAxis domain={['dataMin - 1', 'dataMax + 1']} hide />
              <Tooltip
                contentStyle={{ background: '#0f172a', border: '1px solid #334155', borderRadius: 6 }}
                labelFormatter={() => ''}
                formatter={(v: number) => [`$${fmt(v)}`, 'NAV']}
              />
              <Area
                type="monotone"
                dataKey="v"
                stroke={strokeColor}
                strokeWidth={1.5}
                fill="url(#navGrad)"
                dot={false}
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      )}
    </div>
  )
}
