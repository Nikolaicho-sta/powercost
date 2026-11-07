# The progress window used by update.ps1 (Windows Forms; ships with Windows PowerShell, nothing to install).
# Every Ui-* function also works without a window (-NoGui): it then prints to the console instead.

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$script:Gui = $true
$script:U = @{}

function Ui-IsDark {
    try { return ((Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -ErrorAction Stop).AppsUseLightTheme -eq 0) } catch { return $false }
}

function Ui-Init([string]$title, [string]$subtitle, [string[]]$steps, [bool]$gui) {
    $script:Gui = $gui
    $script:U = @{ Steps = $steps; State = @{}; Log = New-Object System.Collections.Generic.List[string]; Done = $false }
    if (-not $gui) { Write-Host $title -ForegroundColor Green; if ($subtitle) { Write-Host "    $subtitle" }; return }

    $dark = Ui-IsDark
    $bg = if ($dark) { [Drawing.Color]::FromArgb(26, 26, 25) } else { [Drawing.Color]::FromArgb(249, 249, 247) }
    $fg = if ($dark) { [Drawing.Color]::FromArgb(240, 239, 236) } else { [Drawing.Color]::FromArgb(11, 11, 11) }
    $muted = if ($dark) { [Drawing.Color]::FromArgb(160, 159, 152) } else { [Drawing.Color]::FromArgb(82, 81, 78) }
    $script:U.Colors = @{
        Fg = $fg; Muted = $muted; Accent = [Drawing.Color]::FromArgb(42, 120, 214)
        Good = if ($dark) { [Drawing.Color]::FromArgb(12, 163, 12) } else { [Drawing.Color]::FromArgb(0, 99, 0) }
        Bad = if ($dark) { [Drawing.Color]::FromArgb(230, 103, 103) } else { [Drawing.Color]::FromArgb(200, 40, 40) }
    }

    $f = New-Object System.Windows.Forms.Form
    $f.Text = 'PowerCost update'
    $f.StartPosition = 'CenterScreen'
    $f.FormBorderStyle = 'FixedDialog'
    $f.MaximizeBox = $false
    $f.BackColor = $bg
    $f.ForeColor = $fg
    $f.Font = New-Object System.Drawing.Font('Segoe UI', 9.5)
    $f.ClientSize = New-Object System.Drawing.Size(520, 290)
    $f.ShowInTaskbar = $true

    $h = New-Object System.Windows.Forms.Label
    $h.Text = $title; $h.Font = New-Object System.Drawing.Font('Segoe UI Semibold', 15)
    $h.AutoSize = $true; $h.Location = New-Object System.Drawing.Point(22, 16); $f.Controls.Add($h)

    $s = New-Object System.Windows.Forms.Label
    $s.Text = $subtitle; $s.ForeColor = $muted; $s.AutoSize = $false
    $s.Size = New-Object System.Drawing.Size(476, 20); $s.Location = New-Object System.Drawing.Point(24, 48); $f.Controls.Add($s)

    $y = 82
    $script:U.StepLabels = @()
    foreach ($t in $steps) {
        $l = New-Object System.Windows.Forms.Label
        $l.AutoSize = $false; $l.Size = New-Object System.Drawing.Size(476, 22)
        $l.Location = New-Object System.Drawing.Point(24, $y); $l.ForeColor = $muted
        $l.Text = "$([char]0x25CB)  $t"
        $f.Controls.Add($l); $script:U.StepLabels += $l; $y += 24
    }

    $p = New-Object System.Windows.Forms.ProgressBar
    $p.Location = New-Object System.Drawing.Point(24, ($y + 8)); $p.Size = New-Object System.Drawing.Size(472, 14)
    $p.Minimum = 0; $p.Maximum = 100; $p.Style = 'Continuous'
    $f.Controls.Add($p); $script:U.Bar = $p

    $st = New-Object System.Windows.Forms.Label
    $st.AutoSize = $false; $st.Location = New-Object System.Drawing.Point(24, ($y + 30)); $st.Size = New-Object System.Drawing.Size(472, 40)
    $st.ForeColor = $muted; $st.Text = 'Starting...'
    $f.Controls.Add($st); $script:U.Status = $st

    $btnY = $f.ClientSize.Height - 40
    $det = New-Object System.Windows.Forms.Button
    $det.Text = 'Show details'; $det.FlatStyle = 'System'
    $det.Size = New-Object System.Drawing.Size(110, 28); $det.Location = New-Object System.Drawing.Point(24, $btnY)
    $f.Controls.Add($det); $script:U.DetBtn = $det

    $log = New-Object System.Windows.Forms.Button
    $log.Text = 'Open log'; $log.FlatStyle = 'System'; $log.Visible = $false
    $log.Size = New-Object System.Drawing.Size(90, 28); $log.Location = New-Object System.Drawing.Point(310, $btnY)
    $f.Controls.Add($log); $script:U.LogBtn = $log

    $close = New-Object System.Windows.Forms.Button
    $close.Text = 'Close'; $close.FlatStyle = 'System'; $close.Enabled = $false
    $close.Size = New-Object System.Drawing.Size(90, 28); $close.Location = New-Object System.Drawing.Point(406, $btnY)
    $f.Controls.Add($close); $script:U.CloseBtn = $close
    $f.CancelButton = $close

    $box = New-Object System.Windows.Forms.TextBox
    $box.Multiline = $true; $box.ReadOnly = $true; $box.ScrollBars = 'Vertical'; $box.WordWrap = $false
    $box.Font = New-Object System.Drawing.Font('Consolas', 8.5)
    $box.BackColor = if ($dark) { [Drawing.Color]::FromArgb(13, 13, 13) } else { [Drawing.Color]::White }
    $box.ForeColor = $fg
    $box.Location = New-Object System.Drawing.Point(24, ($f.ClientSize.Height + 4)); $box.Size = New-Object System.Drawing.Size(472, 170)
    $f.Controls.Add($box); $script:U.Box = $box

    $det.Add_Click({
        $f2 = $script:U.Form
        if ($f2.ClientSize.Height -lt 400) {
            $f2.ClientSize = New-Object System.Drawing.Size(520, 480); $script:U.DetBtn.Text = 'Hide details'
            $script:U.DetBtn.Location = New-Object System.Drawing.Point(24, 250)
        } else {
            $f2.ClientSize = New-Object System.Drawing.Size(520, 290); $script:U.DetBtn.Text = 'Show details'
            $script:U.DetBtn.Location = New-Object System.Drawing.Point(24, 250)
        }
    })
    $close.Add_Click({ $script:U.Form.Close() })
    $f.Add_FormClosing({ if (-not $script:U.Done) { $_.Cancel = $true } })   # not while it is working
    $script:U.Form = $f
    # the buttons sit in a fixed band at y=250; the details box is revealed below it
    foreach ($c in @($det, $log, $close)) { $c.Top = 250 }
    $box.Top = 288
    $f.Show()
    Ui-Pump
}

