@echo off
:: ─────────────────────────────────────────────────────────────────────────────
:: tunnel.bat — SSH port-forward tunnel to Blink server with auto-reconnect
::
:: Tunnels:
::   localhost:3030  →  server:3030   (Blink engine API + dashboard)
::   localhost:7878  →  server:7878   (JSON-RPC / alpha sidecar)
::
:: Why it drops: NAT/routers kill idle TCP connections after ~30-60 seconds.
:: Fix: ServerAliveInterval=20 sends a keepalive every 20s so the connection
:: never goes idle. ExitOnForwardFailure ensures we reconnect if the tunnel
:: breaks instead of silently hanging.
::
:: Usage:
::   Double-click tunnel.bat          — uses SERVER and SSH_KEY below
::   tunnel.bat 5.161.100.38          — override server IP from command line
::   tunnel.bat 5.161.100.38 mykey    — override IP and key
:: ─────────────────────────────────────────────────────────────────────────────

:: ── CONFIG — edit these ───────────────────────────────────────────────────────
set SERVER=5.161.100.38
set SSH_USER=root
set SSH_KEY=%USERPROFILE%\.ssh\blink_hetzner
set DASHBOARD_PORT=3030
set RPC_PORT=7878
:: ─────────────────────────────────────────────────────────────────────────────

:: Allow overriding SERVER from CLI arg
if not "%~1"=="" set SERVER=%~1
if not "%~2"=="" set SSH_KEY=%~2

title Blink SSH Tunnel — %SERVER%

echo.
echo  ╔══════════════════════════════════════════════════╗
echo  ║          BLINK SSH TUNNEL — AUTO-RECONNECT       ║
echo  ╚══════════════════════════════════════════════════╝
echo.
echo  Server     : %SSH_USER%@%SERVER%
echo  Dashboard  : http://localhost:%DASHBOARD_PORT%
echo  RPC/Alpha  : http://localhost:%RPC_PORT%
echo  SSH key    : %SSH_KEY%
echo.
echo  Press Ctrl+C to stop.
echo.

:: ── Fix SSH key permissions ───────────────────────────────────────────────────
:: Windows OpenSSH kräver att BARA din användare har läsrättigheter till nyckeln.
:: Annars: "WARNING: UNPROTECTED PRIVATE KEY FILE!" → Permission denied (publickey)
if exist "%SSH_KEY%" (
    echo [%TIME%] Fixar SSH-nyckelrattigheter...
    icacls "%SSH_KEY%" /inheritance:r /grant:r "%USERNAME%:R" >nul 2>&1
    echo [%TIME%] Rattigheter OK.
    echo.
) else (
    echo.
    echo  FEL: SSH-nyckel hittades inte: %SSH_KEY%
    echo  Kontrollera att SSH_KEY stammer langst upp i den har filen.
    echo  Generera ny nyckel med:  ssh-keygen -t ed25519
    echo.
    pause
    exit /b 1
)

:reconnect
echo [%TIME%] Connecting...

ssh -N ^
    -o "BatchMode=yes" ^
    -o "ServerAliveInterval=20" ^
    -o "ServerAliveCountMax=3" ^
    -o "StrictHostKeyChecking=accept-new" ^
    -o "ConnectTimeout=15" ^
    -i "%SSH_KEY%" ^
    -L %DASHBOARD_PORT%:localhost:%DASHBOARD_PORT% ^
    -L %RPC_PORT%:localhost:%RPC_PORT% ^
    %SSH_USER%@%SERVER%

set EXIT=%ERRORLEVEL%

:: Exit code 255 = auth/connection failure
if %EXIT%==255 (
    echo.
    echo  FEL: Autentisering misslyckades!
    echo  Din publika nyckel ar troligtvis inte installerad pa servern.
    echo.
    echo  Kop nyckeln till servern (krav lossenord forsta gangen):
    echo.
    for /f "delims=" %%k in ('type "%SSH_KEY%.pub" 2^>nul') do (
        echo    ssh %SSH_USER%@%SERVER% "mkdir -p ~/.ssh ^&^& echo '%%k' ^>^> ~/.ssh/authorized_keys ^&^& chmod 600 ~/.ssh/authorized_keys"
    )
    echo.
    echo  Darefter fungerar tunnel.bat utan lossenord.
    pause
    exit /b 1
)

echo [%TIME%] Tunnel dropped (exit %EXIT%). Reconnecting in 5s...
timeout /t 5 /nobreak >nul
goto reconnect
