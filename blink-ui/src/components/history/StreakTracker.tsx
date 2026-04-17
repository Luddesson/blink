import type { TradeStats } from '../../hooks/useTradeStats'

type Props = Pick<TradeStats, 'currentStreak' | 'maxWinStreak' | 'maxLossStreak'>

export default function StreakTracker({ currentStreak, maxWinStreak, maxLossStreak }: Props) {
  const isWin = currentStreak.type === 'win'

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Streaks
      </span>
      <div className="grid grid-cols-3 gap-4">
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Current</div>
          <div className={`text-lg font-mono font-semibold ${isWin ? 'text-emerald-400' : 'text-red-400'}`}>
            {currentStreak.count} {isWin ? 'W' : 'L'}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Best Win Streak</div>
          <div className="text-sm font-mono text-emerald-400">{maxWinStreak}</div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Worst Loss Streak</div>
          <div className="text-sm font-mono text-red-400">{maxLossStreak}</div>
        </div>
      </div>
    </div>
  )
}
