$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$appsRoot = "$env:APPDATA\Glint\apps"

function Install-DevApp {
    param(
        [string]$Id,
        [string]$SourceDir,
        [hashtable]$StoreSeed
    )

    $dest = Join-Path $appsRoot $Id
    $dataDir = Join-Path $dest "data"
    $storePath = Join-Path $dataDir "store.json"

    Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

    Copy-Item (Join-Path $SourceDir "manifest.json") $dest\
    $distSrc = Join-Path $SourceDir "dist"
    if (-not (Test-Path $distSrc)) { throw "Missing $distSrc" }
    Copy-Item -Recurse -Force $distSrc (Join-Path $dest "dist")

    if ($StoreSeed) {
        $json = $StoreSeed | ConvertTo-Json
        [System.IO.File]::WriteAllText($storePath, $json, [System.Text.UTF8Encoding]::new($false))
    }

    Write-Host "Installed $Id app to $dest"
}

New-Item -ItemType Directory -Force -Path $appsRoot | Out-Null

# Built-in apps (metrics, browser, achievements) load from repo internal-apps/.
# Cloud save sync is launcher Settings → Cloud Sync (no overlay Save Manager).
# Call Install-DevApp here when developing an external AppData app.

# Unregister leftover Save Manager app install without deleting Ludusavi cache
# (packages/save-manifest still uses apps/save-manager/data).
$legacySaveManager = Join-Path $appsRoot "save-manager"
if (Test-Path $legacySaveManager) {
    Remove-Item -Force (Join-Path $legacySaveManager "manifest.json") -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force (Join-Path $legacySaveManager "dist") -ErrorAction SilentlyContinue
    Remove-Item -Force (Join-Path $legacySaveManager "index.js") -ErrorAction SilentlyContinue
    Write-Host "Unregistered legacy Save Manager app under $legacySaveManager (kept data/)"
}

Write-Host "No bundled external apps to install under $appsRoot"
Write-Host "Built-ins ship from internal-apps/; cloud sync is launcher-only."
