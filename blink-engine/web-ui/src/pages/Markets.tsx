import { useState } from 'react';
import { useFetch } from '../hooks/useApi';
import { Card, Stat } from '../components/Card';

export default function Markets() {
  const { data: allBooks } = useFetch<any>('/api/orderbooks', 5000);
  const [selected, setSelected] = useState<string | null>(null);
  const { data: bookDetail } = useFetch<any>(
    selected ? `/api/orderbook/${selected}` : '/api/status',
    selected ? 5000 : 999999
  );

  const books = allBooks?.orderbooks || [];

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-bold text-white">Markets</h2>

      {/* Market list */}
      <Card title={`Subscribed Markets (${books.length})`}>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-gray-500 border-b border-gray-800">
                <th className="text-left py-2 px-2">Token ID</th>
                <th className="text-right py-2 px-2">Best Bid</th>
                <th className="text-right py-2 px-2">Best Ask</th>
                <th className="text-right py-2 px-2">Spread (bps)</th>
                <th className="text-right py-2 px-2">Bid Depth</th>
                <th className="text-right py-2 px-2">Ask Depth</th>
                <th className="text-center py-2 px-2">View</th>
              </tr>
            </thead>
            <tbody>
              {books.map((b: any) => (
                <tr key={b.token_id} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                  <td className="py-1.5 px-2 text-gray-300 font-mono">{b.token_id.slice(0, 16)}...</td>
                  <td className="py-1.5 px-2 text-right text-emerald-400">
                    {b.best_bid != null ? b.best_bid.toFixed(3) : '-'}
                  </td>
                  <td className="py-1.5 px-2 text-right text-red-400">
                    {b.best_ask != null ? b.best_ask.toFixed(3) : '-'}
                  </td>
                  <td className="py-1.5 px-2 text-right">{b.spread_bps ?? '-'}</td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{b.bid_depth}</td>
                  <td className="py-1.5 px-2 text-right text-gray-400">{b.ask_depth}</td>
                  <td className="py-1.5 px-2 text-center">
                    <button
                      onClick={() => setSelected(b.token_id)}
                      className="text-cyan-400 hover:text-cyan-300 text-xs"
                    >
                      Book
                    </button>
                  </td>
                </tr>
              ))}
              {books.length === 0 && (
                <tr><td colSpan={7} className="py-4 text-gray-600 text-center">No markets subscribed</td></tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>

      {/* Order book detail */}
      {selected && bookDetail && !bookDetail.error && (
        <Card title={`Order Book — ${selected.slice(0, 16)}...`}>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {/* Bids */}
            <div>
              <h4 className="text-xs text-emerald-400 font-semibold mb-2">BIDS</h4>
              <table className="w-full text-xs">
                <thead><tr className="text-gray-500"><th className="text-right py-1 px-2">Price</th><th className="text-right py-1 px-2">Size</th></tr></thead>
                <tbody>
                  {(bookDetail.bids || []).slice(0, 12).map((b: number[], i: number) => (
                    <tr key={i} className="border-b border-gray-800/40">
                      <td className="py-0.5 px-2 text-right text-emerald-400">{b[0]?.toFixed(3)}</td>
                      <td className="py-0.5 px-2 text-right text-gray-300">{b[1]?.toFixed(1)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {/* Asks */}
            <div>
              <h4 className="text-xs text-red-400 font-semibold mb-2">ASKS</h4>
              <table className="w-full text-xs">
                <thead><tr className="text-gray-500"><th className="text-right py-1 px-2">Price</th><th className="text-right py-1 px-2">Size</th></tr></thead>
                <tbody>
                  {(bookDetail.asks || []).slice(0, 12).map((a: number[], i: number) => (
                    <tr key={i} className="border-b border-gray-800/40">
                      <td className="py-0.5 px-2 text-right text-red-400">{a[0]?.toFixed(3)}</td>
                      <td className="py-0.5 px-2 text-right text-gray-300">{a[1]?.toFixed(1)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
          <div className="flex gap-6 mt-3 text-xs">
            <Stat label="Best Bid" value={bookDetail.best_bid?.toFixed(3) || '-'} color="text-emerald-400" />
            <Stat label="Best Ask" value={bookDetail.best_ask?.toFixed(3) || '-'} color="text-red-400" />
            <Stat label="Spread" value={`${bookDetail.spread_bps ?? '-'} bps`} />
          </div>
          <button onClick={() => setSelected(null)} className="mt-2 text-xs text-gray-500 hover:text-gray-300">Close</button>
        </Card>
      )}
    </div>
  );
}
