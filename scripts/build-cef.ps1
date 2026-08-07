# Build glint-cef.exe (requires scripts/fetch-cef.ps1 first + VS 2022).
# Also FetchContent-pulls protobuf 21.12 on first configure (network) and
# regenerates host/cef/src/ipc/gen from core/overlay/cef-protocol/proto/cef_ipc.proto.
param(
  [ValidateSet("Debug", "Release")]
  [string]$Config = "Release"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$CefCurrent = Join-Path $Root "third_party\cef\current"

if (-not (Test-Path (Join-Path $CefCurrent "cmake\FindCEF.cmake"))) {
  Write-Host "CEF missing - fetching..."
  & (Join-Path $PSScriptRoot "fetch-cef.ps1")
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "Visual Studio 2022 with C++ tools not found" }

$BuildDir = Join-Path $Root "host\cef\build"
New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null

Push-Location $BuildDir
try {
  $gen = Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
  if (-not (Test-Path $gen)) { $gen = "cmake" }
  # CMake string parsing breaks on Windows backslashes (\U in C:\Users\...).
  $CefRootCmake = ($CefCurrent -replace '\\', '/')
  Write-Host "Configuring glint-cef (protobuf FetchContent on first run)..."
  & $gen -G "Visual Studio 17 2022" -A x64 "-DCEF_ROOT=$CefRootCmake" ..
  if ($LASTEXITCODE -ne 0) { throw "cmake configure failed" }
  & $gen --build . --config $Config --parallel
  if ($LASTEXITCODE -ne 0) { throw "cmake build failed" }
} finally {
  Pop-Location
}

$staged = Join-Path $Root "host\native\cef\glint-cef.exe"
$genPb = Join-Path $Root "host\cef\src\ipc\gen\cef_ipc.pb.h"
Write-Host "Build finished. Staged exe:"
Write-Host $staged
Write-Host "Proto gen:"
Write-Host $genPb
if (-not (Test-Path $genPb)) {
  Write-Warning "Proto gen missing at $genPb - CMake protoc custom command may have failed"
}
