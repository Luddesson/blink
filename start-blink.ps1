# start-blink.ps1
# Usage: .\start-blink.ps1                  (auto-detect build, release)
#        .\start-blink.ps1 -Debug            (debug build, fast compile)
#        .\start-blink.ps1 -SkipBuild        (skip cargo entirely, run existing binary)
#        .\start-blink.ps1 -Watch            (AFK mode — auto-restart engine on crash)
#        .\start-blink.ps1 -SkipBuild -Watch (fast AFK restart — no rebuild)
param([switch]$Debug, [switch]$Watch, [switch]$SkipBuild)
$root = $PSScriptRoot
$logs = "$root\logs"
New-Item -ItemType Directory -Force -Path $logs | Out-Null

function Kill-Tree($id) {
    Get-WmiObject Win32_Process | Where-Object { $_.ParentProcessId -eq $id } |
        ForEach-Object { Kill-Tree $_.ProcessId }
    try { [System.Diagnostics.Process]::GetProcessById($id).Kill() } catch {}
}

# Stop old processes
Write-Host "Stopping any running instances..." -ForegroundColor Yellow
foreach ($pidFile in @("$logs\engine.pid", "$logs\vite.pid")) {
    if (Test-Path $pidFile) {
        $old = [int]((Get-Content $pidFile).Trim())
        Kill-Tree $old
        Remove-Item $pidFile -ErrorAction SilentlyContinue
    }
}
# Kill engine.exe by name in case pid file is stale
Get-Process | Where-Object { $_.Name -eq "engine" } | ForEach-Object {
    try { $_.Kill() } catch {}
}
Start-Sleep 4

# ── [1/4] Build Engine ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== [1/4] Building engine (cargo build) ===" -ForegroundColor Cyan
Push-Location "$root\blink-engine"
$cargoArgs = if ($Debug) { @("build") } else { @("build", "--release") }
$buildProfile = if ($Debug) { "debug" } else { "release" }

# Smart skip: bypass cargo if binary is newer than all source files
$binaryPath = "$root\blink-engine\target\$buildProfile\engine.exe"
$needsBuild = $true
if ($SkipBuild) {
    Write-Host "  -SkipBuild flag set — skipping cargo build" -ForegroundColor Green
    $needsBuild = $false
} elseif (Test-Path $binaryPath) {
    $binaryTime = (Get-Item $binaryPath).LastWriteTime
    $sourceFiles = Get-ChildItem "$root\blink-engine" -Recurse -Include "*.rs","*.toml" -File |
                   Where-Object { $_.FullName -notlike "*\target\*" }
    $latestSource = ($sourceFiles | Sort-Object LastWriteTime -Descending | Select-Object -First 1).LastWriteTime
    if ($latestSource -and $binaryTime -gt $latestSource) {
        Write-Host "  Binary is up to date ($(Split-Path $binaryPath -Leaf) newer than all sources) — skipping build" -ForegroundColor Green
        $needsBuild = $false
    }
}

if ($needsBuild) {
    $p = Start-Process "cargo" -ArgumentList $cargoArgs -NoNewWindow -Wait -PassThru
    Pop-Location
    if ($p.ExitCode -ne 0) {
        Write-Host "ERROR: cargo build failed (exit code $($p.ExitCode))" -ForegroundColor Red
        exit 1
    }
    Write-Host "Engine build OK" -ForegroundColor Green
} else {
    Pop-Location
    Write-Host "Engine build skipped" -ForegroundColor Green
}

# ── [2/4] Frontend deps ───────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== [2/4] Frontend dependencies ===" -ForegroundColor Cyan
if (-not (Test-Path "$root\blink-ui\node_modules")) {
    Push-Location "$root\blink-ui"
    $p2 = Start-Process "cmd" -ArgumentList "/c npm install" -NoNewWindow -Wait -PassThru
    Pop-Location
    if ($p2.ExitCode -ne 0) {
        Write-Host "ERROR: npm install failed" -ForegroundColor Red
        exit 1
    }
}
Write-Host "Frontend deps OK" -ForegroundColor Green

# ── [3/4] Start Engine ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== [3/4] Starting engine ===" -ForegroundColor Cyan
$engineLog = "$logs\engine-stdout.log"
"" | Set-Content $engineLog

# Write a tiny launcher script so env vars are set reliably
$engineLauncher = "$logs\run-engine.ps1"
@"
Set-Location '$root\blink-engine'
`$env:WEB_UI = 'true'
`$env:WEB_UI_PORT = '3030'
`$env:PAPER_TRADING = 'true'
`$env:TRADING_ENABLED = 'true'
`$env:WS_BROADCAST_INTERVAL_SECS = '1'
`$env:VAR_THRESHOLD_PCT = '0.50'
`$env:MIN_SIGNAL_NOTIONAL_USD = '5.0'
`$env:TUI = 'false'
& '.\target\$buildProfile\engine.exe' > '$engineLog' 2>&1
"@ | Set-Content $engineLauncher

