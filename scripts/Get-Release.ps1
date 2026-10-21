[CmdletBinding()]
param(
    [string]$Version = 'v1.1.1',
    [string]$OutDir = (Join-Path $env:USERPROFILE "Downloads\dsa-$Version")
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

# Wait for the release run for this tag to finish.
$run = $null
for ($i = 0; $i -lt 60 -and -not $run; $i++) {
    $run = gh run list --workflow 'Build installer' --branch $Version --limit 1 --json databaseId,status,conclusion | ConvertFrom-Json | Select-Object -First 1
    if (-not $run) { Start-Sleep 5 }
}
if (-not $run) { throw "No Release workflow run found for $Version" }
Write-Host "Watching release run $($run.databaseId)..."
gh run watch $run.databaseId --exit-status
if ($LASTEXITCODE) { throw "Release run failed. See: gh run view $($run.databaseId) --log-failed" }

# Download into a new empty folder; release assets are untrusted until verified.
if ((Test-Path $OutDir) -and (Get-ChildItem $OutDir -Force)) {
    # Re-run: move the old download aside so only fresh release assets get verified.
    $old = "$OutDir.old-$(Get-Date -Format yyyyMMdd-HHmmss)"
    Move-Item $OutDir $old
    Write-Host "Existing $OutDir moved to $old"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
gh release download $Version --dir $OutDir
if ($LASTEXITCODE) { throw 'gh release download failed' }

# Verify the checksums listed in SHA256SUMS.txt.
$sums = Join-Path $OutDir 'SHA256SUMS.txt'

# GitHub turns spaces in asset names into dots; the kit files inside the zip are
# checked from an extracted copy.
$kitDir = Join-Path $OutDir 'kit'
$zip = Get-ChildItem $OutDir -Filter '*usb-kit.zip' | Select-Object -First 1
if ($zip) { Expand-Archive $zip.FullName -DestinationPath $kitDir }

foreach ($line in Get-Content $sums) {
    $hash, $name = $line -split '\s+', 2
    $name = $name.Trim()
    $file = @(
        Join-Path $OutDir $name
        Join-Path $OutDir ($name -replace ' ', '.')
        Join-Path $kitDir $name
    ) | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
    if (-not $file) { Write-Host "MISSING  $name"; continue }
    $actual = (Get-FileHash $file -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host ('{0,-8} {1}' -f ($(if ($actual -eq $hash) { 'OK' } else { 'MISMATCH' })), $name)
}
Write-Host ('{0,-8} {1}' -f 'INFO', 'the zip itself is not in SHA256SUMS.txt; its contents are checked above')

# Certificate thumbprint to compare with the release notes.
$cer = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 (Join-Path $OutDir 'dsa-signing.cer')
Write-Host "`ndsa-signing.cer thumbprint: $($cer.Thumbprint)"
Write-Host 'Compare with the thumbprint in the release notes:'
gh release view $Version --json body -q .body | Select-String -Pattern 'thumbprint' -Context 0, 2
Write-Host "`nFiles in $OutDir"
Get-ChildItem $OutDir | Select-Object Name, Length
