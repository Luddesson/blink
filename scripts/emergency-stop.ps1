#!/usr/bin/env pwsh
# EMERGENCY STOP — Cancels all orders via Bullpen CLI and stops Blink engine.
#
# Usage:
#   ./emergency-stop.ps1              # Normal stop
#   ./emergency-stop.ps1 -Force       # Force kill engine process
#   ./emergency-stop.ps1 -SkipBullpen # Skip Bullpen cancel, engine only

param(
    [switch]$Force,
    [switch]$SkipBullpen
)

$ErrorActionPreference = "Continue"
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

Write-Host "`n`e[91m🚨 EMERGENCY STOP initiated at $timestamp`e[0m`n" 

# ── Layer 1: Cancel via Bullpen CLI (independent of Blink process) ────────
if (-not $SkipBullpen) {
    Write-Host "  [1/3] Cancelling all orders via Bullpen CLI..." -ForegroundColor Yellow
    try {
        $result = wsl -d Ubuntu -- bullpen polymarket orders --cancel-all --yes --output json 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  ✅ Bullpen cancel succeeded" -ForegroundColor Green
        } else {
            Write-Host "  ⚠️  Bullpen cancel returned exit code $LASTEXITCODE" -ForegroundColor Red
            Write-Host "     $result" -ForegroundColor DarkGray
        }
    } catch {
        Write-Host "  ⚠️  Bullpen cancel failed: $_" -ForegroundColor Red
    }
} else {
    Write-Host "  [1/3] Skipping Bullpen cancel (--SkipBullpen)" -ForegroundColor DarkGray
}

# ── Layer 2: Stop Blink engine ────────────────────────────────────────────
Write-Host "  [2/3] Stopping Blink engine..." -ForegroundColor Yellow
try {
    $stopScript = Join-Path $PSScriptRoot "stop-blink.ps1"
    if (Test-Path $stopScript) {
        & $stopScript
        Write-Host "  ✅ Blink engine stopped" -ForegroundColor Green
    } else {
        # Fallback: kill engine process directly
        $engineProc = Get-Process -Name "engine" -ErrorAction SilentlyContinue
        if ($engineProc) {
            Stop-Process -Id $engineProc.Id -Force
            Write-Host "  ✅ Engine process killed (PID $($engineProc.Id))" -ForegroundColor Green
        } else {
            Write-Host "  ℹ️  No engine process found" -ForegroundColor DarkGray
        }
    }
} catch {
    Write-Host "  ⚠️  Engine stop failed: $_" -ForegroundColor Red
    if ($Force) {
        Write-Host "  [FORCE] Killing all engine processes..." -ForegroundColor Red
        Get-Process -Name "engine" -ErrorAction SilentlyContinue | ForEach-Object {
            Stop-Process -Id $_.Id -Force
        }
    }
}

# ── Layer 3: Verify clean state ──────────────────────────────────────────
if (-not $SkipBullpen) {
    Write-Host "  [3/3] Verifying no open orders..." -ForegroundColor Yellow
    try {
        $orders = wsl -d Ubuntu -- bullpen polymarket orders --output json 2>$null | ConvertFrom-Json
        if ($null -eq $orders -or $orders.Count -eq 0) {
            Write-Host "  ✅ No open orders — clean state confirmed" -ForegroundColor Green
        } else {
            Write-Host "  ⚠️  $($orders.Count) orders still open!" -ForegroundColor Red
            if ($Force) {
                Write-Host "  [FORCE] Retrying cancel..." -ForegroundColor Red
                wsl -d Ubuntu -- bullpen polymarket orders --cancel-all --yes 2>$null
            }
        }
    } catch {
        Write-Host "  ⚠️  Verification failed: $_" -ForegroundColor Red
    }
} else {
    Write-Host "  [3/3] Skipping verification (--SkipBullpen)" -ForegroundColor DarkGray
}

# ── Log ───────────────────────────────────────────────────────────────────
$logDir = Join-Path $PSScriptRoot "logs"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$logEntry = "$timestamp | EMERGENCY_STOP | Force=$($Force.IsPresent) | SkipBullpen=$($SkipBullpen.IsPresent)"
Add-Content -Path (Join-Path $logDir "emergency_stop.log") -Value $logEntry

Write-Host "`n`e[91m🛑 Emergency stop complete`e[0m`n"
