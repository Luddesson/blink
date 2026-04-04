import { useState } from 'react';
import Dashboard from './pages/Dashboard';
import Markets from './pages/Markets';
import History from './pages/History';
import Risk from './pages/Risk';
import Twin from './pages/Twin';

const tabs = [
  { id: 'dashboard', label: 'Dashboard', shortcut: '1' },
  { id: 'markets', label: 'Markets', shortcut: '2' },
  { id: 'history', label: 'History', shortcut: '3' },
  { id: 'risk', label: 'Risk', shortcut: '4' },
  { id: 'twin', label: 'Twin', shortcut: '5' },
] as const;

type TabId = typeof tabs[number]['id'];

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('dashboard');

  return (
    <div className="min-h-screen bg-[#0a0b0f]">
      {/* Header */}
      <header className="border-b border-gray-800 bg-[#0d0e13] sticky top-0 z-50">
        <div className="max-w-7xl mx-auto px-4 py-2 flex items-center justify-between">
          <div className="flex items-center gap-6">
            <div className="flex items-center gap-2">
              <div className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
              <span className="text-sm font-bold text-white tracking-wide">BLINK</span>
              <span className="text-xs text-gray-500">v0.2</span>
            </div>
            <nav className="flex gap-1">
              {tabs.map((tab) => (
                <button
                  key={tab.id}
                  onClick={() => setActiveTab(tab.id)}
                  className={`px-3 py-1.5 text-xs rounded transition-colors ${
                    activeTab === tab.id
                      ? 'bg-gray-800 text-white'
                      : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800/50'
                  }`}
                >
                  <span className="text-gray-600 mr-1">{tab.shortcut}</span>
                  {tab.label}
                </button>
              ))}
            </nav>
          </div>
          <div className="text-[10px] text-gray-600">
            Polymarket Shadow Maker
          </div>
        </div>
      </header>

      {/* Content */}
      <main className="max-w-7xl mx-auto px-4 py-4">
        {activeTab === 'dashboard' && <Dashboard />}
        {activeTab === 'markets' && <Markets />}
        {activeTab === 'history' && <History />}
        {activeTab === 'risk' && <Risk />}
        {activeTab === 'twin' && <Twin />}
      </main>

      {/* Footer */}
      <footer className="border-t border-gray-800 mt-8">
        <div className="max-w-7xl mx-auto px-4 py-2 text-[10px] text-gray-700 text-center">
          Blink Engine Web UI — Not financial advice. Paper trading simulation only.
        </div>
      </footer>
    </div>
  );
}
