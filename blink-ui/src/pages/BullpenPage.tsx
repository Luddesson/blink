import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import BullpenHeader from '../components/BullpenHeader'
import DiscoveryTable from '../components/DiscoveryTable'
import ConvergenceMonitor from '../components/ConvergenceMonitor'

export default function BullpenPage() {
  const { data: health } = usePoll(api.bullpenHealth, 10_000)
  const { data: discovery } = usePoll(api.bullpenDiscovery, 15_000)
  const { data: convergence } = usePoll(api.bullpenConvergence, 5_000)

  // health is null while loading; health.enabled=false means BULLPEN_ENABLED not set
  const notEnabled = health !== null && !health.enabled
  const failingCritical = !!(health?.enabled && (health?.consecutive_failures ?? 0) >= 3)

  if (notEnabled) {
    return (
      <div className="flex-1 flex items-center justify-center p-2">
        <div className="card max-w-md text-center py-12">
          <div className="text-4xl mb-4">🔌</div>
          <h2 className="text-lg font-semibold text-slate-200 mb-2">Bullpen Not Connected</h2>
          <p className="text-sm text-slate-500 mb-4">
            Bullpen requires BULLPEN_ENABLED. Make sure your <code className="text-cyan-400">.env</code> contains:
          </p>
          <div className="bg-slate-900 rounded p-3 text-left text-xs font-mono text-slate-400 space-y-1">
            <div><span className="text-cyan-400">BULLPEN_CLI_PATH</span>=wsl -d Ubuntu -- bullpen</div>
            <div><span className="text-cyan-400">BULLPEN_USE_WSL</span>=true</div>
            <div><span className="text-cyan-400">BULLPEN_ENABLED</span>=true</div>
          </div>
          <p className="text-[11px] text-slate-500 mt-4">
            Then <span className="text-amber-400 font-semibold">restart Blink engine</span> — env vars are read at startup.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-hidden min-h-0">
      {failingCritical && (
        <div className="bg-gradient-to-r from-red-950/80 to-red-900/60 border border-red-700/60 rounded-lg px-4 py-3 shrink-0 backdrop-blur-sm">
          <div className="flex items-start gap-3">
            <span className="text-lg mt-0.5">⚠️</span>
            <div className="flex-1 min-w-0">
              <h3 className="text-sm font-semibold text-red-200 mb-1">Bullpen Offline</h3>
              <p className="text-xs text-red-300/90 mb-2">
                {health?.consecutive_failures} consecutive failures — sidecar may have crashed
              </p>
              <p className="text-xs text-red-400 font-mono bg-red-950/40 rounded px-2 py-1.5 inline-block">
                Check alpha sidecar logs / restart sidecar
              </p>
              {health?.last_error && (
                <p className="text-xs text-red-300/70 mt-2 italic truncate" title={health.last_error}>
                  Last error: {health.last_error}
                </p>
              )}
            </div>
          </div>
        </div>
      )}

      <ErrorBoundary label="BullpenHeader">
        <BullpenHeader health={health} discovery={discovery} />
      </ErrorBoundary>

      {failingCritical ? (
        <div className="flex-1 flex items-center justify-center text-center text-slate-500 p-4">
          <div>
            <p className="text-sm mb-2">Discovery and convergence data unavailable</p>
            <p className="text-xs text-slate-600">Waiting for sidecar recovery...</p>
          </div>
        </div>
      ) : (
        <div className="flex-1 grid grid-cols-[1fr_380px] gap-2 overflow-hidden min-h-0">
          <ErrorBoundary label="DiscoveryTable">
            <div className="overflow-y-auto min-h-0">
              <DiscoveryTable discovery={discovery} />
            </div>
          </ErrorBoundary>

          <ErrorBoundary label="ConvergenceMonitor">
            <div className="overflow-y-auto min-h-0">
              <ConvergenceMonitor convergence={convergence} />
            </div>
          </ErrorBoundary>
        </div>
      )}
    </div>
  )
}
