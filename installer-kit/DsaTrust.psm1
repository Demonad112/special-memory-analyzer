# Shared helpers for the Deep Server Analyzer USB kit.
# Used by Install-DSA.ps1, Remove-Trust.ps1, Show-Trust-Timer.ps1 and tests\Test-Kit.ps1.
# Licensed under Apache-2.0. Only run on machines you are authorized to change.

Set-StrictMode -Version 2.0

$script:TaskName = 'DSA-TrustExpiry'
$script:MinHours = 0.25
$script:MaxHours = 72.0
$script:DefaultHours = 8.0

function Get-DsaDataDir {
    # DSA_TRUST_DIR lets the tests use a scratch folder instead of ProgramData.
    if ($env:DSA_TRUST_DIR) { return $env:DSA_TRUST_DIR }
    return (Join-Path $env:ProgramData 'DSA')
}

function Get-DsaExpiryPath {
    return (Join-Path (Get-DsaDataDir) 'trust-expiry.json')
}

function Write-TrustLog {
    param([string]$Message, [string]$Level = 'INFO')
    $dir = Get-DsaDataDir
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $line = '{0} [{1}] {2}' -f [DateTime]::UtcNow.ToString('o'), $Level, $Message
    Add-Content -LiteralPath (Join-Path $dir 'trust.log') -Value $line -Encoding UTF8
    Write-Host $Message
}

function ConvertTo-TrustHours {
    # "3" and "3h" are hours, "90m" and "90min" are minutes, "2.5h" is fractional.
    # Returns hours clamped to 15 min .. 72 h, or $null when the text is not a duration.
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return $null }
    $match = [regex]::Match($Text.Trim(), '^(\d+(?:\.\d+)?)\s*(h|hr|hours?|m|min|mins|minutes?)?$', 'IgnoreCase')
    if (-not $match.Success) { return $null }
    $value = [double]::Parse($match.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)
    $unit = $match.Groups[2].Value.ToLowerInvariant()
    $hours = $value
    if ($unit.StartsWith('m')) { $hours = $value / 60.0 }
    if ($hours -lt $script:MinHours) { $hours = $script:MinHours }
    if ($hours -gt $script:MaxHours) { $hours = $script:MaxHours }
    return [Math]::Round($hours, 4)
}

function Get-CerThumbprint {
    param([string]$CerPath)
    $cert = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Certificate2 -ArgumentList $CerPath
    return $cert.Thumbprint
}

function Test-Sha256Sums {
    # Checks each named file against a sha256sum-style list ("<hash>  <name>").
    # Throws on a missing entry or a mismatch.
    param([string]$SumsPath, [string[]]$FilePaths)
    $entries = @{}
    foreach ($line in (Get-Content -LiteralPath $SumsPath)) {
        $m = [regex]::Match($line, '^([0-9a-fA-F]{64})\s+\*?(.+)$')
        if ($m.Success) { $entries[$m.Groups[2].Value.Trim()] = $m.Groups[1].Value.ToLowerInvariant() }
    }
    foreach ($file in $FilePaths) {
        $name = Split-Path -Leaf $file
        if (-not $entries.ContainsKey($name)) { throw "SHA256SUMS.txt has no entry for $name" }
        $actual = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $entries[$name]) { throw "Hash mismatch for $name" }
    }
}

function Write-TrustExpiry {
    param([string]$Thumbprint, [DateTime]$ExpiresUtc, [double]$Hours, [bool]$RemoveAfterInstall)
    New-Item -ItemType Directory -Force -Path (Get-DsaDataDir) | Out-Null
    $record = [ordered]@{
        thumbprint         = $Thumbprint
        expiresUtc         = $ExpiresUtc.ToUniversalTime().ToString('o')
        hours              = $Hours
        removeAfterInstall = $RemoveAfterInstall
    }
    $record | ConvertTo-Json | Set-Content -LiteralPath (Get-DsaExpiryPath) -Encoding UTF8
}

