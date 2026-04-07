interface Props {
  className?: string
}

export default function ExperimentPanel({ className }: Props) {
  return (
    <div className={`card ${className ?? ''}`}>
      <div className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3">
        A/B Experiments
      </div>
      <p className="text-xs text-slate-600 text-center py-4">
        No active experiments
      </p>
    </div>
  )
}
