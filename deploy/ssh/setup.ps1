# Blink SSH bootstrap — run once per machine (Windows).
#
# What it does:
#   1. Ensures ~/.ssh/blink_hetzner exists (generates ed25519 if missing, no passphrase)
#   2. Locks ACLs on the private key so OpenSSH accepts it
#   3. Adds an Include line to ~/.ssh/config pointing at blink-ssh-config in this repo
#   4. Copies the public key into deploy/ssh/authorized_keys.d/<hostname>.pub
#
# After running: commit the new .pub file, push, then run
#   bash deploy/ssh/sync-authorized-keys.sh
# from an already-authorized machine.

$ErrorActionPreference = 'Stop'

$scriptDir  = $PSScriptRoot
$sshDir     = Join-Path $env:USERPROFILE '.ssh'
$privateKey = Join-Path $sshDir 'blink_hetzner'
$publicKey  = "$privateKey.pub"
$sshConfig  = Join-Path $sshDir 'config'
$repoConfig = Join-Path $scriptDir 'blink-ssh-config'
$authDir    = Join-Path $scriptDir 'authorized_keys.d'

# 1. ~/.ssh must exist with tight ACLs
if (-not (Test-Path $sshDir)) {
    New-Item -ItemType Directory -Path $sshDir | Out-Null
    Write-Host "Created $sshDir"
}

# 2. Generate key if missing (no passphrase — add one later with ssh-keygen -p)
if (-not (Test-Path $privateKey)) {
    Write-Host "Generating ed25519 key at $privateKey ..."
    $comment = "blink-$($env:COMPUTERNAME.ToLower())"
    # Empty string for -N is tricky in PowerShell; invoke via cmd for reliability.
    & cmd /c "ssh-keygen -t ed25519 -N `"`" -C `"$comment`" -f `"$privateKey`""
    if ($LASTEXITCODE -ne 0) { throw "ssh-keygen failed" }
} else {
    Write-Host "Private key already exists at $privateKey"
}

# 3. Fix ACLs on the private key (Windows OpenSSH refuses 'too open' files)
icacls $privateKey /inheritance:r | Out-Null
icacls $privateKey /grant:r "$($env:USERNAME):(R,W)" | Out-Null
Write-Host "ACLs locked on $privateKey"

# 4. Add Include line to ~/.ssh/config
# OpenSSH on Windows (both Git's and Microsoft's) accepts C:/... forward-slash paths;
# backslash paths break Git's OpenSSH (it treats "\" as an escape character).
$repoConfigUnix = $repoConfig -replace '\\', '/'
$includeLine = "Include `"$repoConfigUnix`""
$existing = if (Test-Path $sshConfig) { Get-Content $sshConfig -Raw } else { '' }
if ($existing -notmatch [regex]::Escape($repoConfigUnix)) {
    $prefix = if ($existing.Length -gt 0 -and -not $existing.EndsWith("`n")) { "`n" } else { '' }
    Add-Content -Path $sshConfig -Value "$prefix`n# Blink (auto-added by deploy/ssh/setup.ps1)`n$includeLine`n"
    Write-Host "Added Include to $sshConfig"
} else {
    Write-Host "~/.ssh/config already includes $repoConfigUnix"
}

# 5. Copy public key into authorized_keys.d/<hostname>.pub
$hostName = $env:COMPUTERNAME.ToLower()
$pubTarget = Join-Path $authDir "$hostName.pub"
$needsCopy = $true
if (Test-Path $pubTarget) {
    if ((Get-Content $pubTarget -Raw).Trim() -eq (Get-Content $publicKey -Raw).Trim()) {
        $needsCopy = $false
    }
}
if ($needsCopy) {
    Copy-Item $publicKey $pubTarget -Force
    Write-Host "Copied public key to authorized_keys.d/$hostName.pub"
    Write-Host ''
    Write-Host 'NEXT STEPS:' -ForegroundColor Yellow
    Write-Host "  git add deploy/ssh/authorized_keys.d/$hostName.pub"
    Write-Host "  git commit -m 'ssh: authorize $hostName'"
    Write-Host "  git push"
    Write-Host '  # Then from an already-authorized machine:'
    Write-Host '  bash deploy/ssh/sync-authorized-keys.sh'
} else {
    Write-Host "authorized_keys.d/$hostName.pub is already up to date"
}

Write-Host ''
Write-Host 'Setup complete. Test with: ssh blink' -ForegroundColor Green
