@echo off
REM Removes the signing trust now and unregisters DSA-TrustExpiry.
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Remove-Trust.ps1" %*
exit /b %errorlevel%
