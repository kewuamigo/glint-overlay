[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$SkipInstall,
    [switch]$NoRust,
    [switch]$NoNode,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoRoot

# TypeScript / Vite outputs for the active production stack (see package.json "build" filters).
$NodeDistPaths = @(
    "ui/shell/dist",
    "ui/launcher/dist",
    "sdk/plugin/dist",
    "packages/static-server/dist",
    "sdk/bridge/dist",
    "host/launcher/dist",
    "internal-apps/metrics/dist",
    "internal-apps/browser/dist",
    "internal-apps/achievements/dist"
)

function Invoke-Step {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$Command
    )

    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    Write-Host "    $Command" -ForegroundColor DarkGray

    if ($DryRun) {
        return
    }

    & powershell -NoProfile -ExecutionPolicy Bypass -Command $Command
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed: $Name (exit code $LASTEXITCODE)"
    }
}

function Remove-DistOutputs {
    foreach ($rel in $NodeDistPaths) {
        $full = Join-Path $RepoRoot $rel
        if (Test-Path $full) {
            Write-Host "    removing $rel" -ForegroundColor DarkGray
            Remove-Item $full -Recurse -Force
        }
    }
}

function Assert-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CommandName
    )

    if (-not (Get-Command $CommandName -ErrorAction SilentlyContinue)) {
        throw "Required command not found on PATH: $CommandName"
    }
}

Write-Host "Glint full build" -ForegroundColor Green
Write-Host "Repo: $RepoRoot"

if (-not $NoRust) {
    Assert-Command "cargo"
}

if (-not $NoNode) {
    Assert-Command "node"
    Assert-Command "npm"
}

if ($Clean) {
    if (-not $NoRust) {
        Invoke-Step "Clean Rust target" "cargo clean"
    }

    if (-not $NoNode) {
        if ($DryRun) {
            Write-Host ""
            Write-Host "==> Clean TypeScript/Vite outputs" -ForegroundColor Cyan
            foreach ($rel in $NodeDistPaths) {
                Write-Host "    would remove $rel" -ForegroundColor DarkGray
            }
        } else {
            Write-Host ""
            Write-Host "==> Clean TypeScript/Vite outputs" -ForegroundColor Cyan
            Remove-DistOutputs
        }
    }
}

if (-not $NoNode -and -not $SkipInstall) {
    Invoke-Step "Install/update Node dependencies" "npm run install:deps"
}

if (-not $NoRust) {
    # overlay-hook needs Detours before any cargo build (CI checkout has empty gitlink).
    Invoke-Step "Ensure Microsoft Detours" "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ensure-detours.ps1"
    Invoke-Step "Build Rust release binaries" "cargo build --release"
    Invoke-Step "Build overlay DLL (MSVC)" "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-overlay.ps1"
    Invoke-Step "Build + stage glint-cef.exe" "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-cef.ps1"
    Invoke-Step "Stage glint-browser.exe" "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-browser.ps1"
}

if (-not $NoNode) {
    Invoke-Step "Build active Node packages + builtin apps" "npm run build"
}

Write-Host ""
Write-Host "Build complete." -ForegroundColor Green

if (-not $NoRust) {
    Write-Host ""
    Write-Host "Rust outputs:" -ForegroundColor Yellow
    Write-Host "  Launcher:     target\release\glint-launcher.exe"
    Write-Host "  ETW metrics:  target\release\glint-metrics-etw.exe"
    Write-Host "  Overlay DLL:  target\release\glint-overlay\glint_overlay-x64.dll"
    Write-Host "  Browser exe:  host\native\glint-browser.exe"
}

if (-not $NoNode) {
    Write-Host ""
    Write-Host "Node outputs:" -ForegroundColor Yellow
    Write-Host "  Launcher UI:   ui\launcher\dist + host\launcher\dist"
    Write-Host "  Shell UI:      ui\shell\dist"
    Write-Host "  Builtin apps:  internal-apps\metrics\dist, internal-apps\browser\dist, internal-apps\achievements\dist"
    Write-Host ""
    Write-Host "Note: in-game UI is CEF (glint-browser); Electron is launcher-only." -ForegroundColor DarkGray
    Write-Host "Cloud save sync lives in launcher Settings → Cloud Sync (not an overlay app)." -ForegroundColor DarkGray
}
