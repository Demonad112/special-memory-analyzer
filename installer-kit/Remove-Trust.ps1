#Requires -RunAsAdministrator
<#
.SYNOPSIS
Removes the Deep Server Analyzer signing trust now, or when the expiry task calls it.

.PARAMETER FromTask
Set by the DSA-TrustExpiry task. Removes trust only if the expiry has passed; otherwise it does nothing.
#>
[CmdletBinding()]
param(
    [switch]$FromTask
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'DsaTrust.psm1') -Force

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
        exit 0
    }
    Remove-ActiveTrust
    exit 0
}
catch {
    Write-TrustLog "Remove-Trust failed: $($_.Exception.Message)" 'ERROR'
    exit 1
}
