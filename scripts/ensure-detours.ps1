# Ensure Microsoft Detours sources exist for glint-overlay-hook.
# The tree is a legacy gitlink without .gitmodules, so CI checkouts have an
# empty directory; clone on demand (same as scripts/build-overlay.ps1).
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$DetoursDir = Join-Path $RepoRoot "core\overlay\overlay-hook\detours"
$Marker = Join-Path $DetoursDir "src\detours.cpp"

if (Test-Path $Marker) {
    Write-Host "Detours already present: $DetoursDir" -ForegroundColor DarkGray
    exit 0
}

Write-Host "Cloning Microsoft Detours for overlay-hook..." -ForegroundColor Cyan
if (Test-Path $DetoursDir) {
    Remove-Item -Recurse -Force $DetoursDir
}
git clone --depth 1 https://github.com/microsoft/Detours $DetoursDir
if ($LASTEXITCODE -ne 0) { throw "Failed to clone Microsoft Detours" }
if (-not (Test-Path $Marker)) {
    throw "Detours clone missing $Marker"
}
Write-Host "Detours ready." -ForegroundColor Green
