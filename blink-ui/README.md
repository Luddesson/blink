# blink-ui

React + Vite dashboard for Blink engine monitoring and controls.

## Requirements

- Node.js 18+
- npm

## Local Development

```bash
npm install
npm run dev
```

Default dev URL: `http://localhost:5173`

## Production Build

```bash
npm run build
```

This runs:

1. TypeScript project build (`tsc -b`)
2. Vite production bundle (`vite build`)

## Lint

```bash
npm run lint
```

## Runtime Expectations

The UI expects the Blink engine API at `http://localhost:3030` and consumes:

- REST endpoints under `/api/*`
- real-time snapshots via `/ws`

## Common Failures

- **TypeScript parse errors at `<<<<<<<` / `=======` / `>>>>>>>`:** unresolved git conflict markers are present.
- **`npm run build` type mismatch errors after merges:** resolve conflicts first, then rerun build to catch strict typing regressions.

## Related Files

- `src/App.tsx` - app shell and tab routing
- `src/lib/api.ts` - API client
- `src/types.ts` - shared frontend API types
