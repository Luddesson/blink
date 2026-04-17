import { useState, useEffect, useMemo, useRef } from 'react'
import { motion, AnimatePresence } from 'motion/react'
import { Search, LayoutDashboard, TrendingUp, History, Brain, Activity, Settings, Sparkles, Pause, Play, CircleStop } from 'lucide-react'
import type { TabId } from '../hooks/useTab'
import { cn } from '../lib/cn'

interface Command {
  id: string
  label: string
  hint?: string
  icon: React.ReactNode
  keywords: string[]
  run: () => void
}

interface Props {
  open: boolean
  onClose: () => void
  onSwitchTab: (tab: TabId) => void
  onPause: () => void
  paused: boolean
}

const TAB_ITEMS: { id: TabId; label: string; icon: React.ReactNode; hint: string }[] = [
  { id: 'dashboard', label: 'Dashboard', icon: <LayoutDashboard size={14} />, hint: '1' },
  { id: 'markets', label: 'Markets', icon: <TrendingUp size={14} />, hint: '2' },
  { id: 'history', label: 'History', icon: <History size={14} />, hint: '3' },
  { id: 'intelligence', label: 'Intelligence', icon: <Brain size={14} />, hint: '4' },
  { id: 'performance', label: 'Performance', icon: <Activity size={14} />, hint: '5' },
  { id: 'config', label: 'Config', icon: <Settings size={14} />, hint: '6' },
  { id: 'alpha', label: 'Alpha AI', icon: <Sparkles size={14} />, hint: '7' },
]

export default function CommandPalette({ open, onClose, onSwitchTab, onPause, paused }: Props) {
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  const commands = useMemo<Command[]>(() => {
    const tabCmds = TAB_ITEMS.map<Command>((t) => ({
      id: `goto:${t.id}`,
      label: `Go to ${t.label}`,
      hint: t.hint,
      icon: t.icon,
      keywords: [t.label.toLowerCase(), t.id, 'goto', 'navigate'],
      run: () => { onSwitchTab(t.id); onClose() },
    }))
    const actions: Command[] = [
      {
        id: 'trading:pause',
        label: paused ? 'Resume trading' : 'Pause trading',
        hint: 'P',
        icon: paused ? <Play size={14} /> : <Pause size={14} />,
        keywords: ['pause', 'resume', 'halt', 'trading'],
        run: () => { onPause(); onClose() },
      },
      {
        id: 'trading:killswitch',
        label: 'Kill switch (emergency stop)',
        hint: '⌘K',
        icon: <CircleStop size={14} />,
        keywords: ['kill', 'stop', 'emergency', 'halt'],
        run: () => { onPause(); onClose() },
      },
    ]
    return [...tabCmds, ...actions]
  }, [onSwitchTab, onPause, onClose, paused])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return commands
    return commands.filter((c) =>
      c.label.toLowerCase().includes(q) || c.keywords.some((k) => k.includes(q)),
    )
  }, [commands, query])

  useEffect(() => {
    if (open) {
      setQuery('')
      setSelected(0)
      setTimeout(() => inputRef.current?.focus(), 50)
    }
  }, [open])

  useEffect(() => { setSelected(0) }, [query])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { e.preventDefault(); onClose() }
      else if (e.key === 'ArrowDown') { e.preventDefault(); setSelected((s) => Math.min(s + 1, filtered.length - 1)) }
      else if (e.key === 'ArrowUp') { e.preventDefault(); setSelected((s) => Math.max(s - 1, 0)) }
      else if (e.key === 'Enter') { e.preventDefault(); filtered[selected]?.run() }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, filtered, selected, onClose])

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.14 }}
          className="fixed inset-0 z-50 flex items-start justify-center pt-[18vh] px-4"
          onClick={onClose}
        >
          <div className="absolute inset-0 bg-[color:oklch(0.10_0.02_260/0.65)] backdrop-blur-sm" />
          <motion.div
            initial={{ opacity: 0, y: -8, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -8, scale: 0.98 }}
            transition={{ duration: 0.18, ease: [0.2, 0, 0, 1] }}
            onClick={(e) => e.stopPropagation()}
            className="relative w-full max-w-xl glass rounded-xl overflow-hidden"
            style={{
              boxShadow: 'inset 0 0 0 1px oklch(0.72 0.16 285 / 0.3), 0 30px 80px -20px oklch(0.10 0.02 260 / 0.7)',
            }}
          >
            <div className="flex items-center gap-2 px-4 py-3 border-b border-[color:var(--color-border-subtle)]">
              <Search size={14} className="text-[color:var(--color-text-muted)]" />
              <input
                ref={inputRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Type a command or search…"
                className="flex-1 bg-transparent outline-none text-sm text-[color:var(--color-text-primary)] placeholder:text-[color:var(--color-text-dim)]"
              />
              <kbd className="text-[10px] font-mono text-[color:var(--color-text-dim)] px-1.5 py-0.5 rounded border border-[color:var(--color-border-subtle)]">
                Esc
              </kbd>
            </div>
            <div className="max-h-[44vh] overflow-y-auto py-1">
              {filtered.length === 0 ? (
                <div className="px-4 py-6 text-center text-xs text-[color:var(--color-text-muted)]">
                  No commands match "{query}"
                </div>
              ) : (
                filtered.map((c, i) => (
                  <button
                    key={c.id}
                    onMouseEnter={() => setSelected(i)}
                    onClick={() => c.run()}
                    className={cn(
                      'w-full flex items-center gap-3 px-4 py-2.5 text-sm text-left transition-colors',
                      i === selected
                        ? 'bg-[color:oklch(0.72_0.16_285/0.15)] text-[color:var(--color-text-primary)]'
                        : 'text-[color:var(--color-text-secondary)]',
                    )}
                  >
                    <span className="opacity-70">{c.icon}</span>
                    <span className="flex-1 truncate">{c.label}</span>
                    {c.hint && (
                      <kbd className="text-[10px] font-mono text-[color:var(--color-text-dim)] px-1.5 py-0.5 rounded border border-[color:var(--color-border-subtle)]">
                        {c.hint}
                      </kbd>
                    )}
                  </button>
                ))
              )}
            </div>
            <div className="px-4 py-2 border-t border-[color:var(--color-border-subtle)] flex items-center gap-3 text-[10px] text-[color:var(--color-text-dim)]">
              <span>↑↓ navigate</span>
              <span>↵ select</span>
              <span>Esc close</span>
              <span className="ml-auto">⌘P open</span>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
