<#
.SYNOPSIS
  Publish the newest commit as a GitHub release, which is what installed copies of PowerCost
  update from. Releases are numbered only by milestone: the commit "M27 ..." becomes release M27.

.DESCRIPTION
  1. reads the milestone from the newest commit subject ("M27 Something" -> 27); refuses if there is none
  2. builds the installer (unless -NoBuild) with the Windows file version 27.0.0 (Windows installers
     need a numeric version; the app and the release only ever show "M27")
  3. creates the release M27 with the installer attached, twice:
       PowerCost_27.0.0_x64-setup.exe      what the in-app updater downloads
       PowerCost-setup.exe                 a fixed name, so the website can always link to the latest
  4. puts the SHA-256 in the release notes (the updater refuses a release it cannot verify)

  No version number is edited by hand anywhere. The FIRST LINE of -Notes is shown in the app next to
  "A newer version is ready", so keep it short.

.PARAMETER Notes    Release notes (first line = one-sentence summary).
.PARAMETER NoBuild  Use the installer already in target\release\bundle\nsis.
.PARAMETER DryRun   Show what would be published; create nothing.
#>
param(
    [Parameter(Mandatory = $true)][string]$Notes,
    [switch]$NoBuild,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
if (-not $gh) { $gh = 'C:\Program Files\GitHub CLI\gh.exe' }
if (-not (Test-Path $gh)) { throw "GitHub CLI (gh) not found. Install it with: winget install GitHub.cli, then run: gh auth login" }

$subject = (& git -C $root log -1 --format=%s)
if ($subject -notmatch '^M(\d+)\b') { throw "The newest commit is not named 'M<number> ...' ($subject), so there is no release number." }
$n = [int]$Matches[1]
$m = "M$n"
$fileVersion = "$n.0.0"
$tag = $m
Write-Host "Releasing PowerCost $m (installer file version $fileVersion)" -ForegroundColor Cyan

if (-not $DryRun) {
    $dirty = (& git -C $root status --porcelain --untracked-files=no)
    if ($dirty) { throw "There are uncommitted changes. Commit them first so the build matches the release." }
}

if (-not $NoBuild -and -not $DryRun) {
    $env:Path += ";$env:USERPROFILE\.cargo\bin"
    Push-Location (Join-Path $root 'ui'); try { npm run build; if ($LASTEXITCODE) { throw "interface build failed" } } finally { Pop-Location }
    $cfg = Join-Path ([IO.Path]::GetTempPath()) 'powercost-release-config.json'
    [IO.File]::WriteAllText($cfg, "{`"version`":`"$fileVersion`"}", (New-Object System.Text.UTF8Encoding($false)))
    Push-Location (Join-Path $root 'crates\app'); try { & (Join-Path $root 'ui\node_modules\.bin\tauri.cmd') build --config $cfg; if ($LASTEXITCODE) { throw "installer build failed" } } finally { Pop-Location }
}

$installer = Join-Path $root "target\release\bundle\nsis\PowerCost_${fileVersion}_x64-setup.exe"
if (-not (Test-Path $installer)) { if ($DryRun) { Write-Host "(dry run) installer not built yet: $installer" } else { throw "installer not found: $installer" } }
$name = Split-Path $installer -Leaf
$sha = if (Test-Path $installer) { (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower() } else { '<sha256>' }

# Put the M label in front of the notes so the app and the releases page show it (e.g. "M24: Faster start").
if ($Notes -notmatch '^M\d+\b') { $Notes = "${m}: $Notes" }
$title = "PowerCost $m"

$body = "$Notes`r`n`r`nSHA-256 of ``$name``:`r`n``$sha```r`n`r`nThe installer is not code-signed, so Windows SmartScreen may warn: click `"More info`", then `"Run anyway`"."
$notesFile = Join-Path ([IO.Path]::GetTempPath()) "powercost-release-notes.md"
[IO.File]::WriteAllText($notesFile, $body, (New-Object System.Text.UTF8Encoding($false)))  # no BOM: it would show in the app

if ($DryRun) { Write-Host "(dry run) would publish $tag with $name and PowerCost-setup.exe`n$body"; exit 0 }

$stable = Join-Path ([IO.Path]::GetTempPath()) 'PowerCost-setup.exe'
Copy-Item -LiteralPath $installer -Destination $stable -Force
& $gh release create $tag $installer $stable --title $title --notes-file $notesFile --target main
if ($LASTEXITCODE) { throw "gh release create failed" }
Write-Host "Published: https://github.com/Nikolaicho-sta/powercost/releases/tag/$tag" -ForegroundColor Green
