import { useState } from 'react'
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, Cell } from 'recharts'
import { Chip } from '../ui/Chip'
import type { TradeStats } from '../../hooks/useTradeStats'

type Props = Pick<TradeStats,
  'pnlByHour' | 'tradesByHour' | 'winRateByHour' |
  'pnlByDayOfWeek' | 'tradesByDayOfWeek' | 'winRateByDayOfWeek'
>

type ViewMode = 'pnl' | 'count' | 'winrate'

const DOW_LABELS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

function HourChart({ data, mode }: { data: { hour: string; value: number }[]; mode: ViewMode }) {
  return (
    <ResponsiveContainer width="100%" height={150}>
      <BarChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        <XAxis dataKey="hour" tick={{ fill: '#64748b', fontSize: 8 }} axisLine={false} tickLine={false} />
        <YAxis tick={{ fill: '#64748b', fontSize: 9 }} axisLine={false} tickLine={false} width={35}
          tickFormatter={v => mode === 'pnl' ? `$${v.toFixed(2)}` : mode === 'winrate' ? `${v.toFixed(0)}%` : String(v)} />
        <Tooltip contentStyle={{ background: '#1e293b', border: '1px solid #334155', borderRadius: 6, fontSize: 11 }} />
        <Bar dataKey="value" radius={[2, 2, 0, 0]}>
          {data.map((d, i) => (
            <Cell key={i}
              fill={mode === 'pnl' ? (d.value >= 0 ? '#10b981' : '#ef4444')
                : mode === 'winrate' ? (d.value >= 50 ? '#10b981' : d.value >= 40 ? '#f59e0b' : '#ef4444')
                : '#6366f1'}
              fillOpacity={0.7}
            />
          ))}
        </Bar>
      </BarChart>
    </ResponsiveContainer>
  )
}

export default function TimeAnalysis(props: Props) {
  const [mode, setMode] = useState<ViewMode>('pnl')

  const hourData = props.pnlByHour.map((_, i) => ({
    hour: String(i).padStart(2, '0'),
    value: mode === 'pnl' ? props.pnlByHour[i]
      : mode === 'count' ? props.tradesByHour[i]
      : props.winRateByHour[i],
  }))

  const dowData = DOW_LABELS.map((label, i) => ({
    hour: label,
    value: mode === 'pnl' ? props.pnlByDayOfWeek[i]
      : mode === 'count' ? props.tradesByDayOfWeek[i]
      : props.winRateByDayOfWeek[i],
  }))

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Time Analysis
        </span>
        <div className="flex gap-1">
          <Chip label="P&L" active={mode === 'pnl'} onClick={() => setMode('pnl')} />
          <Chip label="Count" active={mode === 'count'} onClick={() => setMode('count')} />
          <Chip label="Win %" active={mode === 'winrate'} onClick={() => setMode('winrate')} />
        </div>
      </div>
      <div className="grid grid-cols-2 gap-4">
        <div>
          <p className="text-[10px] uppercase tracking-widest text-slate-500 mb-1">By Hour</p>
          <HourChart data={hourData} mode={mode} />
        </div>
        <div>
          <p className="text-[10px] uppercase tracking-widest text-slate-500 mb-1">By Day of Week</p>
          <HourChart data={dowData} mode={mode} />
        </div>
      </div>
    </div>
  )
}
