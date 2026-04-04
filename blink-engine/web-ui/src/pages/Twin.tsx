import { useFetch } from '../hooks/useApi';
import { Card, Stat, Badge } from '../components/Card';

export default function Twin() {
  const { data } = useFetch<any>('/api/twin', 3000);

  if (!data || data.error) {
    return (
      <div className="space-y-4">
        <h2 className="text-lg font-bold text-white">Blink Twin</h2>
        <Card>
          <div className="text-gray-600 py-4">
            {data?.error || 'Loading...'}
            {data?.error === 'Twin not available' && (
              <p className="mt-2 text-xs">Enable with <code className="bg-gray-800 px-1.5 py-0.5 rounded">BLINK_TWIN=true</code></p>
            )}
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-bold text-white">Blink Twin — Adversarial Shadow</h2>
      <p className="text-xs text-gray-500">Self-improving digital twin that simulates worst-case execution to find profitability boundaries.</p>

      <div className="flex gap-3">
        <Badge text={`Gen ${data.generation}`} variant="gray" />
      </div>

      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3">
        <Card>
          <Stat label="Twin NAV" value={`$${data.nav.toFixed(2)}`} color="text-white" />
        </Card>
        <Card>
          <Stat
            label="NAV Return"
            value={`${data.nav_return_pct >= 0 ? '+' : ''}${data.nav_return_pct.toFixed(1)}%`}
            color={data.nav_return_pct >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat
            label="Realized P&L"
            value={`${data.realized_pnl >= 0 ? '+' : ''}$${data.realized_pnl.toFixed(2)}`}
            color={data.realized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat
            label="Unrealized P&L"
            value={`${data.unrealized_pnl >= 0 ? '+' : ''}$${data.unrealized_pnl.toFixed(2)}`}
            color={data.unrealized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat label="Filled Orders" value={data.filled_orders} color="text-cyan-400" />
        </Card>
        <Card>
          <Stat label="Aborted Orders" value={data.aborted_orders} color="text-red-400" />
        </Card>
        <Card>
          <Stat label="Win Rate" value={`${data.win_rate_pct.toFixed(1)}%`} color="text-cyan-400" />
        </Card>
        <Card>
          <Stat label="Max Drawdown" value={`${data.max_drawdown_pct.toFixed(1)}%`} color="text-yellow-400" />
        </Card>
      </div>

      {/* Twin adversarial parameters */}
      <Card title="Adversarial Parameters">
        <div className="grid grid-cols-3 gap-4">
          <div>
            <div className="text-xs text-gray-500">Extra Latency</div>
            <div className="text-lg font-bold text-yellow-400">{data.extra_latency_ms}ms</div>
            <div className="text-[10px] text-gray-600">Added to every signal</div>
          </div>
          <div>
            <div className="text-xs text-gray-500">Slippage Penalty</div>
            <div className="text-lg font-bold text-yellow-400">{data.slippage_penalty_bps.toFixed(1)} bps</div>
            <div className="text-[10px] text-gray-600">Added to observed slippage</div>
          </div>
          <div>
            <div className="text-xs text-gray-500">Drift Multiplier</div>
            <div className="text-lg font-bold text-yellow-400">{data.drift_multiplier.toFixed(2)}x</div>
            <div className="text-[10px] text-gray-600">&lt;1.0 = aborts earlier</div>
          </div>
        </div>
      </Card>
    </div>
  );
}
