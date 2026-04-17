import { createContext, useContext, useState, useCallback, type ReactNode } from 'react'
import { X, CheckCircle2, AlertTriangle, Info } from 'lucide-react'
import { motion, AnimatePresence } from 'motion/react'
import { cn } from '../../lib/cn'

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
      <div className="fixed bottom-5 right-5 z-50 flex flex-col gap-2 pointer-events-none">
        <AnimatePresence initial={false}>
          {toasts.map((t) => (
            <ToastItem key={t.id} toast={t} onDismiss={dismiss} />
          ))}
        </AnimatePresence>
      </div>
    </ToastContext.Provider>
  )
}

function ToastItem({ toast, onDismiss }: { toast: Toast; onDismiss: (id: number) => void }) {
  const icon = {
    success: <CheckCircle2 size={14} className="text-[color:var(--color-bull-400)] shrink-0" />,
    error:   <AlertTriangle size={14} className="text-[color:var(--color-bear-400)] shrink-0" />,
    info:    <Info size={14} className="text-[color:var(--color-aurora-3)] shrink-0" />,
  }[toast.type]

  const ring = {
    success: 'shadow-[0_0_0_1px_oklch(0.72_0.19_155/0.3),0_18px_40px_-12px_oklch(0.72_0.19_155/0.3)]',
    error:   'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.3),0_18px_40px_-12px_oklch(0.65_0.24_25/0.3)]',
    info:    'shadow-[0_0_0_1px_oklch(0.75_0.18_170/0.25),0_18px_40px_-12px_oklch(0.70_0.22_290/0.3)]',
  }[toast.type]

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 16, scale: 0.95 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, x: 24, transition: { duration: 0.15 } }}
      transition={{ type: 'spring', stiffness: 380, damping: 26 }}
      className={cn(
        'pointer-events-auto flex items-start gap-2.5 glass rounded-lg px-3.5 py-2.5 text-xs max-w-sm',
        ring,
      )}
    >
      {icon}
      <span className="flex-1 text-[color:var(--color-text-primary)] leading-snug">{toast.message}</span>
      <button
        onClick={() => onDismiss(toast.id)}
        className="text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)] transition-colors -mr-1 -mt-0.5"
        aria-label="Dismiss"
      >
        <X size={12} />
      </button>
    </motion.div>
  )
}

export function useToast() {
  return useContext(ToastContext)
}
