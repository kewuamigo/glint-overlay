# Build glint-browser and stage to host/native (same layout as overlay DLL).
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

Write-Host "Building glint-browser (release)..." -ForegroundColor Cyan
cargo build --release -p glint-browser
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
}
$Src = Join-Path $TargetDir "release\glint-browser.exe"
if (-not (Test-Path $Src)) {
    throw "Expected glint-browser.exe not found at $Src"
}

$PkgDir = Join-Path $RepoRoot "host\native"
New-Item -ItemType Directory -Force -Path $PkgDir | Out-Null
$Dest = Join-Path $PkgDir "glint-browser.exe"
Copy-Item -Force $Src $Dest
Write-Host "Copied -> host\native\glint-browser.exe" -ForegroundColor Green