function Read-TrustExpiry {
    # Returns $null when no trust is recorded or the file cannot be read.
    $path = Get-DsaExpiryPath
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try {
        $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        # PowerShell 7 turns ISO dates into DateTime during ConvertFrom-Json, so accept both forms.
        $expires = $record.expiresUtc
        if ($expires -isnot [DateTime]) {
            $expires = [DateTime]::Parse([string]$expires, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
        }
        return [pscustomobject]@{
            Thumbprint         = [string]$record.thumbprint
            ExpiresUtc         = $expires.ToUniversalTime()
            Hours              = [double]$record.hours
            RemoveAfterInstall = [bool]$record.removeAfterInstall
        }
    }
    catch {
        Write-TrustLog "Could not read $path : $($_.Exception.Message)" 'WARN'
        return $null
    }
}

function Test-TrustDue {
    # The removal rule: trust is only removed once now is at or past the expiry.
    param([DateTime]$ExpiresUtc, [DateTime]$NowUtc = [DateTime]::UtcNow)
    return ($NowUtc -ge $ExpiresUtc)
}

function Add-TrustStores {
    param([string]$CerPath)
    $cert = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Certificate2 -ArgumentList $CerPath
    foreach ($name in 'Root', 'TrustedPublisher') {
        $store = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Store -ArgumentList $name, 'LocalMachine'
        $store.Open('ReadWrite')
        try {
            $present = @($store.Certificates | Where-Object { $_.Thumbprint -eq $cert.Thumbprint })
            if ($present.Count -eq 0) {
                $store.Add($cert)
                Write-TrustLog "Trusted $($cert.Thumbprint) in LocalMachine\$name"
            }
        }
        finally { $store.Close() }
    }
}

function Remove-TrustStores {
    param([string]$Thumbprint)
    foreach ($name in 'Root', 'TrustedPublisher') {
        $store = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Store -ArgumentList $name, 'LocalMachine'
        $store.Open('ReadWrite')
        try {
            foreach ($cert in @($store.Certificates | Where-Object { $_.Thumbprint -eq $Thumbprint })) {
                $store.Remove($cert)
                Write-TrustLog "Removed $Thumbprint from LocalMachine\$name"
            }
        }
        finally { $store.Close() }
    }
}

function Register-TrustTask {
    # Replaces any existing task, so extending the timer swaps the trigger rather than adding one.
    param([DateTime]$ExpiresUtc, [string]$RemoveScriptPath)
    $argument = '-NoProfile -ExecutionPolicy Bypass -File "{0}"' -f $RemoveScriptPath
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ($argument + ' -FromTask')
    # One minute of slack so the expiry check always sees now >= expiry.
    $atExpiry = New-ScheduledTaskTrigger -Once -At $ExpiresUtc.ToLocalTime().AddMinutes(1)
    $atStartup = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable
    Register-ScheduledTask -TaskName $script:TaskName -Action $action -Trigger @($atExpiry, $atStartup) `
        -Principal $principal -Settings $settings -Force | Out-Null
    Write-TrustLog "Registered $script:TaskName for $($ExpiresUtc.ToString('o')) UTC"
}

function Add-TrustHours {
    # Moves the expiry out by $Hours (capped at 72 h from now) and swaps the task trigger.
    # Returns the new expiry, or $null when no trust is active.
    param([string]$RemoveScriptPath, [double]$Hours = 1)
    $record = Read-TrustExpiry
    if (-not $record) { return $null }
    $newExpiry = $record.ExpiresUtc.AddHours($Hours)
    $cap = [DateTime]::UtcNow.AddHours($script:MaxHours)
    if ($newExpiry -gt $cap) { $newExpiry = $cap }
    Write-TrustExpiry -Thumbprint $record.Thumbprint -ExpiresUtc $newExpiry `
        -Hours ($record.Hours + $Hours) -RemoveAfterInstall $record.RemoveAfterInstall
    Register-TrustTask -ExpiresUtc $newExpiry -RemoveScriptPath $RemoveScriptPath
    Write-TrustLog "Extended trust by $Hours h, now until $($newExpiry.ToString('o')) UTC"
    return $newExpiry
}

function Test-TrustRemoved {
    # One result per check: @{ Name; Passed }. Empty $Thumbprint skips the store checks.
    param([string]$Thumbprint)
    $results = @()
    foreach ($name in 'Root', 'TrustedPublisher') {
        if (-not $Thumbprint) { continue }
        $store = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Store -ArgumentList $name, 'LocalMachine'
        $store.Open('ReadOnly')
        try { $count = @($store.Certificates | Where-Object { $_.Thumbprint -eq $Thumbprint }).Count }
        finally { $store.Close() }
        $results += [pscustomobject]@{ Name = "Certificate absent from LocalMachine\$name"; Passed = ($count -eq 0) }
    }
    $task = Get-ScheduledTask -TaskName $script:TaskName -ErrorAction SilentlyContinue
    $results += [pscustomobject]@{ Name = "Scheduled task $script:TaskName is gone"; Passed = ($null -eq $task) }
    $results += [pscustomobject]@{ Name = 'Expiry record is gone'; Passed = (-not (Test-Path -LiteralPath (Get-DsaExpiryPath))) }
    return $results
}

function Unregister-TrustTask {
    if (Get-ScheduledTask -TaskName $script:TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $script:TaskName -Confirm:$false
    }
}

function Remove-ActiveTrust {
    # Removes the certificate from both stores, the task and the expiry record.
    $record = Read-TrustExpiry
    if ($record -and $record.Thumbprint) { Remove-TrustStores -Thumbprint $record.Thumbprint }
    Unregister-TrustTask
    Remove-Item -LiteralPath (Get-DsaExpiryPath) -Force -ErrorAction SilentlyContinue
    Write-TrustLog 'Trust removed'
}

function Format-Remaining {
    param([TimeSpan]$Span)
    $total = [int][Math]::Max(0, [Math]::Floor($Span.TotalSeconds))
    return '{0:00}:{1:00}:{2:00}' -f [int][Math]::Floor($total / 3600), [int][Math]::Floor(($total % 3600) / 60), ($total % 60)
}

function Show-TrustCountdown {
    # Live countdown. The scheduled task is the source of truth, so closing the window never stops it.
    param([string]$RemoveScriptPath)
    $top = [Console]::CursorTop
    Write-Host ''
    Write-Host ''
    $lastDrawn = -1
    while ($true) {
        $record = Read-TrustExpiry
        if (-not $record) { Write-Host 'No active trust'; return }
        $now = [DateTime]::UtcNow
        if (Test-TrustDue -ExpiresUtc $record.ExpiresUtc -NowUtc $now) {
            Remove-ActiveTrust
            Write-Host 'Trust expired and was removed.'
            return
        }
        $second = [int][Math]::Floor(($record.ExpiresUtc - $now).TotalSeconds)
        if ($second -ne $lastDrawn) {
            $lastDrawn = $second
            $local = $record.ExpiresUtc.ToLocalTime().ToString('HH:mm')
            $headline = 'Trust expires in {0}  ({1} local)' -f (Format-Remaining ($record.ExpiresUtc - $now)), $local
            $hint = '[E] extend 1 h  [R] remove now  [Q] close this window - the timer keeps running'
            [Console]::SetCursorPosition(0, $top)
            Write-Host $headline.PadRight(100)
            Write-Host $hint.PadRight(100)
        }
        if ([Console]::KeyAvailable) {
            $key = [Console]::ReadKey($true).KeyChar.ToString().ToUpperInvariant()
            if ($key -eq 'E') {
                $null = Add-TrustHours -RemoveScriptPath $RemoveScriptPath
                $lastDrawn = -1
            }
            elseif ($key -eq 'R') {
                Remove-ActiveTrust
                Write-Host 'Trust removed.'
                return
            }
            elseif ($key -eq 'Q') {
                Write-Host 'Closed. The timer keeps running.'
                return
            }
        }
        Start-Sleep -Milliseconds 200
    }
}

function ConvertTo-NormalThumbprint {
    # Strips spaces, colons and dashes so a pasted thumbprint compares cleanly.
    param([string]$Text)
    if ($null -eq $Text) { return '' }
    return ($Text -replace '[\s:-]', '').ToUpperInvariant()
}

function Test-DsaCertificate {
    # Refuses anything but the short-lived, self-signed, code-signing-only certificate the release builds.
    # Throws on the first failed check; -ExpectedThumbprint (from the release page) must match the full SHA-1.
    param(
        [Parameter(Mandatory)] [Security.Cryptography.X509Certificates.X509Certificate2]$Cert,
        [string]$ExpectedThumbprint
    )
    $eku = @($Cert.Extensions | Where-Object { $_ -is [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] })
    if ($eku.Count -ne 1) { throw 'Certificate refused: no enhanced key usage extension.' }
    $oids = @($eku[0].EnhancedKeyUsages | ForEach-Object { $_.Value })
    if ($oids.Count -ne 1 -or $oids[0] -ne '1.3.6.1.5.5.7.3.3') {
        throw "Certificate refused: it must be for code signing only (found $($oids -join ', '))."
    }
    $basic = $Cert.Extensions | Where-Object { $_ -is [Security.Cryptography.X509Certificates.X509BasicConstraintsExtension] }
    if ($basic -and $basic.CertificateAuthority) { throw 'Certificate refused: it is a certificate authority.' }
    if ($Cert.Subject -cne 'CN=Deep Server Analyzer' -or $Cert.Issuer -cne $Cert.Subject) {
        throw "Certificate refused: it must be self-signed as CN=Deep Server Analyzer (subject $($Cert.Subject), issuer $($Cert.Issuer))."
    }
    $now = [DateTime]::UtcNow
    if ($Cert.NotAfter.ToUniversalTime() -le $now) { throw 'Certificate refused: it has expired.' }
    if (($Cert.NotAfter - $Cert.NotBefore).TotalDays -gt 181) { throw 'Certificate refused: its lifetime is longer than 181 days.' }
    if ($Cert.PublicKey.Oid.Value -ne '1.2.840.113549.1.1.1') { throw 'Certificate refused: the public key is not RSA.' }
    $rsa = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($Cert)
    if ($null -eq $rsa -or $rsa.KeySize -lt 3072) { throw 'Certificate refused: the RSA key is shorter than 3072 bits.' }
    if ($ExpectedThumbprint) {
        if ((ConvertTo-NormalThumbprint $ExpectedThumbprint) -cne (ConvertTo-NormalThumbprint $Cert.Thumbprint)) {
            throw "Certificate refused: its thumbprint $($Cert.Thumbprint) does not match the expected $ExpectedThumbprint."
        }
    }
}

function Initialize-DsaDataDir {
    # Makes the data folder admin-only for writes. The SYSTEM task runs scripts from here, so Users must not be able to change them.
    # A folder owned by anyone else is moved aside, because its owner could keep changing it.
    $dir = Get-DsaDataDir
    $trustedOwners = @('BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM')
    # Ownership checks need an elevated process (the installer always is). Unelevated test runs only get the DACL.
    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($elevated -and (Test-Path -LiteralPath $dir)) {
        $owner = (Get-Acl -LiteralPath $dir).Owner
        if ($trustedOwners -notcontains $owner) {
            $aside = '{0}.untrusted-{1:yyyyMMddHHmmss}' -f $dir, (Get-Date)
            Move-Item -LiteralPath $dir -Destination $aside
            Write-TrustLog "Data folder was owned by $owner; moved aside to $aside" 'WARN'
        }
    }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $admins = New-Object Security.Principal.SecurityIdentifier ([Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid, $null)
    $system = New-Object Security.Principal.SecurityIdentifier ([Security.Principal.WellKnownSidType]::LocalSystemSid, $null)
    $users = New-Object Security.Principal.SecurityIdentifier ([Security.Principal.WellKnownSidType]::BuiltinUsersSid, $null)
    $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    $none = [Security.AccessControl.PropagationFlags]::None
    $allow = [Security.AccessControl.AccessControlType]::Allow
    $acl = New-Object Security.AccessControl.DirectorySecurity
    if ($elevated) { $acl.SetOwner($admins) }
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @($admins, $system)) {
        $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule ($sid, 'FullControl', $inherit, $none, $allow)))
    }
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule ($users, 'ReadAndExecute', $inherit, $none, $allow)))
    Set-Acl -LiteralPath $dir -AclObject $acl
}

Export-ModuleMember -Function Get-DsaDataDir, Get-DsaExpiryPath, Write-TrustLog, ConvertTo-TrustHours, Get-CerThumbprint, `
    Test-Sha256Sums, Write-TrustExpiry, Read-TrustExpiry, Test-TrustDue, Add-TrustStores, `
    Remove-TrustStores, Register-TrustTask, Add-TrustHours, Test-TrustRemoved, Unregister-TrustTask, `
    Remove-ActiveTrust, Format-Remaining, Show-TrustCountdown, ConvertTo-NormalThumbprint, Test-DsaCertificate, Initialize-DsaDataDir
