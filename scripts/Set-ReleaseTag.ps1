[CmdletBinding()]
param(
    [string]$Version = 'v1.1.1'
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

if ($Version -notmatch '^v\d+\.\d+\.\d+$') { throw "Version '$Version' must look like v1.2.3" }

git fetch origin main --tags
if ($LASTEXITCODE) { throw 'git fetch failed' }

if (git tag -l $Version) { throw "Tag $Version already exists locally" }
if (git ls-remote --tags origin $Version) { throw "Tag $Version already exists on origin" }

$head = (git rev-parse origin/main).Trim()
$ci = gh run list --branch main --workflow CI --limit 1 --json conclusion,headSha | ConvertFrom-Json
if (-not $ci -or $ci[0].headSha -ne $head -or $ci[0].conclusion -ne 'success') {
    throw "CI on origin/main ($($head.Substring(0,7))) is not a finished success. Wait for it, then rerun."
}

Write-Host "Tagging $Version at $($head.Substring(0,7)) (origin/main) and pushing. This starts the release workflow."
if ((Read-Host 'Type yes to continue') -ne 'yes') { Write-Host 'Cancelled.'; return }

git tag -a $Version $head -m "Deep Server Analyzer $Version"
git push origin $Version
if ($LASTEXITCODE) { throw 'git push failed' }

Write-Host "Pushed $Version. Next: scripts\Get-Release.ps1 -Version $Version"
