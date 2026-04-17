<#
.SYNOPSIS
    Blink Engine — lokal utvärderingsmall (16 sektioner, noll LLM-tokens).

.DESCRIPTION
    Läser paper_portfolio_state.json, paper_rejections.json och postrun-reviews
    och producerar en fullständig prestandaanalys helt lokalt.
    Sektion [16] genererar ett kompakt AI-paste-block (<500 tokens).

.PARAMETER State
    Sökväg till paper_portfolio_state.json.
    Default: blink-engine/logs/paper_portfolio_state.json

.PARAMETER Rejections
    Sökväg till paper_rejections.json.
    Default: blink-engine/logs/paper_rejections.json

.PARAMETER Reports
    Mapp med postrun-review-*.txt.
    Default: blink-engine/logs/reports

.PARAMETER Env
    Sökväg till .env-filen.
    Default: blink-engine/.env

.PARAMETER Runs
    Antal historiska sessioner att visa i cross-run trend. Default: 5

.PARAMETER Compact
    Visa bara sektion [16]: det kompakta AI-paste-blocket.

.PARAMETER Section
    Komma-separerade sektionsnummer att visa, t.ex. "3,5,14". Default: alla.

.PARAMETER NoColor
    Inaktivera ANSI-färger (bra vid pipe till fil).

.EXAMPLE
    .\scripts\eval.ps1
    .\scripts\eval.ps1 -Compact
    .\scripts\eval.ps1 -Runs 10 -Section 14
    .\scripts\eval.ps1 -NoColor | Out-File eval-report.txt
#>

