import { useState, useCallback, useEffect } from 'react'

const TABS = ['dashboard', 'markets', 'history', 'intelligence', 'performance', 'inventory', 'config', 'alpha'] as const
export type TabId = (typeof TABS)[number]

export function useTab() {
  const [activeTab, setActiveTab] = useState<TabId>(() => {
    const hash = window.location.hash.replace('#', '')
    return TABS.includes(hash as TabId) ? (hash as TabId) : 'dashboard'
  })

  const switchTab = useCallback((tab: TabId) => {
    setActiveTab(tab)
    window.location.hash = tab
  }, [])

  const switchByIndex = useCallback((index: number) => {
    if (index >= 0 && index < TABS.length) {
      switchTab(TABS[index])
    }
  }, [switchTab])

  useEffect(() => {
    const onHash = () => {
      const hash = window.location.hash.replace('#', '')
      if (TABS.includes(hash as TabId)) setActiveTab(hash as TabId)
    }
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  return { activeTab, switchTab, switchByIndex, tabs: TABS }
}
