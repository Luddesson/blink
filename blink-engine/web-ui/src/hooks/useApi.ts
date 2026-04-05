import { useState, useEffect, useRef } from 'react';

const BASE = '';

export function useFetch<T>(path: string, intervalMs = 2000) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const fetchData = async () => {
      try {
        const res = await fetch(`${BASE}${path}`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const json = await res.json();
        if (active) { setData(json); setError(null); }
      } catch (e: any) {
        if (active) setError(e.message);
      }
    };
    fetchData();
    const id = setInterval(fetchData, intervalMs);
    return () => { active = false; clearInterval(id); };
  }, [path, intervalMs]);

  return { data, error };
}

export function useWebSocket() {
  const [snapshot, setSnapshot] = useState<any>(null);
  const [connected, setConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${window.location.host}/ws`;

    function connect() {
      const ws = new WebSocket(url);
      wsRef.current = ws;
      ws.onopen = () => setConnected(true);
      ws.onclose = () => {
        setConnected(false);
        setTimeout(connect, 3000);
      };
      ws.onmessage = (e) => {
        try { setSnapshot(JSON.parse(e.data)); } catch {}
      };
    }
    connect();
    return () => { wsRef.current?.close(); };
  }, []);

  return { snapshot, connected };
}

export async function postPause(paused: boolean) {
  await fetch(`${BASE}/api/pause`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ paused }),
  });
}
