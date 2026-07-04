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

.PARAMETER Memory
Run the CD-35 memory baseline gate instead of the findings-count smoke
test: launch the binary against the checked-in `examples/` corpus (portable
to CI, unlike the -repos corpus below which lives on the dev machine only),
capture peak RSS, and fail if it grew more than the baseline's tolerance.
Combine with -Update to record a fresh baseline.

.EXAMPLE
.\scripts\smoke.ps1 -Debug
.\scripts\smoke.ps1 -Update
.\scripts\smoke.ps1 -Memory
.\scripts\smoke.ps1 -Memory -Update
#>

param(
    [switch]$Debug,
    [switch]$Update,
    [switch]$Memory
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

if ($Memory) {
    # CD-35 memory baseline gate. Uses `examples/` (checked into the
    # repo) rather than the `$baseline.repos` corpus above, which
    # points at local-only dev-machine repos and would no-op on CI.
    #
    # `Process.PeakWorkingSet64` reads /proc/<pid>/status VmHWM on
    # Linux and the Windows working-set counter on Windows via the
    # same .NET API, so this branch runs unmodified on both CI OSes
    # (pwsh ships on ubuntu-latest and windows-latest runners).
    $corpus = Join-Path $workspaceRoot "examples"
    if (-not (Test-Path $corpus)) {
        Write-Error "Corpus not found: $corpus"
        exit 1
    }

    Write-Host "Running memory gate with $profile binary: $binary"
    Write-Host "Corpus: $corpus"

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $binary
    $psi.ArgumentList.Add("check")
    $psi.ArgumentList.Add($corpus)
    $psi.ArgumentList.Add("--no-cache")
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    # Drain stdout/stderr asynchronously *before* WaitForExit — `check`
    # can emit more output than the pipe buffer holds, and waiting
    # synchronously first would deadlock (child blocks writing, we
    # block waiting).
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()

    # `Process` properties are a point-in-time snapshot taken at
    # `Start()` — `PeakWorkingSet64` reads back 0 unless `Refresh()` is
    # called *while the process is still alive*; querying only after
    # `WaitForExit()` reliably returns nothing. Poll until exit,
    # tracking the max observed value ourselves as a safety net against
    # missing the true peak between polls.
    $peakRssBytes = 0
    while (-not $proc.HasExited) {
        try {
            $proc.Refresh()
            if ($proc.PeakWorkingSet64 -gt $peakRssBytes) {
                $peakRssBytes = $proc.PeakWorkingSet64
            }
        } catch {}
    }
    $proc.WaitForExit()
    $stdoutTask.Wait()
    $stderrTask.Wait()
    $peakRssKb = [Math]::Round($peakRssBytes / 1KB)
    $proc.Close()

    Write-Host "Peak RSS: $peakRssKb KB"

    if ($Update) {
        $memoryEntry = [PSCustomObject]@{
            corpus       = "examples"
            peak_rss_kb  = $peakRssKb
            tolerance_pct = 20
        }
        $baseline | Add-Member -MemberType NoteProperty -Name "memory" -Value $memoryEntry -Force
        $baseline | ConvertTo-Json -Depth 5 | Set-Content -Path $baselineFile -Encoding UTF8
        Write-Host "Memory baseline updated: $baselineFile"
        exit 0
    }

    if (-not $baseline.memory) {
        Write-Error "No 'memory' baseline entry in $baselineFile. Run with -Memory -Update first."
        exit 1
    }

    $baselineKb = $baseline.memory.peak_rss_kb
    $tolerancePct = $baseline.memory.tolerance_pct
    $growthPct = (($peakRssKb - $baselineKb) / $baselineKb) * 100

    Write-Host "Baseline: $baselineKb KB, tolerance: +$tolerancePct%, growth: $([Math]::Round($growthPct, 1))%"

    if ($growthPct -gt $tolerancePct) {
        Write-Host ""
        Write-Host "Memory regression: peak RSS grew $([Math]::Round($growthPct, 1))% vs baseline (gate: +$tolerancePct%)." -ForegroundColor Red
        exit 1
    }

    Write-Host ""
    Write-Host "Memory gate passed." -ForegroundColor Green
    exit 0
}

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

    # Run cofferdam and measure time. Native-command stderr lines arrive
    # wrapped as ErrorRecords; we explicitly drop ErrorAction to Continue
    # so a non-zero exit (e.g. "no TypeScript files found") doesn't halt
    # the harness mid-loop.
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $output = & {
        $ErrorActionPreference = "Continue"
        & $binary check "$repoPath" 2>&1
    }
    $sw.Stop()

    # Count only finding lines — ignore stderr noise like
    # "no TypeScript files found" so a fully-skipped repo registers as 0
    # rather than failing the harness.
    $stdoutLines = @($output | Where-Object { $_ -is [string] })
    $actualFindings = $stdoutLines.Count
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
