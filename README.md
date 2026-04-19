# Blink Workspace

Unified workspace for:

- `blink-engine` (Rust backend, APIs, execution logic)
- `blink-ui` (React/Vite dashboard)
- root PowerShell helpers for local start/stop flows

## Quick Start (Windows)

From repo root:

```powershell
.\scripts\start-blink.ps1
```

What this does:

1. Builds the engine (release by default, smart-skip when unchanged)
2. Installs `blink-ui` deps if missing
3. Starts engine on `http://localhost:3030`
4. Starts UI on `http://localhost:5173`

Stop everything:

```powershell
.\scripts\stop-blink.ps1
```

## Useful Start Script Modes

```powershell
.\scripts\start-blink.ps1 -Debug
.\scripts\start-blink.ps1 -SkipBuild
.\scripts\start-blink.ps1 -Watch
.\scripts\start-blink.ps1 -Debug -Watch
```

## Project Structure

```text
Blink/
  blink-engine/      # Rust trading engine + web server
  blink-ui/          # React UI client
  logs/              # runtime logs / pid files
  scripts/start-blink.ps1  # local orchestrator
  scripts/stop-blink.ps1   # local stop helper
```

## Build & Verify

Frontend:

```powershell
cd blink-ui
npm run build
```

Backend:

```powershell
cd blink-engine
cargo build
```

## Troubleshooting

- If the UI fails with TS parser errors mentioning `<<<<<<<`, `=======`, or `>>>>>>>`, unresolved merge markers are present.
- Resolve markers, then re-run:

```powershell
cd blink-ui
npm run build
```

- If startup hangs, inspect:
  - `logs\engine-stdout.log`
  - `logs\vite-stdout.log`

## Notes

- Local editor/session files and build artifacts are ignored in `.gitignore`.
- Keep secrets in `.env` files only; never commit credentials.