function Ui-Pump { if ($script:Gui) { [System.Windows.Forms.Application]::DoEvents() } }

function Ui-Step([int]$index, [string]$state) {
    # state: pending | active | done | failed
    $script:U.State[$index] = $state
    if (-not $script:Gui) {
        $mark = switch ($state) { 'active' { '>' } 'done' { 'ok' } 'failed' { '!!' } default { '-' } }
        Write-Host ("[{0}] {1} {2}" -f ($index + 1), $mark, $script:U.Steps[$index]) -ForegroundColor Cyan
        return
    }
    $l = $script:U.StepLabels[$index]; $c = $script:U.Colors
    switch ($state) {
        'active' { $l.Text = "$([char]0x25CF)  $($script:U.Steps[$index])"; $l.ForeColor = $c.Accent; $l.Font = New-Object System.Drawing.Font($l.Font, [Drawing.FontStyle]::Bold) }
        'done' { $l.Text = "$([char]0x2713)  $($script:U.Steps[$index])"; $l.ForeColor = $c.Good; $l.Font = New-Object System.Drawing.Font($l.Font, [Drawing.FontStyle]::Regular) }
        'failed' { $l.Text = "$([char]0x2717)  $($script:U.Steps[$index])"; $l.ForeColor = $c.Bad }
        default { $l.Text = "$([char]0x25CB)  $($script:U.Steps[$index])"; $l.ForeColor = $c.Muted }
    }
    Ui-Pump
}

function Ui-Status([string]$text) {
    if (-not $script:Gui) { Write-Host "    $text"; return }
    $script:U.Status.Text = $text; Ui-Pump
}

