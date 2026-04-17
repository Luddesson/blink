import { useMemo } from 'react'
import type { ClosedTrade } from '../types'

// ─── Filter types ────────────────────────────────────────────────────────────
export interface TradeFilters {
  dateRange?: [Date, Date]
  signalSource?: 'all' | 'rn1' | 'alpha'
  side?: 'all' | 'buy' | 'sell'
}

// ─── Output types ────────────────────────────────────────────────────────────
export interface TradeStats {
  filtered: ClosedTrade[]
  totalTrades: number
  wins: number
  losses: number
  winRate: number
  totalPnl: number
  netPnl: number
  avgPnl: number
  medianPnl: number
  totalFees: number
  avgDuration: number
  medianDuration: number
  profitFactor: number
  expectancy: number
  avgRiskReward: number
  payoffRatio: number
  avgSlippage: number

  biggestWin: ClosedTrade | null
  biggestLoss: ClosedTrade | null
  top5Wins: ClosedTrade[]
  top5Losses: ClosedTrade[]

  currentStreak: { type: 'win' | 'loss'; count: number }
  maxWinStreak: number
  maxLossStreak: number

  dailyPnl: Map<string, number>
  dailyTrades: Map<string, number>

  pnlByHour: number[]
  tradesByHour: number[]
  winRateByHour: number[]
  pnlByDayOfWeek: number[]
  tradesByDayOfWeek: number[]
  winRateByDayOfWeek: number[]

  pnlDistribution: { bucket: string; count: number }[]
  durationDistribution: { bucket: string; count: number }[]

  bySignalSource: Record<string, SourceStats>
  byExitReason: Record<string, ReasonStats>
  byMarket: MarketStats[]

  maxDrawdown: number
  maxDrawdownPct: number
  sharpeRatio: number
  sortinoRatio: number
  calmarRatio: number

  equityCurve: { timestamp: number; equity: number }[]
}

export interface SourceStats {
  count: number
  wins: number
  losses: number
  winRate: number
  totalPnl: number
  avgPnl: number
  avgDuration: number
}

export interface ReasonStats {
  count: number
  totalPnl: number
  avgPnl: number
  winRate: number
}

export interface MarketStats {
  title: string
  tokenId: string
  count: number
  totalPnl: number
  winRate: number
  avgDuration: number
}

// ─── Helpers ─────────────────────────────────────────────────────────────────
function toStockholm(iso: string): Date {
  const raw = iso.replace(/(\.\d{3})\d+/, '$1')
  return new Date(raw)
}

