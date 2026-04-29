-- 1. Skapa en vy för Hourly Equity (Downsampled)
-- Detta gör att 30-dagars grafen laddas omedelbart
CREATE OR REPLACE VIEW blink.v_equity_hourly AS
SELECT 
    date_trunc('hour', to_timestamp(timestamp_ms / 1000)) as bucket,
    avg(nav_usdc) as nav_usdc,
    min(nav_usdc) as min_nav,
    max(nav_usdc) as max_nav
FROM blink.equity_snapshots
GROUP BY 1
ORDER BY 1 ASC;

-- 2. Avancerad Trading Statistik (Real-time)
CREATE OR REPLACE VIEW blink.v_quant_metrics AS
WITH trade_stats AS (
    SELECT 
        COUNT(*) as total_trades,
        COUNT(*) FILTER (WHERE realized_pnl > 0) as wins,
        COUNT(*) FILTER (WHERE realized_pnl < 0) as losses,
        SUM(realized_pnl) as net_pnl,
        SUM(CASE WHEN realized_pnl > 0 THEN realized_pnl ELSE 0 END) as gross_wins,
        SUM(CASE WHEN realized_pnl < 0 THEN ABS(realized_pnl) ELSE 0 END) as gross_losses,
        AVG(realized_pnl) as avg_trade_pnl,
        STDDEV(realized_pnl) as stddev_pnl
    FROM blink.closed_trades_full
),
drawdown_calc AS (
    SELECT 
        nav_usdc,
        MAX(nav_usdc) OVER (ORDER BY timestamp_ms) as peak_nav
    FROM blink.equity_snapshots
    ORDER BY timestamp_ms DESC
    LIMIT 1
)
SELECT 
    *,
    CASE WHEN total_trades > 0 THEN (wins::float / total_trades) * 100 ELSE 0 END as win_rate_pct,
    CASE WHEN gross_losses > 0 THEN gross_wins / gross_losses ELSE 1.0 END as profit_factor,
    -- Sharpe Ratio (förenklad, antar 0 risk-free rate på kort sikt)
    CASE WHEN stddev_pnl > 0 THEN avg_trade_pnl / stddev_pnl ELSE 0 END as sharpe_ratio,
    -- Current Drawdown
    ((nav_usdc - peak_nav) / NULLIF(peak_nav, 0)) * 100 as current_drawdown_pct
FROM trade_stats, drawdown_calc;

-- 3. Token Performance Matrix
CREATE OR REPLACE VIEW blink.v_token_performance AS
SELECT 
    token_id,
    COUNT(*) as num_trades,
    SUM(realized_pnl) as total_pnl,
    AVG(duration_secs) as avg_hold_time,
    SUM(fees_paid_usdc) as total_fees
FROM blink.closed_trades_full
GROUP BY token_id
ORDER BY total_pnl DESC;
