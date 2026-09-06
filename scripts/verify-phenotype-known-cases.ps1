param(
  [string]$FixtureRoot = '',
  [string]$TestBinary = '',
  [string]$PhenopacketRoot = '',
  [string]$MembershipOracle = '',
  [string]$MondoFixture = '',
  [string]$ReactomeFixture = '',
  [ValidateSet('manifest', 'challenges', 'full')]
  [string]$PatientValidation = 'manifest',
  [string]$ReportPath = '',
  [string]$GeneRanksPath = ''
)

$ErrorActionPreference = 'Stop'
$temporaryFixture = -not $FixtureRoot
$temporaryPhenopackets = -not $PhenopacketRoot
$temporaryOracle = -not $MembershipOracle
$temporaryMondo = -not $MondoFixture
$temporaryReactome = -not $ReactomeFixture
if (-not $temporaryFixture) {
  $FixtureRoot = (Resolve-Path -LiteralPath $FixtureRoot).Path
}
if (-not $temporaryPhenopackets) {
  $PhenopacketRoot = (Resolve-Path -LiteralPath $PhenopacketRoot).Path
}
if (-not $temporaryOracle) {
  $MembershipOracle = (Resolve-Path -LiteralPath $MembershipOracle).Path
}
if (-not $temporaryMondo) {
  $MondoFixture = (Resolve-Path -LiteralPath $MondoFixture).Path
}
if (-not $temporaryReactome) {
  $ReactomeFixture = (Resolve-Path -LiteralPath $ReactomeFixture).Path
}
if ($TestBinary) {
  $TestBinary = (Resolve-Path -LiteralPath $TestBinary).Path
}
$fixtureBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$validationId = [Guid]::NewGuid().ToString('N')
$manifest = Get-Content -LiteralPath 'config/hpo-assets.json' -Raw | ConvertFrom-Json
$mondoManifest = Get-Content -LiteralPath 'config/mondo-assets.json' -Raw | ConvertFrom-Json
$reactomeAsset = Get-Content -LiteralPath 'config/reactome-validation-assets.json' -Raw | ConvertFrom-Json
$validation = Get-Content -LiteralPath 'config/phenotype-validation-assets.json' -Raw | ConvertFrom-Json
$patientManifest = Get-Content -LiteralPath 'config/phenotype-patient-validation.json' -Raw | ConvertFrom-Json
$mondoAsset = $mondoManifest.assets | Where-Object kind -eq 'condition-ontology' | Select-Object -First 1
$oracleAsset = $validation.assets | Where-Object kind -eq 'hpo-membership-oracle' | Select-Object -First 1
if (-not $mondoAsset) {
  throw 'The MONDO validation manifest has no condition ontology'
}
if (-not $oracleAsset) {
  throw 'The phenotype validation manifest has no HPO membership oracle'
}
if ($oracleAsset.release -ne $manifest.release) {
  throw 'The HPO runtime and membership-oracle releases differ'
}
if ($temporaryFixture) {
  $resourcesRoot = Join-Path $fixtureBase "annocat-phenotype-validation-$validationId"
  $FixtureRoot = Join-Path (Join-Path $resourcesRoot 'hpo') $manifest.release
}
$raw = Join-Path $FixtureRoot 'raw'
if ($temporaryOracle) {
  $MembershipOracle = Join-Path $fixtureBase "annocat-hpo-oracle-$validationId.txt"
}
if ($temporaryMondo) {
  $MondoFixture = Join-Path $fixtureBase "annocat-mondo-$validationId.json"
}
if ($temporaryReactome) {
  $ReactomeFixture = Join-Path $fixtureBase "annocat-reactome-$validationId.zip"
}
if ($temporaryPhenopackets) {
  $PhenopacketRoot = Join-Path $fixtureBase "annocat-phenopacket-validation-$validationId"
}

function Confirm-Asset([string]$Path, $Asset) {
  if (-not (Test-Path -LiteralPath $Path)) {
    return $false
  }
  if ((Get-Item -LiteralPath $Path).Length -ne [long]$Asset.bytes) {
    return $false
  }
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -eq $Asset.sha256
}

function Get-VerifiedAsset([string]$Path, $Asset) {
  if (-not (Confirm-Asset $Path $Asset)) {
    Invoke-WebRequest -Uri $Asset.url -OutFile $Path
  }
  if (-not (Confirm-Asset $Path $Asset)) {
    throw "$($Asset.filename) failed size or SHA-256 verification"
  }
}

