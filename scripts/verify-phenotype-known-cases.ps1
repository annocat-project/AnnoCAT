param(
  [string]$FixtureRoot = ''
)

$ErrorActionPreference = 'Stop'
if (-not $FixtureRoot) {
  $fixtureBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
  $FixtureRoot = Join-Path $fixtureBase 'annocat-hpo-fixture'
}
$manifest = Get-Content -LiteralPath 'config/hpo-assets.json' -Raw | ConvertFrom-Json
$raw = Join-Path $FixtureRoot 'raw'
New-Item -ItemType Directory -Force -Path $raw | Out-Null

foreach ($asset in $manifest.assets) {
  if ($asset.kind -notin @('ontology', 'disease-annotations', 'disease-genes')) {
    continue
  }
  $destination = Join-Path $raw $asset.filename
  $valid = Test-Path -LiteralPath $destination
  if ($valid) {
    $valid = (Get-Item -LiteralPath $destination).Length -eq [long]$asset.bytes
  }
  if ($valid) {
    $valid = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -eq $asset.sha256
  }
  if (-not $valid) {
    Invoke-WebRequest -Uri $asset.url -OutFile $destination
  }
  if ((Get-Item -LiteralPath $destination).Length -ne [long]$asset.bytes) {
    throw "$($asset.filename) has an unexpected size"
  }
  if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -ne $asset.sha256) {
    throw "$($asset.filename) has an unexpected SHA-256"
  }
}

$env:ANNOCAT_HPO_FIXTURE_ROOT = $FixtureRoot
cargo test -p annocat-cli --bin annocat --locked official_hpo_known_cases_rank_within_top_twenty -- --ignored --nocapture --test-threads=1
if ($LASTEXITCODE -ne 0) {
  throw 'Phenotype known-case validation failed'
}
