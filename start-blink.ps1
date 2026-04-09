# start-blink.ps1
# Usage: .\start-blink.ps1                  (release build, auto if needed)
#        .\start-blink.ps1 -Debug            (debug build, fast compile)
#        .\start-blink.ps1 -SkipBuild        (skip cargo, use existing binary)
#        .\start-blink.ps1 -Watch            (auto-restart engine if it crashes)
#        .\start-blink.ps1 -Debug -Watch     (debug + watchdog loop)
param([switch]$Debug, [switch]$Watch, [switch]$SkipBuild)
$root = $PSScriptRoot
$logs = "$root\logs"
New-Item -ItemType Directory -Force -Path $logs | Out-Null

function Kill-Tree($id) {
    Get-WmiObject Win32_Process | Where-Object { $_.ParentProcessId -eq $id } |
        ForEach-Object { Kill-Tree $_.ProcessId }
    try { [System.Diagnostics.Process]::GetProcessById($id).Kill() } catch {}
}

function Start-EngineProcess($rootPath, $profile, $engineLogPath) {
    $engineArgs = "-NoProfile -Command " +
        "Set-Location '$rootPath\blink-engine'; " +
        "`$env:WEB_UI='true'; `$env:WEB_UI_PORT='3030'; " +
        "`$env:PAPER_TRADING='true'; `$env:TUI='false'; " +
        ".\target\$profile\engine.exe >> '$engineLogPath' 2>&1"
    return Start-Process powershell -ArgumentList $engineArgs -WindowStyle Hidden -PassThru
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
    $p2 = Start-Process "npm" -ArgumentList "install" -NoNewWindow -Wait -PassThru
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
$ep = Start-EngineProcess -rootPath $root -profile $buildProfile -engineLogPath $engineLog
$ep.Id | Out-File "$logs\engine.pid"

# ── [4/4] Start Vite ──────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== [4/4] Starting Web UI ===" -ForegroundColor Cyan
$viteLog = "$logs\vite-stdout.log"
"" | Set-Content $viteLog
$viteArgs = "-NoProfile -Command " +
    "Set-Location '$root\blink-ui'; " +
    "npm run dev >> '$viteLog' 2>&1"
$vp = Start-Process powershell -ArgumentList $viteArgs -WindowStyle Hidden -PassThru
$vp.Id | Out-File "$logs\vite.pid"

# ── Wait for engine ───────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Waiting for engine (up to 120s)..." -ForegroundColor Yellow
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep 2
    try {
        $health = Invoke-RestMethod "http://localhost:3030/health" -TimeoutSec 5
        if ($health.status -eq "ok") {
            $ready = $true
            break
        }
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
    while ($true) {
        Start-Sleep 5

        # Check if engine is still responding
        $alive = $false
        try {
            $null = Invoke-RestMethod "http://localhost:3030/api/status" -TimeoutSec 5
            $alive = $true
        } catch {}

        if (-not $alive) {
            $restartCount++
            $ts = Get-Date -Format "HH:mm:ss"
            Write-Host "[$ts] Engine not responding — restart #$restartCount" -ForegroundColor Yellow

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
                $ep2 = Start-EngineProcess -rootPath $root -profile $buildProfile -engineLogPath $engineLog
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
}
