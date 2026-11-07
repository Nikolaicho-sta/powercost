@echo off
rem Double-click to update PowerCost. A small progress window opens; this console closes at once.
rem For a console instead of the window, run:  "Update PowerCost.cmd" -NoGui
echo %* | find /i "-NoGui" >nul
if not errorlevel 1 (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File "%~dp0scripts\update.ps1" %*
    echo.
    pause
    exit /b
)
start "" powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -WindowStyle Hidden -File "%~dp0scripts\update.ps1" %*
