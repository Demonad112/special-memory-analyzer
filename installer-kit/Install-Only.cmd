@echo off
REM Runs the setup only. The signing certificate must already be trusted (see Trust-Only.cmd).
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install-DSA.ps1" -Mode Install %*
exit /b %errorlevel%
