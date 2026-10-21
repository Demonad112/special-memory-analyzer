@echo off
REM Self-elevating entry point. Any arguments are passed through to Install-DSA.ps1.
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install-DSA.ps1" %*
exit /b %errorlevel%
