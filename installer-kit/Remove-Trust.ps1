#Requires -RunAsAdministrator
<#
.SYNOPSIS
Removes the Deep Server Analyzer signing trust now, or when the expiry task calls it.

.PARAMETER FromTask
Set by the DSA-TrustExpiry task. Removes trust only if the expiry has passed; otherwise it does nothing.

.PARAMETER Verify
After removing trust, print a check of each store, the task and the expiry record, then wait for a key.
Exits 2 when anything is left behind. Used by the red button in DSA-Installer.
#>
[CmdletBinding()]
param(
    [switch]$FromTask,
    [switch]$Verify
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'DsaTrust.psm1') -Force

function Show-Verification {
    param([string]$Thumbprint)
    $thumbFile = Join-Path $PSScriptRoot 'thumbprint.txt'
    if (-not $Thumbprint -and (Test-Path -LiteralPath $thumbFile)) { $Thumbprint = (Get-Content -LiteralPath $thumbFile -Raw).Trim() }
    Write-Host ''
    Write-Host 'Verifying removal'
    $failed = 0
    foreach ($check in (Test-TrustRemoved -Thumbprint $Thumbprint)) {
        if ($check.Passed) { Write-Host "  OK    $($check.Name)" -ForegroundColor Green }
        else { Write-Host "  FAIL  $($check.Name)" -ForegroundColor Red; $failed++ }
    }
    Write-Host ''
    if ($failed) { Write-Host "$failed check(s) failed." -ForegroundColor Red }
    else { Write-Host 'All clear: nothing is trusted and nothing is scheduled.' -ForegroundColor Green }
    Write-Host 'Press any key to close.'
    [void][Console]::ReadKey($true)
    return $failed
}

try {
    $record = Read-TrustExpiry
    if ($FromTask) {
        if (-not $record) {
            Write-TrustLog 'Expiry check: no trust recorded; unregistering the task'
            Unregister-TrustTask
            exit 0
        }
        if (-not (Test-TrustDue -ExpiresUtc $record.ExpiresUtc)) {
            Write-TrustLog "Expiry check: trust runs until $($record.ExpiresUtc.ToString('o')) UTC; leaving it in place"
            exit 0
        }
        Remove-ActiveTrust
        exit 0
    }

    if (-not $record) {
        Write-Host 'No active trust'
        Unregister-TrustTask
    }
    else {
        Remove-ActiveTrust
    }
    if ($Verify) {
        $thumb = if ($record) { $record.Thumbprint } else { $null }
        if (Show-Verification -Thumbprint $thumb) { exit 2 }
    }
    exit 0
}
catch {
    Write-TrustLog "Remove-Trust failed: $($_.Exception.Message)" 'ERROR'
    exit 1
}
