<#
.SYNOPSIS
Tests for the USB kit. Unit checks run anywhere. -Full also runs the install, expiry and removal path
on this machine with a throwaway certificate, so run it only on a test machine or CI runner (as Administrator).

.PARAMETER Full
Run the end-to-end path: install with -TrustHours 0.01, check idempotency, early and expiry removal.
#>
[CmdletBinding()]
param(
    [switch]$Full
)

$ErrorActionPreference = 'Stop'
$kitDir = Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $kitDir 'DsaTrust.psm1') -Force

$script:Passes = 0
$script:Failures = 0

function Assert-True {
    param([string]$Name, [bool]$Condition)
    if ($Condition) {
        $script:Passes++
        Write-Host "  ok    $Name"
    }
    else {
        $script:Failures++
        Write-Host "  FAIL  $Name"
    }
}

function Assert-Throws {
    param([string]$Name, [scriptblock]$Block)
    $threw = $false
    try { & $Block } catch { $threw = $true }
    Assert-True $Name $threw
}

function Get-StoreCount {
    param([string]$Thumbprint, [string]$StoreName)
    $store = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Store -ArgumentList $StoreName, 'LocalMachine'
    $store.Open('ReadOnly')
    try { return @($store.Certificates | Where-Object { $_.Thumbprint -eq $Thumbprint }).Count }
    finally { $store.Close() }
}

function Invoke-KitScript {
    param([string]$ScriptPath, [string[]]$Arguments = @())
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $ScriptPath) + $Arguments
    # Runs in this console so the script's output shows in the CI log.
    & powershell.exe @argList | Out-Host
    return $LASTEXITCODE
}

