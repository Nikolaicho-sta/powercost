@echo off
rem Updates PowerCost. For no flicker at all, double-click "Update PowerCost.vbs" instead.
rem For console output instead of the progress window, run:  "Update PowerCost.cmd" -NoGui
echo %* | find /i "-NoGui" >nul
if not errorlevel 1 (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File "%~dp0scripts\update.ps1" %*
    echo.
    pause
    exit /b
)
wscript.exe //nologo "%~dp0Update PowerCost.vbs" %*
