import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import IntelligenceHeader from '../components/IntelligenceHeader'
import DiscoveryTable from '../components/DiscoveryTable'
import ConvergenceMonitor from '../components/ConvergenceMonitor'

export default function IntelligencePage() {
  const { data: health } = usePoll(api.bullpenHealth, 10_000)
  const { data: discovery } = usePoll(api.bullpenDiscovery, 15_000)
  const { data: convergence } = usePoll(api.bullpenConvergence, 5_000)

  // health is null while loading; health.enabled=false means BULLPEN_ENABLED not set
  const notEnabled = health !== null && !health.enabled
  const failingHard = !!(health?.enabled && (health?.consecutive_failures ?? 0) >= 5)

  if (notEnabled) {
    return (
      <div className="flex-1 flex items-center justify-center p-2">
        <div className="card max-w-md text-center py-12">
          <div className="text-4xl mb-4">🔌</div>
          <h2 className="text-lg font-semibold text-slate-200 mb-2">Bullpen Not Connected</h2>
          <p className="text-sm text-slate-500 mb-4">
            Intelligence requires Bullpen to be enabled. Make sure your <code className="text-cyan-400">.env</code> contains:
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
      {failingHard && (
        <div className="bg-red-900/40 border border-red-700/50 rounded px-3 py-2 text-xs text-red-300 flex items-center gap-2 shrink-0">
          <span>⚠</span>
          <span>Bullpen CLI has {health?.consecutive_failures} consecutive failures — check WSL2 is running and bullpen is installed.</span>
        </div>
      )}
      <ErrorBoundary label="IntelligenceHeader">
        <IntelligenceHeader health={health} discovery={discovery} />
      </ErrorBoundary>

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
    </div>
  )
}
