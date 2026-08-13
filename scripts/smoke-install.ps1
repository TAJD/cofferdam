# Hermetic install smoke test for the `cofferdam` npm package (cd-u58).
# Windows-native PowerShell mirror of scripts/smoke-install.sh.
#
# Local repro:
#   cargo build --release --bin cofferdam
#   .\scripts\smoke-install.ps1
#
# Override the binary location:
#   $env:COFFERDAM_BINARY_PATH = "C:\path\to\cofferdam.exe"
#   .\scripts\smoke-install.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot  = Resolve-Path (Join-Path $ScriptDir "..")

$BinName = if ($IsWindows -or $env:OS -eq "Windows_NT") { "cofferdam.exe" } else { "cofferdam" }

$Binary = if ($env:COFFERDAM_BINARY_PATH) {
  $env:COFFERDAM_BINARY_PATH
} else {
  Join-Path $RepoRoot "target\release\$BinName"
}

if (-not (Test-Path -LiteralPath $Binary)) {
  Write-Error @"
cofferdam binary not found at $Binary
Build it first:  cargo build --release --bin cofferdam
Or set `$env:COFFERDAM_BINARY_PATH to point at an existing binary.
"@
  exit 1
}

$PkgDir = Join-Path $RepoRoot "packages\cofferdam"
Push-Location $PkgDir
try {
  Write-Host "==> npm pack ($PkgDir)"
  $packJson = & npm pack --json --silent
  if ($LASTEXITCODE -ne 0) { throw "npm pack failed (exit $LASTEXITCODE)" }
  $packArr = $packJson | ConvertFrom-Json
  $tarball = $packArr[0].filename
  $tarballAbs = Join-Path $PkgDir $tarball

  $WorkDir = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("cofferdam-smoke-" + [System.IO.Path]::GetRandomFileName()))
  try {
    Write-Host "==> workdir: $WorkDir"
    Push-Location $WorkDir
    try {
      & npm init -y | Out-Null
      if ($LASTEXITCODE -ne 0) { throw "npm init failed (exit $LASTEXITCODE)" }

      Write-Host "==> npm install (COFFERDAM_BINARY_PATH=$Binary)"
      $env:COFFERDAM_BINARY_PATH = $Binary
      try {
        & npm install --no-audit --no-fund --foreground-scripts $tarballAbs
        if ($LASTEXITCODE -ne 0) { throw "npm install failed (exit $LASTEXITCODE)" }
      } finally {
        Remove-Item Env:COFFERDAM_BINARY_PATH -ErrorAction SilentlyContinue
      }

      $InstalledBin = Join-Path $WorkDir "node_modules\@cofferdam\cofferdam\bin\$BinName"
      if (-not (Test-Path -LiteralPath $InstalledBin)) {
        Write-Error "FAIL: binary not at $InstalledBin"
        Get-ChildItem (Join-Path $WorkDir "node_modules\@cofferdam\cofferdam\bin") -ErrorAction SilentlyContinue | Format-Table | Out-String | Write-Host
        exit 1
      }
      Write-Host "OK: binary present at $InstalledBin"

      $Fixture = Join-Path $RepoRoot "examples\smoke_fixture.ts"
      Write-Host "==> npx cofferdam check $Fixture --format json"

      # Run npx and capture stdout — don't let a non-zero exit explode out of $PSNativeCommandUseErrorActionPreference.
      $prev = $PSNativeCommandUseErrorActionPreference
      $PSNativeCommandUseErrorActionPreference = $false
      try {
        $out = & npx --no-install cofferdam check $Fixture --format json
        $ec  = $LASTEXITCODE
      } finally {
        $PSNativeCommandUseErrorActionPreference = $prev
      }

      if ($ec -ne 0 -and $ec -ne 1) {
        Write-Error "FAIL: unexpected exit code $ec from cofferdam check"
        Write-Host "--- stdout ---"
        Write-Host ($out -join "`n")
        exit 1
      }

      $stdout = $out -join "`n"
      try {
        $report = $stdout | ConvertFrom-Json
      } catch {
        Write-Error "FAIL: stdout was not valid JSON: $($_.Exception.Message)"
        Write-Host "--- raw stdout ---"
        Write-Host $stdout
        exit 1
      }

      if (-not $report.findings -or $report.findings.Count -eq 0) {
        Write-Error "FAIL: zero findings against examples\smoke_fixture.ts"
        exit 1
      }

      $readonlyArrayParam = $report.findings | Where-Object { $_.id -eq "Design.ReadonlyArrayParam" } | Select-Object -First 1
      if (-not $readonlyArrayParam) {
        $ids = ($report.findings | ForEach-Object { $_.id }) -join ", "
        Write-Error "FAIL: Design.ReadonlyArrayParam not in findings (got: $ids)"
        exit 1
      }
      $loc = "$($readonlyArrayParam.file):$($readonlyArrayParam.line)"
      Write-Host "OK: Design.ReadonlyArrayParam at $loc"

      Write-Host ""
      Write-Host "[OK] smoke install passed (binary=$Binary)"
    } finally {
      Pop-Location
    }
  } finally {
    # Best-effort cleanup; node_modules can briefly hold locks on Windows.
    Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
  }
} finally {
  Pop-Location
  if ($tarball) {
    Remove-Item -LiteralPath (Join-Path $PkgDir $tarball) -Force -ErrorAction SilentlyContinue
  }
}

# Cleanup may leave $LASTEXITCODE non-zero (e.g. node_modules holds a
# file lock on Windows during Remove-Item -Recurse). The script body
# itself succeeded — explicitly exit 0 so the runner doesn't flag
# cleanup noise as a smoke-test failure.
exit 0
