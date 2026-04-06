import { useFetch } from '../hooks/useApi';
import { Card, Stat, Badge } from '../components/Card';

export default function Risk() {
  const { data } = useFetch<any>('/api/risk', 2000);

  if (!data || data.error) {
    return (
      <div className="space-y-4">
        <h2 className="text-lg font-bold text-white">Risk Management</h2>
        <Card><div className="text-gray-600 py-4">{data?.error || 'Loading...'}</div></Card>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-bold text-white">Risk Management</h2>

      {/* Status badges */}
      <div className="flex gap-3">
        <Badge
          text={data.trading_enabled ? 'TRADING ENABLED' : 'KILL SWITCH OFF'}
          variant={data.trading_enabled ? 'green' : 'red'}
        />
        <Badge
          text={data.circuit_breaker_tripped ? 'CIRCUIT BREAKER TRIPPED' : 'CIRCUIT BREAKER OK'}
          variant={data.circuit_breaker_tripped ? 'red' : 'green'}
        />
      </div>

      {/* Risk parameters */}
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3">
        <Card>
          <Stat
            label="Daily P&L"
            value={`${data.daily_pnl >= 0 ? '+' : ''}$${data.daily_pnl.toFixed(2)}`}
            color={data.daily_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat label="Max Daily Loss" value={`${(data.max_daily_loss_pct * 100).toFixed(0)}%`} />
        </Card>
        <Card>
          <Stat label="Max Concurrent Positions" value={data.max_concurrent_positions} />
        </Card>
        <Card>
          <Stat label="Max Order Size" value={`$${data.max_single_order_usdc.toFixed(0)}`} />
        </Card>
        <Card>
          <Stat label="Max Orders/sec" value={data.max_orders_per_second} />
        </Card>
        <Card>
          <Stat label="VaR Threshold" value={`${(data.var_threshold_pct * 100).toFixed(1)}%`} />
        </Card>
        <Card>
          <Stat label="Stop Loss" value={data.stop_loss_enabled ? `-${(data.stop_loss_pct).toFixed(0)}%` : 'Disabled'} color={data.stop_loss_enabled ? 'text-red-400' : 'text-gray-300'} />
        </Card>
      </div>

      {/* Info */}
      <Card title="Risk Rules">
        <div className="text-xs text-gray-400 space-y-2">
          <p><span className="text-gray-300 font-semibold">Kill Switch:</span> When OFF, all order submission is blocked regardless of other checks.</p>
          <p><span className="text-gray-300 font-semibold">Circuit Breaker:</span> Trips on catastrophic loss or VaR breach. Auto-resets when rolling exposure decays.</p>
          <p><span className="text-gray-300 font-semibold">Daily Loss Limit:</span> Blocks trading when cumulative daily losses exceed {(data.max_daily_loss_pct * 100).toFixed(0)}% of starting NAV.</p>
          <p><span className="text-gray-300 font-semibold">Rate Limit:</span> Max {data.max_orders_per_second} orders/sec to stay within CLOB API limits.</p>
          <p><span className="text-gray-300 font-semibold">VaR:</span> Rolling 60s exposure must not exceed {(data.var_threshold_pct * 100).toFixed(1)}% of portfolio NAV.</p>
        </div>
      </Card>
    </div>
  );
}
