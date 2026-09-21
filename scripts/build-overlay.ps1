# Build gameoverlay overlay DLL (requires MSVC / VS Build Tools).
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RepoRoot

$VcVars = "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path $VcVars)) {
    $VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $VsWhere) {
        $VsRoot = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($VsRoot) {
            $VcVars = Join-Path $VsRoot "VC\Auxiliary\Build\vcvars64.bat"
        }
    }
}

if (-not (Test-Path $VcVars)) {
    Write-Host "MSVC not found - skip overlay DLL build. Install VS 2022 Build Tools with C++ workload." -ForegroundColor Yellow
    exit 0
}

Write-Host "Building glint-overlay-dll (x64) with MSVC..." -ForegroundColor Cyan

& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $RepoRoot "scripts\ensure-detours.ps1")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$buildDll = "call `"$VcVars`" && cargo build --release -p glint-overlay-dll"
cmd /c $buildDll
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$OutDir = Join-Path $RepoRoot "target\release\glint-overlay"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$Dll = Join-Path $RepoRoot "target\release\glint_overlay.dll"
if (-not (Test-Path $Dll)) {
    $Dll = Join-Path $RepoRoot "target\release\deps\glint_overlay.dll"
}
if (-not (Test-Path $Dll)) {
    throw "Expected glint_overlay.dll not found in target\release"
}

function Copy-OverlayDll {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [int]$Attempts = 5,
        [int]$DelayMs = 400
    )
    $last = $null
    for ($i = 1; $i -le $Attempts; $i++) {
        try {
            Copy-Item -Force $Source $Destination
            return
        } catch {
            $last = $_
            $msg = $_.Exception.Message
            $locked = $msg -match 'user-mapped section|being used by another process|cannot access the file'
            if (-not $locked -or $i -eq $Attempts) { break }
            Write-Host "Destination locked (attempt $i/$Attempts); retry in ${DelayMs}ms..." -ForegroundColor Yellow
            Start-Sleep -Milliseconds $DelayMs
        }
    }
    Write-Host @"
Cannot copy overlay DLL to:
  $Destination

Windows still has the file memory-mapped (typical: game with Glint injected, or glint-browser still running).
Close the game / overlay host, then re-run this script.
"@ -ForegroundColor Red
    throw $last
}

$DllDest = Join-Path $OutDir "glint_overlay-x64.dll"
Copy-OverlayDll -Source $Dll -Destination $DllDest
Write-Host "Copied -> $DllDest" -ForegroundColor Green

$PkgDir = Join-Path $RepoRoot "host\native"
New-Item -ItemType Directory -Force -Path $PkgDir | Out-Null
Copy-OverlayDll -Source $Dll -Destination (Join-Path $PkgDir "glint_overlay-x64.dll")
Write-Host "Copied -> host\native\glint_overlay-x64.dll" -ForegroundColor Green

Write-Host "Overlay native artifacts ready." -ForegroundColor Green
