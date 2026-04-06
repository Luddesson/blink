import { useState } from 'react'
import { AlertTriangle, CheckCircle2, XCircle } from 'lucide-react'

interface Props {
  onConfirm: () => void
  onCancel: () => void
}

const CHECKLIST = [
  'Pre-flight checks passed (--preflight-live)',
  '7-day paper run completed',
  'Circuit breaker tested and verified',
  'Vault credentials confirmed',
  'Risk limits reviewed (max 20 USDC/order)',
]

export default function LiveConfirmModal({ onConfirm, onCancel }: Props) {
  const [input, setInput] = useState('')
  const confirmed = input.trim() === 'CONFIRM'

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 backdrop-blur-sm">
      <div className="bg-surface-800 border border-red-800 rounded-xl shadow-2xl w-full max-w-md mx-4 p-6">
        <div className="flex items-start gap-3 mb-5">
          <AlertTriangle size={22} className="text-red-400 mt-0.5 shrink-0" />
          <div>
            <h2 className="text-base font-semibold text-slate-100">
              Switch to Live Trading View
            </h2>
            <p className="text-xs text-slate-400 mt-1">
              You are about to view <strong className="text-amber-400">real capital positions</strong>.
              Emergency controls will become active.
            </p>
          </div>
        </div>

        {/* Checklist */}
        <ul className="space-y-2 mb-5">
          {CHECKLIST.map((item) => (
            <li key={item} className="flex items-center gap-2 text-xs text-slate-300">
              <CheckCircle2 size={13} className="text-emerald-500 shrink-0" />
              {item}
            </li>
          ))}
        </ul>

        <div className="mb-4">
          <label className="block text-xs text-slate-400 mb-1.5">
            Type <span className="font-mono text-red-400 font-bold">CONFIRM</span> to continue
          </label>
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="CONFIRM"
            autoFocus
            className="w-full bg-surface-900 border border-surface-600 text-slate-100 text-sm font-mono rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-red-600 placeholder-slate-600"
          />
        </div>

        <div className="flex gap-2">
          <button
            onClick={onCancel}
            className="flex-1 flex items-center justify-center gap-1.5 btn btn-ghost text-xs py-2"
          >
            <XCircle size={13} /> Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={!confirmed}
            className={`flex-1 text-xs py-2 rounded-md font-semibold transition-all ${
              confirmed
                ? 'bg-red-700 hover:bg-red-600 text-white'
                : 'bg-surface-700 text-slate-600 cursor-not-allowed'
            }`}
          >
            Enter Live View
          </button>
        </div>
      </div>
    </div>
  )
}
