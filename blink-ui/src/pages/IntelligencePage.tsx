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

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-hidden min-h-0">
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
