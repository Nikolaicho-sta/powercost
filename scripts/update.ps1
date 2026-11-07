<#
.SYNOPSIS
  Update PowerCost from this project folder: build the new version, then close the running app for
  a few seconds, install over it (your data is kept) and start it again. Shows a small progress
  window instead of a console.

.DESCRIPTION
  Double-click "Update PowerCost.cmd" in the project folder, press "Update now" in PowerCost, or run
  this script. Steps: 1 build (the app keeps running and tracking meanwhile)  2 close PowerCost
  3 install  4 start.

  The build uses quicker compiler settings than the published installer (no link-time optimisation,
  incremental, its own folder target\update), so it does not disturb or slow down normal release
  builds. Use -Optimized for the exact release settings (slower).

  It never force-kills anything. If a running copy is too old to understand the quit request, it
  tells you to use the tray icon's "Quit PowerCost" and waits.

.PARAMETER NoInstall   Build only; do not close, install or start anything.
.PARAMETER NoLaunch    Install but do not start the app afterwards.
.PARAMETER DryRun      Show the window and the steps without building, quitting or installing.
.PARAMETER NoGui       Print to the console instead of showing the progress window.
.PARAMETER Optimized   Build with the release profile (slower; identical to the published installer).
.PARAMETER WaitSeconds How long to wait for you to quit an old copy by hand (default 90).
.PARAMETER InstallDir  Where to install (default: %LOCALAPPDATA%\PowerCost).
#>
param(
    [switch]$NoInstall,
    [switch]$NoLaunch,
    [switch]$DryRun,
    [switch]$NoGui,
    [switch]$Optimized,
    [int]$WaitSeconds = 90,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'PowerCost')
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$installedExe = Join-Path $InstallDir 'powercost.exe'
$logFile = Join-Path $root 'update.log'
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) { $env:Path += ";$cargoBin" }

. (Join-Path $PSScriptRoot 'update-ui.ps1')

Set-Content -LiteralPath $logFile -Value "PowerCost update $(Get-Date -Format s)" -Encoding ascii
function Note([string]$text) { Add-Content -LiteralPath $logFile -Value $text -Encoding ascii; Ui-Detail $text }

$label = (& git -C $root log -1 --format="%s %h" 2>$null)
$steps = @('Building the new version', 'Closing PowerCost', 'Installing', 'Starting PowerCost')
Ui-Init 'Updating PowerCost' "Latest version in the project folder: $label" $steps (-not $NoGui)
if ($DryRun) { Ui-Status 'Dry run: nothing will be changed.' }
$current = 0
$clock = [Diagnostics.Stopwatch]::StartNew()