try {
  New-Item -ItemType Directory -Force -Path $raw | Out-Null
  foreach ($asset in $manifest.assets) {
    Get-VerifiedAsset (Join-Path $raw $asset.filename) $asset
  }
  Copy-Item -LiteralPath 'config/hpo-assets.json' -Destination (Join-Path $FixtureRoot 'hpo-assets.json') -Force
  $assetBytes = ($manifest.assets | Measure-Object -Property bytes -Sum).Sum
  $ready = [ordered]@{
    schemaVersion = 1
    release = $manifest.release
    installedAt = [DateTimeOffset]::UtcNow.ToString('o')
    assetBytes = [long]$assetBytes
    termCount = 0
    diseaseCount = 0
    diseaseGeneAssociationCount = 0
    hgncRelease = $manifest.hgncRelease
  }
  $ready | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $FixtureRoot 'hpo-ready.json') -Encoding utf8

  Get-VerifiedAsset $MembershipOracle $oracleAsset
  Get-VerifiedAsset $MondoFixture $mondoAsset
  Get-VerifiedAsset $ReactomeFixture $reactomeAsset
  $env:ANNOCAT_HPO_FIXTURE_ROOT = $FixtureRoot
  $env:ANNOCAT_HPO_MEMBERSHIP_ORACLE = $MembershipOracle
  $env:ANNOCAT_MONDO_FIXTURE = $MondoFixture
  $env:ANNOCAT_REACTOME_FIXTURE = $ReactomeFixture
  if ($temporaryPhenopackets) {
    git clone --quiet --depth 1 --branch $patientManifest.source.release --single-branch $patientManifest.source.repository $PhenopacketRoot
    if ($LASTEXITCODE -ne 0) {
      throw 'Could not clone the pinned Phenopacket Store release'
    }
  }
  $actualCommit = (git -C $PhenopacketRoot rev-parse HEAD).Trim()
  if ($LASTEXITCODE -ne 0 -or $actualCommit -ne $patientManifest.source.commit) {
    throw "Phenopacket Store commit mismatch: expected $($patientManifest.source.commit), found $actualCommit"
  }
  $env:ANNOCAT_PHENOPACKET_ROOT = $PhenopacketRoot
  if ($ReportPath) {
    $env:ANNOCAT_PHENOTYPE_VALIDATION_REPORT = $ReportPath
  } else {
    Remove-Item Env:ANNOCAT_PHENOTYPE_VALIDATION_REPORT -ErrorAction SilentlyContinue
  }
  if ($GeneRanksPath) {
    $env:ANNOCAT_VALIDATION_GENE_RANKS = $GeneRanksPath
  } else {
    Remove-Item Env:ANNOCAT_VALIDATION_GENE_RANKS -ErrorAction SilentlyContinue
  }
  if ($TestBinary) {
    & $TestBinary 'official_hpo_' --ignored --nocapture --test-threads=1
  } else {
    cargo test -p annocat-cli --bin annocat --locked official_hpo_ -- --ignored --nocapture --test-threads=1
  }
  if ($LASTEXITCODE -ne 0) {
    throw 'Phenotype source and known-case validation failed'
  }
  if ($TestBinary) {
    & $TestBinary 'official_mondo_' --ignored --nocapture --test-threads=1
  } else {
    cargo test -p annocat-cli --bin annocat --locked official_mondo_ -- --ignored --nocapture --test-threads=1
  }
  if ($LASTEXITCODE -ne 0) {
    throw 'MONDO graph and condition-gene validation failed'
  }
  if ($TestBinary) {
    & $TestBinary 'official_reactome_' --ignored --nocapture --test-threads=1
  } else {
    cargo test -p annocat-cli --bin annocat --locked official_reactome_ -- --ignored --nocapture --test-threads=1
  }
  if ($LASTEXITCODE -ne 0) {
    throw 'Reactome pathway-gene validation failed'
  }
  $testName = if ($PatientValidation -eq 'full') {
    'phenotype::patient_validation::phenopacket_store_public_cohort_report'
  } elseif ($PatientValidation -eq 'challenges') {
    'phenotype::patient_validation::phenopacket_store_frozen_challenge_case_report'
  } else {
    'phenotype::patient_validation::phenopacket_store_snapshot_and_manifest_are_valid'
  }
  if ($TestBinary) {
    & $TestBinary $testName --ignored --exact --nocapture --test-threads=1
  } else {
    cargo test -p annocat-cli --bin annocat --locked $testName -- --ignored --exact --nocapture --test-threads=1
  }
  if ($LASTEXITCODE -ne 0) {
    throw "Phenopacket Store $PatientValidation validation failed"
  }
} finally {
  if ($temporaryOracle) {
    Remove-Item -LiteralPath $MembershipOracle -Force -ErrorAction SilentlyContinue
  }
  if ($temporaryMondo) {
    Remove-Item -LiteralPath $MondoFixture -Force -ErrorAction SilentlyContinue
  }
  if ($temporaryReactome) {
    Remove-Item -LiteralPath $ReactomeFixture -Force -ErrorAction SilentlyContinue
  }
  if ($temporaryFixture -and (Test-Path -LiteralPath $resourcesRoot)) {
    Remove-Item -LiteralPath $resourcesRoot -Recurse -Force
  }
  if ($temporaryPhenopackets -and (Test-Path -LiteralPath $PhenopacketRoot)) {
    Remove-Item -LiteralPath $PhenopacketRoot -Recurse -Force
  }
}
