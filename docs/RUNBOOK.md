# Blink Local Runbook

Practical commands for clean local operation.

## Start / Stop

From repository root:

```powershell
.\start-blink.ps1
.\stop-blink.ps1
```

## Rebuild UI Only

```powershell
cd blink-ui
npm run build
```

## Rebuild Engine Only

```powershell
cd blink-engine
cargo build --release
```

## Clean Up Local Artifacts

```powershell
git clean -fdX
```

Use this only for ignored files (build outputs/logs), not tracked changes.

## Merge Conflict Recovery Checklist

1. Search for conflict markers:

```powershell
rg "^(<<<<<<<|=======|>>>>>>>)" .
```

2. Resolve each conflict by selecting/combining the intended logic.
3. Re-run validation:
   - `npm run build` in `blink-ui`
   - `cargo build` in `blink-engine` (if backend files changed)

## Log Locations

- `logs\engine-stdout.log`
- `logs\vite-stdout.log`
