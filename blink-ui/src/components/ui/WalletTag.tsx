import { useState } from 'react'
import { Copy, Check } from 'lucide-react'

interface WalletTagProps {
  address: string
  /** Optional human label shown before the address */
  label?: string
  /** Shorten to 0x1234…5678 format (default: true) */
  shorten?: boolean
  copyable?: boolean
  className?: string
}

function shorten(addr: string) {
  if (addr.length < 12) return addr
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`
}

export function WalletTag({
  address,
  label,
  shorten: doShorten = true,
  copyable = true,
  className = '',
}: WalletTagProps) {
  const [copied, setCopied] = useState(false)

  function handleCopy() {
    navigator.clipboard.writeText(address).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  const display = doShorten ? shorten(address) : address

  return (
    <span
      className={`inline-flex items-center gap-1 font-mono text-[11px] text-slate-400 ${className}`}
    >
      {label && <span className="text-slate-500 mr-0.5">{label}</span>}
      <span className="text-slate-300">{display}</span>
      {copyable && (
        <button
          onClick={handleCopy}
          className="text-slate-600 hover:text-slate-400 transition-colors"
          title="Copy address"
          aria-label="Copy wallet address"
        >
          {copied ? <Check size={10} className="text-emerald-400" /> : <Copy size={10} />}
        </button>
      )}
    </span>
  )
}
