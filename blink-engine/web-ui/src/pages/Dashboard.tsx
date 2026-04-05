import { useFetch, useWebSocket, postPause } from '../hooks/useApi';
import { Card, Stat, Badge } from '../components/Card';
import { XAxis, YAxis, Tooltip, ResponsiveContainer, Area, AreaChart } from 'recharts';

export default function Dashboard() {
  const { data: status } = useFetch<any>('/api/status', 2000);
  const { data: portfolio } = useFetch<any>('/api/portfolio', 2000);
  const { data: activityData } = useFetch<any>('/api/activity', 3000);
  const { snapshot, connected } = useWebSocket();

  const s = snapshot || status;
  const p = portfolio;
  const equityCurve = p?.equity_curve?.map((v: number, i: number) => ({ i, nav: v })) || [];

  const handlePause = () => {
    if (s) postPause(!s.trading_paused);
  };

  return (
    <div className="space-y-4">
      {/* Status bar */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-bold text-white">Blink Engine</h1>
          <Badge
            text={s?.ws_connected ? 'WS LIVE' : 'WS DOWN'}
            variant={s?.ws_connected ? 'green' : 'red'}
          />
          <Badge
            text={connected ? 'UI CONNECTED' : 'UI DISCONNECTED'}
            variant={connected ? 'green' : 'gray'}
          />
          {s?.trading_paused && <Badge text="PAUSED" variant="yellow" />}
        </div>
        <div className="flex items-center gap-3">
          <span className="text-xs text-gray-500">
            msgs: {(s?.messages_total || 0).toLocaleString()}
          </span>
          <button
            onClick={handlePause}
            className={`px-3 py-1 rounded text-xs font-semibold border ${
              s?.trading_paused
                ? 'border-emerald-600 text-emerald-400 hover:bg-emerald-900/30'
                : 'border-yellow-600 text-yellow-400 hover:bg-yellow-900/30'
            }`}
          >
            {s?.trading_paused ? 'RESUME' : 'PAUSE'}
          </button>
        </div>
      </div>

      {/* Portfolio overview */}
      <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-6 gap-3">
        <Card>
          <Stat label="NAV" value={`$${(p?.nav_usdc || 0).toFixed(2)}`} color="text-white" />
        </Card>
        <Card>
          <Stat label="Cash" value={`$${(p?.cash_usdc || 0).toFixed(2)}`} />
        </Card>
        <Card>
          <Stat label="Invested" value={`$${(p?.invested_usdc || 0).toFixed(2)}`} />
        </Card>
        <Card>
          <Stat
            label="Unrealized P&L"
            value={`${(p?.unrealized_pnl_usdc || 0) >= 0 ? '+' : ''}$${(p?.unrealized_pnl_usdc || 0).toFixed(2)}`}
            color={(p?.unrealized_pnl_usdc || 0) >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat
            label="Realized P&L"
            value={`${(p?.realized_pnl_usdc || 0) >= 0 ? '+' : ''}$${(p?.realized_pnl_usdc || 0).toFixed(2)}`}
            color={(p?.realized_pnl_usdc || 0) >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat label="Fill Rate" value={`${(p?.fill_rate_pct || 0).toFixed(1)}%`} color="text-cyan-400" />
        </Card>
      </div>

      {/* Equity curve + Activity log */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card title="Equity Curve" className="lg:col-span-2">
          {equityCurve.length > 1 ? (
            <ResponsiveContainer width="100%" height={220}>
              <AreaChart data={equityCurve}>
                <defs>
                  <linearGradient id="navGrad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="#10b981" stopOpacity={0.3}/>
                    <stop offset="95%" stopColor="#10b981" stopOpacity={0}/>
                  </linearGradient>
                </defs>
                <XAxis dataKey="i" hide />
                <YAxis domain={['auto', 'auto']} tick={{ fontSize: 11, fill: '#6b7280' }} width={50} />
                <Tooltip
                  contentStyle={{ background: '#1f2937', border: '1px solid #374151', borderRadius: 8 }}
                  labelStyle={{ color: '#9ca3af' }}
                  formatter={(v) => [`$${Number(v).toFixed(2)}`, 'NAV']}
                />
                <Area type="monotone" dataKey="nav" stroke="#10b981" fill="url(#navGrad)" strokeWidth={2} dot={false} />
              </AreaChart>
            </ResponsiveContainer>
          ) : (
            <div className="h-[220px] flex items-center justify-center text-gray-600">No data yet</div>
          )}
        </Card>

        <Card title="Activity Log">
          <div className="h-[220px] overflow-y-auto space-y-1 text-xs">
            {(activityData?.entries || []).map((e: any, i: number) => (
              <div key={i} className="flex gap-2">
                <span className="text-gray-600 shrink-0">{e.timestamp}</span>
                <span className={
                  e.kind === 'Fill' ? 'text-emerald-400' :
                  e.kind === 'Signal' ? 'text-cyan-400' :
                  e.kind === 'Abort' ? 'text-red-400' :
                  e.kind === 'Skip' || e.kind === 'Warn' ? 'text-yellow-400' :
                  'text-gray-400'
                }>{e.message}</span>
              </div>
            ))}
            {(!activityData?.entries || activityData.entries.length === 0) && (
              <div className="text-gray-600">No activity yet</div>
            )}
          </div>
        </Card>
      </div>

      {/* Open positions */}
      <Card title={`Open Positions (${p?.open_positions?.length || 0})`}>
        {(p?.open_positions?.length || 0) > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-gray-500 border-b border-gray-800">
                  <th className="text-left py-2 px-2">ID</th>
                  <th className="text-left py-2 px-2">Market</th>
                  <th className="text-left py-2 px-2">Side</th>
                  <th className="text-right py-2 px-2">Entry</th>
                  <th className="text-right py-2 px-2">Current</th>
                  <th className="text-right py-2 px-2">Shares</th>
                  <th className="text-right py-2 px-2">USDC</th>
                  <th className="text-right py-2 px-2">P&L</th>
                  <th className="text-right py-2 px-2">P&L %</th>
                </tr>
              </thead>
              <tbody>
                {p.open_positions.map((pos: any) => (
                  <tr key={pos.id} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                    <td className="py-1.5 px-2 text-gray-400">#{pos.id}</td>
                    <td className="py-1.5 px-2 text-white">{pos.market_title || pos.token_id.slice(0, 12) + '...'}</td>
                    <td className="py-1.5 px-2">
                      <Badge text={pos.side} variant={pos.side === 'BUY' ? 'green' : 'red'} />
                    </td>
                    <td className="py-1.5 px-2 text-right">{pos.entry_price.toFixed(3)}</td>
                    <td className="py-1.5 px-2 text-right">{pos.current_price.toFixed(3)}</td>
                    <td className="py-1.5 px-2 text-right">{pos.shares.toFixed(1)}</td>
                    <td className="py-1.5 px-2 text-right">${pos.usdc_spent.toFixed(2)}</td>
                    <td className={`py-1.5 px-2 text-right ${pos.unrealized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                      {pos.unrealized_pnl >= 0 ? '+' : ''}${pos.unrealized_pnl.toFixed(2)}
                    </td>
                    <td className={`py-1.5 px-2 text-right ${pos.unrealized_pnl_pct >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                      {pos.unrealized_pnl_pct >= 0 ? '+' : ''}{pos.unrealized_pnl_pct.toFixed(1)}%
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="text-gray-600 text-sm py-4">No open positions</div>
        )}
      </Card>

      {/* Stats bar */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        <Card><Stat label="Total Signals" value={p?.total_signals || 0} /></Card>
        <Card><Stat label="Filled" value={p?.filled_orders || 0} color="text-emerald-400" /></Card>
        <Card><Stat label="Skipped" value={p?.skipped_orders || 0} color="text-yellow-400" /></Card>
        <Card><Stat label="Aborted" value={p?.aborted_orders || 0} color="text-red-400" /></Card>
        <Card><Stat label="Avg Slippage" value={`${(p?.avg_slippage_bps || 0).toFixed(1)} bps`} /></Card>
      </div>
    </div>
  );
}
