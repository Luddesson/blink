import { useEffect, useRef, useState } from 'react'

export function usePoll<T>(
  fetchFn: () => Promise<T>,
  intervalMs: number,
  enabled = true,
): { data: T | null; loading: boolean; error: string | null } {
  const [data, setData] = useState<T | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const mountedRef = useRef(true)

  useEffect(() => {
    mountedRef.current = true
    if (!enabled) { setLoading(false); return }

    let timer: ReturnType<typeof setTimeout>

    const run = async () => {
      try {
        const result = await fetchFn()
        if (mountedRef.current) { setData(result); setError(null) }
      } catch (e) {
        if (mountedRef.current) setError(String(e))
        // keep stale data on error — don't reset to null
      } finally {
        if (mountedRef.current) {
          setLoading(false)
          timer = setTimeout(run, intervalMs)
        }
      }
    }

    run()
    return () => {
      mountedRef.current = false
      clearTimeout(timer)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs, enabled])

  return { data, loading, error }
}
