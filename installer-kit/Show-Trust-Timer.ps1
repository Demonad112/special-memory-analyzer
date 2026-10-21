#Requires -RunAsAdministrator
<#
.SYNOPSIS
Re-attaches to the live trust countdown. Keys: E extend 1 h, R remove now, Q close (the timer keeps running).
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'DsaTrust.psm1') -Force

try {
    $record = Read-TrustExpiry
    if (-not $record) {
        Write-Host 'No active trust'
        exit 0
    }
    if (Test-TrustDue -ExpiresUtc $record.ExpiresUtc) {
        Remove-ActiveTrust
        Write-Host 'Trust expired and was removed.'
        exit 0
    }
    Show-TrustCountdown -RemoveScriptPath (Join-Path (Get-DsaDataDir) 'Remove-Trust.ps1')
    exit 0
}
catch {
    Write-TrustLog "Show-Trust-Timer failed: $($_.Exception.Message)" 'ERROR'
    exit 1
}
