#!/usr/bin/env pwsh
param(
    [switch]$Apply,
    [switch]$NoRestart,
    [switch]$SkipSidecar,
    [string]$Server = "root@5.161.100.38",
    [string]$BlinkDir = "/opt/blink",
    [string]$SshConfig
)

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $SshConfig) {
    $SshConfig = Join-Path $scriptDir "ssh\blink-ssh-config"
}

function Invoke-Remote([string]$command) {
    $sshArgs = @()
    if (Test-Path $SshConfig) {
        $sshArgs += @("-F", $SshConfig)
    }
    $sshArgs += @($Server, $command)
    & ssh @sshArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Remote command failed with exit code $LASTEXITCODE"
    }
}

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$envFile = "$BlinkDir/.env"
$backupFile = "$BlinkDir/.env.rollback.$timestamp.bak"
$restartFlag = if ($NoRestart) { "0" } else { "1" }
$restartSidecarFlag = if ($SkipSidecar) { "0" } else { "1" }

$remoteScript = @"
set -euo pipefail
ENV_FILE='$envFile'
BACKUP_FILE='$backupFile'
RESTART='$restartFlag'
RESTART_SIDECAR='$restartSidecarFlag'

if [ ! -f "\$ENV_FILE" ]; then
  echo "ERROR: env file not found at \$ENV_FILE" >&2
  exit 1
fi

sudo cp "\$ENV_FILE" "\$BACKUP_FILE"
echo "Backup created: \$BACKUP_FILE"

set_or_append() {
  local key="\$1"
  local value="\$2"
  if grep -q "^\${key}=" "\$ENV_FILE"; then
    sudo sed -i "s/^\${key}=.*/\${key}=\${value}/" "\$ENV_FILE"
  else
    echo "\${key}=\${value}" | sudo tee -a "\$ENV_FILE" >/dev/null
  fi
}

set_or_append "TRADING_ENABLED" "false"
set_or_append "LIVE_TRADING" "false"
set_or_append "PAPER_TRADING" "true"
set_or_append "ALPHA_TRADING_ENABLED" "false"

echo "Applied rollback env values:"
grep -E '^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|ALPHA_TRADING_ENABLED)=' "\$ENV_FILE"

if [ "\$RESTART" = "1" ]; then
  sudo systemctl restart blink-engine
  if [ "\$RESTART_SIDECAR" = "1" ] && systemctl list-unit-files | grep -q '^blink-sidecar\.service'; then
    sudo systemctl restart blink-sidecar
  fi
fi

echo "Service states:"
systemctl is-active blink-engine
if systemctl list-unit-files | grep -q '^blink-sidecar\.service'; then
  systemctl is-active blink-sidecar
fi

echo "Engine API check:"
curl -sf http://127.0.0.1:3030/api/status >/dev/null && echo "api_status=ok" || echo "api_status=unreachable"
"@

$restartCmd = if (-not $NoRestart) { "sudo systemctl restart blink-engine" } else { "skip restart (--NoRestart)" }
$sidecarCmd = if (-not $NoRestart -and -not $SkipSidecar) {
    "sudo systemctl restart blink-sidecar (if installed)"
} elseif (-not $NoRestart) {
    "skip sidecar restart (--SkipSidecar)"
} else {
    "sidecar restart skipped (no restart)"
}

$previewCommands = @(
    "sudo cp $envFile $backupFile",
    "set TRADING_ENABLED=false",
    "set LIVE_TRADING=false",
    "set PAPER_TRADING=true",
    "set ALPHA_TRADING_ENABLED=false",
    $restartCmd,
    $sidecarCmd,
    "verify env + systemctl is-active + curl http://127.0.0.1:3030/api/status"
)

Write-Host "=== Blink rollback helper ===" -ForegroundColor Cyan
Write-Host "Server: $Server"
Write-Host "Env:    $envFile"
Write-Host ""

if (-not $Apply) {
    Write-Host "Preview mode (non-destructive). Planned actions:" -ForegroundColor Yellow
    $previewCommands | ForEach-Object { Write-Host "  - $_" }
    Write-Host ""
    Write-Host "Current remote state snapshot:" -ForegroundColor Cyan
    Invoke-Remote @"
set -e
echo "env:"
grep -E '^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|ALPHA_TRADING_ENABLED)=' '$envFile' || true
echo ""
echo "services:"
systemctl is-active blink-engine || true
if systemctl list-unit-files | grep -q '^blink-sidecar\.service'; then systemctl is-active blink-sidecar || true; fi
"@
    Write-Host ""
    Write-Host "Run with -Apply to execute rollback." -ForegroundColor Green
    exit 0
}

Write-Host "Apply mode enabled. Executing rollback..." -ForegroundColor Yellow
Invoke-Remote $remoteScript
Write-Host "Rollback complete." -ForegroundColor Green
