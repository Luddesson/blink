import { useState } from 'react'
import { Power } from 'lucide-react'
import { api } from '../lib/api'

interface Props {
  paused: boolean
  onToggled: (paused: boolean) => void
}

export default function EmergencyStop({ paused, onToggled }: Props) {
  const [clicks, setClicks] = useState(0)
  const [busy, setBusy] = useState(false)

  async function handleClick() {
    if (busy) return
    if (!paused && clicks < 1) {
      setClicks(1)
      setTimeout(() => setClicks(0), 3000)
      return
    }
    setBusy(true)
    try {
      const result = await api.pause(!paused)
      onToggled(result.trading_paused)
      setClicks(0)
    } catch (e) {
      alert(`Failed to toggle pause: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const needsDoubleClick = !paused && clicks === 0

  return (
    <div className="card border-red-900/50">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 block mb-3">
        Emergency Stop
      </span>

      <button
        onClick={handleClick}
        disabled={busy}
        className={`w-full flex items-center justify-center gap-2 py-3 rounded-lg font-semibold text-sm transition-all ${
          paused
            ? 'bg-emerald-700 hover:bg-emerald-600 text-white'
            : needsDoubleClick
            ? 'bg-red-900/60 border border-red-700 text-red-300 hover:bg-red-800/80'
            : 'bg-red-600 hover:bg-red-500 text-white animate-pulse'
        } ${busy ? 'opacity-50 cursor-wait' : ''}`}
      >
        <Power size={15} />
        {paused
          ? 'RESUME TRADING'
          : needsDoubleClick
          ? 'HALT TRADING'
          : '⚠ CONFIRM HALT'}
      </button>

      <p className="text-xs text-slate-600 mt-2 text-center">
        {paused
          ? 'Trading is currently HALTED'
          : needsDoubleClick
          ? 'Click once more within 3s to confirm halt'
          : 'Click to confirm emergency halt'}
      </p>
    </div>
  )
}