Write-Host 'Unit checks'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('dsa-kit-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
$env:DSA_TRUST_DIR = $scratch

try {
    # Duration parser: valid, invalid and clamped input.
    Assert-True 'bare number is hours' ((ConvertTo-TrustHours '3') -eq 3)
    Assert-True '2.5h is 2.5 hours' ((ConvertTo-TrustHours '2.5h') -eq 2.5)
    Assert-True '90m is 1.5 hours' ((ConvertTo-TrustHours '90m') -eq 1.5)
    Assert-True '90 min is 1.5 hours' ((ConvertTo-TrustHours '90 min') -eq 1.5)
    Assert-True '1m clamps up to 15 min' ((ConvertTo-TrustHours '1m') -eq 0.25)
    Assert-True '100 clamps down to 72 h' ((ConvertTo-TrustHours '100') -eq 72)
    Assert-True 'junk is rejected' ($null -eq (ConvertTo-TrustHours 'soon'))
    Assert-True 'empty is rejected' ($null -eq (ConvertTo-TrustHours ''))
    Assert-True 'negative is rejected' ($null -eq (ConvertTo-TrustHours '-2'))
    Assert-True 'unknown unit is rejected' ($null -eq (ConvertTo-TrustHours '1d'))

    # Expiry record round trip.
    $when = [DateTime]::new(2030, 1, 2, 3, 4, 5, [DateTimeKind]::Utc)
    Write-TrustExpiry -Thumbprint 'ABCDEF' -ExpiresUtc $when -Hours 2.5 -RemoveAfterInstall $true
    $back = Read-TrustExpiry
    Assert-True 'expiry round-trips the instant' ($back.ExpiresUtc -eq $when)
    Assert-True 'expiry round-trips the thumbprint' ($back.Thumbprint -eq 'ABCDEF')
    Assert-True 'expiry round-trips hours' ($back.Hours -eq 2.5)
    Assert-True 'expiry round-trips removeAfterInstall' ($back.RemoveAfterInstall -eq $true)
    Write-TrustExpiry -Thumbprint 'ABCDEF' -ExpiresUtc $when.AddHours(1) -Hours 3.5 -RemoveAfterInstall $false
    Assert-True 'a second write replaces the first' ((Read-TrustExpiry).ExpiresUtc -eq $when.AddHours(1))
    Set-Content -LiteralPath (Get-DsaExpiryPath) -Value 'not json' -Encoding UTF8
    Assert-True 'a corrupt record reads as no trust' ($null -eq (Read-TrustExpiry))

    # Removal checks and the extend helper's no-trust path.
    $storeChecks = @(Test-TrustRemoved -Thumbprint '0000000000000000000000000000000000000000' | Where-Object { $_.Name -like 'Certificate*' })
    Assert-True 'removal check covers both stores' ($storeChecks.Count -eq 2)
    Assert-True 'an unknown thumbprint counts as absent' (@($storeChecks | Where-Object { -not $_.Passed }).Count -eq 0)
    $recordCheck = Test-TrustRemoved -Thumbprint '' | Where-Object { $_.Name -like 'Expiry record*' }
    Assert-True 'a leftover expiry record fails the check' (-not $recordCheck.Passed)
    Remove-Item -LiteralPath (Get-DsaExpiryPath) -Force
    Assert-True 'no leftover record passes the check' ((Test-TrustRemoved -Thumbprint '' | Where-Object { $_.Name -like 'Expiry record*' }).Passed)
    Assert-True 'extending with no trust returns null' ($null -eq (Add-TrustHours -RemoveScriptPath 'x'))

    # Startup and expiry rule: never remove before the expiry.
    $expiry = [DateTime]::UtcNow.AddMinutes(10)
    Assert-True 'not due one second early' (-not (Test-TrustDue -ExpiresUtc $expiry -NowUtc $expiry.AddSeconds(-1)))
    Assert-True 'due at the expiry instant' (Test-TrustDue -ExpiresUtc $expiry -NowUtc $expiry)
    Assert-True 'due after the expiry' (Test-TrustDue -ExpiresUtc $expiry -NowUtc $expiry.AddMinutes(5))
    Assert-True 'not due for a window still open' (-not (Test-TrustDue -ExpiresUtc ([DateTime]::UtcNow.AddHours(1))))

    # SHA256SUMS checks.
    $sample = Join-Path $scratch 'sample.bin'
    Set-Content -LiteralPath $sample -Value 'hello' -Encoding ascii -NoNewline
    $sumsPath = Join-Path $scratch 'SHA256SUMS.txt'
    $hash = (Get-FileHash -LiteralPath $sample -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $sumsPath -Value "$hash  sample.bin" -Encoding ascii
    $matched = $true
    try { Test-Sha256Sums -SumsPath $sumsPath -FilePaths @($sample) } catch { $matched = $false }
    Assert-True 'matching hash passes' $matched
    Assert-Throws 'tampered file fails' {
        Set-Content -LiteralPath $sample -Value 'tampered' -Encoding ascii -NoNewline
        Test-Sha256Sums -SumsPath $sumsPath -FilePaths @($sample)
    }
    Assert-Throws 'file missing from SHA256SUMS fails' {
        Test-Sha256Sums -SumsPath $sumsPath -FilePaths @((Join-Path $scratch 'absent.bin'))
    }

    if ($Full) {
        Write-Host 'End-to-end (administrator, real stores and task)'
        $env:DSA_TRUST_DIR = $null
        $stage = Join-Path $scratch 'kit'
        New-Item -ItemType Directory -Path $stage | Out-Null
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=DSA Kit Test' `
            -CertStoreLocation Cert:\CurrentUser\My -KeyAlgorithm RSA -KeyLength 2048 `
            -HashAlgorithm SHA256 -NotAfter (Get-Date).AddDays(2)
        $thumb = $cert.Thumbprint
        try {
            $cerPath = Join-Path $stage 'dsa-signing.cer'
            Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
            # notepad.exe copy stands in for the NSIS setup; it is re-signed with the throwaway certificate.
            $setupPath = Join-Path $stage 'DSA-Test_0.0.0_x64-setup.exe'
            Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\notepad.exe') -Destination $setupPath
            # notepad.exe is catalog-signed by Microsoft, and the catalog signer wins over an embedded one.
            # Trailing bytes change the file hash so no catalog matches and only our signature counts.
            $tail = [System.Text.Encoding]::ASCII.GetBytes([guid]::NewGuid().ToString())
            $stream = [System.IO.File]::Open($setupPath, [System.IO.FileMode]::Append)
            try { $stream.Write($tail, 0, $tail.Length) } finally { $stream.Dispose() }
            $signed = Set-AuthenticodeSignature -FilePath $setupPath -Certificate $cert -HashAlgorithm SHA256
            # Status is UnknownError until the installer trusts the certificate; only the signer matters here.
            if ($signed.Status -in 'NotSigned', 'HashMismatch', 'NotSupportedFileFormat') { throw "Re-signing the test setup failed: $($signed.Status) $($signed.StatusMessage)" }
            $actual = (Get-AuthenticodeSignature -LiteralPath $setupPath).SignerCertificate.Thumbprint
            if ($actual -ine $thumb) { throw "Test setup is signed by $actual, not the test certificate $thumb." }
            Set-Content -LiteralPath (Join-Path $stage 'thumbprint.txt') -Value $thumb -Encoding ascii
            $sums = foreach ($file in @($cerPath, $setupPath)) {
                '{0}  {1}' -f (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant(), (Split-Path -Leaf $file)
            }
            Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS.txt') -Value $sums -Encoding ascii
            Get-ChildItem -LiteralPath $kitDir -File | Copy-Item -Destination $stage

            $install = Join-Path $stage 'Install-DSA.ps1'
            $remove = Join-Path $stage 'Remove-Trust.ps1'
            $installArgs = @('-NoPrompt', '-SkipInstall', '-TrustHours', '0.01')

            $installCode = Invoke-KitScript $install $installArgs
            if ($installCode -ne 0) {
                Write-Host '--- trust.log ---'
                Get-Content -LiteralPath (Join-Path (Get-DsaDataDir) 'trust.log') -Tail 40 -ErrorAction SilentlyContinue | Out-Host
            }
            Assert-True 'install exits 0' ($installCode -eq 0)
            Assert-True 'certificate in LocalMachine Root' ((Get-StoreCount $thumb 'Root') -eq 1)
            Assert-True 'certificate in LocalMachine TrustedPublisher' ((Get-StoreCount $thumb 'TrustedPublisher') -eq 1)
            Assert-True 'DSA-TrustExpiry task exists' ($null -ne (Get-ScheduledTask -TaskName 'DSA-TrustExpiry' -ErrorAction SilentlyContinue))
            $record = Read-TrustExpiry
            Assert-True 'expiry record names the thumbprint' ($record.Thumbprint -ieq $thumb)

            $extended = Add-TrustHours -RemoveScriptPath (Join-Path (Get-DsaDataDir) 'Remove-Trust.ps1')
            Assert-True 'extend moves the expiry out by an hour' ($extended -gt $record.ExpiresUtc.AddMinutes(59))
            Assert-True 'extend keeps one task' (@(Get-ScheduledTask -TaskName 'DSA-TrustExpiry' -ErrorAction SilentlyContinue).Count -eq 1)
            Write-TrustExpiry -Thumbprint $record.Thumbprint -ExpiresUtc $record.ExpiresUtc -Hours $record.Hours -RemoveAfterInstall $false
            Register-TrustTask -ExpiresUtc $record.ExpiresUtc -RemoveScriptPath (Join-Path (Get-DsaDataDir) 'Remove-Trust.ps1')

            Assert-True 'second install exits 0' ((Invoke-KitScript $install $installArgs) -eq 0)
            Assert-True 'second install keeps one Root copy' ((Get-StoreCount $thumb 'Root') -eq 1)
            Assert-True 'second install keeps one task' (@(Get-ScheduledTask -TaskName 'DSA-TrustExpiry' -ErrorAction SilentlyContinue).Count -eq 1)

            Assert-True 'startup check before expiry leaves trust' ((Invoke-KitScript $remove @('-FromTask')) -eq 0)
            Assert-True 'trust still present after early check' ((Get-StoreCount $thumb 'Root') -eq 1)

            if ($null -eq $record) { throw 'No expiry record was written, so there is nothing to wait for.' }
            $wait = [Math]::Ceiling(($record.ExpiresUtc - [DateTime]::UtcNow).TotalSeconds) + 3
            Write-Host "  waiting $wait s for the window to expire"
            Start-Sleep -Seconds $wait
            Assert-True 'expiry check after expiry removes trust' ((Invoke-KitScript $remove @('-FromTask')) -eq 0)
            Assert-True 'Root copy gone' ((Get-StoreCount $thumb 'Root') -eq 0)
            Assert-True 'TrustedPublisher copy gone' ((Get-StoreCount $thumb 'TrustedPublisher') -eq 0)
            Assert-True 'task gone' ($null -eq (Get-ScheduledTask -TaskName 'DSA-TrustExpiry' -ErrorAction SilentlyContinue))
            Assert-True 'expiry record gone' (-not (Test-Path -LiteralPath (Get-DsaExpiryPath)))
            Assert-True 'manual remove with no trust exits 0' ((Invoke-KitScript $remove) -eq 0)
        }
        finally {
            Get-ChildItem -Path Cert:\CurrentUser\My, Cert:\LocalMachine\Root, Cert:\LocalMachine\TrustedPublisher -ErrorAction SilentlyContinue |
                Where-Object { $_.Thumbprint -eq $thumb } | ForEach-Object {
                    if ($_.PSParentPath -like '*\My') { Remove-Item -LiteralPath $_.PSPath -DeleteKey } else { Remove-Item -LiteralPath $_.PSPath }
                }
            Unregister-TrustTask
            Remove-Item -LiteralPath (Get-DsaExpiryPath), (Join-Path (Get-DsaDataDir) 'Remove-Trust.ps1'), (Join-Path (Get-DsaDataDir) 'DsaTrust.psm1') -Force -ErrorAction SilentlyContinue
        }
    }
}
finally {
    $env:DSA_TRUST_DIR = $scratch
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Passed $script:Passes, failed $script:Failures"
if ($script:Failures -gt 0) { exit 1 }
exit 0
