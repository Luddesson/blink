export function fmt(n: number, decimals = 2): string {
  return n.toLocaleString('en-US', {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  })
}

export function fmtPnl(n: number): string {
  const sign = n >= 0 ? '+' : ''
  return `${sign}${fmt(n)}`
}

export function fmtPct(n: number): string {
  const sign = n >= 0 ? '+' : ''
  return `${sign}${fmt(n, 2)}%`
}

export function fmtDuration(secs: number): string {
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  return `${h}h ${m}m`
}

export function pnlClass(n: number): string {
  if (n > 0) return 'pnl-positive'
  if (n < 0) return 'pnl-negative'
  return 'pnl-neutral'
}

export function formatTimestamp(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString('sv-SE', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
      timeZone: 'Europe/Stockholm',
    })
  } catch {
    return iso
  }
}

/** Neon-styled time string like [17:30] */
export function fmtNeonTime(input: string | number | Date): string {
  try {
    // Handle bare HH:MM:SS or HH:MM strings from backend
    if (typeof input === 'string' && /^\d{1,2}:\d{2}(:\d{2})?$/.test(input.trim())) {
      const parts = input.trim().split(':')
      return `[${parts[0].padStart(2, '0')}:${parts[1]}]`
    }
    // Rust chrono emits nanoseconds (9 sub-second digits): new Date() only handles ms (3 digits).
    // Strip any sub-second precision beyond 3 digits so JS Date parses correctly.
    const normalized = typeof input === 'string'
      ? input.replace(/(\.\d{3})\d+/, '$1')
      : input
    const d = normalized instanceof Date ? normalized : new Date(normalized)
    if (isNaN(d.getTime())) return '[--:--]'
    const hh = d.toLocaleTimeString('sv-SE', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
      timeZone: 'Europe/Stockholm',
    })
    return `[${hh}]`
  } catch {
    return '[--:--]'
  }
}

/** Format Stockholm time for chart tooltips (HH:mm:ss) */
export function fmtChartTime(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString('sv-SE', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
    timeZone: 'Europe/Stockholm',
  })
}

/** Format event timing relative to now */
export function formatEventTiming(startTime?: number, endTime?: number): { text: string; className: string } {
  const now = Math.floor(Date.now() / 1000)

  if (endTime && endTime < now) {
    const ago = now - endTime
    return { text: `Ended ${fmtDuration(ago)} ago`, className: 'text-slate-500' }
  }

  if (startTime) {
    if (startTime > now) {
      const until = startTime - now
      return { text: `Starts in ${fmtDuration(until)}`, className: 'text-amber-400' }
    }
    // Started but not ended
    if (!endTime || endTime > now) {
      return { text: 'In progress', className: 'text-emerald-400' }
    }
  }

  return { text: '—', className: 'text-slate-600' }
}
