import { usePolymarketUrl } from '../hooks/usePolymarketUrl'

interface Props {
  tokenId: string
  label: string
  className?: string
  titleOverride?: string
  maxWidth?: string
}

/**
 * Renders a market title as a clickable Polymarket link.
 * Resolves the real event URL via the backend; shows text only while loading.
 */
export default function MarketLink({ tokenId, label, className = '', titleOverride, maxWidth }: Props) {
  const url = usePolymarketUrl(tokenId)
  const baseClass = `hover:text-emerald-400 hover:underline transition-colors ${className}`

  if (!url) {
    // Still resolving — show as plain text, no broken link
    return (
      <span className={className} title={titleOverride ?? label}>
        {label}
      </span>
    )
  }

  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className={baseClass}
      title={titleOverride ?? label}
      style={maxWidth ? { maxWidth } : undefined}
      onClick={(e) => e.stopPropagation()}
    >
      {label}
    </a>
  )
}
