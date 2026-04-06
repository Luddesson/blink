import { useState } from 'react';
import { useHistory } from '../hooks/useApi';
import { Card, Badge } from '../components/Card';

const PER_PAGE = 50;

interface Trade {
  market_title?: string;
  token_id?: string;
  side: string;
  entry_price: number;
  exit_price: number;
  shares: number;
  realized_pnl: number;
  fees_paid_usdc?: number;
  slippage_bps: number;
  duration_secs: number;
  reason: string;
  closed_at: string;
}

export default function History() {
  const [page, setPage] = useState(1);
  const { data, loading } = useHistory(page, PER_PAGE);

  const trades: Trade[] = (data?.trades as Trade[]) || [];
  const totalTrades: number = (data?.total as number) || 0;
  const totalPages: number = (data?.total_pages as number) || 1;

  const pagePnl = trades.reduce((sum, t) => sum + t.realized_pnl, 0);
  const winners = trades.filter(t => t.realized_pnl > 0).length;
  const losers = trades.filter(t => t.realized_pnl < 0).length;
  const winRate = trades.length > 0 ? (winners / trades.length * 100) : 0;

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-bold text-white">Trade History</h2>

      {/* Summary stats (current page) */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        <Card>
          <div className="text-xs text-gray-500">Total Trades</div>
          <div className="text-lg font-bold text-white">{totalTrades}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Winners (page)</div>
          <div className="text-lg font-bold text-emerald-400">{winners}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Losers (page)</div>
          <div className="text-lg font-bold text-red-400">{losers}</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">Win Rate (page)</div>
          <div className="text-lg font-bold text-cyan-400">{winRate.toFixed(1)}%</div>
        </Card>
        <Card>
          <div className="text-xs text-gray-500">P&L (page)</div>
          <div className={`text-lg font-bold ${pagePnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            {pagePnl >= 0 ? '+' : ''}${pagePnl.toFixed(2)}
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
                <th className="text-right py-2 px-2">Fees</th>
                <th className="text-right py-2 px-2">Slippage</th>
                <th className="text-right py-2 px-2">Duration</th>
                <th className="text-left py-2 px-2">Reason</th>
                <th className="text-left py-2 px-2">Closed</th>
              </tr>
            </thead>
            <tbody>
              {trades.map((t, i) => (
                <tr key={i} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                  <td className="py-1.5 px-2 font-mono text-gray-300">{t.market_title || (t.token_id ? t.token_id.slice(0, 12) + '...' : '')}</td>
                  <td className="py-1.5 px-2">
                    <Badge text={t.side} variant={t.side === 'BUY' ? 'green' : 'red'} />
                  </td>
                  <td className="py-1.5 px-2 text-right">{t.entry_price.toFixed(3)}</td>
                  <td className="py-1.5 px-2 text-right">{t.exit_price.toFixed(3)}</td>
                  <td className="py-1.5 px-2 text-right">{t.shares.toFixed(1)}</td>
                  <td className={`py-1.5 px-2 text-right font-semibold ${t.realized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                    {t.realized_pnl >= 0 ? '+' : ''}${t.realized_pnl.toFixed(2)}
                  </td>
                  <td className="py-1.5 px-2 text-right text-yellow-400">${(t.fees_paid_usdc || 0).toFixed(2)}</td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{t.slippage_bps.toFixed(1)} bps</td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{formatDuration(t.duration_secs)}</td>
                  <td className="py-1.5 px-2 text-gray-400">{t.reason}</td>
                  <td className="py-1.5 px-2 text-gray-500">{new Date(t.closed_at).toLocaleTimeString()}</td>
                </tr>
              ))}
              {!loading && trades.length === 0 && (
                <tr><td colSpan={11} className="py-8 text-gray-600 text-center">No closed trades yet</td></tr>
              )}
              {loading && (
                <tr><td colSpan={11} className="py-8 text-gray-500 text-center">Loading…</td></tr>
              )}
            </tbody>
          </table>
        </div>

        {/* Pagination */}
        {totalPages > 1 && (
          <div className="flex items-center justify-between mt-3 pt-3 border-t border-gray-800">
            <button
              onClick={() => setPage(p => Math.max(1, p - 1))}
              disabled={page === 1}
              className="px-3 py-1 rounded text-xs border border-gray-700 text-gray-400 hover:bg-gray-800 disabled:opacity-30 disabled:cursor-not-allowed"
            >
              ← Prev
            </button>
            <span className="text-xs text-gray-500">
              Page {page} of {totalPages} ({totalTrades} trades)
            </span>
            <button
              onClick={() => setPage(p => Math.min(totalPages, p + 1))}
              disabled={page === totalPages}
              className="px-3 py-1 rounded text-xs border border-gray-700 text-gray-400 hover:bg-gray-800 disabled:opacity-30 disabled:cursor-not-allowed"
            >
              Next →
            </button>
          </div>
        )}
      </Card>
    </div>
  );
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
