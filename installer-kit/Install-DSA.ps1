#Requires -RunAsAdministrator
<#
.SYNOPSIS
Trusts the Deep Server Analyzer signing certificate for a limited time, then installs the app.

.DESCRIPTION
Checks the kit against thumbprint.txt and SHA256SUMS.txt, asks how long trust should last,
imports the certificate, schedules its removal (DSA-TrustExpiry), verifies the setup signature,
runs the silent setup and shows a live countdown. Only run on servers you are authorized to change.

.PARAMETER TrustHours
Trust window in hours (0.001 to 72). Skips the time picker.

.PARAMETER RemoveAfterInstall
Remove trust as soon as the install finishes.

.PARAMETER Interactive
Run the setup with its own UI instead of /S.

.PARAMETER SkipInstall
Trust only: do not run the setup.

.PARAMETER NoPrompt
Never ask; use -TrustHours or 8 h.

.PARAMETER Mode
Trust (import and schedule only), Install (setup only, certificate must already be trusted) or Both.
#>
[CmdletBinding()]
param(
    [ValidateRange(0.001, 72)] [double]$TrustHours,
    [switch]$RemoveAfterInstall,
    [switch]$Interactive,
    [switch]$SkipInstall,
    [switch]$NoPrompt,
    [ValidateSet('Trust', 'Install', 'Both')] [string]$Mode = 'Both'
)

$ErrorActionPreference = 'Stop'
$kitDir = $PSScriptRoot
Import-Module (Join-Path $kitDir 'DsaTrust.psm1') -Force
$dataDir = Get-DsaDataDir
$removeScript = Join-Path $dataDir 'Remove-Trust.ps1'
$trustAdded = $false

function Test-InteractiveSession {
    return ([Environment]::UserInteractive -and -not [Console]::IsInputRedirected)
}

function Read-TrustChoice {
    # Returns @{ Hours; Remove }. Enter or 30 s picks the default (8 h).
    $presets = @{ '1' = 1.0; '2' = 4.0; '3' = 8.0; '4' = 12.0; '5' = 24.0 }
    Write-Host 'How long should the trust stay active?  [1] 1 h  [2] 4 h  [3] 8 h (default)  [4] 12 h  [5] 24 h  [C] custom (e.g. 90m, 2.5h)  [R] remove right after install'
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $lastShown = -1
    while ($true) {
        $remaining = [int][Math]::Ceiling(30 - $clock.Elapsed.TotalSeconds)
        if ($remaining -le 0) { return @{ Hours = $script:DefaultHours; Remove = $false } }
        if ($remaining -ne $lastShown) {
            $lastShown = $remaining
            Write-Host ("`r" + "default in $remaining s".PadRight(60)) -NoNewline
        }
        if ([Console]::KeyAvailable) {
            $key = [Console]::ReadKey($true).KeyChar.ToString().ToUpperInvariant()
            if ($key -eq "`r" -or $key -eq "`n") { Write-Host ''; return @{ Hours = $script:DefaultHours; Remove = $false } }
            if ($presets.ContainsKey($key)) { Write-Host ''; return @{ Hours = $presets[$key]; Remove = $false } }
            if ($key -eq 'R') { Write-Host ''; return @{ Hours = $script:DefaultHours; Remove = $true } }
            if ($key -eq 'C') {
                Write-Host ''
                for ($attempt = 1; $attempt -le 2; $attempt++) {
                    $hours = ConvertTo-TrustHours (Read-Host 'Custom duration (e.g. 90m, 2.5h)')
                    if ($hours) { return @{ Hours = $hours; Remove = $false } }
                    Write-Host 'Not a duration. Use 90m, 2.5h or 3.'
                }
                Write-Host 'Using the 8 h default.'
                return @{ Hours = $script:DefaultHours; Remove = $false }
            }
        }
        Start-Sleep -Milliseconds 200
    }
}

