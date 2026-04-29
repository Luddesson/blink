import { useState, useEffect, useRef } from 'react'
import { motion, AnimatePresence } from 'motion/react'
import { Terminal, Search, Zap, ArrowRight, X } from 'lucide-react'
import { cn } from '../lib/cn'

interface CommandPaletteProps {
  open: boolean
  onClose: () => void
  onSwitchTab: (tab: any) => void
  onPause: () => void
  paused: boolean
}

export default function CommandPalette({ open, onClose, onSwitchTab, onPause, paused }: CommandPaletteProps) {
  const [query, setQuery] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setQuery('')
      setTimeout(() => inputRef.current?.focus(), 10)
    }
  }, [open])

  const commands = [
    { id: 'buy', label: 'Buy Asset...', icon: Zap, shortcut: 'B', color: 'text-[color:var(--color-bull-400)]' },
    { id: 'sell', label: 'Sell Asset...', icon: Zap, shortcut: 'S', color: 'text-[color:var(--color-bear-400)]' },
    { id: 'pause', label: paused ? 'Resume Engine' : 'Pause Engine', icon: X, shortcut: 'P', color: 'text-[color:var(--color-whale-400)]' },
    { id: 'dashboard', label: 'Go to Dashboard', icon: Terminal, shortcut: 'D' },
    { id: 'alpha', label: 'Go to Alpha AI', icon: Terminal, shortcut: 'A' },
  ]

  const filtered = commands.filter(c => c.label.toLowerCase().includes(query.toLowerCase()))

  if (!open) return null

  return (
    <AnimatePresence>
      <div className="fixed inset-0 z-[100] flex items-start justify-center pt-[15vh] px-4">
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onClick={onClose}
          className="absolute inset-0 bg-[color:var(--color-void)/0.8] backdrop-blur-md"
        />
        
        <motion.div
          initial={{ opacity: 0, scale: 0.95, y: -20 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: -20 }}
          className="relative w-full max-w-xl overflow-hidden rounded-2xl border border-[color:var(--color-border-strong)] bg-[color:var(--color-surface-900)/0.9] shadow-2xl shadow-black"
        >
          <div className="flex items-center px-4 py-3 border-b border-[color:var(--color-border-subtle)]">
            <Search size={18} className="text-[color:var(--color-text-dim)] mr-3" />
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Type a command or trade..."
              className="flex-1 bg-transparent border-none outline-none text-sm text-[color:var(--color-text-primary)] placeholder:text-[color:var(--color-text-dim)]"
            />
            <div className="flex items-center gap-1.5">
              <kbd className="px-1.5 py-0.5 rounded border border-[color:var(--color-border-subtle)] bg-[color:var(--color-surface-700)] text-[10px] font-mono text-[color:var(--color-text-dim)]">ESC</kbd>
            </div>
          </div>

          <div className="max-h-[60vh] overflow-y-auto p-2">
            {filtered.length > 0 ? (
              filtered.map((cmd) => (
                <button
                  key={cmd.id}
                  className="group flex w-full items-center justify-between rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-[color:var(--color-surface-700)/0.5]"
                  onClick={() => {
                    if (cmd.id === 'pause') onPause()
                    else if (['dashboard', 'alpha'].includes(cmd.id)) onSwitchTab(cmd.id)
                    onClose()
                  }}
                >
                  <div className="flex items-center gap-3">
                    <div className={cn("p-1.5 rounded-md bg-[color:var(--color-surface-800)]", cmd.color)}>
                      <cmd.icon size={14} />
                    </div>
                    <span className="text-sm font-medium text-[color:var(--color-text-secondary)] group-hover:text-[color:var(--color-text-primary)] transition-colors">
                      {cmd.label}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] font-mono text-[color:var(--color-text-dim)] uppercase tracking-widest">{cmd.shortcut}</span>
                    <ArrowRight size={12} className="text-[color:var(--color-text-dim)] opacity-0 group-hover:opacity-100 transition-all -translate-x-1 group-hover:translate-x-0" />
                  </div>
                </button>
              ))
            ) : (
              <div className="py-12 text-center">
                <p className="text-xs text-[color:var(--color-text-dim)] uppercase tracking-widest">No matching commands</p>
              </div>
            )}
          </div>

          <div className="flex items-center justify-between px-4 py-2 bg-[color:var(--color-surface-950)/0.5] border-t border-[color:var(--color-border-subtle)] text-[10px] text-[color:var(--color-text-dim)]">
            <div className="flex items-center gap-3">
              <span className="flex items-center gap-1"><Zap size={10} className="text-[color:var(--color-aurora-1)]" /> AI Powered</span>
              <span className="flex items-center gap-1"><Terminal size={10} /> Blink Terminal v3.0</span>
            </div>
          </div>
        </motion.div>
      </div>
    </AnimatePresence>
  )
}