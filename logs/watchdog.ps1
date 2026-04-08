$engineLauncher = 'C:\Users\ludvi\Documents\GitHub\blink\logs\run-engine.ps1'
$engineLog      = 'C:\Users\ludvi\Documents\GitHub\blink\logs\engine-stdout.log'
$logs           = 'C:\Users\ludvi\Documents\GitHub\blink\logs'
$failStreak     = 0
$failThreshold  = 3
$restartCount   = 0
while ($true) {
    Start-Sleep 5
    $alive = $false
    try {
        $null = Invoke-RestMethod 'http://localhost:3030/api/status' -TimeoutSec 8
        $alive = $true
    } catch {}

    if ($alive) { $failStreak = 0; continue }

    $failStreak++
    $ts = Get-Date -Format 'HH:mm:ss'
    if ($failStreak -lt $failThreshold) {
        "[$ts] Engine slow (streak $failStreak/$failThreshold)" | Out-File -Append "$logs\watchdog.log"
        continue
    }

    $failStreak = 0
    $restartCount++
    "[$ts] Engine DOWN -- restart #$restartCount" | Out-File -Append "$logs\watchdog.log"

    # Check for panic sentinel
    $panicFile = 'C:\Users\ludvi\Documents\GitHub\blink\blink-engine\logs\paper_portfolio_state.json.panic'
    if (Test-Path $panicFile) {
        $panicMsg = Get-Content $panicFile -Raw
        "[$ts] PANIC: $panicMsg" | Out-File -Append "$logs\watchdog.log"
        Remove-Item $panicFile -ErrorAction SilentlyContinue
    }

    # Only restart if port is free
    $portInUse = (netstat -an | Select-String '0.0.0.0:3030.*LISTENING') -ne $null
    if (-not $portInUse) {
        $ep = Start-Process powershell -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$engineLauncher`"" -WindowStyle Hidden -PassThru
        $ep.Id | Out-File "$logs\engine.pid"
        "[$ts] Engine restarted (PID $($ep.Id))" | Out-File -Append "$logs\watchdog.log"

        $back = $false
        for ($j = 0; $j -lt 15; $j++) {
            Start-Sleep 2
            try { $null = Invoke-RestMethod 'http://localhost:3030/api/status' -TimeoutSec 5; $back = $true; break } catch {}
        }
        if ($back) { "[$ts] Engine back online." | Out-File -Append "$logs\watchdog.log" }
        else { "[$ts] Engine failed to restart -- see $engineLog" | Out-File -Append "$logs\watchdog.log" }
    }
}