[CmdletBinding()]
param(
    [string]$State      = "",
    [string]$Rejections = "",
    [string]$Reports    = "",
    [string]$Env        = "",
    [int]$Runs          = 5,
    [switch]$Compact,
    [string]$Section    = "",
    [switch]$NoColor
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Force invariant culture for numeric formatting (avoid locale decimal commas)
[System.Threading.Thread]::CurrentThread.CurrentCulture   = [System.Globalization.CultureInfo]::InvariantCulture
[System.Threading.Thread]::CurrentThread.CurrentUICulture = [System.Globalization.CultureInfo]::InvariantCulture

# ── Paths ─────────────────────────────────────────────────────────────────────
$Root       = Split-Path $PSScriptRoot -Parent
$EngineRoot = Join-Path $Root "blink-engine"
$LogsDir    = Join-Path $EngineRoot "logs"

if (-not $State)      { $State      = Join-Path $LogsDir "paper_portfolio_state.json" }
if (-not $Rejections) { $Rejections = Join-Path $LogsDir "paper_rejections.json" }
if (-not $Reports)    { $Reports    = Join-Path $LogsDir "reports" }
if (-not $Env)        { $Env        = Join-Path $EngineRoot ".env" }

# ── Section filter ────────────────────────────────────────────────────────────
[int[]]$SectionFilter = @()
if ($Section) {
    $SectionFilter = $Section -split "," | ForEach-Object { [int]$_.Trim() }
}
function Should-Show([int]$n) {
    if ($Compact) { return $n -eq 16 }
    if ($SectionFilter.Count -gt 0) { return $SectionFilter -contains $n }
    return $true
}

# ── Color primitives ──────────────────────────────────────────────────────────
function hdr([string]$txt) {
    if ($NoColor) { Write-Host "`n$txt" }
    else          { Write-Host "`n$txt" -ForegroundColor Cyan }
}

# Print with named color: "ok","warn","bad","dim","row"
function Print([string]$txt, [string]$c = "row") {
    if ($NoColor) { Write-Host $txt; return }
    switch ($c) {
        "ok"   { Write-Host $txt -ForegroundColor Green }
        "warn" { Write-Host $txt -ForegroundColor Yellow }
        "bad"  { Write-Host $txt -ForegroundColor Red }
        "dim"  { Write-Host $txt -ForegroundColor DarkGray }
        default{ Write-Host $txt }
    }
}

function fmt([double]$v, [int]$d = 2) { [math]::Round($v, $d).ToString("F$d") }
function pct([double]$v, [int]$d = 1) { "$(fmt $v $d)%" }
function usd([double]$v)              { "`$$([math]::Round($v,2).ToString('F2'))" }

function Percentile([double[]]$arr, [double]$p) {
    if ($arr.Count -eq 0) { return 0.0 }
    $sorted = $arr | Sort-Object
    $idx = [math]::Max(0, [math]::Min([math]::Ceiling($p / 100.0 * $sorted.Count) - 1, $sorted.Count - 1))
    return $sorted[$idx]
}

function Stddev([double[]]$arr) {
    if ($arr.Count -lt 2) { return 0.0 }
    $avg = ($arr | Measure-Object -Average).Average
    $sq  = ($arr | ForEach-Object { ($_ - $avg) * ($_ - $avg) } | Measure-Object -Sum).Sum
    return [math]::Sqrt($sq / ($arr.Count - 1))
}

# ── Load data ─────────────────────────────────────────────────────────────────
if (-not (Test-Path $State)) {
    Write-Error "Portfolio state not found: $State"; exit 1
}

$port      = Get-Content $State -Raw | ConvertFrom-Json
$trades    = @($port.closed_trades)
$equity    = [double[]]@($port.equity_curve)
$etimes    = [double[]]@($port.equity_timestamps)
$positions = @($port.positions)

$rejReasons = [System.Collections.Generic.Dictionary[string,int]]::new()
if (Test-Path $Rejections) {
    $rejData = Get-Content $Rejections -Raw | ConvertFrom-Json
    $rejData.reasons.PSObject.Properties | ForEach-Object {
        $rejReasons[$_.Name] = @($_.Value).Count
    }
}

$envVars = [System.Collections.Generic.Dictionary[string,string]]::new()
if (Test-Path $Env) {
    Get-Content $Env | ForEach-Object {
        if ($_ -match "^\s*([A-Z_][A-Z0-9_]*)=(.*)$") {
            $envVars[$Matches[1]] = $Matches[2].Trim()
        }
    }
}

# ── Base metrics ──────────────────────────────────────────────────────────────
$startNav  = 100.0
$finalNav  = if ($equity.Count -gt 0) { $equity[-1] } else { $port.cash_usdc }
$grossPnl  = ($trades | Measure-Object -Property realized_pnl -Sum).Sum
if ($null -eq $grossPnl) { $grossPnl = 0.0 }
$totalFees = [double]$port.total_fees_paid_usdc
$netPnl    = $grossPnl - $totalFees
$returnPct = ($finalNav - $startNav) / $startNav * 100

$winners    = @($trades | Where-Object { $_.realized_pnl -gt 0 })
$losers     = @($trades | Where-Object { $_.realized_pnl -lt 0 })
$breakevens = @($trades | Where-Object { $_.realized_pnl -eq 0 })

$winRate     = if ($trades.Count -gt 0) { $winners.Count / $trades.Count * 100 } else { 0.0 }
$avgWin      = if ($winners.Count -gt 0) { ($winners | Measure-Object -Property realized_pnl -Sum).Sum / $winners.Count } else { 0.0 }
$avgLoss     = if ($losers.Count  -gt 0) { ($losers  | Measure-Object -Property realized_pnl -Sum).Sum / $losers.Count  } else { 0.0 }
$grossWins   = if ($winners.Count -gt 0) { ($winners | Measure-Object -Property realized_pnl -Sum).Sum } else { 0.0 }
$grossLoss   = if ($losers.Count  -gt 0) { [math]::Abs(($losers | Measure-Object -Property realized_pnl -Sum).Sum) } else { 1.0 }
$profitFactor = if ($grossLoss -gt 0) { $grossWins / $grossLoss } else { 999.0 }
$payoffRatio  = if ($avgLoss -ne 0)   { [math]::Abs($avgWin / $avgLoss) } else { 999.0 }
$expectancy   = ($winRate / 100) * $avgWin + (1 - $winRate / 100) * $avgLoss

$pnlArr    = [double[]]($trades | ForEach-Object { [double]$_.realized_pnl })
$durations = [double[]]($trades | ForEach-Object { [double]$_.duration_secs / 60.0 })

# Streaks
$maxWinStreak = 0; $maxLossStreak = 0; $curWin = 0; $curLoss = 0
foreach ($t in $trades) {
    if ($t.realized_pnl -gt 0)     { $curWin++; $curLoss = 0 }
    elseif ($t.realized_pnl -lt 0) { $curLoss++; $curWin = 0 }
    else                            { $curWin = 0; $curLoss = 0 }
    if ($curWin  -gt $maxWinStreak)  { $maxWinStreak  = $curWin }
    if ($curLoss -gt $maxLossStreak) { $maxLossStreak = $curLoss }
}

$sessionDurationMin = 0.0
if ($etimes.Count -ge 2) { $sessionDurationMin = ($etimes[-1] - $etimes[0]) / 60.0 }

# Equity metrics
$peakNav = $startNav; $troughNav = $startNav; $maxDD = 0.0
foreach ($e in $equity) {
    if ($e -gt $peakNav) { $peakNav = $e }
    $dd = ($peakNav - $e) / $peakNav * 100
    if ($dd -gt $maxDD) { $maxDD = $dd; $troughNav = $e }
}
$recoveryFactor = if ($maxDD -gt 0) { $returnPct / $maxDD } else { 0.0 }

# Hourly returns → Sharpe / Sortino / Calmar
$hourlyReturns = [double[]]@()
if ($equity.Count -ge 4 -and $sessionDurationMin -gt 0) {
    $step = [math]::Max(1, [int]($equity.Count / ($sessionDurationMin / 60.0 + 1)))
    for ($i = $step; $i -lt $equity.Count; $i += $step) {
        $prev = $equity[$i - $step]
        if ($prev -gt 0) { $hourlyReturns += ($equity[$i] - $prev) / $prev * 100 }
    }
}
$sharpeProxy = 0.0; $sortinoProxy = 0.0; $calmarProxy = 0.0
if ($hourlyReturns.Count -ge 2) {
    $avgHr  = ($hourlyReturns | Measure-Object -Average).Average
    $sdHr   = Stddev $hourlyReturns
    $negHr  = [double[]]($hourlyReturns | Where-Object { $_ -lt 0 })
    $sdNeg  = if ($negHr.Count -ge 2) { Stddev $negHr } else { $sdHr }
    $sharpeProxy  = if ($sdHr  -gt 0) { $avgHr / $sdHr  } else { 0.0 }
    $sortinoProxy = if ($sdNeg -gt 0) { $avgHr / $sdNeg } else { 0.0 }
}
$annualFactor = if ($sessionDurationMin -gt 0) { 525600.0 / $sessionDurationMin } else { 0.0 }
$calmarProxy  = if ($maxDD -gt 0) { ($returnPct * $annualFactor) / $maxDD } else { 0.0 }

# Fee drag
$feeDrag = if ($grossPnl -gt 0) { $totalFees / $grossPnl * 100 } else { 0.0 }

# Category helper
function Get-Category([string]$title) {
    $t = $title.ToLower()
    if ($t -match "vs |nba|nfl|mlb|nhl|soccer|tennis|premier|football|cricket|rugby|f1 |gp |liga|bundesliga|serie|champions|europa|fifa|eintracht|arsenal|chelsea|city |united|liverpool|dortmund|madrid|barcelona|match|tournament|cup |grand slam") { return "sports" }
    if ($t -match "bitcoin|btc|ethereum|eth |solana|sol |crypto|defi|nft|token|blockchain|usdc|usdt|binance|coinbase") { return "crypto" }
    if ($t -match "president|election|congress|trump|biden|harris|senate|governor|parliament|vote|poll|party|democrat|republican|referendum") { return "politics" }
    if ($t -match "war |nato|sanction|ukraine|russia|china|military|treaty|geopolit|iran|israel|nuclear|ceasefire|invasion|conflict") { return "geopolitics" }
    return "other"
}

# Fill/abort/skip
$filledN  = @($port.filled_orders).Count
$abortedN = @($port.aborted_orders).Count
$skippedN = @($port.skipped_orders).Count
$totalSig = [int]$port.total_signals
$fillRate  = if ($totalSig -gt 0) { $filledN  / $totalSig * 100 } else { 0.0 }
$abortRate = if ($totalSig -gt 0) { $abortedN / $totalSig * 100 } else { 0.0 }
$skipRate  = if ($totalSig -gt 0) { $skippedN / $totalSig * 100 } else { 0.0 }

# ══════════════════════════════════════════════════════════════════════════════
Write-Host ""
Print "▓▓▓ BLINK ENGINE — PERFORMANCE EVALUATOR v3 ▓▓▓" "ok"
Print "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') | $State"

# ════════════════════════════════════════════════════════════════════════════════
# [1] SESSION INFO
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 1) {
    hdr "━━━ [1] SESSION INFO ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "Schema version    : $($port.schema_version)"
    Print "Equity points     : $($equity.Count)  (proxy for uptime)"
    $ageLabel = if ($sessionDurationMin -gt 2000) { "portfolio age (cumulative state)" } else { "approx session" }
    Print "Duration          : $(fmt $sessionDurationMin 1) min  ($(fmt ($sessionDurationMin/60) 2) h)  [$ageLabel]"
    Print "Total signals     : $totalSig"
    Print "Open positions    : $($positions.Count)"
    Print "Closed trades     : $($trades.Count)"
}