function Ui-Progress([int]$percent) {
    if (-not $script:Gui) { return }
    $script:U.Bar.Style = 'Continuous'
    $script:U.Bar.Value = [Math]::Max(0, [Math]::Min(100, $percent)); Ui-Pump
}

function Ui-Detail([string]$line) {
    $script:U.Log.Add($line)
    if (-not $script:Gui) { Write-Host "    $line" -ForegroundColor DarkGray; return }
    $b = $script:U.Box
    $b.AppendText($line + "`r`n")
    if ($b.Lines.Count -gt 600) { $b.Lines = $b.Lines | Select-Object -Last 400 }
}

# Finish: shows the result and waits for the user to close the window (success closes itself after 8 s).
function Ui-Finish([bool]$ok, [string]$message, [string]$logPath) {
    $script:U.Done = $true
    if (-not $script:Gui) {
        if ($ok) { Write-Host ""; Write-Host $message -ForegroundColor Green } else { Write-Host ""; Write-Host "UPDATE FAILED: $message" -ForegroundColor Red; Write-Host "(details are in $logPath)" }
        return
    }
    $c = $script:U.Colors
    $script:U.Bar.Style = 'Continuous'
    if ($ok) { $script:U.Bar.Value = 100; $script:U.Status.ForeColor = $c.Good } else { $script:U.Status.ForeColor = $c.Bad }
    $script:U.Status.Text = $message
    $script:U.CloseBtn.Enabled = $true
    if (-not $ok) {
        $script:U.LogBtn.Visible = $true
        $script:U.LogBtn.Add_Click({ Start-Process notepad.exe -ArgumentList ('"' + $script:U.LogPath + '"') })
        $script:U.LogPath = $logPath
        if ($script:U.Form.ClientSize.Height -lt 400) { $script:U.DetBtn.PerformClick() }   # show what happened
    }
    Ui-Pump
    $deadline = (Get-Date).AddSeconds(8)
    while ($script:U.Form.Visible) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 60
        if ($ok -and $script:U.Form.ClientSize.Height -lt 400) {
            $left = [int][Math]::Ceiling(($deadline - (Get-Date)).TotalSeconds)
            $script:U.CloseBtn.Text = "Close ($([Math]::Max($left,0)))"
            if ((Get-Date) -gt $deadline) { $script:U.Form.Close() }
        }
    }
}

# Runs a command line through cmd.exe, keeps the window alive while it runs and feeds its output to
# Ui-Detail. $onChunk gets each new piece of text (for progress parsing). Returns the exit code.
function Invoke-Logged([string]$commandLine, [string]$workDir, [scriptblock]$onChunk) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("powercost-update-" + [guid]::NewGuid().ToString('N') + '.log')
    $args = '/d /s /c "' + $commandLine + ' > "' + $tmp + '" 2>&1"'
    # -NoNewWindow: the command runs in this (hidden or no) console. Without it Windows 11 opens a
    # visible Windows Terminal window for every console program, even with -WindowStyle Hidden.
    $p = Start-Process -FilePath $env:ComSpec -ArgumentList $args -WorkingDirectory $workDir -NoNewWindow -PassThru
    $null = $p.Handle   # keep the process handle: without it ExitCode can come back empty (PowerShell quirk with -NoNewWindow)
    $pos = 0L
    $read = {
        if (Test-Path -LiteralPath $tmp) {
            $fs = [IO.File]::Open($tmp, 'Open', 'Read', 'ReadWrite')
            try {
                if ($fs.Length -gt $pos) {
                    $fs.Seek($pos, 'Begin') | Out-Null
                    $buf = New-Object byte[] ($fs.Length - $pos)
                    $n = $fs.Read($buf, 0, $buf.Length)
                    $pos += $n
                    $text = [Text.Encoding]::UTF8.GetString($buf, 0, $n)
                    if ($onChunk) { & $onChunk $text }
                }
            } finally { $fs.Dispose() }
        }
    }
    while (-not $p.HasExited) { Ui-Pump; & $read; Start-Sleep -Milliseconds 150 }
    & $read
    $p.WaitForExit()
    $code = $p.ExitCode
    Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    if ($null -eq $code) { throw "could not read the exit code of: $commandLine" }
    return [int]$code
}
