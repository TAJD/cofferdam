# Manual release helper (PowerShell). Builds a Windows binary locally and
# uploads it to a GitHub release. Useful for cutting a release without
# waiting on the full CI matrix - typically v0.1.x patch cuts.
#
# Usage:
#   pwsh scripts/release.ps1 -Tag v0.1.0
#   pwsh scripts/release.ps1 -Tag v0.1.0 -Draft
#
# Requires: cargo (with the GNU toolchain on this Windows box, see README),
# gh CLI authenticated, git working tree clean.

[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)]
    [string]$Tag,
    [switch]$Draft,
    [switch]$SkipUpload
)

$ErrorActionPreference = 'Stop'

if ($Tag -notmatch '^v\d+\.\d+\.\d+(-[A-Za-z0-9.-]+)?$') {
    throw "Tag '$Tag' must match vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-PRERELEASE"
}

$expectedVersion = $Tag.TrimStart('v')
$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if ($cargoVersion -ne $expectedVersion) {
    throw "Cargo.toml version is $cargoVersion, expected $expectedVersion. Bump it first."
}

git diff --quiet
if ($LASTEXITCODE -ne 0) {
    throw "Working tree is dirty - commit or stash before releasing."
}

$target = 'x86_64-pc-windows-gnu'
Write-Host "Building cofferdam for $target ..." -ForegroundColor Cyan
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
cargo build --release --bin cofferdam --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$binary = "target/$target/release/cofferdam.exe"
if (-not (Test-Path $binary)) { throw "Binary not produced at $binary" }

$artifactName = "cofferdam-$Tag-$target"
New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item $binary dist/cofferdam.exe -Force
Copy-Item README.md, LICENSE dist/ -Force

$zipPath = "dist/$artifactName.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath }
Compress-Archive -Path dist/cofferdam.exe, dist/README.md, dist/LICENSE -DestinationPath $zipPath
$hash = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
$hash | Out-File -Encoding ascii "$zipPath.sha256"

Write-Host "Built: $zipPath ($hash)" -ForegroundColor Green

if ($SkipUpload) {
    Write-Host "Skipping upload (--SkipUpload)." -ForegroundColor Yellow
    return
}

$tagExists = (git tag --list $Tag) -ne ''
if (-not $tagExists) {
    Write-Host "Creating tag $Tag ..." -ForegroundColor Cyan
    git tag -a $Tag -m "release $Tag"
    git push origin $Tag
}

$ghArgs = @('release', 'create', $Tag, $zipPath, "$zipPath.sha256",
            '--title', "cofferdam $Tag",
            '--generate-notes')
if ($Draft) { $ghArgs += '--draft' }

Write-Host "Creating GH release ..." -ForegroundColor Cyan
& gh @ghArgs
if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }

Write-Host "Done." -ForegroundColor Green
