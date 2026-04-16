import { useState, useEffect, useRef } from 'react'
import { resolveMarketUrl } from '../lib/api'

// Module-level cache so resolved URLs persist across component re-renders
const urlCache = new Map<string, string>()
const pendingFetches = new Set<string>()

/**
 * Resolves a Polymarket token ID to a live event URL.
 * Returns null while loading, the resolved URL once ready.
 * Falls back gracefully if the backend or Gamma API is unavailable.
 */
export function usePolymarketUrl(tokenId: string | undefined): string | null {
  const [url, setUrl] = useState<string | null>(() =>
    tokenId ? (urlCache.get(tokenId) ?? null) : null
  )
  const mountedRef = useRef(true)

  useEffect(() => {
    mountedRef.current = true
    return () => { mountedRef.current = false }
  }, [])

  useEffect(() => {
    if (!tokenId) return
    // Already cached
    if (urlCache.has(tokenId)) {
      setUrl(urlCache.get(tokenId)!)
      return
    }
    // Already fetching — don't double-fetch
    if (pendingFetches.has(tokenId)) return
    pendingFetches.add(tokenId)

    resolveMarketUrl(tokenId).then(resolved => {
      pendingFetches.delete(tokenId)
      if (resolved) {
        urlCache.set(tokenId, resolved)
        if (mountedRef.current) setUrl(resolved)
      }
    }).catch(() => {
      pendingFetches.delete(tokenId)
    })
  }, [tokenId])

  return url
}