try {
    Write-TrustLog "Install-DSA started (mode $Mode) by $env:USERNAME"

    # 1. Verify the kit.
    $cer = Join-Path $kitDir 'dsa-signing.cer'
    $thumbFile = Join-Path $kitDir 'thumbprint.txt'
    $sums = Join-Path $kitDir 'SHA256SUMS.txt'
    $setup = Get-ChildItem -LiteralPath $kitDir -Filter '*-setup.exe' -File | Select-Object -First 1
    foreach ($required in @($cer, $thumbFile, $sums)) {
        if (-not (Test-Path -LiteralPath $required)) { throw "Missing $(Split-Path -Leaf $required) next to this script." }
    }
    if (-not $setup) { throw 'Missing the *-setup.exe next to this script.' }

    $thumb = Get-CerThumbprint -CerPath $cer
    $published = (Get-Content -LiteralPath $thumbFile -Raw).Trim()
    if ($published -ine $thumb) { throw "thumbprint.txt ($published) does not match the certificate ($thumb)." }
    Test-Sha256Sums -SumsPath $sums -FilePaths @($cer, $setup.FullName)
    Write-TrustLog "Verified kit hashes. Signing thumbprint: $thumb"

    # 2. Choose the trust window.
    $hours = $null
    $remove = [bool]$RemoveAfterInstall
    if ($Mode -ne 'Install') {
        if ($PSBoundParameters.ContainsKey('TrustHours')) {
            $hours = $TrustHours
        }
        elseif ($NoPrompt -or -not (Test-InteractiveSession)) {
            $hours = 8.0
        }
        else {
            $choice = Read-TrustChoice
            $hours = $choice.Hours
            $remove = $remove -or $choice.Remove
        }
    }

    # 3-4. Trust the certificate, record the expiry and schedule its removal.
    if ($Mode -ne 'Install') {
        Add-TrustStores -CerPath $cer
        $trustAdded = $true
        $expires = [DateTime]::UtcNow.AddHours($hours)
        Write-TrustExpiry -Thumbprint $thumb -ExpiresUtc $expires -Hours $hours -RemoveAfterInstall $remove
        New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
        # Copies outlive the USB stick, so the expiry task still works after the kit is unplugged.
        Copy-Item -LiteralPath (Join-Path $kitDir 'DsaTrust.psm1'), (Join-Path $kitDir 'Remove-Trust.ps1') -Destination $dataDir -Force
        Register-TrustTask -ExpiresUtc $expires -RemoveScriptPath $removeScript
        Write-TrustLog "Trust active until $($expires.ToString('o')) UTC ($hours h)"
    }

    # 5. Check the setup signature against the trusted certificate.
    $sig = Get-AuthenticodeSignature -LiteralPath $setup.FullName
    $signedBy = $null
    if ($sig.SignerCertificate) { $signedBy = $sig.SignerCertificate.Thumbprint }
    if ($sig.Status -ne 'Valid' -or $signedBy -ine $thumb) {
        throw "Setup signature check failed (status $($sig.Status)). For Install mode, trust the certificate first with Trust-Only.cmd."
    }
    Write-TrustLog 'Setup signature is valid and matches the certificate'

    # 6. Run the setup.
    if (-not $SkipInstall -and $Mode -ne 'Trust') {
        $setupArgs = @()
        if (-not $Interactive) { $setupArgs = @('/S') }
        if ($setupArgs.Count -gt 0) {
            $proc = Start-Process -FilePath $setup.FullName -ArgumentList $setupArgs -Wait -PassThru
        }
        else {
            $proc = Start-Process -FilePath $setup.FullName -Wait -PassThru
        }
        if ($proc.ExitCode -ne 0) { throw "Setup exited with code $($proc.ExitCode)." }
        Write-TrustLog 'Setup finished'
    }

    # 7. Countdown, or remove straight away.
    if ($remove -and $Mode -ne 'Install') {
        Remove-ActiveTrust
        Write-TrustLog 'Removed trust right after install, as requested'
        exit 0
    }
    if ($Mode -ne 'Install') {
        if (Test-InteractiveSession) {
            Show-TrustCountdown -RemoveScriptPath $removeScript
        }
        else {
            Write-TrustLog "Trust expires at $($expires.ToLocalTime().ToString('HH:mm')) local. Use Show-Trust-Timer.cmd to watch it."
        }
    }
    exit 0
}
catch {
    Write-TrustLog "Install-DSA failed: $($_.Exception.Message)" 'ERROR'
    if ($trustAdded) {
        Remove-ActiveTrust
    }
    exit 1
}
