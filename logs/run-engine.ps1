Set-Location 'C:\Users\ludvi\Documents\GitHub\blink\blink-engine'
$env:WEB_UI = 'true'
$env:WEB_UI_PORT = '3030'
$env:PAPER_TRADING = 'true'
$env:TRADING_ENABLED = 'true'
$env:WS_BROADCAST_INTERVAL_SECS = '2'
$env:VAR_THRESHOLD_PCT = '0.50'
$env:TUI = 'false'
& '.\target\debug\engine.exe' 2>&1 | Tee-Object -FilePath 'C:\Users\ludvi\Documents\GitHub\blink\logs\engine-stdout.log' -Append
