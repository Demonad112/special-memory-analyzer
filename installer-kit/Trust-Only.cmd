@echo off
REM Trusts the signing certificate and schedules its removal. Does not run the setup.
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install-DSA.ps1" -Mode Trust %*
exit /b %errorlevel%
