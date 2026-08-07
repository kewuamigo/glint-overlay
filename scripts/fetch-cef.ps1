# Download Chromium Embedded Framework (Windows x64 standard) into third_party/cef.
# Binaries are gitignored ??? run this before building host/cef.
param(
  [string]$Version = "150.0.11+gb887805+chromium-150.0.7871.115",
  [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $Root "third_party\cef" }

$FileName = "cef_binary_${Version}_windows64.tar.bz2"
$Url = "https://cef-builds.spotifycdn.com/$FileName"
$Cache = Join-Path $OutDir "cache"
$Archive = Join-Path $Cache $FileName
$Extracted = Join-Path $OutDir "cef_binary_${Version}_windows64"
$CurrentLink = Join-Path $OutDir "current"

New-Item -ItemType Directory -Force -Path $Cache | Out-Null

if (-not (Test-Path $Archive)) {
  Write-Host "Downloading CEF $Version (~330MB)..."
  Write-Host "  $Url"
  curl.exe -L --fail --retry 3 -o $Archive $Url
  if ($LASTEXITCODE -ne 0) { throw "CEF download failed" }
} else {
  Write-Host "Using cached archive: $Archive"
}

if (-not (Test-Path (Join-Path $Extracted "CMakeLists.txt"))) {
  Write-Host "Extracting..."
  if (Test-Path $Extracted) { Remove-Item -Recurse -Force $Extracted }
  # Windows 10+ tar supports .tar.bz2
  tar -xjf $Archive -C $OutDir
  if ($LASTEXITCODE -ne 0) { throw "CEF extract failed (need tar with bzip2 support)" }
}

if (Test-Path $CurrentLink) { Remove-Item -Force $CurrentLink -ErrorAction SilentlyContinue }
# Directory junction so CMake always points at third_party/cef/current
cmd /c mklink /J "$CurrentLink" "$Extracted" | Out-Null
if (-not (Test-Path $CurrentLink)) {
  # Fallback: copy marker file with path
  Set-Content -Path (Join-Path $OutDir "CURRENT_PATH.txt") -Value $Extracted
}

Write-Host "CEF ready:"
Write-Host "  $Extracted"
Write-Host "Build helper:"
Write-Host "  pwsh scripts/build-cef.ps1"