# ════════════════════════════════════════════════════════════════════════════════
# [2] P&L OVERVIEW
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 2) {
    hdr "━━━ [2] P&L OVERVIEW ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "Starting NAV      : $(usd $startNav)"
    Print "Final NAV         : $(usd $finalNav)"
    $rc = if ($returnPct -ge 0) { "ok" } else { "bad" }
    Print "Net return        : $(pct $returnPct 3)" $rc
    Print "Gross P&L         : $(usd $grossPnl)"
    Print "Total fees paid   : $(usd $totalFees)"
    $nc = if ($netPnl -ge 0) { "ok" } else { "bad" }
    Print "Net P&L           : $(usd $netPnl)" $nc
    $dc = if ($feeDrag -gt 30) { "warn" } else { "row" }
    Print "Fee drag          : $(pct $feeDrag)  of gross P&L" $dc
    if ($annualFactor -gt 0) {
        Print "Annualised return : $(pct ($returnPct * $annualFactor) 1)  (extrapolated)"
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [3] TRADE METRICS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 3) {
    hdr "━━━ [3] TRADE METRICS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "Total trades      : $($trades.Count)  (W:$($winners.Count)  L:$($losers.Count)  B:$($breakevens.Count))"
    $wrc = if ($winRate -ge 55) { "ok" } elseif ($winRate -ge 45) { "warn" } else { "bad" }
    Print "Win rate          : $(pct $winRate)" $wrc
    $pfc = if ($profitFactor -ge 1.5) { "ok" } elseif ($profitFactor -ge 1.0) { "warn" } else { "bad" }
    Print "Profit factor     : $(fmt $profitFactor 3)  (>1.5 = good)" $pfc
    $poc = if ($payoffRatio -ge 1.0) { "ok" } else { "warn" }
    Print "Payoff ratio      : $(fmt $payoffRatio 3)  (avg_win / |avg_loss|)" $poc
    $ec  = if ($expectancy -gt 0) { "ok" } else { "bad" }
    Print "Expectancy/trade  : $(usd $expectancy)" $ec
    Print "Avg win           : $(usd $avgWin)"
    Print "Avg loss          : $(usd $avgLoss)"
    Print "Max win streak    : $maxWinStreak"
    $lsc = if ($maxLossStreak -gt 5) { "bad" } elseif ($maxLossStreak -gt 3) { "warn" } else { "row" }
    Print "Max loss streak   : $maxLossStreak" $lsc

    if ($pnlArr.Count -gt 0) {
        Print ""
        Print "P&L Percentiles:"
        Print ("  P10={0}  P25={1}  P50={2}  P75={3}  P90={4}  P95={5}" -f `
            (usd (Percentile $pnlArr 10)), (usd (Percentile $pnlArr 25)),
            (usd (Percentile $pnlArr 50)), (usd (Percentile $pnlArr 75)),
            (usd (Percentile $pnlArr 90)), (usd (Percentile $pnlArr 95)))
    }
    if ($durations.Count -gt 0) {
        $avgDur = ($durations | Measure-Object -Average).Average
        $p90Dur = Percentile $durations 90
        Print "Avg duration      : $(fmt $avgDur 1) min   P90: $(fmt $p90Dur 1) min"
    }
    Print ""
    Print "TOP 3 WINNERS:"
    $trades | Sort-Object { [double]$_.realized_pnl } -Descending | Select-Object -First 3 | ForEach-Object {
        Print "  +$(usd $_.realized_pnl)  $($_.reason.PadRight(22))  $($_.market_title)" "ok"
    }
    Print "TOP 3 LOSERS:"
    $trades | Sort-Object { [double]$_.realized_pnl } | Select-Object -First 3 | ForEach-Object {
        Print "  $(usd $_.realized_pnl)  $($_.reason.PadRight(22))  $($_.market_title)" "bad"
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [4] EXIT ANALYSIS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 4) {
    hdr "━━━ [4] EXIT ANALYSIS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print ("{0,-26} {1,6} {2,8} {3,8} {4,10}" -f "Exit Reason","Count","Win%","AvgP&L","TotalP&L")
    Print ("{0,-26} {1,6} {2,8} {3,8} {4,10}" -f ("-"*26),("-"*6),("-"*8),("-"*8),("-"*10))
    $trades | Group-Object { $_.reason -replace "@\d+%.*","" -replace "\[\d+%\]","" } |
        Sort-Object Count -Descending | ForEach-Object {
            $gw  = @($_.Group | Where-Object { $_.realized_pnl -gt 0 })
            $wr  = if ($_.Count -gt 0) { $gw.Count / $_.Count * 100 } else { 0 }
            $avg = ($_.Group | Measure-Object -Property realized_pnl -Average).Average
            $tot = ($_.Group | Measure-Object -Property realized_pnl -Sum).Sum
            $c   = if ($tot -ge 0) { "ok" } else { "bad" }
            Print ("{0,-26} {1,6} {2,8} {3,8} {4,10}" -f $_.Name, $_.Count, (pct $wr), (usd $avg), (usd $tot)) $c
        }
}

# ════════════════════════════════════════════════════════════════════════════════
# [5] REJECTION FUNNEL
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 5) {
    hdr "━━━ [5] REJECTION FUNNEL ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "Total signals     : $totalSig"
    Print "  → Filled        : $filledN  ($(pct $fillRate))" "ok"
    $ac = if ($abortRate -gt 30) { "bad" } elseif ($abortRate -gt 15) { "warn" } else { "ok" }
    Print "  → Aborted       : $abortedN  ($(pct $abortRate))" $ac
    Print "  → Skipped       : $skippedN  ($(pct $skipRate))"

    if ($rejReasons.Count -gt 0) {
        $totalRej = ($rejReasons.Values | Measure-Object -Sum).Sum
        Print ""
        Print "TOP 10 REJECTION REASONS:"
        Print ("{0,-35} {1,7} {2,8}" -f "Reason","Count","%Total")
        Print ("{0,-35} {1,7} {2,8}" -f ("-"*35),("-"*7),("-"*8))
        $rejReasons.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 10 | ForEach-Object {
            $pctOf = if ($totalRej -gt 0) { $_.Value / $totalRej * 100 } else { 0 }
            $c = if ($_.Key -match "drift|circuit|daily_loss|var_") { "warn" } else { "row" }
            Print ("{0,-35} {1,7} {2,8}" -f $_.Key, $_.Value, (pct $pctOf)) $c
        }

        # Trend: H1 vs H2
        if ($etimes.Count -ge 2) {
            $midTs = ($etimes[0] + $etimes[-1]) / 2
            $h1 = 0; $h2 = 0
            if (Test-Path $Rejections) {
                $rj = Get-Content $Rejections -Raw | ConvertFrom-Json
                $rj.reasons.PSObject.Properties | ForEach-Object {
                    foreach ($ts in @($_.Value)) {
                        if ($ts -lt $midTs) { $h1++ } else { $h2++ }
                    }
                }
            }
            $trend = if ($h2 -gt $h1 * 1.2) { "↑ worsening" } elseif ($h2 -lt $h1 * 0.8) { "↓ improving" } else { "stable" }
            $tc = if ($trend -like "*worsening*") { "warn" } elseif ($trend -like "*improving*") { "ok" } else { "row" }
            Print ""
            Print "Rejection trend   : H1=$h1  H2=$h2  ($trend)" $tc
        }
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [6] PRICE BAND ANALYSIS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 6) {
    hdr "━━━ [6] PRICE BAND ANALYSIS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print ("{0,-14} {1,7} {2,8} {3,9} {4,12}" -f "Band","Count","Win%","AvgP&L","AvgSlip(bps)")
    Print ("{0,-14} {1,7} {2,8} {3,9} {4,12}" -f ("-"*14),("-"*7),("-"*8),("-"*9),("-"*12))
    @(@(0.05,0.20,"[0.05-0.20)"), @(0.20,0.35,"[0.20-0.35)"), @(0.35,0.50,"[0.35-0.50)"),
      @(0.50,0.65,"[0.50-0.65)"), @(0.65,0.80,"[0.65-0.80)"), @(0.80,0.95,"[0.80-0.95)")) | ForEach-Object {
        $lo = $_[0]; $hi = $_[1]; $lbl = $_[2]
        $g = @($trades | Where-Object { [double]$_.entry_price -ge $lo -and [double]$_.entry_price -lt $hi })
        if ($g.Count -eq 0) { return }
        $gw   = @($g | Where-Object { $_.realized_pnl -gt 0 })
        $wr   = $gw.Count / $g.Count * 100
        $avg  = ($g | Measure-Object -Property realized_pnl -Average).Average
        $slips = [double[]]($g | ForEach-Object { if ($_.scorecard -and $null -ne $_.scorecard.slippage_bps) { [double]$_.scorecard.slippage_bps } else { 0.0 } })
        $avgSlip = if ($slips.Count -gt 0) { ($slips | Measure-Object -Average).Average } else { 0.0 }
        $c = if ($avg -ge 0) { "ok" } else { "bad" }
        Print ("{0,-14} {1,7} {2,8} {3,9} {4,12}" -f $lbl, $g.Count, (pct $wr), (usd $avg), (fmt $avgSlip 0)) $c
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [7] DURATION BUCKET ANALYSIS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 7) {
    hdr "━━━ [7] DURATION BUCKET ANALYSIS ━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print ("{0,-12} {1,7} {2,8} {3,9}" -f "Duration","Count","Win%","AvgP&L")
    Print ("{0,-12} {1,7} {2,8} {3,9}" -f ("-"*12),("-"*7),("-"*8),("-"*9))
    @(@(0,60,"<1 min"), @(60,300,"1-5 min"), @(300,1800,"5-30 min"),
      @(1800,7200,"30-120 min"), @(7200,28800,"2-8 h"), @(28800,999999,"8 h+")) | ForEach-Object {
        $lo = $_[0]; $hi = $_[1]; $lbl = $_[2]
        $g = @($trades | Where-Object { [double]$_.duration_secs -ge $lo -and [double]$_.duration_secs -lt $hi })
        if ($g.Count -eq 0) { return }
        $gw  = @($g | Where-Object { $_.realized_pnl -gt 0 })
        $wr  = $gw.Count / $g.Count * 100
        $avg = ($g | Measure-Object -Property realized_pnl -Average).Average
        $c   = if ($avg -ge 0) { "ok" } else { "bad" }
        Print ("{0,-12} {1,7} {2,8} {3,9}" -f $lbl, $g.Count, (pct $wr), (usd $avg)) $c
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [8] CATEGORY BREAKDOWN
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 8) {
    hdr "━━━ [8] CATEGORY BREAKDOWN ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print ("{0,-14} {1,7} {2,8} {3,9} {4,10} {5,10}" -f "Category","Count","Win%","AvgP&L","TotalP&L","FeeDrag%")
    Print ("{0,-14} {1,7} {2,8} {3,9} {4,10} {5,10}" -f ("-"*14),("-"*7),("-"*8),("-"*9),("-"*10),("-"*10))
    $catMap = @{}
    foreach ($t in $trades) {
        $cat = Get-Category ([string]($t.market_title ?? ""))
        if (-not $catMap.ContainsKey($cat)) { $catMap[$cat] = [System.Collections.Generic.List[object]]::new() }
        $catMap[$cat].Add($t)
    }
    foreach ($cat in ($catMap.Keys | Sort-Object)) {
        $g    = @($catMap[$cat])
        $gw   = @($g | Where-Object { $_.realized_pnl -gt 0 })
        $wr   = if ($g.Count -gt 0) { $gw.Count / $g.Count * 100 } else { 0 }
        $avg  = ($g | Measure-Object -Property realized_pnl -Average).Average
        $tot  = ($g | Measure-Object -Property realized_pnl -Sum).Sum
        $fees = ($g | Measure-Object -Property fees_paid_usdc -Sum).Sum
        $drag = if ($tot -gt 0) { $fees / $tot * 100 } else { 0 }
        $c    = if ($tot -ge 0) { "ok" } else { "bad" }
        Print ("{0,-14} {1,7} {2,8} {3,9} {4,10} {5,10}" -f $cat, $g.Count, (pct $wr), (usd $avg), (usd $tot), (pct $drag)) $c
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [9] EXPERIMENT VARIANT A vs B
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 9) {
    hdr "━━━ [9] EXPERIMENT VARIANT A vs B ━━━━━━━━━━━━━━━━━━━━━━━━"
    $varA = @($trades | Where-Object { $_.scorecard -and $_.scorecard.outcome_tags -contains "variant:A" })
    $varB = @($trades | Where-Object { $_.scorecard -and $_.scorecard.outcome_tags -contains "variant:B" })
    if ($varA.Count -eq 0 -and $varB.Count -eq 0) {
        Print "  No experiment variant tags found in trades." "dim"
    } else {
        Print ("{0,-10} {1,7} {2,8} {3,9} {4,9}" -f "Variant","Count","Win%","AvgP&L","TotalP&L")
        Print ("{0,-10} {1,7} {2,8} {3,9} {4,9}" -f ("-"*10),("-"*7),("-"*8),("-"*9),("-"*9))
        foreach ($pair in @(@("A",$varA),@("B",$varB))) {
            $vn = $pair[0]; $vg = @($pair[1])
            $vw = @($vg | Where-Object { $_.realized_pnl -gt 0 })
            $wr = if ($vg.Count -gt 0) { $vw.Count / $vg.Count * 100 } else { 0 }
            $av = if ($vg.Count -gt 0) { ($vg | Measure-Object -Property realized_pnl -Average).Average } else { 0 }
            $vt = if ($vg.Count -gt 0) { ($vg | Measure-Object -Property realized_pnl -Sum).Sum } else { 0 }
            Print ("{0,-10} {1,7} {2,8} {3,9} {4,9}" -f "Variant $vn", $vg.Count, (pct $wr), (usd $av), (usd $vt))
        }
        if ($varA.Count -gt 0 -and $varB.Count -gt 0) {
            $wA = @($varA | Where-Object {$_.realized_pnl -gt 0}).Count / $varA.Count * 100
            $wB = @($varB | Where-Object {$_.realized_pnl -gt 0}).Count / $varB.Count * 100
            $winner = if ($wA -ge $wB) { "A" } else { "B" }
            $wc = if ($winner -eq "A") { "ok" } else { "warn" }
            Print "  → Winner: Variant $winner  (A=$(pct $wA) vs B=$(pct $wB))" $wc
        }
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [10] SLIPPAGE & FEES DEEP DIVE
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 10) {
    hdr "━━━ [10] SLIPPAGE & FEES DEEP DIVE ━━━━━━━━━━━━━━━━━━━━━━━"
    $slipAll = [double[]]($trades | ForEach-Object {
        if ($_.scorecard -and $null -ne $_.scorecard.slippage_bps) { [double]$_.scorecard.slippage_bps } else { 0.0 }
    })
    if ($slipAll.Count -gt 0) {
        $avgSlip = ($slipAll | Measure-Object -Average).Average
        Print "Slippage (bps):"
        Print "  Avg=$(fmt $avgSlip 1)  Median=$(fmt (Percentile $slipAll 50) 1)  P75=$(fmt (Percentile $slipAll 75) 1)  P90=$(fmt (Percentile $slipAll 90) 1)  P95=$(fmt (Percentile $slipAll 95) 1)"
        $highSlip = @($slipAll | Where-Object { $_ -gt 100 })
        $hsc = if ($highSlip.Count -gt $trades.Count * 0.2) { "bad" } else { "warn" }
        Print "  >100bps: $($highSlip.Count) trades ($(pct ($highSlip.Count / [math]::Max($trades.Count,1) * 100)))" $hsc

        # Quartile correlation
        $q75 = Percentile $slipAll 75
        $hiSlipTrades = @($trades | Where-Object { ($_.scorecard.slippage_bps ?? 0) -ge $q75 })
        $loSlipTrades = @($trades | Where-Object { ($_.scorecard.slippage_bps ?? 0) -lt $q75 })
        if ($hiSlipTrades.Count -gt 0 -and $loSlipTrades.Count -gt 0) {
            $avgHi = ($hiSlipTrades | Measure-Object -Property realized_pnl -Average).Average
            $avgLo = ($loSlipTrades  | Measure-Object -Property realized_pnl -Average).Average
            Print "  High-slip (Q4) avg P&L: $(usd $avgHi)  vs  Low-slip avg P&L: $(usd $avgLo)"
        }
    }
    Print ""
    $avgFeePerTrade = $totalFees / [math]::Max($trades.Count, 1)
    $feeOfGross     = if ($grossPnl -gt 0) { $totalFees / $grossPnl * 100 } else { 0 }
    Print "Fees:"
    Print "  Total=$(usd $totalFees)  Avg/trade=$(usd $avgFeePerTrade)  Fee/gross-pnl=$(pct $feeOfGross)"
}

# ════════════════════════════════════════════════════════════════════════════════
# [11] EQUITY CURVE ANALYSIS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 11) {
    hdr "━━━ [11] EQUITY CURVE ANALYSIS ━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "Starting NAV      : $(usd $startNav)"
    Print "Peak NAV          : $(usd $peakNav)" "ok"
    $tc = if ($troughNav -lt $startNav * 0.95) { "bad" } else { "warn" }
    Print "Trough NAV        : $(usd $troughNav)" $tc
    Print "Final NAV         : $(usd $finalNav)"
    $ddc = if ($maxDD -lt 3) { "ok" } elseif ($maxDD -lt 8) { "warn" } else { "bad" }
    Print "Max drawdown      : $(pct $maxDD)  (peak→trough)" $ddc
    $rfc = if ($recoveryFactor -ge 2) { "ok" } elseif ($recoveryFactor -ge 0.5) { "warn" } else { "bad" }
    Print "Recovery factor   : $(fmt $recoveryFactor 2)  (return / max_drawdown)" $rfc
    Print ""
    Print "Risk-adjusted ratios (proxy, hourly granularity):"
    $sc = if ($sharpeProxy -ge 1.5) { "ok" } elseif ($sharpeProxy -ge 0.5) { "warn" } else { "bad" }
    Print "  Sharpe proxy    : $(fmt $sharpeProxy 3)  (>1.5 strong, >0.5 ok)" $sc
    $soc = if ($sortinoProxy -ge 2) { "ok" } elseif ($sortinoProxy -ge 1) { "warn" } else { "bad" }
    Print "  Sortino proxy   : $(fmt $sortinoProxy 3)  (>2 strong, >1 ok)" $soc
    $cac = if ($calmarProxy -ge 3) { "ok" } elseif ($calmarProxy -ge 1) { "warn" } else { "bad" }
    Print "  Calmar proxy    : $(fmt $calmarProxy 2)  (annualised, >3 strong)" $cac
}

# ════════════════════════════════════════════════════════════════════════════════
# [12] OPEN POSITIONS SNAPSHOT
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 12) {
    hdr "━━━ [12] OPEN POSITIONS SNAPSHOT ━━━━━━━━━━━━━━━━━━━━━━━━━"
    if ($positions.Count -eq 0) {
        Print "  No open positions." "dim"
    } else {
        $totalUnrealized = 0.0; $totalAtRisk = 0.0
        Print ("{0,-18} {1,5} {2,8} {3,8} {4,10}" -f "Token(short)","Side","Entry","Current","UnrealP&L")
        Print ("{0,-18} {1,5} {2,8} {3,8} {4,10}" -f ("-"*18),("-"*5),("-"*8),("-"*8),("-"*10))
        foreach ($pos in $positions) {
            $tid  = $pos.token_id.Substring(0, [math]::Min(16, $pos.token_id.Length)) + ".."
            $ep   = [double]$pos.entry_price
            $cp   = if ($null -ne $pos.current_price -and $pos.current_price -ne 0) { [double]$pos.current_price } else { $ep }
            $sh   = [double]$pos.shares
            $unr  = ($cp - $ep) * $sh
            $atr  = $ep * $sh
            $totalUnrealized += $unr; $totalAtRisk += $atr
            $c = if ($unr -ge 0) { "ok" } else { "bad" }
            Print ("{0,-18} {1,5} {2,8} {3,8} {4,10}" -f $tid, $pos.side, (fmt $ep 3), (fmt $cp 3), (usd $unr)) $c
        }
        Print ""
        $uc = if ($totalUnrealized -ge 0) { "ok" } else { "bad" }
        Print "  Total unrealized : $(usd $totalUnrealized)" $uc
        Print "  Capital at risk  : $(usd $totalAtRisk)"
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [13] CONFIG SNAPSHOT
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 13) {
    hdr "━━━ [13] CONFIG SNAPSHOT ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    $defaults = @{
        "PAPER_MAX_ORDER_USDC"        = "8"
        "SIZE_MULTIPLIER"             = "0.10"
        "PAPER_REALISM_MODE"          = "true"
        "PAPER_DRIFT_THRESHOLD_PCT"   = "8"
        "PAPER_EXIT_SLIPPAGE_BPS"     = "10"
        "MAX_CONCURRENT_POSITIONS"    = "5"
        "MAX_DAILY_LOSS_PCT"          = "10"
        "BULLPEN_SIGNAL_GEN_ENABLED"  = "false"
        "BULLPEN_DISCOVER_LENSES"     = "sports,crypto,traders"
        "ALPHA_MAX_EXPIRY_HOURS"      = "6"
    }
    $keys = @(
        "PAPER_MAX_ORDER_USDC","SIZE_MULTIPLIER","PAPER_REALISM_MODE",
        "PAPER_DRIFT_THRESHOLD_PCT","PAPER_ENTRY_SPREAD_BPS","PAPER_EXIT_SLIPPAGE_BPS",
        "MAX_CONCURRENT_POSITIONS","MAX_SINGLE_ORDER_USDC","MAX_DAILY_LOSS_PCT",
        "PAPER_STOP_LOSS_PCT","PAPER_TRAILING_STOP_PCT","AUTOCLAIM_TIERS",
        "EXTREME_PRICE_HI","EXTREME_PRICE_LO","BULLPEN_SIGNAL_GEN_ENABLED",
        "BULLPEN_DISCOVER_LENSES","ALPHA_MAX_EXPIRY_HOURS"
    )
    Print "  (dim = default value, yellow = non-default)"
    foreach ($k in $keys) {
        $v = if ($envVars.ContainsKey($k)) { $envVars[$k] } else { "(not set)" }
        $isDefault = $defaults.ContainsKey($k) -and $defaults[$k] -eq $v
        $line = "  {0,-35} = {1}" -f $k, $v
        Print $line $(if ($isDefault) { "dim" } else { "warn" })
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [14] CROSS-RUN TREND
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 14) {
    hdr "━━━ [14] CROSS-RUN TREND ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    if (-not (Test-Path $Reports)) {
        Print "  No reports folder: $Reports" "dim"
    } else {
        $files = Get-ChildItem $Reports -Filter "postrun-review-*.txt" | Sort-Object Name -Descending | Select-Object -First $Runs
        if ($files.Count -eq 0) {
            Print "  No postrun review files found." "dim"
        } else {
            $rows = @()
            foreach ($f in ($files | Sort-Object Name)) {
                $content = Get-Content $f.FullName -Raw
                $ex = @{ File = $f.Name -replace "postrun-review-","" -replace "\.txt$","" }
                ($content -split "`n") | Where-Object { $_ -match "^summary\." } | ForEach-Object {
                    if ($_ -match "^summary\.(\w+)=(.+)$") { $ex[$Matches[1]] = $Matches[2].Trim() }
                }
                $rows += $ex
            }
            Print ("{0,-18} {1,9} {2,9} {3,10} {4,12} {5,10}" -f "Session","FillRate","Abort%","NavRet%","Reconnects","Realism")
            Print ("{0,-18} {1,9} {2,9} {3,10} {4,12} {5,10}" -f ("-"*18),("-"*9),("-"*9),("-"*10),("-"*12),("-"*10))
            $prevRet = $null
            foreach ($r in $rows) {
                $ret  = $r["nav_return_pct"] ?? "?"
                $fill = $r["fill_rate_pct"] ?? "?"
                $abrt = $r["abort_rate_pct"] ?? "?"
                $rec  = $r["reconnect_hints"] ?? "?"
                $ral  = $r["realism_alert"] ?? "?"
                $delta = ""
                if ($null -ne $prevRet -and $ret -match "^-?\d") {
                    $d = [double]$ret - [double]$prevRet
                    $delta = if ($d -gt 0) { " ↑" } elseif ($d -lt 0) { " ↓" } else { " =" }
                }
                $prevRet = $ret
                $c = if ($ral -eq "LOW") { "ok" } elseif ($ral -eq "MEDIUM") { "warn" } else { "bad" }
                Print ("{0,-18} {1,9} {2,9} {3,10} {4,12} {5,10}" -f $r.File, $fill, $abrt, "$ret$delta", $rec, $ral) $c
            }
        }
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [15] RISK GATE ANALYSIS
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 15) {
    hdr "━━━ [15] RISK GATE ANALYSIS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    if ($rejReasons.Count -eq 0) {
        Print "  No rejection data available." "dim"
    } else {
        $gates = [ordered]@{
            "kill_switch"     = "Kill switch (trading disabled)"
            "circuit_breaker" = "Circuit breaker"
            "daily_loss"      = "Daily loss limit"
            "position_cap"    = "Max concurrent positions"
            "order_cap"       = "Max single order size"
            "rate_limit"      = "Rate limit"
            "var_threshold"   = "VaR threshold exceeded"
            "drift_abort"     = "In-play drift abort"
            "fee_gate"        = "Fee-to-edge ratio"
            "min_notional"    = 'Min notional ($10)'
            "extreme_price"   = "Extreme price filter"
            "esports"         = "Esports market blocked"
            "dedup"           = "Duplicate order_id"
            "event_horizon"   = "Event too far out (>6h)"
            "concentration"   = "Market concentration cap"
        }
        Print ("{0,-35} {1,7} {2,9} {3,9}" -f "Risk Gate","Count","%Signals","%Rejects")
        Print ("{0,-35} {1,7} {2,9} {3,9}" -f ("-"*35),("-"*7),("-"*9),("-"*9))
        $totalRej = ($rejReasons.Values | Measure-Object -Sum).Sum
        foreach ($gate in $gates.GetEnumerator()) {
            $cnt = 0
            $rejReasons.Keys | Where-Object { $_ -like "*$($gate.Key)*" } | ForEach-Object { $cnt += $rejReasons[$_] }
            if ($cnt -eq 0) { continue }
            $pctS = if ($totalSig  -gt 0) { $cnt / $totalSig  * 100 } else { 0 }
            $pctR = if ($totalRej  -gt 0) { $cnt / $totalRej  * 100 } else { 0 }
            $c    = if ($pctS -gt 20) { "bad" } elseif ($pctS -gt 10) { "warn" } else { "row" }
            Print ("{0,-35} {1,7} {2,9} {3,9}" -f $gate.Value, $cnt, (pct $pctS), (pct $pctR)) $c
        }
    }
}