try {
    # tools ----------------------------------------------------------------------------------------
    foreach ($tool in @('cargo', 'npm', 'node')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "'$tool' was not found. Install Rust (rustup.rs) and Node.js first; see README.md." }
    }

    # 1. build (the app keeps running and tracking meanwhile) --------------------------------------
    $current = 0
    Ui-Step 0 'active'
    # Quicker compile for your own copy: no LTO, many codegen units, incremental, separate folder.
    if ($Optimized) { $targetDir = Join-Path $root 'target' } else {
        $targetDir = Join-Path $root 'target\update'
        $env:CARGO_TARGET_DIR = $targetDir
        $env:CARGO_PROFILE_RELEASE_LTO = 'false'
        $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'
        $env:CARGO_PROFILE_RELEASE_INCREMENTAL = 'true'
    }
    $env:CARGO_TERM_PROGRESS_WHEN = 'always'   # lets the window show "123 of 556"
    $env:CARGO_TERM_PROGRESS_WIDTH = '100'
    $nsisDir = Join-Path $targetDir 'release\bundle\nsis'

    $onChunk = {
        param($text)
        foreach ($ln in ($text -split "[\r\n]+")) {
            $t = $ln.Trim()
            if (-not $t) { continue }
            if ($t -match 'Building \[[^\]]*\]\s+(\d+)/(\d+)(?::\s*(.*))?') {
                $cur = [int]$Matches[1]; $tot = [Math]::Max(1, [int]$Matches[2])
                Ui-Progress (6 + [int](78 * $cur / $tot))
                $what = if ($Matches[3]) { $Matches[3] } else { 'dependencies' }
                Ui-Status "Compiling $what ($cur of $tot)"
            } else {
                Note $t
                if ($t -match 'Running makensis|Bundling') { Ui-Status 'Packaging the installer'; Ui-Progress 85 }
            }
        }
    }
    if ($DryRun) {
        Note '(dry run) would run: npm run build, then tauri build'
        foreach ($p in 10, 40, 70, 85) { Ui-Progress $p; Ui-Status "Pretending to build ($p%)"; Start-Sleep -Milliseconds 350 }
    } else {
        $ui = Join-Path $root 'ui'
        if (-not (Test-Path (Join-Path $ui 'node_modules'))) {
            Ui-Status 'Installing the interface packages (first time only)'
            if ((Invoke-Logged 'npm install --no-audit --no-fund' $ui $onChunk) -ne 0) { throw 'npm install failed' }
        }
        Ui-Status 'Building the interface'; Ui-Progress 3
        if ((Invoke-Logged 'npm run build' $ui $onChunk) -ne 0) { throw 'the interface build failed' }
        Ui-Status 'Compiling PowerCost (the first build is slow, later ones are quick)'; Ui-Progress 6
        $tauri = Join-Path $root 'ui\node_modules\.bin\tauri.cmd'
        if ((Invoke-Logged ('"' + $tauri + '" build') (Join-Path $root 'crates\app') $onChunk) -ne 0) { throw 'the installer build failed' }
    }
    $setup = Get-ChildItem $nsisDir -Filter '*_x64-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $setup -and -not $DryRun) { throw 'the installer was not produced' }
    if ($setup) { Note ("installer: {0} ({1} MB, built {2})" -f $setup.FullName, [math]::Round($setup.Length / 1MB, 2), $setup.LastWriteTime) }
    Ui-Step 0 'done'
    Ui-Progress 85
    Note ("build took {0:N0} s" -f $clock.Elapsed.TotalSeconds)

    if ($NoInstall) {
        Ui-Finish $true 'Built. The installer is ready (nothing was installed).' $logFile
        exit 0
    }

    # 2. close the running copy cleanly (only the few seconds of the install are without tracking) -
    $current = 1
    Ui-Step 1 'active'
    $running = @(Get-Process powercost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedExe })
    if ($running.Count -eq 0) {
        Note 'PowerCost is not running'
    } elseif ($DryRun) {
        Note '(dry run) would ask it to quit with --quit and wait for it to exit'
    } else {
        Ui-Status 'Asking PowerCost to close (it saves its last minute first)'
        Start-Process -FilePath $installedExe -ArgumentList '--quit' -WindowStyle Hidden
        $deadline = (Get-Date).AddSeconds(20)
        while ((Get-Process powercost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedExe }) -and (Get-Date) -lt $deadline) { Ui-Pump; Start-Sleep -Milliseconds 200 }
        if (Get-Process powercost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedExe }) {
            Note 'This copy did not quit by itself (it is probably an older version).'
            $deadline = (Get-Date).AddSeconds($WaitSeconds)
            while ((Get-Process powercost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedExe }) -and (Get-Date) -lt $deadline) {
                $left = [int][Math]::Ceiling(($deadline - (Get-Date)).TotalSeconds)
                Ui-Status "Please right-click the PowerCost tray icon (near the clock) and choose 'Quit PowerCost'. Waiting $left s..."
                Ui-Pump; Start-Sleep -Milliseconds 300
            }
            if (Get-Process powercost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedExe }) { throw 'PowerCost is still running. Quit it from the tray icon and run the update again.' }
        }
        Note 'PowerCost has quit (its last minute was saved)'
    }
    Ui-Step 1 'done'; Ui-Progress 90

    # 3. install -----------------------------------------------------------------------------------
    $current = 2
    Ui-Step 2 'active'
    Ui-Status "Installing to $InstallDir (your data in %APPDATA%\PowerCost is kept)"
    if ($DryRun) {
        Note "(dry run) would run the installer silently with /S /D=$InstallDir"
        Start-Sleep -Milliseconds 500
    } else {
        # An explicit folder is always used: an old test install may be registered elsewhere.
        $p = Start-Process -FilePath $setup.FullName -ArgumentList '/S', "/D=$InstallDir" -PassThru
        $deadline = (Get-Date).AddSeconds(180)
        while (-not $p.HasExited -and (Get-Date) -lt $deadline) { Ui-Pump; Start-Sleep -Milliseconds 150 }
        if (-not $p.HasExited) { throw 'the installer did not finish within 3 minutes' }
        if ($p.ExitCode -ne 0) { throw "the installer returned exit code $($p.ExitCode)" }
        if (-not (Test-Path $installedExe)) { throw "the program was not found at $installedExe after installing" }
        Note ("installed: {0} ({1})" -f $installedExe, (Get-Item $installedExe).LastWriteTime)
    }
    Ui-Step 2 'done'; Ui-Progress 97

    # 4. start -------------------------------------------------------------------------------------
    $current = 3
    Ui-Step 3 'active'
    if ($NoLaunch -or $DryRun) {
        Note '(not started)'
    } else {
        Ui-Status 'Starting PowerCost'
        Start-Process -FilePath $installedExe
        Note "started; Settings (bottom of the page) shows the build label: $label"
    }
    Ui-Step 3 'done'
    Note ("update took {0:N0} s" -f $clock.Elapsed.TotalSeconds)
    Ui-Finish $true "PowerCost is updated ($label). It took $([int]$clock.Elapsed.TotalSeconds) s." $logFile
    exit 0
}
catch {
    $msg = $_.Exception.Message
    Add-Content -LiteralPath $logFile -Value "FAILED: $msg" -Encoding ascii
    try { Ui-Step $current 'failed' } catch {}
    Ui-Finish $false $msg $logFile
    exit 1
}
