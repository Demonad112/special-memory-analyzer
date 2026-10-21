@echo off
REM Re-attaches to the live trust countdown. E extends 1 h, R removes now, Q closes the window.
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Show-Trust-Timer.ps1" %*
exit /b %errorlevel%
