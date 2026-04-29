import { useCallback, useEffect, useRef, useState } from 'react'

// ─── Types ───────────────────────────────────────────────────────────────────

export type EquityRange = '1h' | '6h' | '24h' | '7d' | '30d'

export const EQUITY_RANGES: EquityRange[] = ['1h', '6h', '24h', '7d', '30d']

/** Canonical window in ms for each range — matches the backend's resolver. */
export const RANGE_WINDOW_MS: Record<EquityRange, number> = {
  '1h': 60 * 60 * 1000,
  '6h': 6 * 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
}

export interface EquityPoint {
  timestamp_ms: number
  nav_usdc: number
}

export interface EquitySeriesResponse {
  source: 'clickhouse' | 'postgres' | 'memory' | 'none' | 'timeout' | 'live_wallet_truth' | 'live_wallet_unverified'
  range: EquityRange
  bucket_ms: number
  window_ms: number
  start_ms: number
  end_ms: number
  first_ms: number | null
  last_ms: number | null
  truncated: boolean
  points: EquityPoint[]
  reality_status?: 'matched' | 'mismatch' | 'unverified'
  reality_issues?: string[]
  truth_checked_at_ms?: number | null
  wallet_truth_verified?: boolean
  exchange_positions_verified?: boolean
  onchain_cash_verified?: boolean
  wallet_nav_usdc?: number | null
  wallet_position_value_usdc?: number | null
  wallet_position_initial_value_usdc?: number | null
  wallet_open_pnl_usdc?: number | null
  wallet_pnl_source?: string
}

export interface EquitySeriesState {
  points: EquityPoint[]
  bucketMs: number
  windowMs: number
  startMs: number
  endMs: number
  firstMs: number | null
  lastMs: number | null
  source: EquitySeriesResponse['source']
  truncated: boolean
  loading: boolean
  fromCache: boolean
  fetchedAt: number | null
  error: string | null
}

// ─── Config ──────────────────────────────────────────────────────────────────

/**
 * Adaptive poll cadence per range.
 * Short ranges refresh aggressively for a live feel; long ranges poll slowly
 * because a 7d/30d curve barely moves between consecutive samples.
 */
const POLL_MS: Record<EquityRange, number> = {
  '1h':  10_000,
  '6h':  30_000,
  '24h': 30_000,
  '7d':  120_000,
  '30d': 120_000,
}

interface CacheEntry {
  data: EquitySeriesResponse
  fetchedAt: number
}

// ─── Hook ────────────────────────────────────────────────────────────────────

/**
 * Shared equity-curve fetcher with a per-range cache and
 * stale-while-revalidate semantics.
 *
 * When the user switches ranges, the previously-cached payload is rendered
 * immediately (no flicker) while a background refetch runs. Each range polls
 * on its own cadence (see `POLL_MS`).
 */
