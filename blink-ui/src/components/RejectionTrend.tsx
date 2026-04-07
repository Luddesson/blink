interface Props {
  rejectionByReason: Record<string, number> | null
  className?: string
}

export default function RejectionTrend({ rejectionByReason, className }: Props) {
  const entries = Object.entries(rejectionByReason ?? {})
    .sort(([, a], [, b]) => b - a)
    .slice(0, 8)

  const max = entries.length > 0 ? Math.max(1, ...entries.map(([, c]) => c)) : 1

  return (
    <div className={`card ${className ?? ''}`}>
      <div className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3">
        Rejection Reasons
      </div>

      {entries.length === 0 ? (
        <p className="text-xs text-slate-600 text-center py-4">
          No rejections — clean execution
        </p>
      ) : (
        <div className="space-y-2">
          {entries.map(([reason, count]) => (
            <div key={reason}>
              <div className="flex items-center justify-between mb-0.5">
                <span className="text-xs text-slate-300 truncate mr-2">{reason}</span>
                <span className="text-xs font-mono text-slate-400 shrink-0">{count}</span>
              </div>
              <div className="w-full bg-surface-900 rounded-full h-1.5 overflow-hidden">
                <div
                  className="h-1.5 rounded-full"
                  style={{
                    width: `${(count / max) * 100}%`,
                    backgroundColor: '#f92672',
                  }}
                />
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
