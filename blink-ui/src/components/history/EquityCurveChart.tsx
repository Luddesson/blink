import { AreaChart, Area, XAxis, YAxis, Tooltip, ResponsiveContainer, ReferenceLine } from 'recharts'
import { fmtChartTime } from '../../lib/format'

interface Props {
  equityCurve: { timestamp: number; equity: number }[]
}

export default function EquityCurveChart({ equityCurve }: Props) {
  if (equityCurve.length < 2) {
    return (
      <div className="card">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
          Equity Curve
        </span>
        <p className="text-xs text-slate-500">Insufficient data</p>
      </div>
    )
  }

  let hwm = -Infinity
  const data = equityCurve.map(pt => {
    if (pt.equity > hwm) hwm = pt.equity
    return { ...pt, hwm }
  })

  const lastEquity = data[data.length - 1]?.equity ?? 0

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Equity Curve
        </span>
        <span className={`text-xs font-mono font-semibold ${lastEquity >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
          {lastEquity >= 0 ? '+' : ''}${lastEquity.toFixed(2)}
        </span>
      </div>
      <ResponsiveContainer width="100%" height={200}>
        <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
          <defs>
            <linearGradient id="eqGreen" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#10b981" stopOpacity={0.3} />
              <stop offset="100%" stopColor="#10b981" stopOpacity={0} />
            </linearGradient>
          </defs>
          <XAxis
            dataKey="timestamp"
            tickFormatter={fmtChartTime}
            tick={{ fill: '#64748b', fontSize: 9 }}
            axisLine={false}
            tickLine={false}
          />
          <YAxis
            tick={{ fill: '#64748b', fontSize: 9 }}
            axisLine={false}
            tickLine={false}
            tickFormatter={v => `$${v.toFixed(2)}`}
            width={50}
          />
          <Tooltip
            contentStyle={{ background: '#1e293b', border: '1px solid #334155', borderRadius: 6, fontSize: 11 }}
            labelFormatter={(v) => fmtChartTime(Number(v))}
            formatter={(v) => [`$${Number(v).toFixed(4)}`, 'Equity']}
          />
          <ReferenceLine y={0} stroke="#475569" strokeDasharray="3 3" />
          <Area
            type="monotone"
            dataKey="hwm"
            stroke="#64748b"
            strokeWidth={1}
            strokeDasharray="4 2"
            fill="none"
            dot={false}
          />
          <Area
            type="monotone"
            dataKey="equity"
            stroke="#10b981"
            strokeWidth={1.5}
            fill="url(#eqGreen)"
            dot={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  )
}
