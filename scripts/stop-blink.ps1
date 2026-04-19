# stop-blink.ps1 — stoppar Blink Engine + Web UI + Watchdog
$repoRoot = Split-Path $PSScriptRoot -Parent
$logs = Join-Path $repoRoot "logs"

function Kill-Tree($id) {
    Get-WmiObject Win32_Process | Where-Object { $_.ParentProcessId -eq $id } |
        ForEach-Object { Kill-Tree $_.ProcessId }
    try { [System.Diagnostics.Process]::GetProcessById($id).Kill(); Write-Host "  Stoppade PID $id" } catch {}
}

foreach ($item in @(
    @{file="$logs\watchdog.pid"; name="Watchdog"},
    @{file="$logs\engine.pid";   name="Engine"},
    @{file="$logs\vite.pid";     name="Vite"}
)) {
    if (Test-Path $item.file) {
        $pid = [int]((Get-Content $item.file).Trim())
        Write-Host "🛑 Stoppar $($item.name) (PID $pid)..." -ForegroundColor Yellow
        Kill-Tree $pid
        Remove-Item $item.file -ErrorAction SilentlyContinue
    } else {
        Write-Host "  $($item.name): hittades inte" -ForegroundColor Gray
    }
}
Write-Host "✅ Klart" -ForegroundColor Green
