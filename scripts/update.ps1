<#
.SYNOPSIS
  Update PowerCost from this project folder in one go: quit the running app cleanly, rebuild,
  install over the existing install (your data is kept) and start it again.

.DESCRIPTION
  Double-click "Update PowerCost.cmd" in the project folder, or run this script.
  Steps: 1 quit the running copy  2 build the interface and the installer  3 install  4 start.

  It never force-kills anything. If a running copy is too old to understand the quit request, it
  tells you to use the tray icon's "Quit PowerCost" and waits.

.PARAMETER NoInstall   Build only; do not install or start.
.PARAMETER NoLaunch    Install but do not start the app afterwards.
.PARAMETER DryRun      Show what would happen without quitting, building or installing anything.
.PARAMETER WaitSeconds How long to wait for you to quit an old copy by hand (default 90).
.PARAMETER InstallDir  Where to install (default: %LOCALAPPDATA%\PowerCost).
#>
param(
    [switch]$NoInstall,
    [switch]$NoLaunch,
    [switch]$DryRun,
    [int]$WaitSeconds = 90,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'PowerCost')
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$installDir = $InstallDir
$installedExe = Join-Path $installDir 'powercost.exe'
$logFile = Join-Path $root 'update.log'
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) { $env:Path += ";$cargoBin" }

function Step([string]$n, [string]$text) { Write-Host ""; Write-Host "[$n] $text" -ForegroundColor Cyan }
function Info([string]$text) { Write-Host "    $text" }
function Fail([string]$text) { Write-Host ""; Write-Host "UPDATE FAILED: $text" -ForegroundColor Red; Write-Host "(details are in $logFile)"; try { Stop-Transcript | Out-Null } catch {}; exit 1 }
function Check([string]$what) { if ($LASTEXITCODE -ne 0) { Fail "$what failed (exit code $LASTEXITCODE)" } }

try { Start-Transcript -Path $logFile -Force | Out-Null } catch {}
Write-Host "PowerCost update" -ForegroundColor Green
if ($DryRun) { Write-Host "(dry run: nothing will be changed)" -ForegroundColor Yellow }
$label = (& git -C $root log -1 --format="%s %h" 2>$null)
Info "project: $root"
Info "latest commit: $label"

# 0. prerequisites -------------------------------------------------------------------------------
Step "0/4" "Checking tools"
foreach ($tool in @('cargo', 'npm', 'node')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Fail "'$tool' was not found. Install Rust (rustup.rs) and Node.js first; see README.md." }
}
Info "cargo, npm and node found"

# 1. quit the running copy cleanly ---------------------------------------------------------------
Step "1/4" "Quitting PowerCost if it is running"
$running = @(Get-Process powercost -ErrorAction SilentlyContinue)
if ($running.Count -eq 0) {
    Info "not running"
} else {
    foreach ($p in $running) { Info "running: $($p.Path) (pid $($p.Id))" }
    if ($DryRun) {
        Info "(dry run) would ask it to quit with '--quit' and wait for it to exit"
    } else {
        # A second launch with --quit makes the running copy save its last minute and exit.
        $exe = $running[0].Path
        Start-Process -FilePath $exe -ArgumentList '--quit' -WindowStyle Hidden
        $deadline = (Get-Date).AddSeconds(20)
        while ((Get-Process powercost -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
        if (Get-Process powercost -ErrorAction SilentlyContinue) {
            Write-Host ""
            Write-Host "    This copy did not quit by itself (it is probably an older version)." -ForegroundColor Yellow
            Write-Host "    Please right-click the PowerCost tray icon (near the clock) and choose 'Quit PowerCost'." -ForegroundColor Yellow
            Write-Host "    Waiting up to $WaitSeconds seconds..." -ForegroundColor Yellow
            $deadline = (Get-Date).AddSeconds($WaitSeconds)
            while ((Get-Process powercost -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
            if (Get-Process powercost -ErrorAction SilentlyContinue) { Fail "PowerCost is still running. Quit it from the tray icon and run the update again." }
        }
        Info "PowerCost has quit (its last minute was saved)"
    }
}

# 2. build ---------------------------------------------------------------------------------------
Step "2/4" "Building the interface and the installer (the first build is slow, later ones are quick)"
if ($DryRun) {
    Info "(dry run) would run: npm run build in ui, then 'tauri build' in crates\app"
} else {
    Push-Location (Join-Path $root 'ui')
    try {
        if (-not (Test-Path 'node_modules')) { Info "installing interface packages (first time only)"; npm install --no-audit --no-fund; Check "npm install" }
        npm run build; Check "the interface build"
    } finally { Pop-Location }
    Push-Location (Join-Path $root 'crates\app')
    try {
        & (Join-Path $root 'ui\node_modules\.bin\tauri.cmd') build
        Check "the installer build"
    } finally { Pop-Location }
}
$setup = Get-ChildItem (Join-Path $root 'target\release\bundle\nsis') -Filter '*_x64-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { if ($DryRun) { Info "(dry run) no installer built yet" } else { Fail "the installer was not produced" } }
else { Info "installer: $($setup.FullName) ($([math]::Round($setup.Length/1MB,2)) MB, built $($setup.LastWriteTime))" }

if ($NoInstall) { Write-Host ""; Write-Host "Done (build only)." -ForegroundColor Green; try { Stop-Transcript | Out-Null } catch {}; exit 0 }

# 3. install -------------------------------------------------------------------------------------
Step "3/4" "Installing to $installDir (your data in %APPDATA%\PowerCost is kept)"
if ($DryRun) {
    Info "(dry run) would run the installer silently with /S /D=$installDir"
} else {
    # An explicit folder is always used: an old test install may be registered elsewhere.
    $p = Start-Process -FilePath $setup.FullName -ArgumentList '/S', "/D=$installDir" -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "the installer returned exit code $($p.ExitCode)" }
    if (-not (Test-Path $installedExe)) { Fail "the program was not found at $installedExe after installing" }
    Info "installed: $installedExe ($((Get-Item $installedExe).LastWriteTime))"
}

# 4. start ---------------------------------------------------------------------------------------
Step "4/4" "Starting PowerCost"
if ($NoLaunch -or $DryRun) {
    Info "(not started)"
} else {
    Start-Process -FilePath $installedExe
    Info "started. Check Settings (bottom of the page) for the build label: $label"
}

Write-Host ""
Write-Host "Update finished." -ForegroundColor Green
try { Stop-Transcript | Out-Null } catch {}
