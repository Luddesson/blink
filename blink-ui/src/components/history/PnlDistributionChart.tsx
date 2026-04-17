import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, Cell } from 'recharts'

interface Props {
  distribution: { bucket: string; count: number }[]
}

export default function PnlDistributionChart({ distribution }: Props) {
  const hasData = distribution.some(d => d.count > 0)

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        P&L Distribution
      </span>
      {!hasData ? (
        <p className="text-xs text-slate-500">No trades</p>
      ) : (
        <ResponsiveContainer width="100%" height={180}>
          <BarChart data={distribution} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
            <XAxis
              dataKey="bucket"
              tick={{ fill: '#64748b', fontSize: 8 }}
              axisLine={false}
              tickLine={false}
              interval={0}
              angle={-35}
              textAnchor="end"
              height={45}
            />
            <YAxis
              tick={{ fill: '#64748b', fontSize: 9 }}
              axisLine={false}
              tickLine={false}
              allowDecimals={false}
              width={25}
            />
            <Tooltip
              contentStyle={{ background: '#1e293b', border: '1px solid #334155', borderRadius: 6, fontSize: 11 }}
            />
            <Bar dataKey="count" radius={[2, 2, 0, 0]}>
              {distribution.map((entry, i) => (
                <Cell
                  key={i}
                  fill={entry.bucket.startsWith('-') || entry.bucket.startsWith('< -') ? '#ef4444' : '#10b981'}
                  fillOpacity={0.7}
                />
              ))}
            </Bar>
          </BarChart>
        </ResponsiveContainer>
      )}
    </div>
  )
}