export function useEquitySeries(initial: EquityRange = '1h'): EquitySeriesState & {
  range: EquityRange
  setRange: (r: EquityRange) => void
  refresh: () => void
} {
  const [range, setRangeState] = useState<EquityRange>(initial)
  const cacheRef = useRef<Map<EquityRange, CacheEntry>>(new Map())
  const mountedRef = useRef(true)

  const [tick, setTick] = useState(0) // bumps on cache update to trigger re-render

  const loadingRef = useRef(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const fetchRange = useCallback(async (r: EquityRange): Promise<void> => {
    if (loadingRef.current) return
    loadingRef.current = true
    setLoading(true)
    try {
      const resp = await fetch(`/api/analytics/equity?range=${r}`)
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
      const data = (await resp.json()) as EquitySeriesResponse
      if (!mountedRef.current) return
      cacheRef.current.set(r, { data, fetchedAt: Date.now() })
      setError(null)
      setTick(t => t + 1)
    } catch (e) {
      if (mountedRef.current) setError(String(e))
      // Keep stale cache on error — don't evict.
    } finally {
      loadingRef.current = false
      if (mountedRef.current) setLoading(false)
    }
  }, [])

  // Mount / unmount lifecycle.
  useEffect(() => {
    mountedRef.current = true
    return () => { mountedRef.current = false }
  }, [])

  // Fetch when range changes; keep previous cache visible until new data lands.
  useEffect(() => {
    void fetchRange(range)
    const id = setInterval(() => { void fetchRange(range) }, POLL_MS[range])
    return () => clearInterval(id)
  }, [range, fetchRange])

  const setRange = useCallback((r: EquityRange) => {
    setRangeState(r)
    // Force re-read of cache even if the same tick.
    setTick(t => t + 1)
  }, [])

  const refresh = useCallback(() => { void fetchRange(range) }, [fetchRange, range])

  // Read current entry from cache (may be from a previous range if not yet fetched).
  void tick // keep dep-linter happy; we read cacheRef.current below
  const entry = cacheRef.current.get(range)
  const data = entry?.data
  const fetchedAt = entry?.fetchedAt ?? null

  return {
    range,
    setRange,
    refresh,
    points: data?.points ?? [],
    bucketMs: data?.bucket_ms ?? 0,
    windowMs: data?.window_ms ?? 0,
    startMs: data?.start_ms ?? 0,
    endMs: data?.end_ms ?? 0,
    firstMs: data?.first_ms ?? null,
    lastMs: data?.last_ms ?? null,
    source: data?.source ?? 'none',
    truncated: data?.truncated ?? false,
    loading,
    fromCache: !!entry && !loading,
    fetchedAt,
    error,
  }
}

// ─── Helpers shared with chart consumers ─────────────────────────────────────

/**
 * Human-readable label for the bucket size returned by the backend.
 * `0` means "raw samples".
 */
export function formatBucket(bucketMs: number): string {
  if (bucketMs <= 0) return 'raw'
  const s = bucketMs / 1000
  if (s < 60) return `${s}s`
  const m = s / 60
  if (m < 60) return `${m}m`
  const h = m / 60
  if (h < 24) return `${h}h`
  return `${h / 24}d`
}

/**
 * Computes the Y-axis `[min, max]` for a series with **relative** padding
 * (5% of range) and an absolute floor so flat series still render a visible
 * band instead of a razor-thin line.
 *
 * `floor` defaults to $0.02, which is tuned for the Blink NAV scale.
 */
export function computeYDomain(
  values: number[],
  { floor = 0.02, padPct = 0.05 }: { floor?: number; padPct?: number } = {},
): [number, number] {
  if (values.length === 0) return [-floor, floor]
  let min = values[0]
  let max = values[0]
  for (const v of values) {
    if (v < min) min = v
    if (v > max) max = v
  }
  const span = max - min
  const pad = Math.max(span * padPct, floor)
  return [min - pad, max + pad]
}

/**
 * Scale-aware decimal precision for Y-tick labels: tight scales keep 3-4
 * decimals for visible detail; wide scales drop to 0-1 to avoid trailing
 * zero noise (`+50.00` → `+50`).
 */
export function pickValuePrecision(span: number): number {
  if (span <= 0.1) return 4
  if (span <= 1) return 3
  if (span <= 10) return 2
  if (span <= 100) return 1
  return 0
}

/**
 * X-axis `[start, end]` domain.
 *
 * **Always fits to the data** so the curve fills 100% of the plot — no empty
 * whitespace, no squashed-to-the-right sliver. Different time ranges are
 * still visually distinct because:
 *   - bucket size changes (raw → 1m → 5m → 30m → 2h)
 *   - tick-label format auto-switches (HH:MM → MM-DD HH)
 *   - the "partial" badge flags when `firstMs > startMs`
 *
 * - Right edge pins to `nowMs` when the latest sample is within ~2 buckets
 *   of now (live tail tracks the clock).
 * - A small left pad (4% of data span, floored at one bucket) keeps the
 *   first sample from touching the Y-axis.
 */
export function computeXDomain(
  firstMs: number | null,
  lastMs: number | null,
  nowMs: number,
  bucketMs: number,
  windowMs: number,
): [number, number] {
  if (firstMs == null || lastMs == null) {
    const win = Math.max(windowMs, 60_000)
    return [nowMs - win, nowMs]
  }

  const bucket = bucketMs > 0 ? bucketMs : 60_000
  const liveWindow = bucket * 2
  const liveTail = nowMs - lastMs <= liveWindow
  const right = liveTail ? nowMs : lastMs

  const dataSpan = Math.max(right - firstMs, bucket)
  const pad = Math.max(dataSpan * 0.04, bucket)
  return [firstMs - pad, right]
}
