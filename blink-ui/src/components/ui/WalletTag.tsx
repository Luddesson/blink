import { useState } from 'react'
import { Copy, Check } from 'lucide-react'
import { cn } from '../../lib/cn'

interface WalletTagProps {
  address: string
  label?: string
  shorten?: boolean
  copyable?: boolean
  className?: string
}

function shortenAddr(addr: string) {
  if (addr.length < 12) return addr
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`
}

export function WalletTag({
  address,
  label,
  shorten: doShorten = true,
  copyable = true,
  className,
}: WalletTagProps) {
  const [copied, setCopied] = useState(false)

  function handleCopy() {
    navigator.clipboard.writeText(address).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  const display = doShorten ? shortenAddr(address) : address

  return (
    <span className={cn('inline-flex items-center gap-1 font-mono text-[11px] text-[color:var(--color-text-secondary)]', className)}>
      {label && <span className="text-[color:var(--color-text-muted)] mr-0.5">{label}</span>}
      <span className="text-[color:var(--color-text-primary)]">{display}</span>
      {copyable && (
        <button
          onClick={handleCopy}
          className="text-[color:var(--color-text-dim)] hover:text-[color:var(--color-aurora-1)] transition-colors"
          title="Copy address"
          aria-label="Copy wallet address"
        >
          {copied ? <Check size={10} className="text-[color:var(--color-bull-400)]" /> : <Copy size={10} />}
        </button>
      )}
    </span>
  )
}
