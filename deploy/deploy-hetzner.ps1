# ─────────────────────────────────────────────────────────────────
#  Blink Engine — Deploy to Hetzner (Windows PowerShell)
#  Usage:
#    .\deploy-hetzner.ps1 -FirstRun     # First time setup
#    .\deploy-hetzner.ps1               # Update & restart
#    .\deploy-hetzner.ps1 -EnvOnly      # Push .env only
#    .\deploy-hetzner.ps1 -Logs         # Tail engine logs
#    .\deploy-hetzner.ps1 -Status       # Check service status
#    .\deploy-hetzner.ps1 -Tunnel       # SSH tunnel to dashboard
# ─────────────────────────────────────────────────────────────────
param(
    [switch]$FirstRun,
    [switch]$EnvOnly,
    [switch]$Logs,
    [switch]$Status,
    [switch]$Tunnel,
    [switch]$Stop,
    [switch]$Restart
)

$SERVER = "root@5.161.100.38"
$BLINK_DIR = "/opt/blink"
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$REPO_ROOT = (Resolve-Path "$SCRIPT_DIR\..").Path
$ENGINE_DIR = "$REPO_ROOT\blink-engine"
$SSH_CONFIG = "$SCRIPT_DIR\ssh\blink-ssh-config"

# ── Quick commands ──────────────────────────────────────────────

if ($Logs) {
    Write-Host "Tailing engine logs (Ctrl+C to stop)..." -ForegroundColor Cyan
    ssh -F $SSH_CONFIG $SERVER "journalctl -u blink-engine -f --no-pager"
    exit 0
}

if ($Status) {
    ssh -F $SSH_CONFIG $SERVER @"
echo '=== Services ==='
systemctl is-active blink-engine blink-sidecar
echo ''
echo '=== Memory ==='
free -h
echo ''
echo '=== Engine Health ==='
curl -sf http://127.0.0.1:3030/api/status 2>/dev/null | jq . || echo 'API not responding'
echo ''
echo '=== Recent Logs ==='
journalctl -u blink-engine --no-pager -n 10
"@
    exit 0
}

if ($Tunnel) {
    Write-Host "Opening SSH tunnel — Dashboard at http://localhost:3030" -ForegroundColor Cyan
    Write-Host "Press Ctrl+C to close tunnel" -ForegroundColor Yellow
    ssh -F $SSH_CONFIG -N -L 3030:localhost:3030 $SERVER
    exit 0
}

if ($Stop) {
    ssh -F $SSH_CONFIG $SERVER "systemctl stop blink-sidecar blink-engine && echo 'Services stopped'"
    exit 0
}

if ($Restart) {
    ssh -F $SSH_CONFIG $SERVER "systemctl restart blink-engine && sleep 3 && systemctl restart blink-sidecar && echo 'Services restarted'"
    exit 0
}

# ── Push .env only ──────────────────────────────────────────────

if ($EnvOnly) {
    Write-Host "Pushing .env to server..." -ForegroundColor Cyan
    scp -F $SSH_CONFIG "$ENGINE_DIR\.env" "${SERVER}:${BLINK_DIR}/.env"
    ssh -F $SSH_CONFIG $SERVER "chown blink:blink ${BLINK_DIR}/.env && chmod 600 ${BLINK_DIR}/.env"
    Write-Host "Done. Restart with: .\deploy-hetzner.ps1 -Restart" -ForegroundColor Green
    exit 0
}

# ── Full deploy ─────────────────────────────────────────────────

Write-Host "══════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  Blink Engine — Deploy to Hetzner CPX11" -ForegroundColor Cyan
Write-Host "══════════════════════════════════════════════" -ForegroundColor Cyan

