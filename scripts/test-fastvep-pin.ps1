[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$FastVepSource
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$pin = Get-Content -LiteralPath (Join-Path $projectRoot "config\fastvep-pin.json") -Raw | ConvertFrom-Json
$sourceRoot = (Resolve-Path -LiteralPath $FastVepSource).Path

$actualCommit = (& git -C $sourceRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $actualCommit -ne $pin.commit) {
    throw "fastVEP source commit mismatch: expected $($pin.commit), found $actualCommit"
}

& git -C $sourceRoot diff --quiet
if ($LASTEXITCODE -ne 0) { throw "fastVEP source has uncommitted tracked changes" }
& git -C $sourceRoot diff --cached --quiet
if ($LASTEXITCODE -ne 0) { throw "fastVEP source has staged changes" }

$lockHash = (Get-FileHash -LiteralPath (Join-Path $sourceRoot "Cargo.lock") -Algorithm SHA256).Hash.ToLowerInvariant()
if ($lockHash -ne $pin.cargoLockSha256) {
    throw "fastVEP Cargo.lock mismatch: expected $($pin.cargoLockSha256), found $lockHash"
}

& git -C $sourceRoot merge-base --is-ancestor $pin.upstreamCommit $pin.commit
if ($LASTEXITCODE -ne 0) {
    throw "Pinned fastVEP commit does not descend from upstream commit $($pin.upstreamCommit)"
}

$actualChanges = @(& git -C $sourceRoot rev-list --reverse "$($pin.upstreamCommit)..$($pin.commit)")
$expectedChanges = @($pin.changes | ForEach-Object { $_.commit })
if ($actualChanges.Count -ne $expectedChanges.Count) {
    throw "fastVEP fork history contains $($actualChanges.Count) commits; expected $($expectedChanges.Count)"
}
for ($index = 0; $index -lt $expectedChanges.Count; $index++) {
    if ($actualChanges[$index] -ne $expectedChanges[$index]) {
        throw "fastVEP fork commit $($index + 1) mismatch: expected $($expectedChanges[$index]), found $($actualChanges[$index])"
    }
}

Write-Host "Verified fastVEP fork commit $($pin.commit) with $($expectedChanges.Count) ordered AnnoCAT changes."
