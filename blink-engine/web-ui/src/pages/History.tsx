import { useFetch } from '../hooks/useApi';
import { Card, Badge } from '../components/Card';

export default function History() {
  const { data } = useFetch<any>('/api/history', 5000);
  const trades = data?.trades || [];

  const totalPnl = trades.reduce((sum: number, t: any) => sum + t.realized_pnl, 0);
  const winners = trades.filter((t: any) => t.realized_pnl > 0).length;
  const losers = trades.filter((t: any) => t.realized_pnl < 0).length;
  const winRate = trades.length > 0 ? (winners / trades.length * 100) : 0;

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-bold text-white">Trade History</h2>

      {/* Summary stats */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        <Card>
          <div className="text-xs text-gray-500">Total Trades</div>
          <div className="text-lg font-bold text-white">{trades.length}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Winners</div>
          <div className="text-lg font-bold text-emerald-400">{winners}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Losers</div>
          <div className="text-lg font-bold text-red-400">{losers}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Win Rate</div>
          <div className="text-lg font-bold text-cyan-400">{winRate.toFixed(1)}%</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Total P&L</div>
          <div className={`text-lg font-bold ${totalPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            {totalPnl >= 0 ? '+' : ''}${totalPnl.toFixed(2)}
          </div>
        </Card>
      </div>

      {/* Trade table */}
      <Card>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-gray-500 border-b border-gray-800">
                <th className="text-left py-2 px-2">Token</th>
                <th className="text-left py-2 px-2">Side</th>
                <th className="text-right py-2 px-2">Entry</th>
                <th className="text-right py-2 px-2">Exit</th>
                <th className="text-right py-2 px-2">Shares</th>
                <th className="text-right py-2 px-2">P&L</th>
                <th className="text-right py-2 px-2">Slippage</th>
                <th className="text-right py-2 px-2">Duration</th>
                <th className="text-left py-2 px-2">Reason</th>
                <th className="text-left py-2 px-2">Closed</th>
              </tr>
            </thead>
            <tbody>
              {trades.map((t: any, i: number) => (
                <tr key={i} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                  <td className="py-1.5 px-2 font-mono text-gray-300">{t.token_id.slice(0, 12)}...</td>
                  <td className="py-1.5 px-2">
                    <Badge text={t.side} variant={t.side === 'BUY' ? 'green' : 'red'} />
                  </td>
                  <td className="py-1.5 px-2 text-right">{t.entry_price.toFixed(3)}</td>
                  <td className="py-1.5 px-2 text-right">{t.exit_price.toFixed(3)}</td>
                  <td className="py-1.5 px-2 text-right">{t.shares.toFixed(1)}</td>
                  <td className={`py-1.5 px-2 text-right font-semibold ${t.realized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                    {t.realized_pnl >= 0 ? '+' : ''}${t.realized_pnl.toFixed(2)}
                  </td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{t.slippage_bps.toFixed(1)} bps</td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{formatDuration(t.duration_secs)}</td>
                  <td className="py-1.5 px-2 text-gray-400">{t.reason}</td>
                  <td className="py-1.5 px-2 text-gray-500">{new Date(t.closed_at).toLocaleTimeString()}</td>
                </tr>
              ))}
              {trades.length === 0 && (
                <tr><td colSpan={10} className="py-8 text-gray-600 text-center">No closed trades yet</td></tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
