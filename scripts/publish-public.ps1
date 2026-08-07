#Requires -Version 5.1
<#
.SYNOPSIS
  Export a denylist-clean product tree for the public Glint repository.

.DESCRIPTION
  Copies allowlisted paths from the private monorepo into -OutDir (default:
  .\.tmp-public-export). Overlays curated public docs from docs/internal/public/.
  Fails if any denylist path appears in the export.

.PARAMETER OutDir
  Destination directory for the export (created/replaced).

.PARAMETER PublicRepoDir
  Optional existing public clone. After a successful export, syncs export → this
  dir (excluding .git) so you can commit there.

.PARAMETER Message
  If -PublicRepoDir is set, run git add -A && git commit -m Message (no push).

.PARAMETER SkipClean
  Do not wipe OutDir before copying (default: wipe).
#>
[CmdletBinding()]
param(
  [string] $OutDir = "",
  [string] $PublicRepoDir = "",
  [string] $Message = "",
  [switch] $SkipClean
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
if (-not $OutDir) {
  $OutDir = Join-Path $RepoRoot ".tmp-public-export"
}

$AllowDirs = @(
  "core",
  "host",
  "ui",
  "sdk",
  "internal-apps",
  "packages",
  "scripts",
  "config",
  "packaging",
  ".github"
)

$AllowRootFiles = @(
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "pnpm-workspace.yaml",
  "build-all.ps1",
  ".gitignore",
  ".npmrc",
  "LICENSE",
  "LICENSE.md",
  "LICENSE-MIT",
  "LICENSE-APACHE"
)

$DenyNameExact = @(
  ".cursor",
  ".claude",
  ".agents",
  ".superpowers",
  "openspec",
  "specs",
  "AGENT.md",
  "skills-lock.json",
  "SpinningCube.exe",
  "node_modules",
  "target",
  "dist",
  ".tmp-public-export"
)

function Test-IsDeniedRelative([string] $Rel) {
  $norm = $Rel.Replace("\", "/")
  if ($norm -match '(^|/)\.git(/|$)') { return $false }
  foreach ($d in $DenyNameExact) {
    if ($norm -eq $d -or $norm -like "$d/*" -or $norm -like "*/$d" -or $norm -like "*/$d/*") {
      return $true
    }
  }
  if ($norm -eq "docs/internal" -or $norm -like "docs/internal/*") { return $true }
  if ($norm -eq "docs/superpowers" -or $norm -like "docs/superpowers/*") { return $true }
  if ($norm -like "*.log") { return $true }
  if ($norm -like "tmp-*" -or $norm -like "*/tmp-*" -or $norm -like "tmp-*/" -or $norm -like "*/tmp-*/") { return $true }
  if ($norm -like ".cursor/tmp-*" -or $norm -like "*/.cursor/tmp-*") { return $true }
  return $false
}

function Copy-AllowDir([string] $Name) {
  $src = Join-Path $RepoRoot $Name
  if (-not (Test-Path $src)) {
    Write-Host "skip missing dir: $Name"
    return
  }
  $dst = Join-Path $OutDir $Name
  Write-Host "copy dir: $Name"
  # Robocopy-like: exclude heavy/build dirs inside packages
  $excludeDirs = @("node_modules", "target", "dist", ".vite", "cloud-sync-data")
  Get-ChildItem $src -Force | ForEach-Object {
    if ($excludeDirs -contains $_.Name) { return }
    $rel = Join-Path $Name $_.Name
    if (Test-IsDeniedRelative $rel) { return }
    $target = Join-Path $OutDir $rel
    if ($_.PSIsContainer) {
      New-Item -ItemType Directory -Force -Path $target | Out-Null
      # recursive copy with denylist filter
      Copy-TreeFiltered -Src $_.FullName -Dst $target -RelPrefix $rel
    } else {
      New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
      Copy-Item -LiteralPath $_.FullName -Destination $target -Force
    }
  }
}

function Copy-TreeFiltered([string] $Src, [string] $Dst, [string] $RelPrefix) {
  $excludeDirs = @("node_modules", "target", "dist", ".vite", "cloud-sync-data", ".git")
  Get-ChildItem -LiteralPath $Src -Force | ForEach-Object {
    if ($excludeDirs -contains $_.Name) { return }
    $rel = Join-Path $RelPrefix $_.Name
    if (Test-IsDeniedRelative $rel) {
      Write-Host "  deny skip: $rel"
      return
    }
    $target = Join-Path $OutDir ($rel.Replace("/", "\"))
    if ($_.PSIsContainer) {
      New-Item -ItemType Directory -Force -Path $target | Out-Null
      Copy-TreeFiltered -Src $_.FullName -Dst $target -RelPrefix $rel
    } else {
      New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
      Copy-Item -LiteralPath $_.FullName -Destination $target -Force
    }
  }
}

function Copy-DocsPublic {
  $docsSrc = Join-Path $RepoRoot "docs"
  if (-not (Test-Path $docsSrc)) { return }
  Write-Host "copy docs/ (excluding internal + superpowers)"
  New-Item -ItemType Directory -Force -Path (Join-Path $OutDir "docs") | Out-Null
  Get-ChildItem -LiteralPath $docsSrc -Force | ForEach-Object {
    $rel = "docs/$($_.Name)"
    if (Test-IsDeniedRelative $rel) {
      Write-Host "  deny skip: $rel"
      return
    }
    $target = Join-Path $OutDir ($rel.Replace("/", "\"))
    if ($_.PSIsContainer) {
      New-Item -ItemType Directory -Force -Path $target | Out-Null
      Copy-TreeFiltered -Src $_.FullName -Dst $target -RelPrefix $rel
    } else {
      Copy-Item -LiteralPath $_.FullName -Destination $target -Force
    }
  }
}

function Overlay-PublicDocs {
  $pub = Join-Path $RepoRoot "docs\internal\public"
  if (-not (Test-Path $pub)) {
    throw "Missing curated public docs at docs/internal/public/"
  }
  foreach ($name in @("README.md", "ROADMAP.md", "CONTRIBUTING.md")) {
    $src = Join-Path $pub $name
    if (-not (Test-Path $src)) { throw "Missing $src" }
    Write-Host "overlay: $name"
    Copy-Item -LiteralPath $src -Destination (Join-Path $OutDir $name) -Force
  }
}

function Assert-ExportClean {
  Write-Host "verify denylist absence…"
  $bad = @()
  Get-ChildItem -LiteralPath $OutDir -Recurse -Force | ForEach-Object {
    $rel = $_.FullName.Substring($OutDir.Length).TrimStart("\", "/")
    $rel = $rel.Replace("\", "/")
    if ([string]::IsNullOrWhiteSpace($rel)) { return }
    if (Test-IsDeniedRelative $rel) { $bad += $rel }
  }
  # Also assert required overlays exist
  foreach ($req in @("README.md", "ROADMAP.md", "CONTRIBUTING.md")) {
    if (-not (Test-Path (Join-Path $OutDir $req))) {
      throw "Export missing required file: $req"
    }
  }
  if ($bad.Count -gt 0) {
    throw ("Denylist paths found in export:`n  " + ($bad -join "`n  "))
  }
  Write-Host "export clean."
}

function Sync-ToPublicRepo {
  param([string] $Dest)
  $Dest = (Resolve-Path $Dest).Path
  Write-Host "sync export → $Dest"
  Get-ChildItem -LiteralPath $Dest -Force | Where-Object { $_.Name -ne ".git" } | ForEach-Object {
    Remove-Item -LiteralPath $_.FullName -Recurse -Force
  }
  Get-ChildItem -LiteralPath $OutDir -Force | ForEach-Object {
    $target = Join-Path $Dest $_.Name
    Copy-Item -LiteralPath $_.FullName -Destination $target -Recurse -Force
  }
  if ($Message) {
    Push-Location $Dest
    try {
      git add -A
      $status = git status --porcelain
      if (-not $status) {
        Write-Host "public repo: nothing to commit"
      } else {
        git commit -m $Message
        Write-Host "committed in public clone (not pushed)"
      }
    } finally {
      Pop-Location
    }
  }
}

# --- main ---
Write-Host "RepoRoot=$RepoRoot"
Write-Host "OutDir=$OutDir"

if (-not $SkipClean -and (Test-Path $OutDir)) {
  Remove-Item -LiteralPath $OutDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

foreach ($d in $AllowDirs) { Copy-AllowDir $d }
Copy-DocsPublic

foreach ($f in $AllowRootFiles) {
  $src = Join-Path $RepoRoot $f
  if (Test-Path $src) {
    Write-Host "copy file: $f"
    Copy-Item -LiteralPath $src -Destination (Join-Path $OutDir $f) -Force
  }
}

Overlay-PublicDocs
Assert-ExportClean

if ($PublicRepoDir) {
  if (-not (Test-Path $PublicRepoDir)) { throw "PublicRepoDir not found: $PublicRepoDir" }
  Sync-ToPublicRepo -Dest $PublicRepoDir
}

Write-Host "Done. Export at: $OutDir"
