import { createContext, useContext, useState, useCallback, type ReactNode } from 'react'
import { X, CheckCircle, AlertTriangle, Info } from 'lucide-react'

type ToastType = 'success' | 'error' | 'info'

interface Toast {
  id: number
  type: ToastType
  message: string
}

interface ToastContextValue {
  toast: (message: string, type?: ToastType) => void
}

const ToastContext = createContext<ToastContextValue>({ toast: () => {} })

let nextId = 0

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([])

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id))
  }, [])

  const toast = useCallback((message: string, type: ToastType = 'info') => {
    const id = ++nextId
    setToasts((prev) => [...prev.slice(-4), { id, type, message }])
    setTimeout(() => dismiss(id), 3500)
  }, [dismiss])

  return (
    <ToastContext.Provider value={{ toast }}>
      {children}
      <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 pointer-events-none">
        {toasts.map((t) => (
          <ToastItem key={t.id} toast={t} onDismiss={dismiss} />
        ))}
      </div>
    </ToastContext.Provider>
  )
}

function ToastItem({ toast, onDismiss }: { toast: Toast; onDismiss: (id: number) => void }) {
  const icon = {
    success: <CheckCircle size={14} className="text-emerald-400 shrink-0" />,
    error:   <AlertTriangle size={14} className="text-red-400 shrink-0" />,
    info:    <Info size={14} className="text-blue-400 shrink-0" />,
  }[toast.type]

  const border = {
    success: 'border-emerald-800',
    error:   'border-red-800',
    info:    'border-blue-800',
  }[toast.type]

  return (
    <div
      className={`
        pointer-events-auto flex items-center gap-2 fade-in-up
        bg-surface-700 border ${border} rounded-lg px-3 py-2 shadow-lg
        text-xs text-slate-200 max-w-xs
      `}
    >
      {icon}
      <span className="flex-1">{toast.message}</span>
      <button onClick={() => onDismiss(toast.id)} className="text-slate-500 hover:text-slate-300">
        <X size={12} />
      </button>
    </div>
  )
}

export function useToast() {
  return useContext(ToastContext)
}
