import { useCallback, useEffect, useRef, useState } from 'react'
import type { WsSnapshot } from '../types'

const INITIAL_BACKOFF_MS = 1_000
const MAX_BACKOFF_MS = 30_000
const BACKOFF_MULTIPLIER = 1.5

export function useEngineSocket() {
  const [snapshot, setSnapshot] = useState<WsSnapshot | null>(null)
  const [connected, setConnected] = useState(false)
  const [lastMessageAt, setLastMessageAt] = useState<number | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const backoffRef = useRef(INITIAL_BACKOFF_MS)
  const mountedRef = useRef(true)
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const connect = useCallback(() => {
    if (!mountedRef.current) return

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = window.location.host
    const ws = new WebSocket(`${protocol}//${host}/ws`)
    wsRef.current = ws

    ws.onopen = () => {
      if (!mountedRef.current) { ws.close(); return }
      setConnected(true)
      backoffRef.current = INITIAL_BACKOFF_MS
    }

    ws.onmessage = (e) => {
      setLastMessageAt(Date.now())
      try {
        const data = JSON.parse(e.data) as WsSnapshot
        if (data.type === 'snapshot') setSnapshot(data)
      } catch {
        // ignore malformed messages
      }
    }

    ws.onclose = () => {
      if (!mountedRef.current) return
      setConnected(false)
      const delay = backoffRef.current
      backoffRef.current = Math.min(delay * BACKOFF_MULTIPLIER, MAX_BACKOFF_MS)
      retryTimerRef.current = setTimeout(connect, delay)
    }

    ws.onerror = () => {
      ws.close()
    }
  }, [])

  useEffect(() => {
    mountedRef.current = true
    connect()
    return () => {
      mountedRef.current = false
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current)
      wsRef.current?.close()
    }
  }, [connect])

  return { snapshot, connected, lastMessageAt }
}
