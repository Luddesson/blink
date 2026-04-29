"use client";

import { useEffect, useState } from "react";
import { createClient } from "@supabase/supabase-js";
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, BarChart, Bar, Cell } from "recharts";
import { AlertCircle, Pause, Play, Skull, Activity, TrendingUp, BarChart3, ShieldCheck, History } from "lucide-react";

// Types for better safety
interface EquitySnapshot {
  timestamp_ms: number;
  nav_usdc: number;
}

interface InventoryRecord {
  token_id: string;
  current_position: number;
  avg_entry_price: number;
  volume_traded_usdc: number;
}

interface AIInsight {
  severity: string;
  message: string;
  timestamp_ms: number;
}

interface OrderBookSnapshot {
  timestamp_ms: number;
  best_bid: number;
  best_ask: number;
  bid_depth: number;
  ask_depth: number;
  spread: number;
}

interface ClosedTrade {
  timestamp_ms: number;
  token_id: string;
  realized_pnl: number;
  side: string;
  market_title: string;
}

// Initialize Supabase Client
const supabase = createClient(
  process.env.NEXT_PUBLIC_SUPABASE_URL!,
  process.env.NEXT_PUBLIC_SUPABASE_ANON_KEY!
);

export default function Dashboard() {
  const [equity, setEquity] = useState<EquitySnapshot[]>([]);
  const [inventory, setInventory] = useState<InventoryRecord[]>([]);
  const [insights, setInsights] = useState<AIInsight[]>([]);
  const [orderBook, setOrderBook] = useState<OrderBookSnapshot[]>([]);
  const [trades, setTrades] = useState<ClosedTrade[]>([]);
  const [timeframe, setTimeframe] = useState<"24h" | "7d" | "30d">("24h");

  useEffect(() => {
    // 1. Fetch Initial Data
    const fetchData = async () => {
      // Determine limit and interval based on timeframe
      const limit = timeframe === "24h" ? 100 : timeframe === "7d" ? 500 : 1000;
      
      const { data: eqData } = await supabase
        .schema("blink")
        .from("equity_snapshots")
        .select("timestamp_ms, nav_usdc")
        .order("timestamp_ms", { ascending: false })
        .limit(limit);

      const { data: invData } = await supabase.from("v_live_inventory").select("*");
      const { data: aiData } = await supabase.from("ai_insights").select("*").order("timestamp_ms", { ascending: false }).limit(5);
      const { data: tradeData } = await supabase.schema("blink").from("closed_trades_full").select("*").order("timestamp_ms", { ascending: false }).limit(20);
      
      // Fetch latest order book snapshots from 'blink' schema
      const { data: obData } = await supabase
        .schema("blink")
        .from("order_book_snapshots")
        .select("*")
        .order("timestamp_ms", { ascending: false })
        .limit(30);

      if (eqData) setEquity(eqData.reverse());
      if (invData) setInventory(invData);
      if (aiData) setInsights(aiData);
      if (obData) setOrderBook(obData.reverse());
      if (tradeData) setTrades(tradeData);
    };

    fetchData();

    // 2. Realtime Subscriptions
    const equitySub = supabase
      .channel("equity_realtime")
      .on("postgres_changes", { event: "INSERT", schema: "blink", table: "equity_snapshots" }, (payload) => {
        setEquity((prev) => {
          const limit = timeframe === "24h" ? 100 : timeframe === "7d" ? 500 : 1000;
          return [...prev.slice(-(limit - 1)), payload.new as EquitySnapshot];
        });
      })
      .subscribe();

    const obSub = supabase
      .channel("ob_realtime")
      .on("postgres_changes", { event: "INSERT", schema: "blink", table: "order_book_snapshots" }, (payload) => {
        setOrderBook((prev) => [...prev.slice(-29), payload.new as OrderBookSnapshot]);
      })
      .subscribe();

    const tradeSub = supabase
      .channel("trade_realtime")
      .on("postgres_changes", { event: "INSERT", schema: "blink", table: "closed_trades_full" }, (payload) => {
        setTrades((prev) => [payload.new as ClosedTrade, ...prev.slice(0, 19)]);
      })
      .subscribe();

    return () => {
      supabase.removeChannel(equitySub);
      supabase.removeChannel(obSub);
      supabase.removeChannel(tradeSub);
    };
  }, [timeframe]);

  // 3. Command Sender
  const sendCommand = async (cmd: string, payload: Record<string, unknown> = {}) => {
    await supabase.from("engine_commands").insert([
      {
        timestamp_ms: Date.now(),
        command_type: cmd,
        payload,
        issued_by: "ADMIN_DASHBOARD",
      },
    ]);
    alert(`Command ${cmd} sent to the engine.`);
  };

  const latestOB = orderBook[orderBook.length - 1] || { best_bid: 0, best_ask: 0, bid_depth: 0, ask_depth: 0, spread: 0 };
  const formattedSpread = (Number(latestOB.spread) / 1e6).toFixed(2); // Assuming spread is in micro-units or similar

  // Calculate Stats
  const winRate = trades.length > 0 
    ? (trades.filter(t => t.realized_pnl > 0).length / trades.length * 100).toFixed(1)
    : "0.0";
  
  const totalNetPnl = trades.reduce((acc, t) => acc + (Number(t.realized_pnl) || 0), 0);
  const profitFactor = (() => {
    const gains = trades.filter(t => t.realized_pnl > 0).reduce((acc, t) => acc + t.realized_pnl, 0);
    const losses = Math.abs(trades.filter(t => t.realized_pnl < 0).reduce((acc, t) => acc + t.realized_pnl, 0));
    return losses === 0 ? (gains > 0 ? "∞" : "1.0") : (gains / losses).toFixed(2);
  })();

  const currentNav = equity.length > 0 ? equity[equity.length - 1].nav_usdc : 0;
  const startNav = equity.length > 0 ? equity[0].nav_usdc : currentNav;
  const pnlPct = startNav !== 0 ? ((currentNav - startNav) / startNav * 100).toFixed(2) : "0.00";

  return (
    <div className="min-h-screen bg-neutral-950 text-neutral-100 p-8 font-mono">
      <header className="flex justify-between items-center mb-8 border-b border-neutral-800 pb-4">
        <div>
          <h1 className="text-2xl font-bold tracking-tight text-emerald-400">BLINK GOD-MODE</h1>
          <p className="text-sm text-neutral-400">HFT Quant Control Center</p>
        </div>
        <div className="flex gap-4">
          <button onClick={() => sendCommand("PAUSE")} className="flex items-center gap-2 bg-yellow-500/10 text-yellow-500 border border-yellow-500/20 px-4 py-2 rounded hover:bg-yellow-500/20 transition-colors">
            <Pause size={16} /> PAUSE
          </button>
          <button onClick={() => sendCommand("RESUME")} className="flex items-center gap-2 bg-emerald-500/10 text-emerald-500 border border-emerald-500/20 px-4 py-2 rounded hover:bg-emerald-500/20 transition-colors">
            <Play size={16} /> RESUME
          </button>
          <button onClick={() => sendCommand("LIQUIDATE_ALL")} className="flex items-center gap-2 bg-red-500/10 text-red-500 border border-red-500/20 px-4 py-2 rounded hover:bg-red-500/20 transition-colors">
            <Skull size={16} /> LIQUIDATE
          </button>
        </div>
      </header>

      {/* STATS OVERVIEW */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
        <div className="bg-neutral-900 border border-neutral-800 p-4 rounded-lg">
          <div className="flex justify-between items-start">
            <div>
              <p className="text-xs text-neutral-500 uppercase tracking-wider mb-1">Win Rate</p>
              <h3 className="text-2xl font-bold text-emerald-400">{winRate}%</h3>
            </div>
            <ShieldCheck className="text-emerald-500/50" size={20} />
          </div>
          <div className="mt-2 text-[10px] text-neutral-600 uppercase">Across last {trades.length} trades</div>
        </div>

        <div className="bg-neutral-900 border border-neutral-800 p-4 rounded-lg">
          <div className="flex justify-between items-start">
            <div>
              <p className="text-xs text-neutral-500 uppercase tracking-wider mb-1">Profit Factor</p>
              <h3 className="text-2xl font-bold text-blue-400">{profitFactor}</h3>
            </div>
            <BarChart3 className="text-blue-500/50" size={20} />
          </div>
          <div className="mt-2 text-[10px] text-neutral-600 uppercase">Gross Gains / Gross Losses</div>
        </div>

        <div className="bg-neutral-900 border border-neutral-800 p-4 rounded-lg">
          <div className="flex justify-between items-start">
            <div>
              <p className="text-xs text-neutral-500 uppercase tracking-wider mb-1">Total Realized</p>
              <h3 className={`text-2xl font-bold ${totalNetPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                ${totalNetPnl.toFixed(2)}
              </h3>
            </div>
            <History className="text-neutral-500/50" size={20} />
          </div>
          <div className="mt-2 text-[10px] text-neutral-600 uppercase">Net Profit from closed trades</div>
        </div>

        <div className="bg-neutral-900 border border-neutral-800 p-4 rounded-lg">
          <div className="flex justify-between items-start">
            <div>
              <p className="text-xs text-neutral-500 uppercase tracking-wider mb-1">Period Return</p>
              <h3 className={`text-2xl font-bold ${Number(pnlPct) >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                {Number(pnlPct) > 0 ? '+' : ''}{pnlPct}%
              </h3>
            </div>
            <TrendingUp className="text-emerald-500/50" size={20} />
          </div>
          <div className="mt-2 text-[10px] text-neutral-600 uppercase">NAV change in {timeframe} window</div>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-8">
        {/* CHART: Equity Curve */}
        <div className="lg:col-span-2 bg-neutral-900 border border-neutral-800 p-6 rounded-lg">
          <div className="flex justify-between items-center mb-4">
            <h2 className="text-lg font-semibold text-neutral-300 flex items-center gap-2">
              <TrendingUp size={18} className="text-emerald-400" /> Equity Curve (Live NAV)
            </h2>
            <div className="flex gap-2 bg-neutral-950 p-1 rounded border border-neutral-800">
              {(["24h", "7d", "30d"] as const).map((tf) => (
                <button
                  key={tf}
                  onClick={() => setTimeframe(tf)}
                  className={`px-3 py-1 text-xs rounded transition-colors ${
                    timeframe === tf
                      ? "bg-emerald-500/20 text-emerald-400 border border-emerald-500/30"
                      : "text-neutral-500 hover:text-neutral-300"
                  }`}
                >
                  {tf.toUpperCase()}
                </button>
              ))}
            </div>
          </div>
          <div className="h-[300px]">
            <ResponsiveContainer width="100%" height="100%">
              <LineChart data={equity}>
                <CartesianGrid strokeDasharray="3 3" stroke="#262626" />
                <XAxis 
                  dataKey="timestamp_ms" 
                  tickFormatter={(t) => {
                    const date = new Date(t);
                    return timeframe === "24h" 
                      ? date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
                      : `${date.getDate()}/${date.getMonth() + 1} ${date.getHours()}:00`;
                  }} 
                  stroke="#525252" 
                  fontSize={10}
                />
                <YAxis 
                  domain={["auto", "auto"]} 
                  stroke="#525252" 
                  fontSize={10}
                  tickFormatter={(val) => `$${(val / 1000).toFixed(1)}k`}
                />
                <Tooltip 
                  contentStyle={{ backgroundColor: "#171717", borderColor: "#262626", fontSize: "12px" }}
                  labelFormatter={(t) => new Date(t).toLocaleString()}
                  formatter={(val: number) => [`$${val.toLocaleString()}`, "NAV"]}
                />
                <Line type="monotone" dataKey="nav_usdc" stroke="#34d399" strokeWidth={2} dot={false} isAnimationActive={false} />
              </LineChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* ORDER BOOK DEPTH */}
        <div className="bg-neutral-900 border border-neutral-800 p-6 rounded-lg">
          <h2 className="text-lg font-semibold mb-4 text-neutral-300 flex items-center gap-2">
            <Activity size={18} className="text-blue-400" /> Order Book Depth
          </h2>
          
          <div className="grid grid-cols-2 gap-4 mb-6">
            <div className="p-3 bg-neutral-950 border border-neutral-800 rounded">
              <span className="text-xs text-neutral-500 uppercase">Best Bid</span>
              <p className="text-xl font-bold text-emerald-400">{(Number(latestOB.best_bid)/1e9).toFixed(4)}</p>
            </div>
            <div className="p-3 bg-neutral-950 border border-neutral-800 rounded">
              <span className="text-xs text-neutral-500 uppercase">Best Ask</span>
              <p className="text-xl font-bold text-red-400">{(Number(latestOB.best_ask)/1e9).toFixed(4)}</p>
            </div>
            <div className="col-span-2 p-2 bg-neutral-800/30 rounded text-center">
              <span className="text-xs text-neutral-500 mr-2 uppercase">Spread:</span>
              <span className="text-sm font-mono text-neutral-300">{formattedSpread} bps</span>
            </div>
          </div>

          <div className="h-[180px]">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={[
                { name: 'Bids', depth: Number(latestOB.bid_depth) },
                { name: 'Asks', depth: Number(latestOB.ask_depth) }
              ]}>
                <XAxis dataKey="name" stroke="#525252" />
                <YAxis hide />
                <Tooltip 
                  cursor={{fill: 'transparent'}}
                  contentStyle={{ backgroundColor: "#171717", borderColor: "#262626" }}
                />
                <Bar dataKey="depth">
                  <Cell fill="#059669" fillOpacity={0.6} stroke="#10b981" />
                  <Cell fill="#dc2626" fillOpacity={0.6} stroke="#ef4444" />
                </Bar>
              </BarChart>
            </ResponsiveContainer>
          </div>
          <p className="text-[10px] text-center text-neutral-600 mt-2 uppercase tracking-widest">Live Liquidity Distribution</p>
        </div>

        {/* INVENTORY */}
        <div className="lg:col-span-2 bg-neutral-900 border border-neutral-800 p-6 rounded-lg">
          <h2 className="text-lg font-semibold mb-4 text-neutral-300">Live Inventory Exposure</h2>
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="border-b border-neutral-800 text-neutral-500">
                  <th className="pb-3 font-medium">Token ID</th>
                  <th className="pb-3 font-medium">Net Position</th>
                  <th className="pb-3 font-medium">Avg Entry (USD)</th>
                  <th className="pb-3 font-medium">Volume (USD)</th>
                </tr>
              </thead>
              <tbody>
                {inventory.map((inv, i) => (
                  <tr key={i} className="border-b border-neutral-800/50">
                    <td className="py-3 text-neutral-400 truncate max-w-[200px]">{inv.token_id}</td>
                    <td className={`py-3 font-medium ${inv.current_position > 0 ? "text-emerald-400" : "text-red-400"}`}>{inv.current_position.toFixed(2)}</td>
                    <td className="py-3 text-neutral-300">${inv.avg_entry_price.toFixed(4)}</td>
                    <td className="py-3 text-neutral-300">${inv.volume_traded_usdc.toFixed(2)}</td>
                  </tr>
                ))}
                {inventory.length === 0 && (
                  <tr>
                    <td colSpan={4} className="py-8 text-center text-neutral-500">No active positions</td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </div>

        {/* AI INSIGHTS */}
        <div className="bg-neutral-900 border border-neutral-800 p-6 rounded-lg flex flex-col gap-4">
          <div className="flex items-center gap-2 text-blue-400 mb-2">
            <AlertCircle size={20} />
            <h2 className="text-lg font-semibold">AI Insights (Gemini)</h2>
          </div>
          {insights.length === 0 ? (
            <p className="text-neutral-500 text-sm">No insights yet. Waiting for AI agent...</p>
          ) : (
            insights.map((insight, idx) => (
              <div key={idx} className="p-4 bg-neutral-950 border border-neutral-800 rounded">
                <span className={`text-xs font-bold px-2 py-1 rounded ${insight.severity === 'CRITICAL' ? 'bg-red-500/20 text-red-500' : 'bg-blue-500/20 text-blue-500'}`}>
                  {insight.severity}
                </span>
                <p className="mt-2 text-sm text-neutral-300">{insight.message}</p>
                <span className="text-xs text-neutral-500 mt-2 block">{new Date(insight.timestamp_ms).toLocaleTimeString()}</span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
