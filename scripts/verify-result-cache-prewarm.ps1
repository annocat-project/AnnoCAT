[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ResultDirectory,
    [string]$BaseUrl,
    [string]$RunId
)

$ErrorActionPreference = "Stop"

if ([bool]$BaseUrl -ne [bool]$RunId) {
    throw "BaseUrl and RunId must be supplied together."
}

$root = (Resolve-Path -LiteralPath $ResultDirectory).Path
$manifestPath = Join-Path $root "manifest.json"
$performancePath = Join-Path $root "annotation-performance.json"
if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Result manifest is missing: $manifestPath"
}
if (!(Test-Path -LiteralPath $performancePath -PathType Leaf)) {
    throw "Annotation performance data is missing: $performancePath"
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.representativeSelectionContract -ne "allele-gene-severity-v1") {
    throw "Result does not use the current representative-selection contract."
}
foreach ($name in @("annotation-state.json", "annotation-state.json.tmp")) {
    if (Test-Path -LiteralPath (Join-Path $root $name)) {
        throw "Completed result still contains $name."
    }
}

$projectionFiles = @(Get-ChildItem -LiteralPath $root -Force -File -Filter ".annocat-query-v3-*.parquet" | Sort-Object Name)
if (!$projectionFiles.Count) {
    throw "Result has no prewarmed viewer projections."
}
$legacyFiles = @(Get-ChildItem -LiteralPath $root -Force -File | Where-Object { $_.Name -like ".annocat-representatives-*" })
if ($legacyFiles.Count) {
    throw "Current result contains a legacy representative cache."
}

$performance = Get-Content -LiteralPath $performancePath -Raw | ConvertFrom-Json
$viewerStage = @($performance.stages | Where-Object { $_.stage -eq "viewer-projections" })
if ($viewerStage.Count -ne 1) {
    throw "Annotation did not record exactly one viewer-projections stage."
}

$cacheFiles = @(Get-ChildItem -LiteralPath $root -Force -File | Where-Object { $_.Name -like ".annocat-*" } | Sort-Object Name)
function Get-CacheState([object[]]$Files) {
    $state = @{}
    foreach ($file in $Files) {
        $state[$file.Name] = "$($file.Length)|$($file.LastWriteTimeUtc.Ticks)"
    }
    return $state
}
$before = Get-CacheState $cacheFiles
$requests = @()

if ($BaseUrl) {
    $fieldIndexes = @($projectionFiles | ForEach-Object {
        if ($_.Name -notmatch '^\.annocat-query-v3-(\d+)-') {
            throw "Cannot read field index from projection name: $($_.Name)"
        }
        [int]$Matches[1]
    })
    $columns = $fieldIndexes -join ","
    $base = $BaseUrl.TrimEnd('/')
    foreach ($index in $fieldIndexes) {
        $query = "offset=0&limit=200&evidenceColumns=$columns&sortEvidence=$index&direction=desc"
        $status = Invoke-RestMethod -Uri "$base/api/runs/$RunId/query-cache-status?$query"
        if (!$status.ready) {
            throw "Viewer projection for field $index is not ready before sorting."
        }
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $response = Invoke-WebRequest -UseBasicParsing -Uri "$base/api/runs/$RunId/variants?$query"
        $timer.Stop()
        if ($response.StatusCode -ne 200) {
            throw "Sort request for field $index returned HTTP $($response.StatusCode)."
        }
        $requests += [ordered]@{
            fieldIndex = $index
            milliseconds = $timer.ElapsedMilliseconds
        }
    }
}

$afterFiles = @(Get-ChildItem -LiteralPath $root -Force -File | Where-Object { $_.Name -like ".annocat-*" } | Sort-Object Name)
$after = Get-CacheState $afterFiles
if ($before.Count -ne $after.Count) {
    throw "Viewer requests changed the number of derived cache files."
}
foreach ($name in $before.Keys) {
    if (!$after.ContainsKey($name) -or $before[$name] -ne $after[$name]) {
        throw "Viewer request rewrote derived cache $name."
    }
}

[ordered]@{
    result = $root
    variants = $manifest.variantCount
    viewerProjectionFiles = $projectionFiles.Count
    selectedEvidenceFiles = @(Get-ChildItem -LiteralPath $root -Force -File -Filter ".annocat-evidence-*.parquet").Count
    viewerProjectionMilliseconds = $viewerStage[0].wallTimeMs
    sortRequests = $requests
    cachesUnchanged = $true
} | ConvertTo-Json -Depth 5
