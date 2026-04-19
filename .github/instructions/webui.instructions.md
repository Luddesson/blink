---
applyTo:
  - blink-ui/src/**
  - blink-engine/web-ui/src/**
description: TypeScript/React conventions for the Blink web dashboard.
---

## TypeScript
- Strict mode is on. Never use `any` — use `unknown` and narrow with type guards.
- Prefer `type` over `interface` for object shapes unless declaration merging is needed.
- Always type component props explicitly. Never rely on inferred prop types from JSX.
- Use `satisfies` operator to validate config objects against types without widening.

## React 19 patterns
- Use Server Components where possible; avoid unnecessary `'use client'` directives.
- Prefer `useCallback` / `useMemo` only when profiling shows a real cost — don't pre-optimize.
- State that derives from other state should be computed during render, not stored in `useState`.
- Use React 19's `use()` hook for async data instead of `useEffect` + fetch patterns.

## WebSocket (live dashboard)
- The engine broadcasts state every `WS_BROADCAST_INTERVAL_SECS` (default 10s).
- Handle `readyState` changes gracefully — show stale-data indicator when disconnected.
- Never mutate WebSocket message objects; treat all incoming data as readonly.

## Styling
- Tailwind CSS 4 utility classes only. No custom CSS files unless absolutely necessary.
- Use design tokens / CSS variables for colors shared across the dashboard.
- All monetary values displayed as `$X.XX` with 2 decimal places.
- Prices displayed as decimals (e.g., `0.65`) not percentages unless explicitly the P&L field.

## Charting (Recharts 3)
- Pass `isAnimationActive={false}` on performance-sensitive charts that update frequently.
- Always provide a `key` prop when re-rendering chart data to force a clean mount.

## API calls
- The Blink dashboard REST/WebSocket surface is served by the engine on port `3030` (`/api`, `/ws` in dev proxy). The separate agent JSON-RPC control plane uses port `7878`.
- Handle 503/network errors gracefully — the engine may not always be running.
- Token IDs from the engine are strings; never coerce to numbers.

## Code quality
- `npm run lint` must pass before any commit. Fix ESLint errors; never use `// eslint-disable`.
- `npm run build` (tsc + vite) must succeed — fix all TypeScript errors before committing.
