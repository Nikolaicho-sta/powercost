@echo off
title Update PowerCost
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\update.ps1" %*
echo.
pause