if ($FirstRun) {
    Write-Host "`n[1/7] Provisioning server..." -ForegroundColor Yellow
    Get-Content "$SCRIPT_DIR\hetzner\provision.sh" | ssh -F $SSH_CONFIG $SERVER 'bash -s'

    Write-Host "`n[2/7] Pushing repository..." -ForegroundColor Yellow
    # Push the entire repo to server (excluding .env and large files)
    ssh -F $SSH_CONFIG $SERVER "mkdir -p ${BLINK_DIR}/src"
    
    # Use git to push — the server needs the repo
    Write-Host "  Syncing code via rsync..."
    # Create exclude file
    $excludeFile = [System.IO.Path]::GetTempFileName()
    @"
.env
*.key
*.key.pub
target/
node_modules/
logs/
data/
__pycache__/
*.egg-info/
_ARCHIVE/
Screenshots/
"@ | Set-Content $excludeFile

    rsync -avz --delete --exclude-from="$excludeFile" -e "ssh -F $SSH_CONFIG" "$REPO_ROOT/" "${SERVER}:${BLINK_DIR}/src/"
    Remove-Item $excludeFile

    Write-Host "`n[3/7] Pushing .env (secrets)..." -ForegroundColor Yellow
    scp -F $SSH_CONFIG "$ENGINE_DIR\.env" "${SERVER}:${BLINK_DIR}/.env"
    ssh -F $SSH_CONFIG $SERVER "chown blink:blink ${BLINK_DIR}/.env && chmod 600 ${BLINK_DIR}/.env"
} else {
    Write-Host "`n[1/7] Syncing code changes..." -ForegroundColor Yellow
    $excludeFile = [System.IO.Path]::GetTempFileName()
    @"
.env
*.key
*.key.pub
target/
node_modules/
logs/
data/
__pycache__/
*.egg-info/
_ARCHIVE/
Screenshots/
"@ | Set-Content $excludeFile

    rsync -avz --delete --exclude-from="$excludeFile" -e "ssh -F $SSH_CONFIG" "$REPO_ROOT/" "${SERVER}:${BLINK_DIR}/src/"
    Remove-Item $excludeFile
}

Write-Host "`n[4/7] Building engine on server (5-10 min first time, ~2 min updates)..." -ForegroundColor Yellow
ssh -F $SSH_CONFIG $SERVER @"
source /root/.cargo/env 2>/dev/null || true
# Also set it up for blink user
if [ ! -f /home/blink/.cargo/env ]; then
    su - blink -c 'curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable' 2>/dev/null
fi
su - blink -c 'source ~/.cargo/env && cd ${BLINK_DIR}/src/blink-engine && cargo build --release -p engine 2>&1 | tail -10'
"@

Write-Host "`n[5/7] Deploying binary and sidecar..." -ForegroundColor Yellow
ssh -F $SSH_CONFIG $SERVER @"
set -e
# Copy engine binary
cp ${BLINK_DIR}/src/blink-engine/target/release/engine ${BLINK_DIR}/engine
chmod +x ${BLINK_DIR}/engine

# Build and copy active web UI static assets
if [ -d "${BLINK_DIR}/src/blink-ui" ]; then
    cd ${BLINK_DIR}/src/blink-ui
    if command -v npm &>/dev/null; then
        npm ci --silent 2>/dev/null && npm run build --silent 2>/dev/null
        mkdir -p ${BLINK_DIR}/static/ui/assets
        rm -f ${BLINK_DIR}/static/ui/index.html
        rm -f ${BLINK_DIR}/static/ui/assets/*
        cp -r ${BLINK_DIR}/src/blink-engine/static/ui/* ${BLINK_DIR}/static/ui/ 2>/dev/null || true
    fi
fi

# Copy alpha sidecar
rsync -a --delete ${BLINK_DIR}/src/blink-engine/alpha-sidecar/ ${BLINK_DIR}/alpha-sidecar/

# Install sidecar deps
${BLINK_DIR}/sidecar-venv/bin/pip install -q -e ${BLINK_DIR}/alpha-sidecar/ 2>/dev/null

# Fix ownership
chown -R blink:blink ${BLINK_DIR}
echo 'Deploy complete'
"@

Write-Host "`n[6/7] Restarting services..." -ForegroundColor Yellow
ssh -F $SSH_CONFIG $SERVER "systemctl restart blink-engine && sleep 3 && systemctl restart blink-sidecar"

Write-Host "`n[7/7] Verifying..." -ForegroundColor Yellow
Start-Sleep 5
ssh -F $SSH_CONFIG $SERVER @"
echo '  Engine:  '`systemctl is-active blink-engine`
echo '  Sidecar: '`systemctl is-active blink-sidecar`
echo '  Memory:  '`free -h | grep Mem | awk '{print `$3 "/" `$2}'`
echo ''
if curl -sf http://127.0.0.1:3030/api/status > /dev/null 2>&1; then
    echo '  ✅ Engine API responding'
else
    echo '  ⚠️  Engine still starting (check: .\deploy-hetzner.ps1 -Logs)'
fi
"@

Write-Host ""
Write-Host "══════════════════════════════════════════════" -ForegroundColor Green
Write-Host "  Deploy complete!" -ForegroundColor Green
Write-Host ""
Write-Host "  Dashboard:  .\deploy-hetzner.ps1 -Tunnel" -ForegroundColor White
Write-Host "  Logs:       .\deploy-hetzner.ps1 -Logs" -ForegroundColor White
Write-Host "  Status:     .\deploy-hetzner.ps1 -Status" -ForegroundColor White
Write-Host "  Restart:    .\deploy-hetzner.ps1 -Restart" -ForegroundColor White
Write-Host "══════════════════════════════════════════════" -ForegroundColor Green
