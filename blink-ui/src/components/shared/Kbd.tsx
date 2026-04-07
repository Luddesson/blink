interface Props {
  shortcut: string
  className?: string
}

export default function Kbd({ shortcut, className }: Props) {
  return (
    <kbd
      className={`inline-block bg-slate-700 border border-slate-600 rounded px-1 text-[9px] text-slate-400 font-mono leading-relaxed ${className ?? ''}`}
    >
      {shortcut}
    </kbd>
  )
}