# Check if port 3030 is already in use (e.g., from a previous run that wasn't stopped)
$portInUse = (netstat -an | Select-String "0.0.0.0:3030.*LISTENING") -ne $null
if ($portInUse) {
    Write-Host "  Port 3030 already in use — skipping engine launch (already running?)" -ForegroundColor Yellow
} else {
    $ep = Start-Process powershell -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$engineLauncher`"" -WindowStyle Hidden -PassThru
    $ep.Id | Out-File "$logs\engine.pid"
}

# ── [4/4] Start Vite ──────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== [4/4] Starting Web UI ===" -ForegroundColor Cyan
$viteLog = "$logs\vite-stdout.log"
"" | Set-Content $viteLog
$viteBat = "$logs\run-vite.bat"
"@echo off`ncd /d `"$root\blink-ui`"`nnpm run dev >> `"$viteLog`" 2>&1" | Set-Content $viteBat
$vp = Start-Process "cmd" -ArgumentList "/c `"$viteBat`"" -WindowStyle Hidden -PassThru
$vp.Id | Out-File "$logs\vite.pid"

# ── Wait for engine ───────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Waiting for engine (up to 120s)..." -ForegroundColor Yellow
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep 2
    try {
        $null = Invoke-RestMethod "http://localhost:3030/api/status" -TimeoutSec 10
        $ready = $true
        break
    } catch {}
}

Write-Host ""
if ($ready) {
    Write-Host "================================================" -ForegroundColor Green
    Write-Host "  Blink is running!" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Dashboard : http://localhost:5173" -ForegroundColor White
    Write-Host "  API       : http://localhost:3030/api/status" -ForegroundColor White
    Write-Host "  Logs      : $logs\" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  To stop   : .\stop-blink.ps1" -ForegroundColor Gray
    Write-Host "================================================" -ForegroundColor Green
    Start-Process "http://localhost:5173"
} else {
    Write-Host "ERROR: Engine did not respond. Check: $engineLog" -ForegroundColor Red
    exit 1
}

# ── Watch mode: auto-restart engine on crash ──────────────────────────────────
if ($Watch) {
    Write-Host ""
    Write-Host "================================================" -ForegroundColor Magenta
    Write-Host "  WATCH MODE: Auto-restart on crash (AFK safe)" -ForegroundColor Magenta
    Write-Host "  Press Ctrl+C to stop." -ForegroundColor Gray
    Write-Host "================================================" -ForegroundColor Magenta

    $restartCount = 0
    $failStreak = 0
    $failThreshold = 3  # require 3 consecutive failures before restarting
    while ($true) {
        Start-Sleep 5

        # Check if engine is still responding
        $alive = $false
        try {
            $null = Invoke-RestMethod "http://localhost:3030/api/status" -TimeoutSec 8
            $alive = $true
        } catch {}

        if ($alive) {
            $failStreak = 0  # reset streak on any successful response
            continue
        }

        $failStreak++
        $ts = Get-Date -Format "HH:mm:ss"
        if ($failStreak -lt $failThreshold) {
            Write-Host "[$ts] Engine slow/unreachable (streak $failStreak/$failThreshold) — waiting..." -ForegroundColor Gray
            continue
        }

        # Three consecutive failures — treat as a real crash
        $failStreak = 0
        $restartCount++
        Write-Host "[$ts] Engine not responding after $failThreshold checks — restart #$restartCount" -ForegroundColor Yellow

            # Check for panic sentinel file
            $panicFile = "$root\blink-engine\logs\paper_portfolio_state.json.panic"
            if (Test-Path $panicFile) {
                $panicMsg = Get-Content $panicFile -Raw
                Write-Host "  PANIC detected: $panicMsg" -ForegroundColor Red
                Remove-Item $panicFile -ErrorAction SilentlyContinue
            }

            # Restart engine only (Vite stays running)
            $portInUse = (netstat -an | Select-String "0.0.0.0:3030.*LISTENING") -ne $null
            if (-not $portInUse) {
                $ep2 = Start-Process powershell -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$engineLauncher`"" -WindowStyle Hidden -PassThru
                $ep2.Id | Out-File "$logs\engine.pid"
                Write-Host "  Engine restarted (PID $($ep2.Id))" -ForegroundColor Green

                # Wait up to 30s for it to come back
                $back = $false
                for ($j = 0; $j -lt 15; $j++) {
                    Start-Sleep 2
                    try {
                        $null = Invoke-RestMethod "http://localhost:3030/api/status" -TimeoutSec 5
                        $back = $true; break
                    } catch {}
                }
                if ($back) {
                    Write-Host "  Engine is back online." -ForegroundColor Green
                } else {
                    Write-Host "  Engine failed to restart — check $engineLog" -ForegroundColor Red
                }
            }
    }
}