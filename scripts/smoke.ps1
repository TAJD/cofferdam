#!/usr/bin/env pwsh
<#
.SYNOPSIS
Smoke-test harness for cofferdam binary against reference repos.

.DESCRIPTION
Runs cofferdam check against three reference repositories, captures findings count,
and compares against a baseline file. Supports -Update flag to refresh baselines.

.PARAMETER Debug
Use debug binary (target/debug). Default is release (target/release).

.PARAMETER Update
Update the baseline file with current measurements.

.EXAMPLE
.\scripts\smoke.ps1 -Debug
.\scripts\smoke.ps1 -Update
#>

param(
    [switch]$Debug,
    [switch]$Update
)

$ErrorActionPreference = "Stop"

# Determine binary path
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$profile = if ($Debug) { "debug" } else { "release" }
$binary = "$workspaceRoot\target\$profile\cofferdam.exe"

if (-not (Test-Path $binary)) {
    Write-Error "Binary not found: $binary`nRun 'cargo build --workspace' first (add '--release' for release build)."
    exit 1
}

# Load baseline
$baselineFile = Join-Path $PSScriptRoot "smoke-baseline.json"
if (-not (Test-Path $baselineFile)) {
    Write-Error "Baseline file not found: $baselineFile"
    exit 1
}

$baseline = Get-Content -Path $baselineFile -Raw | ConvertFrom-Json

# Run tests
Write-Host "Running smoke tests with $profile binary: $binary"
Write-Host ""

$results = @()
$allPassed = $true

foreach ($repoConfig in $baseline.repos) {
    $repoPath = $repoConfig.path
    $expectedFindings = $repoConfig.findings
    $tolerance = $repoConfig.tolerance

    if (-not (Test-Path $repoPath)) {
        Write-Warning "Repo not found: $repoPath (skipping)"
        continue
    }

    Write-Host "Checking: $repoPath"

    # Run cofferdam and measure time
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $output = & $binary check "$repoPath" 2>&1
    $sw.Stop()

    # Count lines (findings)
    $lines = $output | Measure-Object -Line
    $actualFindings = $lines.Lines
    $elapsed = $sw.Elapsed.TotalSeconds

    # Check tolerance
    $diff = [Math]::Abs($actualFindings - $expectedFindings)
    $status = if ($diff -le $tolerance) { "PASS" } else { "FAIL" }

    if ($status -eq "FAIL") {
        $allPassed = $false
    }

    $results += [PSCustomObject]@{
        repo = Split-Path -Leaf $repoPath
        expected = $expectedFindings
        actual = $actualFindings
        diff = if ($actualFindings -gt $expectedFindings) { "+$($actualFindings - $expectedFindings)" } else { "$($actualFindings - $expectedFindings)" }
        time_s = [Math]::Round($elapsed, 2)
        status = $status
    }

    Write-Host "  Expected: $expectedFindings, Actual: $actualFindings, Diff: $(($actualFindings - $expectedFindings)), Time: $elapsed`s [$status]"
}

Write-Host ""
Write-Host "===== Smoke Test Summary ====="
$results | Format-Table -Property repo, expected, actual, diff, time_s, status -AutoSize

if ($Update) {
    Write-Host ""
    Write-Host "Updating baseline file..."
    $newBaseline = @{ repos = @() }
    foreach ($result in $results) {
        $repoConfig = $baseline.repos | Where-Object { $_.path -like "*$($result.repo)" } | Select-Object -First 1
        if ($repoConfig) {
            $newBaseline.repos += @{
                path = $repoConfig.path
                findings = $result.actual
                tolerance = $repoConfig.tolerance
            }
        }
    }
    $newBaseline | ConvertTo-Json | Set-Content -Path $baselineFile -Encoding UTF8
    Write-Host "Baseline updated: $baselineFile"
}

if ($allPassed -or $Update) {
    Write-Host ""
    Write-Host "All tests passed." -ForegroundColor Green
    exit 0
} else {
    Write-Host ""
    Write-Host "Some tests failed. Findings differ by more than tolerance." -ForegroundColor Red
    exit 1
}
