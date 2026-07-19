[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$FastVepSource,
    [string]$OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$pinFile = Join-Path $projectRoot "config\fastvep-pin.json"
$pin = Get-Content -LiteralPath $pinFile -Raw | ConvertFrom-Json
$fastVepRoot = (Resolve-Path -LiteralPath $FastVepSource).Path
$actualCommit = (& git -C $fastVepRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $actualCommit -ne $pin.commit) {
    throw "fastVEP source commit mismatch: expected $($pin.commit), found $actualCommit"
}

$lockFile = Join-Path $fastVepRoot "Cargo.lock"
$lockHash = (Get-FileHash -LiteralPath $lockFile -Algorithm SHA256).Hash.ToLowerInvariant()
if ($lockHash -ne $pin.cargoLockSha256) {
    throw "fastVEP Cargo.lock mismatch: expected $($pin.cargoLockSha256), found $lockHash"
}

& (Join-Path $PSScriptRoot "test-fastvep-pin.ps1") -FastVepSource $fastVepRoot

& cargo test --manifest-path (Join-Path $fastVepRoot "Cargo.toml") --workspace --locked
if ($LASTEXITCODE -ne 0) { throw "fastVEP tests failed" }
& cargo build --manifest-path (Join-Path $fastVepRoot "Cargo.toml") --release --locked -p fastvep-cli
if ($LASTEXITCODE -ne 0) { throw "fastVEP release build failed" }
& cargo build --manifest-path (Join-Path $projectRoot "Cargo.toml") --release --locked -p annocat-cli --bins
if ($LASTEXITCODE -ne 0) { throw "AnnoCat release build failed" }

$version = (& (Join-Path $projectRoot "target\release\annocat.exe") --version).Split()[-1]
$outputRoot = Join-Path $projectRoot $OutputDirectory
$bundleRoot = Join-Path $outputRoot "AnnoCat-$version-windows-x86_64"
$resolvedOutputRoot = [System.IO.Path]::GetFullPath($outputRoot)
$resolvedBundleRoot = [System.IO.Path]::GetFullPath($bundleRoot)
if (-not $resolvedBundleRoot.StartsWith($resolvedOutputRoot + [System.IO.Path]::DirectorySeparatorChar)) {
    throw "Refusing to clean bundle path outside output directory: $resolvedBundleRoot"
}
if (Test-Path -LiteralPath $bundleRoot) {
    Remove-Item -LiteralPath $bundleRoot -Recurse -Force
}
New-Item -ItemType Directory -Path (Join-Path $bundleRoot "tools\fastvep") -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $bundleRoot "licenses") -Force | Out-Null

$annocatExe = Join-Path $projectRoot "target\release\annocat.exe"
$reportWorkerExe = Join-Path $projectRoot "target\release\annocat-report-worker.exe"
$fastVepExe = Join-Path $fastVepRoot "target\release\fastvep.exe"
Copy-Item -LiteralPath $annocatExe -Destination (Join-Path $bundleRoot "annocat.exe")
Copy-Item -LiteralPath $reportWorkerExe -Destination (Join-Path $bundleRoot "annocat-report-worker.exe")
Copy-Item -LiteralPath $fastVepExe -Destination (Join-Path $bundleRoot "tools\fastvep\fastvep.exe")
Copy-Item -LiteralPath (Join-Path $projectRoot "launch-annocat.cmd") -Destination $bundleRoot
Copy-Item -LiteralPath (Join-Path $projectRoot "README.md") -Destination $bundleRoot
Copy-Item -LiteralPath (Join-Path $projectRoot "LICENSE") -Destination (Join-Path $bundleRoot "LICENSE.txt")
Copy-Item -LiteralPath (Join-Path $projectRoot "third-party\fastvep\LICENSE.md") -Destination (Join-Path $bundleRoot "licenses\fastVEP-Apache-2.0.txt")

$fastVepHash = (Get-FileHash -LiteralPath (Join-Path $bundleRoot "tools\fastvep\fastvep.exe") -Algorithm SHA256).Hash.ToLowerInvariant()
$manifest = [ordered]@{
    schemaVersion = 1
    product = "AnnoCat"
    version = $version
    platform = "windows-x86_64"
    createdUtc = [DateTime]::UtcNow.ToString("o")
    fastVep = [ordered]@{
        version = $pin.upstreamVersion
        repository = $pin.repository
        commit = $pin.commit
        branch = $pin.branch
        upstreamRepository = $pin.upstreamRepository
        upstreamCommit = $pin.upstreamCommit
        cargoLockSha256 = $pin.cargoLockSha256
        changes = @($pin.changes | ForEach-Object {
            [ordered]@{
                commit = $_.commit
                purpose = $_.purpose
            }
        })
        executable = "tools/fastvep/fastvep.exe"
        sizeBytes = (Get-Item -LiteralPath (Join-Path $bundleRoot "tools\fastvep\fastvep.exe")).Length
        sha256 = $fastVepHash
        license = $pin.license
    }
    files = [ordered]@{
        "annocat.exe" = (Get-FileHash -LiteralPath (Join-Path $bundleRoot "annocat.exe") -Algorithm SHA256).Hash.ToLowerInvariant()
        "annocat-report-worker.exe" = (Get-FileHash -LiteralPath (Join-Path $bundleRoot "annocat-report-worker.exe") -Algorithm SHA256).Hash.ToLowerInvariant()
        "tools/fastvep/fastvep.exe" = $fastVepHash
        "LICENSE.txt" = (Get-FileHash -LiteralPath (Join-Path $bundleRoot "LICENSE.txt") -Algorithm SHA256).Hash.ToLowerInvariant()
        "licenses/fastVEP-Apache-2.0.txt" = (Get-FileHash -LiteralPath (Join-Path $bundleRoot "licenses\fastVEP-Apache-2.0.txt") -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $bundleRoot "bundle-manifest.json") -Encoding utf8

$zipPath = "$bundleRoot.zip"
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Compress-Archive -LiteralPath $bundleRoot -DestinationPath $zipPath -CompressionLevel Optimal
$zipHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $([System.IO.Path]::GetFileName($zipPath))" | Set-Content -LiteralPath "$zipPath.sha256" -Encoding ascii
Write-Host "Created $zipPath"
Write-Host "SHA-256 $zipHash"
