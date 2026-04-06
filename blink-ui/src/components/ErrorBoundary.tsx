import { Component, ErrorInfo, ReactNode } from 'react'
import { AlertTriangle } from 'lucide-react'

interface Props {
  children: ReactNode
  fallback?: ReactNode
  label?: string
}

interface State {
  hasError: boolean
  error?: Error
}

export default class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ErrorBoundary:${this.props.label ?? 'unknown'}]`, error, info)
  }

  render() {
    if (this.state.hasError) {
      return this.props.fallback ?? (
        <div className="card flex flex-col items-center justify-center gap-2 py-8 text-center">
          <AlertTriangle size={20} className="text-amber-400" />
          <div className="text-xs text-slate-500">
            <span className="font-semibold text-slate-400">{this.props.label ?? 'Panel'}</span> crashed.
          </div>
          <button
            className="text-xs text-indigo-400 hover:text-indigo-300 underline"
            onClick={() => this.setState({ hasError: false })}
          >
            Retry
          </button>
        </div>
      )
    }
    return this.props.children
  }
}
