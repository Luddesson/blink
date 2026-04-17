# cloudflare-tunnel.ps1
# Starts a Cloudflare Quick Tunnel to expose the Blink dashboard remotely.
# No Cloudflare account required — Quick Tunnel generates a random public URL.
#
# Usage:
#   .\cloudflare-tunnel.ps1             # tunnels to port 5173 (Vite UI)
#   .\cloudflare-tunnel.ps1 -Port 3030  # tunnels to engine API instead
#
# Requirements: cloudflared must be installed.
#   winget install Cloudflare.cloudflared
#   or: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/

param([int]$Port = 5173)

# ── Locate cloudflared ────────────────────────────────────────────────────────
$cloudflared = $null
foreach ($candidate in @("cloudflared", "cloudflared.exe")) {
    try {
        $ver = & $candidate --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            $cloudflared = $candidate
            Write-Host "  cloudflared: $ver" -ForegroundColor Gray
            break
        }
    } catch {}
}

if (-not $cloudflared) {
    Write-Host ""
    Write-Host "ERROR: cloudflared not found." -ForegroundColor Red
    Write-Host ""
    Write-Host "Install it with one of:" -ForegroundColor Yellow
    Write-Host "  winget install Cloudflare.cloudflared" -ForegroundColor White
    Write-Host "  choco install cloudflared" -ForegroundColor White
    Write-Host "  or download from: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" -ForegroundColor Gray
    Write-Host ""
    exit 1
}

# ── Verify the local service is reachable ────────────────────────────────────
Write-Host ""
Write-Host "Checking http://localhost:$Port ..." -ForegroundColor Cyan
try {
    $null = Invoke-WebRequest "http://localhost:$Port" -TimeoutSec 5 -UseBasicParsing -ErrorAction Stop
    Write-Host "  Service is reachable." -ForegroundColor Green
} catch {
    Write-Host "  WARNING: http://localhost:$Port did not respond. Make sure Blink is running first." -ForegroundColor Yellow
    Write-Host "  Run: .\start-blink.ps1" -ForegroundColor Gray
}

# ── Start Quick Tunnel ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Starting Cloudflare Quick Tunnel → http://localhost:$Port" -ForegroundColor Cyan
Write-Host "Your public URL will appear below (look for 'trycloudflare.com')." -ForegroundColor Gray
Write-Host "Press Ctrl+C to stop the tunnel." -ForegroundColor Gray
Write-Host ""

# Run cloudflared in the foreground so Ctrl+C stops it cleanly.
# stderr carries the tunnel URL, so we merge stderr into stdout.
& $cloudflared tunnel --url "http://localhost:$Port" 2>&1
