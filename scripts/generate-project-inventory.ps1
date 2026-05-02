param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$outputDir = Join-Path $repoRoot "docs\generated"
$outputPath = Join-Path $outputDir "project-inventory.json"

function Find-LineNumber {
    param(
        [string[]]$Lines,
        [string]$Pattern
    )

    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match $Pattern) {
            return ($i + 1)
        }
    }

    return $null
}

function New-Evidence {
    param(
        [string]$Path,
        [Nullable[int]]$Line,
        [string]$Note
    )

    $ev = [ordered]@{
        path = $Path.Replace('\', '/')
        note = $Note
    }
    if ($null -ne $Line) {
        $ev.line = $Line
    }
    return $ev
}

$items = New-Object System.Collections.Generic.List[object]

function Add-InventoryItem {
    param(
        [string]$Area,
        [string]$Name,
        [string]$Status,
        [string]$Confidence,
        [object[]]$Evidence,
        [string]$Recommendation
    )

    $id = "{0}:{1}" -f $Area, $Name
    $items.Add([ordered]@{
        id = $id
        area = $Area
        name = $Name
        status = $Status
        confidence = $Confidence
        recommendation = $Recommendation
        evidence = @($Evidence)
    })
}

$readmePath = Join-Path $repoRoot "README.md"
$runbookPath = Join-Path $repoRoot "docs\RUNBOOK.md"
$startScriptPath = Join-Path $repoRoot "scripts\start-blink.ps1"
$syncSkillsPath = Join-Path $repoRoot "scripts\sync-skills-and-bind-agents.ps1"
$workspaceCargoPath = Join-Path $repoRoot "blink-engine\Cargo.toml"
$engineLibPath = Join-Path $repoRoot "blink-engine\crates\engine\src\lib.rs"
$engineMainPath = Join-Path $repoRoot "blink-engine\crates\engine\src\main.rs"
$uiTabPath = Join-Path $repoRoot "blink-ui\src\hooks\useTab.ts"
$uiTabBarPath = Join-Path $repoRoot "blink-ui\src\components\TabBar.tsx"
$uiAppPath = Join-Path $repoRoot "blink-ui\src\App.tsx"
$skillsLockPath = Join-Path $repoRoot "skills-lock.json"

$readmeLines = Get-Content $readmePath
$runbookLines = Get-Content $runbookPath
$startLines = Get-Content $startScriptPath
$syncLines = Get-Content $syncSkillsPath
$cargoLines = Get-Content $workspaceCargoPath
$libLines = Get-Content $engineLibPath
$mainLines = Get-Content $engineMainPath
$tabLines = Get-Content $uiTabPath
$tabBarLines = Get-Content $uiTabBarPath
$appLines = Get-Content $uiAppPath
$skillsLockLines = Get-Content $skillsLockPath

# Engine modules
$mainUseModules = @{}
for ($i = 0; $i -lt $mainLines.Count; $i++) {
    if ($mainLines[$i] -match 'use engine::([a-z0-9_]+)::') {
        $mainUseModules[$Matches[1]] = $i + 1
    }
}

for ($i = 0; $i -lt $libLines.Count; $i++) {
    if ($libLines[$i] -match '^pub mod ([a-z0-9_]+);') {
        $module = $Matches[1]
        $status = "compiled-not-wired"
        $confidence = "medium"
        $recommendation = "Review whether module is still needed or wire it explicitly in runtime/docs."
        $evidence = @(
            (New-Evidence -Path "blink-engine/crates/engine/src/lib.rs" -Line ($i + 1) -Note "Module exported by engine crate")
        )

        if ($mainUseModules.ContainsKey($module)) {
            $status = "active-runtime"
            $confidence = "high"
            $recommendation = "Keep; this module is currently wired in the engine runtime."
            $evidence += (New-Evidence -Path "blink-engine/crates/engine/src/main.rs" -Line $mainUseModules[$module] -Note "Imported by engine runtime entrypoint")
        }

        Add-InventoryItem -Area "engine-module" -Name $module -Status $status -Confidence $confidence -Evidence $evidence -Recommendation $recommendation
    }
}

# Workspace crates
$workspaceMembers = @()
for ($i = 0; $i -lt $cargoLines.Count; $i++) {
    if ($cargoLines[$i] -match '"crates/([^"]+)"') {
        $workspaceMembers += [ordered]@{
            crate = $Matches[1]
            line = $i + 1
        }
    }
}

foreach ($member in $workspaceMembers) {
    $crate = $member.crate
    $status = "compiled-not-wired"
    $confidence = "medium"
    $recommendation = "Confirm operational usage and add to runbook if this crate should stay active."
    $evidence = @(
        (New-Evidence -Path "blink-engine/Cargo.toml" -Line $member.line -Note "Workspace member")
    )

    if ($crate -eq "engine") {
        $status = "active-runtime"
        $confidence = "high"
        $recommendation = "Primary runtime crate."
        $evidence += (New-Evidence -Path "README.md" -Line (Find-LineNumber -Lines $readmeLines -Pattern "blink-engine") -Note "Documented as core backend")
    } else {
        $refLine = Find-LineNumber -Lines $readmeLines -Pattern $crate
        if ($null -eq $refLine) {
            $refLine = Find-LineNumber -Lines $runbookLines -Pattern $crate
        }
        if ($null -ne $refLine) {
            $status = "active-ops"
            $confidence = "medium"
            $recommendation = "Keep and maintain docs for operator usage."
            $evidence += (New-Evidence -Path "README.md" -Line $refLine -Note "Referenced in docs")
        }
    }

    Add-InventoryItem -Area "workspace-crate" -Name $crate -Status $status -Confidence $confidence -Evidence $evidence -Recommendation $recommendation
}

# UI tabs
$tabIds = @()
$tabContent = Get-Content -Raw $uiTabPath
if ($tabContent -match "const TABS = \[(?<body>[^\]]+)\] as const") {
    $tabBody = $Matches["body"]
    foreach ($match in [regex]::Matches($tabBody, "'(?<tab>[a-z]+)'")) {
        $tab = $match.Groups["tab"].Value
        $line = Find-LineNumber -Lines $tabLines -Pattern ("'" + [regex]::Escape($tab) + "'")
        $tabIds += [ordered]@{
            id = $tab
            line = $line
        }
    }
}

foreach ($tab in $tabIds) {
    $id = $tab.id
    $tabBarLine = Find-LineNumber -Lines $tabBarLines -Pattern ("id:\s*'" + [regex]::Escape($id) + "'")
    $appLine = Find-LineNumber -Lines $appLines -Pattern ("activeTab === '" + [regex]::Escape($id) + "'")
    $status = if ($null -ne $tabBarLine -and $null -ne $appLine) { "active-runtime" } else { "unknown-needs-review" }
    $confidence = if ($status -eq "active-runtime") { "high" } else { "medium" }
    $recommendation = if ($status -eq "active-runtime") {
        "Tab is wired and rendered."
    } else {
        "Check whether tab wiring is incomplete or intentionally hidden."
    }

    $evidence = @(
        (New-Evidence -Path "blink-ui/src/hooks/useTab.ts" -Line $tab.line -Note "Tab declared in tab registry")
    )
    if ($null -ne $tabBarLine) {
        $evidence += (New-Evidence -Path "blink-ui/src/components/TabBar.tsx" -Line $tabBarLine -Note "Tab shown in UI navigation")
    }
    if ($null -ne $appLine) {
        $evidence += (New-Evidence -Path "blink-ui/src/App.tsx" -Line $appLine -Note "Tab route rendered in app")
    }

    Add-InventoryItem -Area "ui-tab" -Name $id -Status $status -Confidence $confidence -Evidence $evidence -Recommendation $recommendation
}

# Skills
$skillsLock = Get-Content -Raw $skillsLockPath | ConvertFrom-Json
$skillSyncLine = Find-LineNumber -Lines $startLines -Pattern "sync-skills-and-bind-agents\.ps1"
foreach ($property in $skillsLock.skills.PSObject.Properties) {
    $skillName = $property.Name
    $skill = $property.Value
    $line = Find-LineNumber -Lines $skillsLockLines -Pattern ('"' + [regex]::Escape($skillName) + '"\s*:')

    $status = "active-ops"
    $confidence = if ($skill.sourceType -eq "local") { "high" } else { "medium" }
    $recommendation = if ($skill.sourceType -eq "local") {
        "Local skill mirrored into agent catalogs."
    } else {
        "External skill dependency; keep hash and source reviewed."
    }

    $evidence = @(
        (New-Evidence -Path "skills-lock.json" -Line $line -Note ("Skill sourceType=" + $skill.sourceType)),
        (New-Evidence -Path "scripts/start-blink.ps1" -Line $skillSyncLine -Note "Startup syncs skills + manifest")
    )
    Add-InventoryItem -Area "skill" -Name $skillName -Status $status -Confidence $confidence -Evidence $evidence -Recommendation $recommendation
}

# Scripts
$scriptFiles = Get-ChildItem -Path (Join-Path $repoRoot "scripts") -File -Filter "*.ps1" | Sort-Object Name
foreach ($script in $scriptFiles) {
    $scriptRel = ("scripts/" + $script.Name)
    $readmeLine = Find-LineNumber -Lines $readmeLines -Pattern ([regex]::Escape($script.Name))
    $runbookLine = Find-LineNumber -Lines $runbookLines -Pattern ([regex]::Escape($script.Name))
    $startLine = Find-LineNumber -Lines $startLines -Pattern ([regex]::Escape($script.Name))

    $status = "unknown-needs-review"
    $confidence = "medium"
    $recommendation = "Keep if needed in operator workflow; otherwise archive."
    $evidence = @(
        (New-Evidence -Path $scriptRel -Line 1 -Note "Script exists in project scripts folder")
    )

    if ($null -ne $readmeLine) {
        $status = "active-ops"
        $confidence = "high"
        $recommendation = "Operator-documented script."
        $evidence += (New-Evidence -Path "README.md" -Line $readmeLine -Note "Referenced in workspace README")
    } elseif ($null -ne $runbookLine) {
        $status = "active-ops"
        $confidence = "high"
        $recommendation = "Runbook-documented script."
        $evidence += (New-Evidence -Path "docs/RUNBOOK.md" -Line $runbookLine -Note "Referenced in runbook")
    } elseif ($null -ne $startLine) {
        $status = "active-ops"
        $confidence = "medium"
        $recommendation = "Used by startup orchestration."
        $evidence += (New-Evidence -Path "scripts/start-blink.ps1" -Line $startLine -Note "Invoked by start script")
    }

    Add-InventoryItem -Area "script" -Name $script.Name -Status $status -Confidence $confidence -Evidence $evidence -Recommendation $recommendation
}

# Docs
$docFiles = Get-ChildItem -Path (Join-Path $repoRoot "docs") -Recurse -File | Sort-Object FullName
foreach ($doc in $docFiles) {
    $relative = $doc.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
    $status = "unknown-needs-review"
    $confidence = "low"
    $recommendation = "Review and tag this document as active reference or archive candidate."
    if ($relative -like "docs/archive/*") {
        $status = "archived-or-legacy"
        $confidence = "high"
        $recommendation = "Archive-only material; keep out of active operator path."
    } elseif ($relative -eq "docs/RUNBOOK.md" -or $relative -eq "docs/TODO.md") {
        $status = "active-ops"
        $confidence = "high"
        $recommendation = "Active operational documentation."
    } elseif ($relative -eq "docs/generated/config-registry.json" -or $relative -eq "docs/generated/project-inventory.json") {
        $status = "active-ops"
        $confidence = "high"
        $recommendation = "Generated artifact used for auditability."
    }

    Add-InventoryItem -Area "doc" -Name $relative -Status $status -Confidence $confidence -Evidence @(
        (New-Evidence -Path $relative -Line 1 -Note "Documentation file present")
    ) -Recommendation $recommendation
}

$statusCounts = [ordered]@{}
$areaCounts = [ordered]@{}
foreach ($item in $items) {
    $statusKey = [string]$item["status"]
    $areaKey = [string]$item["area"]
    if (-not $statusCounts.Contains($statusKey)) {
        $statusCounts[$statusKey] = 0
    }
    if (-not $areaCounts.Contains($areaKey)) {
        $areaCounts[$areaKey] = 0
    }
    $statusCounts[$statusKey] += 1
    $areaCounts[$areaKey] += 1
}

$report = [ordered]@{
    schemaVersion = 1
    generatedAt = (Get-Date).ToString("o")
    available = $true
    summary = [ordered]@{
        totalItems = $items.Count
        byStatus = $statusCounts
        byArea = $areaCounts
    }
    items = $items
}

New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
$report | ConvertTo-Json -Depth 10 | Set-Content -Path $outputPath

Write-Host ("Generated project inventory: {0}" -f $outputPath.Substring($repoRoot.Length + 1).Replace('\', '/')) -ForegroundColor Green
Write-Host ("Items: {0}" -f $items.Count) -ForegroundColor Green

