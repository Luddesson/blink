import { useEffect, useRef, useState } from 'react'

export function usePoll<T>(
  fetchFn: () => Promise<T>,
  intervalMs: number,
  enabled = true,
  refreshKey?: unknown,
): { data: T | null; loading: boolean; error: string | null } {
  const [data, setData] = useState<T | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const mountedRef = useRef(true)
  const fetchFnRef = useRef(fetchFn)

  useEffect(() => {
    fetchFnRef.current = fetchFn
  }, [fetchFn])

  useEffect(() => {
    mountedRef.current = true
    if (!enabled) { setLoading(false); return }

    let timer: ReturnType<typeof setTimeout>

    const run = async () => {
      if (mountedRef.current) setLoading(true)
      try {
        const result = await fetchFnRef.current()
        if (mountedRef.current) { setData(result); setError(null) }
      } catch (e) {
        if (mountedRef.current) setError(String(e))
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
  }, [intervalMs, enabled, refreshKey])

  return { data, loading, error }
}
