<#
.SYNOPSIS
  Publish the current version as a GitHub release, which is what installed copies of PowerCost
  update from.

.DESCRIPTION
  1. reads the version from Cargo.toml (bump it, and crates\app\tauri.conf.json, first)
  2. builds the installer (unless -NoBuild)
  3. creates the release v<version> with the installer attached, twice:
       PowerCost_<version>_x64-setup.exe   what the in-app updater downloads
       PowerCost-setup.exe                 a fixed name, so the website can always link to the latest
  4. puts the SHA-256 in the release notes (the updater refuses a release it cannot verify)

  The FIRST LINE of -Notes is shown in the app next to "A newer version is ready", so keep it short.

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

$version = ((Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value)
$conf = Get-Content (Join-Path $root 'crates\app\tauri.conf.json') -Raw | ConvertFrom-Json
if ($conf.version -ne $version) { throw "Cargo.toml says $version but tauri.conf.json says $($conf.version). Make them match." }
$tag = "v$version"
Write-Host "Releasing PowerCost $version ($tag)" -ForegroundColor Cyan

if (-not $DryRun) {
    $dirty = (& git -C $root status --porcelain --untracked-files=no)
    if ($dirty) { throw "There are uncommitted changes. Commit them first so the build matches the release." }
}

if (-not $NoBuild -and -not $DryRun) {
    $env:Path += ";$env:USERPROFILE\.cargo\bin"
    Push-Location (Join-Path $root 'ui'); try { npm run build; if ($LASTEXITCODE) { throw "interface build failed" } } finally { Pop-Location }
    Push-Location (Join-Path $root 'crates\app'); try { & (Join-Path $root 'ui\node_modules\.bin\tauri.cmd') build; if ($LASTEXITCODE) { throw "installer build failed" } } finally { Pop-Location }
}

$installer = Join-Path $root "target\release\bundle\nsis\PowerCost_${version}_x64-setup.exe"
if (-not (Test-Path $installer)) { if ($DryRun) { Write-Host "(dry run) installer not built yet: $installer" } else { throw "installer not found: $installer" } }
$name = Split-Path $installer -Leaf
$sha = if (Test-Path $installer) { (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower() } else { '<sha256>' }

$body = "$Notes`r`n`r`nSHA-256 of ``$name``:`r`n``$sha```r`n`r`nThe installer is not code-signed, so Windows SmartScreen may warn: click `"More info`", then `"Run anyway`"."
$notesFile = Join-Path ([IO.Path]::GetTempPath()) "powercost-release-notes.md"
[IO.File]::WriteAllText($notesFile, $body, (New-Object System.Text.UTF8Encoding($false)))  # no BOM: it would show in the app

if ($DryRun) { Write-Host "(dry run) would publish $tag with $name and PowerCost-setup.exe`n$body"; exit 0 }

$stable = Join-Path ([IO.Path]::GetTempPath()) 'PowerCost-setup.exe'
Copy-Item -LiteralPath $installer -Destination $stable -Force
& $gh release create $tag $installer $stable --title "PowerCost $version" --notes-file $notesFile --target main
if ($LASTEXITCODE) { throw "gh release create failed" }
Write-Host "Published: https://github.com/Nikolaicho-sta/powercost/releases/tag/$tag" -ForegroundColor Green
