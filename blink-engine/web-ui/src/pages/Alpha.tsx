import { useAlpha } from '../hooks/useApi';

function Badge({ children, variant = 'neutral' }: { children: React.ReactNode; variant?: 'green' | 'red' | 'yellow' | 'neutral' }) {
  const colors = {
    green:   'bg-emerald-500/20 text-emerald-400 border-emerald-500/30',
    red:     'bg-red-500/20 text-red-400 border-red-500/30',
    yellow:  'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
    neutral: 'bg-gray-800 text-gray-400 border-gray-700',
  } satisfies Record<string, string>;
  return (
    <span className={`px-2 py-0.5 rounded text-[10px] font-bold border ${colors[variant]}`}>
      {children}
    </span>
  );
}

function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="bg-gray-900 border border-gray-800 rounded-lg p-4">
      <div className="text-[10px] text-gray-500 uppercase tracking-widest mb-1">{label}</div>
      <div className="text-2xl font-bold text-white tabular-nums">{value}</div>
      {sub && <div className="text-xs text-gray-600 mt-1">{sub}</div>}
    </div>
  );
}

export default function Alpha() {
  const { data, error } = useAlpha(5000);

  if (!data && !error) {
    return (
      <div className="flex items-center justify-center h-48 text-gray-600 text-sm">
        Loading alpha status…
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-center">
          <div className="text-red-500 text-sm mb-2">Cannot reach engine</div>
          <div className="text-gray-600 text-xs">Make sure the engine is running on port 7878</div>
        </div>
      </div>
    );
  }

  if (!data.enabled) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-center">
          <Badge variant="yellow">DISABLED</Badge>
          <div className="text-gray-400 text-sm mt-3">Alpha sidecar is not active</div>
          <div className="text-gray-600 text-xs mt-1">{data.reason ?? 'Set ALPHA_ENABLED=true in .env and restart the engine'}</div>
        </div>
      </div>
    );
  }

  const pnlTotal = data.realized_pnl_usdc + data.unrealized_pnl_usdc;
  const pnlColor = pnlTotal >= 0 ? 'text-emerald-400' : 'text-red-400';
  const topRejectReasons = Object.entries(data.reject_reasons)
    .sort(([, a], [, b]) => b - a)
    .slice(0, 6);

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-lg font-bold text-white">Alpha Sidecar</h1>
          <p className="text-xs text-gray-500 mt-0.5">AI-generated signals via CLOB + LLM analysis</p>
        </div>
        <Badge variant="green">● ACTIVE</Badge>
      </div>

      {/* Stats row */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatCard
          label="Signals Received"
          value={data.signals_received}
        />
        <StatCard
          label="Accepted"
          value={data.signals_accepted}
          sub={`${data.accept_rate_pct.toFixed(1)}% accept rate`}
        />
        <StatCard
          label="Rejected"
          value={data.signals_rejected}
        />
        <StatCard
          label="AI P&L"
          value={`${pnlTotal >= 0 ? '+' : ''}$${pnlTotal.toFixed(2)}`}
          sub={`$${data.realized_pnl_usdc.toFixed(2)} real · $${data.unrealized_pnl_usdc.toFixed(2)} open`}
        />
      </div>

      {/* P&L detail */}
      <div className="bg-gray-900 border border-gray-800 rounded-lg p-4">
        <div className="text-xs text-gray-500 uppercase tracking-widest mb-3">Position Summary</div>
        <div className="grid grid-cols-3 gap-4">
          <div>
            <div className="text-[10px] text-gray-600 mb-1">Positions Opened</div>
            <div className="text-lg font-bold text-white">{data.positions_opened}</div>
          </div>
          <div>
            <div className="text-[10px] text-gray-600 mb-1">Positions Closed</div>
            <div className="text-lg font-bold text-white">{data.positions_closed}</div>
          </div>
          <div>
            <div className="text-[10px] text-gray-600 mb-1">Total AI P&L</div>
            <div className={`text-lg font-bold ${pnlColor}`}>
              {pnlTotal >= 0 ? '+' : ''}${pnlTotal.toFixed(2)}
            </div>
          </div>
        </div>
      </div>

      {/* Reject reasons */}
      {topRejectReasons.length > 0 && (
        <div className="bg-gray-900 border border-gray-800 rounded-lg p-4">
          <div className="text-xs text-gray-500 uppercase tracking-widest mb-3">Top Rejection Reasons</div>
          <div className="space-y-2">
            {topRejectReasons.map(([reason, count]) => {
              const pct = data.signals_rejected > 0
                ? (count / data.signals_rejected) * 100
                : 0;
              return (
                <div key={reason} className="flex items-center gap-3">
                  <div className="text-xs text-gray-400 w-40 shrink-0 truncate">{reason}</div>
                  <div className="flex-1 bg-gray-800 rounded-full h-1.5 overflow-hidden">
                    <div
                      className="h-full bg-red-500/60 rounded-full"
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                  <div className="text-xs text-gray-500 tabular-nums w-10 text-right">{count}</div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Sidecar info */}
      <div className="bg-gray-900/50 border border-gray-800 rounded-lg p-4 text-xs text-gray-600">
        <div className="font-semibold text-gray-500 mb-1">How Alpha signals work</div>
        <ol className="list-decimal ml-4 space-y-1">
          <li>Scanner filters Polymarket markets by volume ($5k–$500k) and category</li>
          <li>CLOB client fetches live orderbook — spread, depth, and 1h price change</li>
          <li>Grok-3 LLM analyses each market with full CLOB context</li>
          <li>Confidence is calibrated by spread (tight spread → lower confidence)</li>
          <li>Kelly criterion sets position size (quarter-Kelly × $100 bankroll)</li>
          <li>Engine applies a second independent risk layer before execution</li>
        </ol>
        <div className="mt-2 text-gray-700">
          Monitor with: <code className="text-gray-500">curl localhost:7878/rpc -d '&#123;"jsonrpc":"2.0","id":"1","method":"alpha_status","params":&#123;&#125;&#125;'</code>
        </div>
      </div>
    </div>
  );
}
