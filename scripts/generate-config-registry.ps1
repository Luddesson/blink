param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$outputDir = Join-Path $repoRoot "docs\generated"
$outputPath = Join-Path $outputDir "config-registry.json"

$templatePaths = @(
    "blink-engine\.env.example",
    "blink-engine\.env.live.template",
    "deploy\hetzner\.env.production.template"
) | ForEach-Object { Join-Path $repoRoot $_ }

$scanExtensions = @(".rs", ".py", ".ts", ".tsx", ".js", ".jsx")
$excludeFragments = @(
    "\.git\",
    "\node_modules\",
    "\target\",
    "\dist\",
    "\static\ui\"
)

$patternsByExtension = @{
    ".rs"  = @(
        'std::env::var\("(?<name>[A-Z0-9_]+)"\)',
        'std::env::var_os\("(?<name>[A-Z0-9_]+)"\)',
        'env_flag\("(?<name>[A-Z0-9_]+)"\)'
    )
    ".py"  = @(
        'os\.getenv\("(?<name>[A-Z0-9_]+)"',
        'os\.environ\[\s*"(?<name>[A-Z0-9_]+)"\s*\]',
        'environ\.get\("(?<name>[A-Z0-9_]+)"'
    )
    ".ts"  = @(
        'import\.meta\.env\.(?<name>[A-Z0-9_]+)',
        'process\.env\.(?<name>[A-Z0-9_]+)'
    )
    ".tsx" = @(
        'import\.meta\.env\.(?<name>[A-Z0-9_]+)',
        'process\.env\.(?<name>[A-Z0-9_]+)'
    )
    ".js"  = @(
        'process\.env\.(?<name>[A-Z0-9_]+)'
    )
    ".jsx" = @(
        'process\.env\.(?<name>[A-Z0-9_]+)'
    )
}

function Get-RelativeRepoPath {
    param([string]$FullPath)

    return $FullPath.Substring($repoRoot.Length + 1).Replace('\', '/')
}

function Get-OrCreateRecord {
    param(
        [hashtable]$Registry,
        [string]$Name
    )

    if (-not $Registry.ContainsKey($Name)) {
        $Registry[$Name] = @{
            templates = @{}
            references = @{}
        }
    }

    return $Registry[$Name]
}

$registry = @{}

foreach ($templatePath in $templatePaths) {
    if (-not (Test-Path $templatePath)) {
        continue
    }

    $relativePath = Get-RelativeRepoPath -FullPath $templatePath
    $lineNumber = 0
    foreach ($line in Get-Content $templatePath) {
        $lineNumber += 1
        if ($line -match '^\s*([A-Z0-9_]+)\s*=') {
            $name = $Matches[1]
            $record = Get-OrCreateRecord -Registry $registry -Name $name
            $record.templates[$relativePath] = $lineNumber
        }
    }
}

$codeFiles = Get-ChildItem -Path $repoRoot -Recurse -File |
    Where-Object {
        $extension = $_.Extension.ToLowerInvariant()
        if ($extension -notin $scanExtensions) {
            return $false
        }

        foreach ($fragment in $excludeFragments) {
            if ($_.FullName.Contains($fragment)) {
                return $false
            }
        }

        return $true
    }

foreach ($file in $codeFiles) {
    $extension = $file.Extension.ToLowerInvariant()
    $patterns = $patternsByExtension[$extension]
    if ($null -eq $patterns) {
        continue
    }

    $relativePath = Get-RelativeRepoPath -FullPath $file.FullName
    $lineNumber = 0
    foreach ($line in Get-Content $file.FullName) {
        $lineNumber += 1
        foreach ($pattern in $patterns) {
            $allMatches = [regex]::Matches($line, $pattern)
            foreach ($match in $allMatches) {
                $name = $match.Groups["name"].Value
                if ([string]::IsNullOrWhiteSpace($name)) {
                    continue
                }

                $record = Get-OrCreateRecord -Registry $registry -Name $name
                $referenceKey = "{0}:{1}" -f $relativePath, $lineNumber
                $record.references[$referenceKey] = [ordered]@{
                    path = $relativePath
                    line = $lineNumber
                }
            }
        }
    }
}

$variables = foreach ($name in ($registry.Keys | Sort-Object)) {
    $record = $registry[$name]
    $declaredInTemplates = @(
        $record.templates.GetEnumerator() |
            Sort-Object Name |
            ForEach-Object {
                [ordered]@{
                    path = $_.Key
                    line = $_.Value
                }
            }
    )
    $referencedIn = @(
        $record.references.GetEnumerator() |
            Sort-Object Name |
            ForEach-Object { $_.Value }
    )

    $status = if ($declaredInTemplates.Count -gt 0 -and $referencedIn.Count -gt 0) {
        "declared-and-referenced"
    } elseif ($declaredInTemplates.Count -gt 0) {
        "declared-only"
    } else {
        "referenced-only"
    }

    [ordered]@{
        name = $name
        status = $status
        declaredInTemplates = $declaredInTemplates
        referencedIn = $referencedIn
    }
}

$summary = [ordered]@{
    totalVariables = $variables.Count
    declaredAndReferenced = @($variables | Where-Object { $_.status -eq "declared-and-referenced" }).Count
    declaredOnly = @($variables | Where-Object { $_.status -eq "declared-only" }).Count
    referencedOnly = @($variables | Where-Object { $_.status -eq "referenced-only" }).Count
}

$report = [ordered]@{
    generatedAt = (Get-Date).ToString("o")
    templates = @(
        foreach ($templatePath in $templatePaths) {
            if (Test-Path $templatePath) {
                Get-RelativeRepoPath -FullPath $templatePath
            }
        }
    )
    summary = $summary
    variables = $variables
}

New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
$report | ConvertTo-Json -Depth 8 | Set-Content -Path $outputPath

Write-Host ("Generated config registry: {0}" -f (Get-RelativeRepoPath -FullPath $outputPath)) -ForegroundColor Green
Write-Host ("Variables: {0} total ({1} declared+referenced, {2} declared-only, {3} referenced-only)" -f `
    $summary.totalVariables,
    $summary.declaredAndReferenced,
    $summary.declaredOnly,
    $summary.referencedOnly) -ForegroundColor Green