# ════════════════════════════════════════════════════════════════════════════════
# [16] COMPACT AI PASTE BLOCK
# ════════════════════════════════════════════════════════════════════════════════
if (Should-Show 16) {
    hdr "━━━ [16] ▶ COMPACT AI PASTE BLOCK ━━━━━━━━━━━━━━━━━━━━━━━━"
    Print "  Copy everything between the markers into your AI chat." "dim"
    Print ""
    Print "────────────────────── AI PASTE START ──────────────────────"

    # Exits grouped
    $exitMap = [System.Collections.Generic.Dictionary[string,int]]::new()
    foreach ($t in $trades) {
        $k = $t.reason -replace "@\d+%.*","" -replace "\[\d+%\]",""
        $exitMap[$k] = ($exitMap.ContainsKey($k) ? $exitMap[$k] : 0) + 1
    }

    # Top 5 rejections
    $top5 = @()
    $rejReasons.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 5 | ForEach-Object {
        $top5 += ,@($_.Key, $_.Value)
    }

    # Category P&L
    $catPnl = [System.Collections.Generic.Dictionary[string,double]]::new()
    foreach ($t in $trades) {
        $cat = Get-Category ([string]($t.market_title ?? ""))
        $catPnl[$cat] = ($catPnl.ContainsKey($cat) ? $catPnl[$cat] : 0.0) + [double]$t.realized_pnl
    }

    # Experiment
    $varA = @($trades | Where-Object { $_.scorecard -and $_.scorecard.outcome_tags -contains "variant:A" })
    $varB = @($trades | Where-Object { $_.scorecard -and $_.scorecard.outcome_tags -contains "variant:B" })
    $varAWR = if ($varA.Count -gt 0) { [math]::Round(@($varA | Where-Object {$_.realized_pnl -gt 0}).Count / $varA.Count * 100, 1) } else { $null }
    $varBWR = if ($varB.Count -gt 0) { [math]::Round(@($varB | Where-Object {$_.realized_pnl -gt 0}).Count / $varB.Count * 100, 1) } else { $null }

    # Config
    $cfgKeys = @("PAPER_MAX_ORDER_USDC","SIZE_MULTIPLIER","PAPER_REALISM_MODE","PAPER_STOP_LOSS_PCT",
                 "PAPER_TRAILING_STOP_PCT","AUTOCLAIM_TIERS","PAPER_DRIFT_THRESHOLD_PCT",
                 "EXTREME_PRICE_HI","BULLPEN_SIGNAL_GEN_ENABLED","ALPHA_MAX_EXPIRY_HOURS")
    $cfgSnap = [ordered]@{}
    foreach ($k in $cfgKeys) {
        $cfgSnap[$k] = if ($envVars.ContainsKey($k)) { $envVars[$k] } else { $null }
    }

    # Avg slippage
    $slipAll = [double[]]($trades | ForEach-Object { if ($_.scorecard -and $null -ne $_.scorecard.slippage_bps) { [double]$_.scorecard.slippage_bps } else { 0.0 } })
    $avgSlipGlobal = if ($slipAll.Count -gt 0) { [math]::Round(($slipAll | Measure-Object -Average).Average, 1) } else { 0.0 }

    $ai = [ordered]@{
        blink_eval_v3  = $true
        generated_utc  = (Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ")
        state_file     = (Split-Path $State -Leaf)
        session = [ordered]@{
            duration_min    = [math]::Round($sessionDurationMin, 1)
            equity_points   = $equity.Count
            open_positions  = $positions.Count
        }
        pnl = [ordered]@{
            start_nav       = $startNav
            final_nav       = [math]::Round($finalNav, 4)
            return_pct      = [math]::Round($returnPct, 4)
            gross_pnl       = [math]::Round($grossPnl, 4)
            fees_total      = [math]::Round($totalFees, 4)
            net_pnl         = [math]::Round($netPnl, 4)
            fee_drag_pct    = [math]::Round($feeDrag, 2)
        }
        trades = [ordered]@{
            count           = $trades.Count
            winners         = $winners.Count
            losers          = $losers.Count
            win_rate_pct    = [math]::Round($winRate, 2)
            profit_factor   = [math]::Round($profitFactor, 3)
            payoff_ratio    = [math]::Round($payoffRatio, 3)
            expectancy_usd  = [math]::Round($expectancy, 4)
            avg_win_usd     = [math]::Round($avgWin, 4)
            avg_loss_usd    = [math]::Round($avgLoss, 4)
            max_win_streak  = $maxWinStreak
            max_loss_streak = $maxLossStreak
            p10_pnl         = if ($pnlArr.Count -gt 0) { [math]::Round((Percentile $pnlArr 10), 4) } else { 0 }
            p50_pnl         = if ($pnlArr.Count -gt 0) { [math]::Round((Percentile $pnlArr 50), 4) } else { 0 }
            p90_pnl         = if ($pnlArr.Count -gt 0) { [math]::Round((Percentile $pnlArr 90), 4) } else { 0 }
            avg_duration_min= if ($durations.Count -gt 0) { [math]::Round(($durations | Measure-Object -Average).Average, 1) } else { 0 }
            avg_slippage_bps= $avgSlipGlobal
        }
        signals = [ordered]@{
            total           = $totalSig
            filled          = $filledN
            aborted         = $abortedN
            fill_rate_pct   = [math]::Round($fillRate, 2)
            abort_rate_pct  = [math]::Round($abortRate, 2)
        }
        exits       = $exitMap
        rej_top5    = $top5
        equity = [ordered]@{
            peak_nav        = [math]::Round($peakNav, 4)
            trough_nav      = [math]::Round($troughNav, 4)
            max_drawdown_pct= [math]::Round($maxDD, 3)
            recovery_factor = [math]::Round($recoveryFactor, 3)
            sharpe_proxy    = [math]::Round($sharpeProxy, 3)
            sortino_proxy   = [math]::Round($sortinoProxy, 3)
            calmar_proxy    = [math]::Round($calmarProxy, 2)
        }
        category_pnl = $catPnl
        experiment = [ordered]@{
            A_n           = $varA.Count
            B_n           = $varB.Count
            A_win_pct     = $varAWR
            B_win_pct     = $varBWR
        }
        config = $cfgSnap
    }

    Write-Host ($ai | ConvertTo-Json -Depth 6 -Compress)
    Print ""
    Print "────────────────────── AI PASTE END ────────────────────────"
}

Write-Host ""
