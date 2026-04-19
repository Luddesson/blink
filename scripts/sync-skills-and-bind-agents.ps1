param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$skillsRoot = Join-Path $repoRoot "skills"
$mirrorRoots = @(
    Join-Path $repoRoot ".claude\skills"
    Join-Path $repoRoot ".agents\skills"
)
$agentsRoot = Join-Path $repoRoot "Blink-agents"
$manifestPath = Join-Path $agentsRoot "agent-skill-manifest.json"
$lockPath = Join-Path $repoRoot "skills-lock.json"

function Get-FolderHash {
    param([string]$FolderPath)

    $files = Get-ChildItem -Path $FolderPath -Recurse -File |
        Sort-Object { $_.FullName.Substring($FolderPath.Length).TrimStart('\') }

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        foreach ($file in $files) {
            $relativePath = $file.FullName.Substring($FolderPath.Length).TrimStart('\').Replace('\', '/')
            $pathBytes = [System.Text.Encoding]::UTF8.GetBytes($relativePath)
            $null = $sha.TransformBlock($pathBytes, 0, $pathBytes.Length, $pathBytes, 0)

            $contentBytes = [System.IO.File]::ReadAllBytes($file.FullName)
            $null = $sha.TransformBlock($contentBytes, 0, $contentBytes.Length, $contentBytes, 0)
        }

        $null = $sha.TransformFinalBlock(@(), 0, 0)
        return ($sha.Hash | ForEach-Object { $_.ToString("x2") }) -join ""
    } finally {
        $sha.Dispose()
    }
}

if (-not (Test-Path $skillsRoot)) {
    throw "Skills directory not found: $skillsRoot"
}

$skillDirs = Get-ChildItem -Path $skillsRoot -Directory | Sort-Object Name
$skillNames = $skillDirs.Name

foreach ($mirrorRoot in $mirrorRoots) {
    New-Item -ItemType Directory -Force -Path $mirrorRoot | Out-Null

    Get-ChildItem -Path $mirrorRoot -Directory |
        Where-Object { $_.Name -notin $skillNames } |
        ForEach-Object { Remove-Item -Recurse -Force $_.FullName }

    foreach ($skillDir in $skillDirs) {
        $destination = Join-Path $mirrorRoot $skillDir.Name
        if (Test-Path $destination) {
            Remove-Item -Recurse -Force $destination
        }
        Copy-Item -Recurse -Force $skillDir.FullName $destination
    }
}

$skillsLock = Get-Content -Raw $lockPath | ConvertFrom-Json
$localSkillEntries = @{}
foreach ($property in $skillsLock.skills.PSObject.Properties) {
    if ($property.Value.sourceType -eq "local") {
        $localSkillEntries[$property.Name] = $property.Value
    }
}

$skillManifestEntries = foreach ($skillDir in $skillDirs) {
    $sourceHash = Get-FolderHash -FolderPath $skillDir.FullName
    $mirrorStates = foreach ($mirrorRoot in $mirrorRoots) {
        $mirrorPath = Join-Path $mirrorRoot $skillDir.Name
        [ordered]@{
            path = $mirrorPath.Substring($repoRoot.Length + 1).Replace('\', '/')
            hash = Get-FolderHash -FolderPath $mirrorPath
        }
    }

    $lockEntry = $localSkillEntries[$skillDir.Name]
    [ordered]@{
        name = $skillDir.Name
        source = ("skills/{0}" -f $skillDir.Name)
        sourceHash = $sourceHash
        lockHash = if ($null -ne $lockEntry) { $lockEntry.computedHash } else { $null }
        lockVerified = if ($null -ne $lockEntry) { $lockEntry.computedHash -eq $sourceHash } else { $null }
        mirrors = $mirrorStates
        mirrorsVerified = ($mirrorStates | Where-Object { $_.hash -ne $sourceHash }).Count -eq 0
    }
}

$agentDirs = @()
if (Test-Path $agentsRoot) {
    $agentDirs = Get-ChildItem -Path $agentsRoot -Directory | Sort-Object Name
}

$manifest = [ordered]@{
    generatedAt = (Get-Date).ToString("o")
    sourceRoot = "skills"
    mirrorRoots = @(".claude/skills", ".agents/skills")
    bindingMode = "shared-skill-catalog"
    skills = $skillManifestEntries
    agents = @(
        foreach ($agentDir in $agentDirs) {
            [ordered]@{
                name = $agentDir.Name
                path = $agentDir.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
                availableSkills = $skillNames
            }
        }
    )
}

$manifestJson = $manifest | ConvertTo-Json -Depth 8
Set-Content -Path $manifestPath -Value $manifestJson

$mismatch = $skillManifestEntries | Where-Object { -not $_.mirrorsVerified -or ($null -ne $_.lockVerified -and -not $_.lockVerified) }
if ($mismatch.Count -gt 0) {
    throw "Skill sync completed with verification mismatches."
}

Write-Host ("Synced {0} skills across {1} mirrors." -f $skillNames.Count, $mirrorRoots.Count) -ForegroundColor Green
Write-Host ("Generated agent skill manifest: {0}" -f $manifestPath.Substring($repoRoot.Length + 1)) -ForegroundColor Green
