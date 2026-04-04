export function Card({ title, children, className = '' }: { title?: string; children: React.ReactNode; className?: string }) {
  return (
    <div className={`bg-[#111318] border border-gray-800 rounded-lg p-4 ${className}`}>
      {title && <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-3">{title}</h3>}
      {children}
    </div>
  );
}

export function Stat({ label, value, color = 'text-white' }: { label: string; value: string | number; color?: string }) {
  return (
    <div>
      <div className="text-xs text-gray-500 uppercase">{label}</div>
      <div className={`text-lg font-bold ${color}`}>{value}</div>
    </div>
  );
}

export function Badge({ text, variant = 'green' }: { text: string; variant?: 'green' | 'red' | 'yellow' | 'gray' }) {
  const colors = {
    green: 'bg-emerald-900/50 text-emerald-400 border-emerald-700',
    red: 'bg-red-900/50 text-red-400 border-red-700',
    yellow: 'bg-yellow-900/50 text-yellow-400 border-yellow-700',
    gray: 'bg-gray-800 text-gray-400 border-gray-700',
  };
  return (
    <span className={`px-2 py-0.5 rounded text-xs border ${colors[variant]}`}>{text}</span>
  );
}