function dateKey(d: Date): string {
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

function median(arr: number[]): number {
  if (arr.length === 0) return 0
  const s = [...arr].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  return s.length % 2 !== 0 ? s[mid] : (s[mid - 1] + s[mid]) / 2
}

function buildHistogram(values: number[], bucketEdges: number[], labels: string[]): { bucket: string; count: number }[] {
  const counts = new Array(labels.length).fill(0)
  for (const v of values) {
    let placed = false
    for (let i = 0; i < bucketEdges.length; i++) {
      if (v < bucketEdges[i]) {
        counts[i]++
        placed = true
        break
      }
    }
    if (!placed) counts[counts.length - 1]++
  }
  return labels.map((bucket, i) => ({ bucket, count: counts[i] }))
}

// ─── Main hook ───────────────────────────────────────────────────────────────
export function useTradeStats(trades: ClosedTrade[], filters: TradeFilters = {}): TradeStats {
  return useMemo(() => computeStats(trades, filters), [trades, filters])
}

function computeStats(allTrades: ClosedTrade[], filters: TradeFilters): TradeStats {
  // Apply filters
  let filtered = allTrades
  if (filters.dateRange) {
    const [start, end] = filters.dateRange
    filtered = filtered.filter(t => {
      const d = toStockholm(t.closed_at)
      return d >= start && d <= end
    })
  }
  if (filters.signalSource && filters.signalSource !== 'all') {
    filtered = filtered.filter(t => (t.signal_source ?? 'rn1') === filters.signalSource)
  }
  if (filters.side && filters.side !== 'all') {
    filtered = filtered.filter(t => t.side.toLowerCase() === filters.side)
  }

  const n = filtered.length
  const wins = filtered.filter(t => t.realized_pnl > 0)
  const losses = filtered.filter(t => t.realized_pnl <= 0)
  const winCount = wins.length
  const lossCount = losses.length
  const winRate = n > 0 ? (winCount / n) * 100 : 0

  const totalPnl = filtered.reduce((s, t) => s + t.realized_pnl, 0)
  const totalFees = filtered.reduce((s, t) => s + t.fees_paid_usdc, 0)
  const netPnl = totalPnl - totalFees
  const avgPnl = n > 0 ? totalPnl / n : 0
  const medianPnl = median(filtered.map(t => t.realized_pnl))

  const avgDuration = n > 0 ? Math.round(filtered.reduce((s, t) => s + t.duration_secs, 0) / n) : 0
  const medianDuration = median(filtered.map(t => t.duration_secs))

  const grossWins = wins.reduce((s, t) => s + t.realized_pnl, 0)
  const grossLosses = Math.abs(losses.reduce((s, t) => s + t.realized_pnl, 0))
  const profitFactor = grossLosses > 0 ? grossWins / grossLosses : grossWins > 0 ? Infinity : 0

  const avgWin = winCount > 0 ? grossWins / winCount : 0
  const avgLoss = lossCount > 0 ? grossLosses / lossCount : 0
  const expectancy = n > 0 ? (avgWin * (winCount / n)) - (avgLoss * (lossCount / n)) : 0
  const avgRiskReward = avgLoss > 0 ? avgWin / avgLoss : avgWin > 0 ? Infinity : 0
  const payoffRatio = avgRiskReward

  const avgSlippage = n > 0 ? filtered.reduce((s, t) => s + t.slippage_bps, 0) / n : 0

  // Extremes
  const sortedByPnl = [...filtered].sort((a, b) => b.realized_pnl - a.realized_pnl)
  const biggestWin = sortedByPnl.length > 0 && sortedByPnl[0].realized_pnl > 0 ? sortedByPnl[0] : null
  const biggestLoss = sortedByPnl.length > 0 && sortedByPnl[sortedByPnl.length - 1].realized_pnl < 0
    ? sortedByPnl[sortedByPnl.length - 1] : null
  const top5Wins = sortedByPnl.filter(t => t.realized_pnl > 0).slice(0, 5)
  const top5Losses = sortedByPnl.filter(t => t.realized_pnl < 0).slice(-5).reverse()

  // Streaks (chronological order)
  const chronological = [...filtered].sort((a, b) =>
    toStockholm(a.closed_at).getTime() - toStockholm(b.closed_at).getTime()
  )
  let maxWinStreak = 0, maxLossStreak = 0
  let curWin = 0, curLoss = 0
  for (const t of chronological) {
    if (t.realized_pnl > 0) { curWin++; curLoss = 0 }
    else { curLoss++; curWin = 0 }
    if (curWin > maxWinStreak) maxWinStreak = curWin
    if (curLoss > maxLossStreak) maxLossStreak = curLoss
  }
  const lastTrade = chronological[chronological.length - 1]
  const currentStreak: TradeStats['currentStreak'] = !lastTrade
    ? { type: 'win', count: 0 }
    : lastTrade.realized_pnl > 0
      ? { type: 'win', count: curWin }
      : { type: 'loss', count: curLoss }

  // Daily PnL
  const dailyPnl = new Map<string, number>()
  const dailyTrades = new Map<string, number>()
  for (const t of filtered) {
    const key = dateKey(toStockholm(t.closed_at))
    dailyPnl.set(key, (dailyPnl.get(key) ?? 0) + t.realized_pnl)
    dailyTrades.set(key, (dailyTrades.get(key) ?? 0) + 1)
  }

  // Time-based analysis
  const pnlByHour = new Array(24).fill(0)
  const tradesByHour = new Array(24).fill(0)
  const winsByHour = new Array(24).fill(0)
  const pnlByDayOfWeek = new Array(7).fill(0)
  const tradesByDayOfWeek = new Array(7).fill(0)
  const winsByDayOfWeek = new Array(7).fill(0)

  for (const t of filtered) {
    const d = toStockholm(t.closed_at)
    const h = d.getHours()
    const dow = (d.getDay() + 6) % 7 // Mon=0
    pnlByHour[h] += t.realized_pnl
    tradesByHour[h]++
    if (t.realized_pnl > 0) winsByHour[h]++
    pnlByDayOfWeek[dow] += t.realized_pnl
    tradesByDayOfWeek[dow]++
    if (t.realized_pnl > 0) winsByDayOfWeek[dow]++
  }
  const winRateByHour = tradesByHour.map((c, i) => c > 0 ? (winsByHour[i] / c) * 100 : 0)
  const winRateByDayOfWeek = tradesByDayOfWeek.map((c, i) => c > 0 ? (winsByDayOfWeek[i] / c) * 100 : 0)

  // PnL distribution
  const pnlValues = filtered.map(t => t.realized_pnl)
  const pnlDistribution = buildHistogram(
    pnlValues,
    [-1, -0.5, -0.2, -0.1, 0, 0.1, 0.2, 0.5, 1],
    ['< -$1', '-$1 to -$0.50', '-$0.50 to -$0.20', '-$0.20 to -$0.10', '-$0.10 to $0', '$0 to $0.10', '$0.10 to $0.20', '$0.20 to $0.50', '$0.50 to $1', '> $1'],
  )

  // Duration distribution
  const durValues = filtered.map(t => t.duration_secs)
  const durationDistribution = buildHistogram(
    durValues,
    [60, 300, 600, 1800, 3600, 7200],
    ['< 1m', '1-5m', '5-10m', '10-30m', '30m-1h', '1-2h', '> 2h'],
  )

  // By signal source
  const bySignalSource: Record<string, SourceStats> = {}
  for (const t of filtered) {
    const src = t.signal_source ?? 'engine'
    if (!bySignalSource[src]) {
      bySignalSource[src] = { count: 0, wins: 0, losses: 0, winRate: 0, totalPnl: 0, avgPnl: 0, avgDuration: 0 }
    }
    const s = bySignalSource[src]
    s.count++
    s.totalPnl += t.realized_pnl
    s.avgDuration += t.duration_secs
    if (t.realized_pnl > 0) s.wins++
    else s.losses++
  }
  for (const s of Object.values(bySignalSource)) {
    s.winRate = s.count > 0 ? (s.wins / s.count) * 100 : 0
    s.avgPnl = s.count > 0 ? s.totalPnl / s.count : 0
    s.avgDuration = s.count > 0 ? Math.round(s.avgDuration / s.count) : 0
  }

  // By exit reason
  const byExitReason: Record<string, ReasonStats> = {}
  for (const t of filtered) {
    const r = t.reason || 'unknown'
    if (!byExitReason[r]) byExitReason[r] = { count: 0, totalPnl: 0, avgPnl: 0, winRate: 0 }
    const s = byExitReason[r]
    s.count++
    s.totalPnl += t.realized_pnl
    if (t.realized_pnl > 0) s.winRate++
  }
  for (const s of Object.values(byExitReason)) {
    s.avgPnl = s.count > 0 ? s.totalPnl / s.count : 0
    s.winRate = s.count > 0 ? (s.winRate / s.count) * 100 : 0
  }

  // By market
  const marketMap = new Map<string, { title: string; count: number; pnl: number; wins: number; dur: number }>()
  for (const t of filtered) {
    const id = t.token_id
    const existing = marketMap.get(id)
    if (!existing) {
      marketMap.set(id, { title: t.market_title ?? id, count: 1, pnl: t.realized_pnl, wins: t.realized_pnl > 0 ? 1 : 0, dur: t.duration_secs })
    } else {
      existing.count++
      existing.pnl += t.realized_pnl
      existing.dur += t.duration_secs
      if (t.realized_pnl > 0) existing.wins++
    }
  }
  const byMarket: MarketStats[] = [...marketMap.entries()]
    .map(([tokenId, m]) => ({
      title: m.title,
      tokenId,
      count: m.count,
      totalPnl: m.pnl,
      winRate: m.count > 0 ? (m.wins / m.count) * 100 : 0,
      avgDuration: m.count > 0 ? Math.round(m.dur / m.count) : 0,
    }))
    .sort((a, b) => b.count - a.count)

  // Equity curve + risk metrics
  const equityCurve: { timestamp: number; equity: number }[] = []
  let cumPnl = 0
  for (const t of chronological) {
    cumPnl += t.realized_pnl
    equityCurve.push({ timestamp: toStockholm(t.closed_at).getTime(), equity: cumPnl })
  }

  let maxDrawdown = 0
  let maxDrawdownPct = 0
  let hwm = 0
  const initialCapital = 100 // baseline for pct calculations
  for (const pt of equityCurve) {
    const nav = initialCapital + pt.equity
    if (nav > hwm) hwm = nav
    const dd = hwm - nav
    const ddPct = hwm > 0 ? (dd / hwm) * 100 : 0
    if (dd > maxDrawdown) maxDrawdown = dd
    if (ddPct > maxDrawdownPct) maxDrawdownPct = ddPct
  }

  // Daily returns for Sharpe/Sortino
  const dailyReturns: number[] = []
  const sortedDays = [...dailyPnl.entries()].sort((a, b) => a[0].localeCompare(b[0]))
  for (const [, pnl] of sortedDays) {
    dailyReturns.push(pnl)
  }

  const avgReturn = dailyReturns.length > 0 ? dailyReturns.reduce((s, r) => s + r, 0) / dailyReturns.length : 0
  const variance = dailyReturns.length > 1
    ? dailyReturns.reduce((s, r) => s + (r - avgReturn) ** 2, 0) / (dailyReturns.length - 1)
    : 0
  const stdDev = Math.sqrt(variance)
  const downsideVariance = dailyReturns.length > 1
    ? dailyReturns.filter(r => r < 0).reduce((s, r) => s + r ** 2, 0) / dailyReturns.length
    : 0
  const downsideDev = Math.sqrt(downsideVariance)

  const annFactor = Math.sqrt(365)
  const sharpeRatio = stdDev > 0 ? (avgReturn / stdDev) * annFactor : 0
  const sortinoRatio = downsideDev > 0 ? (avgReturn / downsideDev) * annFactor : 0

  const tradingDays = dailyReturns.length || 1
  const annualizedReturn = (totalPnl / initialCapital) * (365 / tradingDays)
  const calmarRatio = maxDrawdownPct > 0 ? (annualizedReturn * 100) / maxDrawdownPct : 0

  return {
    filtered, totalTrades: n, wins: winCount, losses: lossCount, winRate,
    totalPnl, netPnl, avgPnl, medianPnl, totalFees,
    avgDuration, medianDuration,
    profitFactor, expectancy, avgRiskReward, payoffRatio, avgSlippage,
    biggestWin, biggestLoss, top5Wins, top5Losses,
    currentStreak, maxWinStreak, maxLossStreak,
    dailyPnl, dailyTrades,
    pnlByHour, tradesByHour, winRateByHour,
    pnlByDayOfWeek, tradesByDayOfWeek, winRateByDayOfWeek,
    pnlDistribution, durationDistribution,
    bySignalSource, byExitReason, byMarket,
    maxDrawdown, maxDrawdownPct, sharpeRatio, sortinoRatio, calmarRatio,
    equityCurve,
  }
}
