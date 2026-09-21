# Stage Glint release payload and optionally build Inno Setup installer.
[CmdletBinding()]
param(
    [string]$Version = "",
    [switch]$SkipBuild,
    [switch]$SkipInno,
    [switch]$PortableZip
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Resolve-Version {
    param([string]$Explicit)
    if ($Explicit) { return $Explicit.TrimStart("v") }
    $tag = ""
    try { $tag = (git describe --tags --exact-match 2>$null) } catch { }
    if ($tag) { return $tag.TrimStart("v") }
    $cargo = Get-Content (Join-Path $RepoRoot "core\launcher\launcher\Cargo.toml") -Raw
    if ($cargo -match 'version\s*=\s*"([^"]+)"') { return $Matches[1] }
    return "0.1.0"
}

function Find-ElectronDist {
    $candidates = @(
        "host\launcher\node_modules\electron\dist",
        "node_modules\electron\dist"
    )
    $pnpm = Get-ChildItem -Path (Join-Path $RepoRoot "node_modules\.pnpm") -Directory -Filter "electron@*" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($pnpm) {
        $candidates = @((Join-Path $pnpm.FullName "node_modules\electron\dist")) + $candidates
    }

    function Resolve-DistPath([string]$rel) {
        if ([IO.Path]::IsPathRooted($rel)) { return $rel }
        return (Join-Path $RepoRoot $rel)
    }

    foreach ($rel in $candidates) {
        $full = Resolve-DistPath $rel
        if (Test-Path (Join-Path $full "electron.exe")) { return $full }
    }

    # pnpm 10 may install the electron package but skip postinstall (binary download)
    # unless electron is in onlyBuiltDependencies. Recover by running install.js.
    foreach ($rel in $candidates) {
        $dist = Resolve-DistPath $rel
        $pkg = Split-Path -Parent $dist
        $installJs = Join-Path $pkg "install.js"
        if (-not (Test-Path $installJs)) { continue }
        Write-Host "electron.exe missing under $pkg - running install.js" -ForegroundColor Yellow
        Push-Location $pkg
        try {
            & node .\install.js
            if ($LASTEXITCODE -ne 0) { throw "electron install.js failed ($LASTEXITCODE)" }
        } finally {
            Pop-Location
        }
        if (Test-Path (Join-Path $dist "electron.exe")) { return $dist }
    }

    throw "electron.exe dist not found after install.js. Ensure pnpm onlyBuiltDependencies includes electron, then run npm run install:deps."
}

function Find-ISCC {
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    )
    foreach ($p in $candidates) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

function Copy-Tree {
    param([string]$Src, [string]$Dest)
    if (-not (Test-Path $Src)) { throw "Missing source: $Src" }
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    # robocopy handles long paths / junctions better than Copy-Item on pnpm trees
    & robocopy $Src $Dest /E /NFL /NDL /NJH /NJS /NC /NS /NP /R:1 /W:1 | Out-Null
    $code = $LASTEXITCODE
    if ($code -ge 8) {
        throw "robocopy failed ($code): $Src -> $Dest"
    }
}

function Find-ReleaseArtifact {
    param([Parameter(Mandatory = $true)][string]$Name)
    $candidates = @()
    if ($env:CARGO_TARGET_DIR) {
        $candidates += (Join-Path $env:CARGO_TARGET_DIR "release\$Name")
        $candidates += (Join-Path $env:CARGO_TARGET_DIR "release\glint-overlay\$Name")
    }
    $candidates += (Join-Path $RepoRoot "target\release\$Name")
    $candidates += (Join-Path $RepoRoot "target\release\glint-overlay\$Name")
    $candidates += (Join-Path $RepoRoot "host\native\$Name")
    foreach ($p in $candidates) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

$Version = Resolve-Version -Explicit $Version
Write-Host "Packaging Glint $Version" -ForegroundColor Green

if (-not $SkipBuild) {
    Write-Host "==> Full build (build-all.ps1)" -ForegroundColor Cyan
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $RepoRoot "build-all.ps1")
    if ($LASTEXITCODE -ne 0) { throw "build-all.ps1 failed" }
}

$LauncherExe = Find-ReleaseArtifact "glint-launcher.exe"
if (-not $LauncherExe) {
    throw "Missing glint-launcher.exe - build launcher first"
}

$Payload = Join-Path $RepoRoot "dist\release\payload"
if (Test-Path $Payload) { Remove-Item $Payload -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Payload | Out-Null

Copy-Item -Force $LauncherExe (Join-Path $Payload "Glint.exe")

$NativeDest = Join-Path $Payload "native"
New-Item -ItemType Directory -Force -Path $NativeDest | Out-Null

# Required native artifacts — search CARGO_TARGET_DIR, repo target/, and host/native.
$requiredNative = @(
    "glint-browser.exe",
    "glint_overlay-x64.dll",
    "glint_metrics_native.dll",
    "glint_achievements_native.dll",
    "glint-metrics-etw.exe"
)
foreach ($name in $requiredNative) {
    $src = Find-ReleaseArtifact $name
    if (-not $src) {
        throw "Missing $name (searched CARGO_TARGET_DIR, target/release, host/native). Run build-all.ps1."
    }
    Write-Host "    native: $name <- $src" -ForegroundColor DarkGray
    Copy-Item -Force $src $NativeDest
}

$cefSrc = Join-Path $RepoRoot "host\native\cef"
if (Test-Path $cefSrc) {
    Copy-Tree -Src $cefSrc -Dest (Join-Path $NativeDest "cef")
}

$ElectronSrc = Find-ElectronDist
$ElectronDest = Join-Path $Payload "host\electron"
Copy-Tree -Src $ElectronSrc -Dest $ElectronDest

# Stage launcher JS + real (non-junction) node_modules for offline install.
$LauncherHost = Join-Path $Payload "host\launcher"
if (Test-Path $LauncherHost) { Remove-Item $LauncherHost -Recurse -Force }
New-Item -ItemType Directory -Force -Path $LauncherHost | Out-Null
Copy-Tree -Src (Join-Path $RepoRoot "host\launcher\dist") -Dest (Join-Path $LauncherHost "dist")
Copy-Item -Force (Join-Path $RepoRoot "host\launcher\package.json") $LauncherHost
$preload = Join-Path $RepoRoot "host\launcher\preload.cjs"
if (Test-Path $preload) { Copy-Item -Force $preload $LauncherHost }

# Install npm deps as real files (no pnpm junctions) then rebuild native module for Electron.
Write-Host "==> npm install launcher runtime deps" -ForegroundColor Cyan
$NmInstall = Join-Path $Payload "host\_npm_runtime"
if (Test-Path $NmInstall) { Remove-Item $NmInstall -Recurse -Force }
New-Item -ItemType Directory -Force -Path $NmInstall | Out-Null
@'
{
  "name": "glint-launcher-runtime",
  "private": true,
  "dependencies": {
    "better-sqlite3": "12.11.1",
    "zod": "3.24.2",
    "tar": "7.5.22",
    "basic-ftp": "5.0.5",
    "yaml": "2.8.0"
  }
}
'@ | Set-Content -Path (Join-Path $NmInstall "package.json") -Encoding UTF8
Push-Location $NmInstall
try {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & npm install --omit=dev --no-fund --no-audit
    $npmExit = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    if ($npmExit -ne 0) { throw "npm install runtime deps failed ($npmExit)" }
} finally {
    Pop-Location
}

$NmDest = Join-Path $LauncherHost "node_modules"
if (Test-Path $NmDest) { Remove-Item $NmDest -Recurse -Force }
Copy-Tree -Src (Join-Path $NmInstall "node_modules") -Dest $NmDest
Remove-Item $NmInstall -Recurse -Force -ErrorAction SilentlyContinue

# Workspace packages last so they are never shadowed by npm.
$NmGo = Join-Path $NmDest "@glint"
New-Item -ItemType Directory -Force -Path $NmGo | Out-Null
foreach ($pkg in @("achievements-core", "static-server", "cloud-sync", "save-manifest")) {
    $src = Join-Path $RepoRoot "packages\$pkg"
    $dest = Join-Path $NmGo $pkg
    $dist = Join-Path $src "dist"
    if (-not (Test-Path $dist)) {
        throw "Missing $dist - build @glint/$pkg first (npm run build)"
    }
    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item -Force (Join-Path $src "package.json") $dest
    Copy-Tree -Src $dist -Dest (Join-Path $dest "dist")
}

# Fail packaging early if launcher cannot resolve cloud-sync / save-manifest.
Write-Host "==> Verify staged launcher workspace imports" -ForegroundColor Cyan
Push-Location $LauncherHost
try {
    & node --input-type=module -e "import '@glint/cloud-sync'; import '@glint/save-manifest'; import '@glint/achievements-core'; import '@glint/static-server'; console.log('workspace imports ok')"
    if ($LASTEXITCODE -ne 0) { throw "staged workspace import check failed" }
} finally {
    Pop-Location
}

Write-Host "==> electron-rebuild better-sqlite3 for staged Electron" -ForegroundColor Cyan
$electronVersion = $null
$electronPkg = Join-Path $ElectronDest "version"
if (Test-Path $electronPkg) {
    $electronVersion = (Get-Content $electronPkg -Raw).Trim()
}
Push-Location $LauncherHost
try {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    if ($electronVersion) {
        & npx --yes @electron/rebuild@4.2.0 -f -w better-sqlite3 -v $electronVersion
    } else {
        & npx --yes @electron/rebuild@4.2.0 -f -w better-sqlite3
    }
    $rebuildExit = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    if ($rebuildExit -ne 0) { throw "electron-rebuild failed ($rebuildExit)" }
} finally {
    Pop-Location
}

$UiLauncher = Join-Path $RepoRoot "ui\launcher\dist"
Copy-Tree -Src $UiLauncher -Dest (Join-Path $Payload "host\ui")

$ShellDist = Join-Path $RepoRoot "ui\shell\dist"
Copy-Tree -Src $ShellDist -Dest (Join-Path $Payload "ui\shell\dist")

# Store catalog for packaged install
$Catalog = Join-Path $RepoRoot "config\store-catalog.json"
if (Test-Path $Catalog) {
    $cfgDest = Join-Path $Payload "config"
    New-Item -ItemType Directory -Force -Path $cfgDest | Out-Null
    Copy-Item -Force $Catalog $cfgDest
}

# Builtin apps: scan expects internal-apps/<id>/manifest.json + entry file.
Write-Host "==> Stage builtin apps" -ForegroundColor Cyan
foreach ($app in @("metrics", "browser", "achievements")) {
    $srcRoot = Join-Path $RepoRoot "internal-apps\$app"
    $manifest = Join-Path $srcRoot "manifest.json"
    $dist = Join-Path $srcRoot "dist"
    if (-not (Test-Path $manifest)) { throw "Missing $manifest" }
    if (-not (Test-Path $dist)) { throw "Missing $dist - build apps first (npm run build)" }
    $destRoot = Join-Path $Payload "internal-apps\$app"
    New-Item -ItemType Directory -Force -Path $destRoot | Out-Null
    Copy-Item -Force $manifest $destRoot
    Copy-Tree -Src $dist -Dest (Join-Path $destRoot "dist")
}

# React shims for glint-plugin://_shared/*
$sharedSrc = Join-Path $RepoRoot "host\cef\plugin-shared"
if (-not (Test-Path $sharedSrc)) { throw "Missing $sharedSrc" }
Copy-Tree -Src $sharedSrc -Dest (Join-Path $Payload "host\cef\plugin-shared")

# Bake product version for OTA comparison (UTF-8, no BOM — JSON.parse).
$utf8NoBom = New-Object System.Text.UTF8Encoding $false
$versionJsonBody = "{`"version`":`"$Version`"}"
$versionJsonPaths = @(
    (Join-Path $Payload "version.json"),
    (Join-Path $LauncherHost "version.json")
)
foreach ($vp in $versionJsonPaths) {
    [System.IO.File]::WriteAllText($vp, $versionJsonBody, $utf8NoBom)
    Write-Host "    version.json -> $vp" -ForegroundColor DarkGray
}

Write-Host "Payload staged at $Payload" -ForegroundColor Green

$OutDir = Join-Path $RepoRoot "dist\release"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if ($PortableZip) {
    $zip = Join-Path $OutDir "Glint-portable-$Version.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $Payload "*") -DestinationPath $zip
    Write-Host "Portable zip: $zip" -ForegroundColor Green
}

if (-not $SkipInno) {
    $iscc = Find-ISCC
    if (-not $iscc) {
        Write-Host "Inno Setup 6 (ISCC.exe) not found - skipping installer. Install from https://jrsoftware.org/isinfo.php" -ForegroundColor Yellow
        Write-Host "Payload is still ready under dist\release\payload" -ForegroundColor Yellow
    } else {
        $iss = Join-Path $RepoRoot "packaging\windows\Glint.iss"
        & $iscc "/DMyAppVersion=$Version" $iss
        if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
        $setup = Join-Path $OutDir "Glint-Setup-$Version.exe"
        if (-not (Test-Path $setup)) { throw "Expected installer missing: $setup" }
        $shaHex = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToLowerInvariant()
        $shaName = "Glint-Setup-$Version.exe.sha256"
        $shaPath = Join-Path $OutDir $shaName
        $shaLine = "$shaHex  Glint-Setup-$Version.exe`n"
        [System.IO.File]::WriteAllText($shaPath, $shaLine, $utf8NoBom)
        Write-Host "Installer: $setup" -ForegroundColor Green
        Write-Host "Checksum:  $shaPath" -ForegroundColor Green
    }
}

Write-Host "Done." -ForegroundColor Green