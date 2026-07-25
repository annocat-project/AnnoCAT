use duckdb::arrow::array::{ArrayRef, BooleanArray, Int32Array, StringArray};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::{Connection, params};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROFILE_SCHEMA_VERSION: u16 = 3;
const INSTALL_SCHEMA_VERSION: u16 = 1;
const REPORT_OVERLAP_CACHE_SCHEMA_VERSION: u16 = 6;
const CARRIED_ALLELE_SCHEMA_VERSION: i32 = 1;
const PHENOTYPIC_ABNORMALITY_ROOT: &str = "HP:0000118";
const MAX_PROFILE_TERMS: usize = 500;
const READY_FILENAME: &str = "hpo-ready.json";
const INSTALLED_ASSET_MANIFEST_FILENAME: &str = "hpo-assets.json";
const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeTerm {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeProfile {
    pub schema_version: u16,
    pub run_id: String,
    pub updated_at: String,
    pub observed: Vec<PhenotypeTerm>,
    pub excluded: Vec<PhenotypeTerm>,
    pub ranking: Option<PhenotypeRanking>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeRanking {
    #[serde(default = "default_phenotype_algorithm_version")]
    pub algorithm_version: String,
    pub provider: String,
    pub provider_url: String,
    pub hpo_release: String,
    pub metric: String,
    #[serde(default)]
    pub score_interpretation: String,
    pub generated_at: String,
    pub evaluated_diseases: usize,
    pub sample_name: Option<String>,
    pub report_gene_count: usize,
    pub online_enrichment: Option<MonarchGeneRanking>,
    pub online_error: Option<String>,
    pub diseases: Vec<RankedDisease>,
}

fn default_phenotype_algorithm_version() -> String {
    "hpo-lin-query-v3".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonarchGeneRanking {
    pub provider: String,
    pub provider_url: String,
    pub metric: String,
    #[serde(default)]
    pub result_limit: usize,
    pub genes: Vec<MonarchRankedGene>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonarchRankedGene {
    pub rank: usize,
    pub gene_id: String,
    pub symbol: String,
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankedDisease {
    pub phenotype_rank: usize,
    pub disease_id: String,
    pub disease_name: String,
    pub phenotype_score: f64,
    pub query_coverage: f64,
    pub conflict_score: f64,
    #[serde(default)]
    pub conflict_frequency_complete: bool,
    pub matched_phenotypes: Vec<PhenotypeMatch>,
    pub genes: Vec<GeneAssociation>,
    pub report_overlap: DiseaseReportOverlap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeMatch {
    pub query: PhenotypeTerm,
    pub disease_term: PhenotypeTerm,
    pub similarity: f64,
    pub direct: bool,
    #[serde(default)]
    pub disease_annotation: Option<DiseasePhenotypeContext>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiseasePhenotypeContext {
    pub frequency_probability: Option<f64>,
    pub frequency_label: Option<String>,
    pub frequency_raw: Option<String>,
    pub onset: Vec<String>,
    pub sex: Vec<String>,
    pub evidence: Vec<String>,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneAssociation {
    pub gene_id: String,
    pub symbol: String,
    pub association_type: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiseaseReportOverlap {
    pub has_overlap: bool,
    pub tier: u8,
    pub label: String,
    pub genes: Vec<ReportGeneOverlap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportGeneOverlap {
    pub symbol: String,
    pub variant_count: u64,
    pub pass_count: u64,
    pub high_impact_count: u64,
    pub moderate_impact_count: u64,
    pub tier: u8,
    pub tier_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPhenotypeExploration {
    pub generated_at: String,
    pub hpo_release: String,
    pub sample_name: String,
    pub report_gene_count: usize,
    pub associated_diseases: usize,
    pub diseases: Vec<ReportAssociatedDisease>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAssociatedDisease {
    pub order: usize,
    pub disease_id: String,
    pub disease_name: String,
    pub phenotypes: Vec<PhenotypeTerm>,
    pub genes: Vec<GeneAssociation>,
    pub report_overlap: DiseaseReportOverlap,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankRequest {
    pub observed: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub excluded: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub online_consent: bool,
    #[serde(default)]
    pub sample_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExploreRequest {
    #[serde(default)]
    pub sample_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileUpdate {
    pub action: String,
    #[serde(default)]
    pub observed: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub excluded: Vec<PhenotypeTerm>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermSearchResult {
    pub id: String,
    pub label: String,
    pub synonyms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InstallProgress {
    pub phase: String,
    pub detail: String,
    pub network_bytes: u64,
    pub expected_network_bytes: u64,
    pub parsed_records: u64,
    pub prepared_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HpoReadyManifest {
    pub schema_version: u16,
    pub release: String,
    pub installed_at: String,
    pub asset_bytes: u64,
    pub term_count: usize,
    pub disease_count: usize,
    pub disease_gene_association_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HpoAssetManifest {
    schema_version: u16,
    release: String,
    release_url: String,
    assets: Vec<HpoAsset>,
}

impl HpoAssetManifest {
    pub(crate) fn release(&self) -> &str {
        &self.release
    }

    pub(crate) fn expected_bytes(&self) -> u64 {
        self.assets.iter().map(|asset| asset.bytes).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HpoAsset {
    kind: String,
    filename: String,
    url: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug)]
struct RawTerm {
    id: String,
    label: String,
    synonyms: Vec<String>,
    parent_ids: Vec<String>,
    alt_ids: Vec<String>,
    obsolete: bool,
    replacement: Option<String>,
}

type ParsedOntology = (Vec<OntologyTerm>, HashMap<String, usize>, Vec<usize>);

#[derive(Debug)]
struct OntologyTerm {
    id: String,
    label: String,
    synonyms: Vec<String>,
    search_text: String,
    parents: Vec<usize>,
    ancestors: Vec<usize>,
    obsolete: bool,
    replacement: Option<usize>,
}

#[derive(Debug)]
struct DiseaseProfile {
    id: String,
    name: String,
    positive: Vec<usize>,
    negative: Vec<usize>,
    annotations: HashMap<usize, DiseasePhenotypeContext>,
    genes: Vec<GeneAssociation>,
}

#[derive(Debug)]
struct DiseaseBuilder {
    id: String,
    name: String,
    positive: Vec<usize>,
    negative: Vec<usize>,
    annotations: HashMap<usize, DiseasePhenotypeContext>,
    genes: Vec<GeneAssociation>,
}

#[derive(Debug)]
struct HpoKnowledge {
    terms: Vec<OntologyTerm>,
    term_index: HashMap<String, usize>,
    active_terms: Vec<usize>,
    phenotypic_abnormality_root: usize,
    diseases: Vec<DiseaseProfile>,
    information_content: Vec<f64>,
    disease_gene_association_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetIntegrityStamp {
    filename: String,
    bytes: u64,
    modified_nanos: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportOverlapCache {
    schema_version: u16,
    parquet_bytes: u64,
    parquet_modified_nanos: u128,
    sample_name: String,
    genes: Vec<ReportGeneOverlap>,
}

#[derive(Debug, Deserialize)]
struct RawSampleCall {
    name: String,
    value: String,
}

fn knowledge_cache() -> &'static Mutex<HashMap<PathBuf, Arc<HpoKnowledge>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<HpoKnowledge>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn integrity_cache() -> &'static Mutex<HashMap<PathBuf, Vec<AssetIntegrityStamp>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Vec<AssetIntegrityStamp>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn hpo_release(resources: &Path) -> Result<String, String> {
    Ok(installed_asset_manifest(resources)?.release)
}

pub fn release_root(resources: &Path) -> Result<PathBuf, String> {
    installed_release(resources)
        .map(|(root, _, _)| root)
        .ok_or_else(|| {
            "Human Phenotype Ontology data is not installed. Install it from Data sources first."
                .into()
        })
}

pub fn installed_versions(resources: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(resources.join("hpo")) else {
        return Vec::new();
    };
    let mut versions = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            installed_status_and_manifest_at(&entry.path()).map(|(_, manifest)| manifest.release)
        })
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    versions
}

pub fn installed_status(resources: &Path) -> Option<HpoReadyManifest> {
    installed_release(resources).map(|(_, ready, _)| ready)
}

fn installed_release(resources: &Path) -> Option<(PathBuf, HpoReadyManifest, HpoAssetManifest)> {
    fs::read_dir(resources.join("hpo"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            installed_status_and_manifest_at(&entry.path())
                .map(|(ready, manifest)| (entry.path(), ready, manifest))
        })
        .max_by(|left, right| left.1.release.cmp(&right.1.release))
}

fn installed_status_at(root: &Path) -> Option<HpoReadyManifest> {
    installed_status_and_manifest_at(root).map(|(ready, _)| ready)
}

fn installed_status_and_manifest_at(root: &Path) -> Option<(HpoReadyManifest, HpoAssetManifest)> {
    let bytes = fs::read(root.join(READY_FILENAME)).ok()?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return None;
    }
    let ready: HpoReadyManifest = serde_json::from_slice(&bytes).ok()?;
    let manifest = asset_manifest_at(root).ok()?;
    if ready.schema_version != INSTALL_SCHEMA_VERSION
        || ready.release != manifest.release
        || ready.asset_bytes != manifest.expected_bytes()
    {
        return None;
    }
    verified_installation(root, &manifest).then_some((ready, manifest))
}

fn installed_asset_manifest(resources: &Path) -> Result<HpoAssetManifest, String> {
    installed_release(resources)
        .map(|(_, _, manifest)| manifest)
        .ok_or_else(|| {
            "Human Phenotype Ontology data is not installed. Install it from Data sources first."
                .into()
        })
}

fn asset_manifest_at(root: &Path) -> Result<HpoAssetManifest, String> {
    let path = root.join(INSTALLED_ASSET_MANIFEST_FILENAME);
    let manifest = if path.is_file() {
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read installed HPO asset manifest: {error}"))?;
        if bytes.is_empty() || bytes.len() > 128 * 1024 {
            return Err("installed HPO asset manifest exceeds its safety limit".into());
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid installed HPO asset manifest: {error}"))?
    } else {
        embedded_asset_manifest()?
    };
    validate_asset_manifest(&manifest)?;
    if root.file_name().and_then(|value| value.to_str()) != Some(manifest.release.as_str()) {
        return Err("installed HPO directory does not match its release manifest".into());
    }
    Ok(manifest)
}

fn verified_installation(root: &Path, manifest: &HpoAssetManifest) -> bool {
    let mut stamps = Vec::with_capacity(manifest.assets.len());
    for asset in &manifest.assets {
        let path = root.join("raw").join(&asset.filename);
        let Ok(metadata) = fs::metadata(&path) else {
            return false;
        };
        if !metadata.is_file() || metadata.len() != asset.bytes {
            return false;
        }
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        stamps.push(AssetIntegrityStamp {
            filename: asset.filename.clone(),
            bytes: metadata.len(),
            modified_nanos,
        });
    }
    if integrity_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .is_some_and(|cached| cached == &stamps)
    {
        return true;
    }
    if manifest
        .assets
        .iter()
        .any(|asset| verify_sha256(&root.join("raw").join(&asset.filename), &asset.sha256).is_err())
    {
        return false;
    }
    integrity_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(root.to_path_buf(), stamps);
    true
}

pub fn install_hpo(
    resource_root: &Path,
    manifest: &HpoAssetManifest,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(InstallProgress),
) -> Result<HpoReadyManifest, String> {
    validate_asset_manifest(manifest)?;
    if resource_root.file_name().and_then(|value| value.to_str()) != Some(manifest.release.as_str())
    {
        return Err("HPO installation directory does not match the resolved release".into());
    }
    let expected = manifest.expected_bytes();
    let raw_root = resource_root.join("raw");
    fs::create_dir_all(&raw_root)
        .map_err(|error| format!("cannot create the HPO resource directory: {error}"))?;
    let mut completed_bytes = 0_u64;
    for asset in &manifest.assets {
        ensure_not_cancelled(cancelled, "HPO installation")?;
        let final_path = raw_root.join(&asset.filename);
        if verified_asset(&final_path, asset)? {
            completed_bytes = completed_bytes.saturating_add(asset.bytes);
            progress(InstallProgress {
                phase: "downloading".into(),
                detail: format!("Reusing verified {}", asset.filename),
                network_bytes: completed_bytes,
                expected_network_bytes: expected,
                parsed_records: 0,
                prepared_bytes: completed_bytes,
            });
            continue;
        }
        download_asset(
            asset,
            &final_path,
            completed_bytes,
            expected,
            cancelled,
            &mut progress,
        )?;
        completed_bytes = completed_bytes.saturating_add(asset.bytes);
    }

    progress(InstallProgress {
        phase: "validating".into(),
        detail: "Validating the local HPO datasets".into(),
        network_bytes: expected,
        expected_network_bytes: expected,
        parsed_records: 0,
        prepared_bytes: expected,
    });
    ensure_not_cancelled(cancelled, "HPO installation")?;
    let knowledge = Arc::new(load_knowledge_from_root(resource_root)?);
    let ready = HpoReadyManifest {
        schema_version: INSTALL_SCHEMA_VERSION,
        release: manifest.release.clone(),
        installed_at: super::annotation::current_timestamp(),
        asset_bytes: expected,
        term_count: knowledge.active_terms.len(),
        disease_count: knowledge.diseases.len(),
        disease_gene_association_count: knowledge.disease_gene_association_count,
    };
    let bytes = serde_json::to_vec_pretty(&ready)
        .map_err(|error| format!("cannot serialize the HPO ready marker: {error}"))?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("cannot serialize the installed HPO asset manifest: {error}"))?;
    super::library_metadata::atomic_write(
        &resource_root.join(INSTALLED_ASSET_MANIFEST_FILENAME),
        &manifest_bytes,
    )?;
    super::library_metadata::atomic_write(&resource_root.join(READY_FILENAME), &bytes)?;
    knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(resource_root.to_path_buf(), knowledge);
    progress(InstallProgress {
        phase: "ready".into(),
        detail: format!(
            "Validated {} phenotype terms and {} disease profiles",
            ready.term_count, ready.disease_count
        ),
        network_bytes: expected,
        expected_network_bytes: expected,
        parsed_records: ready.disease_count as u64,
        prepared_bytes: directory_size(resource_root),
    });
    Ok(ready)
}

pub fn search_terms(
    resources: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<TermSearchResult>, String> {
    let query = normalize_search(query);
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let knowledge = knowledge(resources)?;
    let mut matches = knowledge
        .active_terms
        .iter()
        .filter_map(|&index| {
            let term = &knowledge.terms[index];
            let id = term.id.to_ascii_lowercase();
            let label = term.label.to_ascii_lowercase();
            let score = if id == query {
                0
            } else if label == query {
                1
            } else if label.starts_with(&query) {
                2
            } else if term
                .synonyms
                .iter()
                .any(|synonym| synonym.eq_ignore_ascii_case(&query))
            {
                3
            } else if label.contains(&query) {
                4
            } else if term.search_text.contains(&query) {
                5
            } else {
                return None;
            };
            Some((
                score,
                term.label.len(),
                TermSearchResult {
                    id: term.id.clone(),
                    label: term.label.clone(),
                    synonyms: term
                        .synonyms
                        .iter()
                        .filter(|synonym| synonym.to_ascii_lowercase().contains(&query))
                        .take(3)
                        .cloned()
                        .collect(),
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.label.cmp(&right.2.label))
    });
    Ok(matches
        .into_iter()
        .take(limit.clamp(1, 100))
        .map(|(_, _, result)| result)
        .collect())
}

pub fn empty_profile(run_id: &str) -> PhenotypeProfile {
    PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: String::new(),
        observed: Vec::new(),
        excluded: Vec::new(),
        ranking: None,
    }
}

pub fn load(runs: &Path, run_id: &str) -> Result<PhenotypeProfile, String> {
    super::library_metadata::validate_run_id(run_id)?;
    let path = profile_path(runs, run_id);
    if !path.exists() {
        return Ok(empty_profile(run_id));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read phenotype profile: {error}"))?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("phenotype profile exceeds its 64 MB safety limit".into());
    }
    let mut profile: PhenotypeProfile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    validate_profile(&profile, run_id)?;
    if profile
        .ranking
        .as_ref()
        .is_some_and(|ranking| ranking.algorithm_version != default_phenotype_algorithm_version())
    {
        profile.ranking = None;
    }
    Ok(profile)
}

pub fn report_sample_names(parquet: &Path) -> Result<Vec<String>, String> {
    let connection = Connection::open_in_memory()
        .map_err(|error| format!("cannot inspect report samples: {error}"))?;
    let mut statement = connection
        .prepare("SELECT sample_names_json FROM read_parquet(?) LIMIT 1")
        .map_err(|error| format!("cannot prepare report sample lookup: {error}"))?;
    let mut rows = statement
        .query(params![parquet.to_string_lossy().as_ref()])
        .map_err(|error| format!("cannot read report samples: {error}"))?;
    let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot read report samples: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let raw: Option<String> = row.get(0).map_err(|error| error.to_string())?;
    let names = match raw {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str::<Vec<String>>(&raw)
            .map_err(|error| format!("invalid report sample metadata: {error}"))?,
        _ => Vec::new(),
    };
    let mut seen = HashSet::new();
    for name in &names {
        if name.trim().is_empty()
            || name.chars().any(char::is_control)
            || !seen.insert(name.clone())
        {
            return Err("report sample metadata contains an invalid or duplicate name".into());
        }
    }
    Ok(names)
}

pub fn profile_json(
    resources: &Path,
    profile: &PhenotypeProfile,
    parquet: &Path,
) -> Result<String, String> {
    let manifest = installed_asset_manifest(resources)?;
    let mut value = serde_json::to_value(profile)
        .map_err(|error| format!("cannot serialize phenotype profile: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or("phenotype profile did not serialize as an object")?;
    object.insert(
        "hpoRelease".into(),
        serde_json::Value::String(manifest.release),
    );
    object.insert(
        "hpoReleaseUrl".into(),
        serde_json::Value::String(manifest.release_url),
    );
    object.insert(
        "sampleNames".into(),
        serde_json::to_value(report_sample_names(parquet)?)
            .map_err(|error| format!("cannot serialize report samples: {error}"))?,
    );
    serde_json::to_string(&value).map_err(|error| error.to_string())
}

fn resolve_sample_name(parquet: &Path, requested: Option<&str>) -> Result<Option<String>, String> {
    let names = report_sample_names(parquet)?;
    if let Some(requested) = requested.filter(|name| !name.is_empty()) {
        return names
            .iter()
            .find(|name| name.as_str() == requested)
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("sample {requested} is not present in this report"));
    }
    Ok(match names.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    })
}

pub fn update(
    resources: &Path,
    runs: &Path,
    run_id: &str,
    request: ProfileUpdate,
) -> Result<PhenotypeProfile, String> {
    match request.action.as_str() {
        "save" => {
            let knowledge = knowledge(resources)?;
            let observed = normalize_terms(
                &knowledge,
                canonical_terms(&knowledge, &request.observed, true)?,
                true,
            )?;
            let excluded = normalize_terms(
                &knowledge,
                canonical_terms(&knowledge, &request.excluded, true)?,
                false,
            )?;
            ensure_consistent_profile(&knowledge, &observed, &excluded)?;
            let existing = load(runs, run_id)?;
            let same_profile = existing.observed == observed && existing.excluded == excluded;
            let profile = PhenotypeProfile {
                schema_version: PROFILE_SCHEMA_VERSION,
                run_id: run_id.to_owned(),
                updated_at: super::annotation::current_timestamp(),
                observed,
                excluded,
                ranking: same_profile.then_some(existing.ranking).flatten(),
            };
            save(runs, &profile)?;
            Ok(profile)
        }
        "clear" => {
            let path = profile_path(runs, run_id);
            if path.exists() {
                fs::remove_file(path)
                    .map_err(|error| format!("cannot clear phenotype profile: {error}"))?;
            }
            Ok(empty_profile(run_id))
        }
        _ => Err("phenotype action must be save or clear".into()),
    }
}

pub fn rank(
    resources: &Path,
    runs: &Path,
    run_id: &str,
    parquet: &Path,
    consequences: Option<&Path>,
    request: RankRequest,
) -> Result<PhenotypeProfile, String> {
    super::library_metadata::validate_run_id(run_id)?;
    let knowledge = knowledge(resources)?;
    let observed = normalize_terms(
        &knowledge,
        canonical_terms(&knowledge, &request.observed, false)?,
        true,
    )?;
    let excluded = normalize_terms(
        &knowledge,
        canonical_terms(&knowledge, &request.excluded, true)?,
        false,
    )?;
    ensure_consistent_profile(&knowledge, &observed, &excluded)?;
    let observed_indexes = term_indexes(&knowledge, &observed)?;
    let excluded_indexes = term_indexes(&knowledge, &excluded)?;
    let sample_name = resolve_sample_name(parquet, request.sample_name.as_deref())?;
    let report_overlaps = match &sample_name {
        Some(sample_name) => {
            report_gene_overlap_summary(runs, run_id, parquet, consequences, sample_name)?
        }
        None => Vec::new(),
    };
    let report_overlap_by_gene = candidate_report_overlap_by_gene(&report_overlaps);

    let mut diseases = knowledge
        .diseases
        .par_iter()
        .map(|disease| {
            rank_disease(
                &knowledge,
                disease,
                &observed_indexes,
                &excluded_indexes,
                &report_overlap_by_gene,
            )
        })
        .collect::<Vec<_>>();
    diseases.sort_by(|left, right| {
        right
            .phenotype_score
            .total_cmp(&left.phenotype_score)
            .then_with(|| right.query_coverage.total_cmp(&left.query_coverage))
            .then_with(|| left.disease_id.cmp(&right.disease_id))
    });
    for (index, disease) in diseases.iter_mut().enumerate() {
        disease.phenotype_rank = index + 1;
        disease.phenotype_score = round(disease.phenotype_score, 2);
        disease.query_coverage = round(disease.query_coverage, 1);
        disease.conflict_score = round(disease.conflict_score, 1);
    }
    let (online_enrichment, online_error) = if request.online_consent {
        match monarch_gene_ranking(&observed) {
            Ok(ranking) => (Some(ranking), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let manifest = installed_asset_manifest(resources)?;
    let ranking = PhenotypeRanking {
        algorithm_version: default_phenotype_algorithm_version(),
        provider: "Human Phenotype Ontology".into(),
        provider_url: manifest.release_url,
        hpo_release: manifest.release,
        metric: "Patient-to-disease best-match Lin semantic similarity; explicit-absence conflict is weighted by disease-feature frequency when HPO reports it".into(),
        score_interpretation: "A relative match of the patient's recorded findings to each disease profile. Unrecorded disease findings are treated as unknown, not absent. This is not a diagnosis or disease probability; variant overlap is displayed separately and does not change the phenotype score.".into(),
        generated_at: super::annotation::current_timestamp(),
        evaluated_diseases: diseases.len(),
        sample_name,
        report_gene_count: report_overlap_by_gene.len(),
        online_enrichment,
        online_error,
        diseases,
    };
    let profile = PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: super::annotation::current_timestamp(),
        observed,
        excluded,
        ranking: Some(ranking),
    };
    save(runs, &profile)?;
    Ok(profile)
}

pub fn explore_report(
    resources: &Path,
    runs: &Path,
    run_id: &str,
    parquet: &Path,
    consequences: Option<&Path>,
    request: ExploreRequest,
) -> Result<ReportPhenotypeExploration, String> {
    super::library_metadata::validate_run_id(run_id)?;
    let knowledge = knowledge(resources)?;
    let sample_name = resolve_sample_name(parquet, request.sample_name.as_deref())?
        .ok_or("choose the patient sample before exploring carried-ALT report overlap")?;
    let report_overlaps =
        report_gene_overlap_summary(runs, run_id, parquet, consequences, &sample_name)?;
    let report_overlap_by_gene = candidate_report_overlap_by_gene(&report_overlaps);
    let mut diseases = knowledge
        .diseases
        .iter()
        .filter_map(|disease| {
            let report_overlap = disease_report_overlap(disease, &report_overlap_by_gene);
            if !report_overlap.has_overlap {
                return None;
            }
            let mut phenotypes = disease
                .positive
                .iter()
                .map(|&term| {
                    (
                        knowledge.information_content[term],
                        term_value(&knowledge, term),
                    )
                })
                .collect::<Vec<_>>();
            phenotypes.sort_by(|left, right| right.0.total_cmp(&left.0));
            phenotypes.dedup_by(|left, right| left.1.id == right.1.id);
            Some(ReportAssociatedDisease {
                order: 0,
                disease_id: disease.id.clone(),
                disease_name: disease.name.clone(),
                phenotypes: phenotypes
                    .into_iter()
                    .take(8)
                    .map(|(_, term)| term)
                    .collect(),
                genes: disease.genes.clone(),
                report_overlap,
            })
        })
        .collect::<Vec<_>>();
    diseases.sort_by(|left, right| {
        right
            .report_overlap
            .tier
            .cmp(&left.report_overlap.tier)
            .then_with(|| {
                let left_count = left
                    .report_overlap
                    .genes
                    .iter()
                    .map(|gene| gene.variant_count)
                    .sum::<u64>();
                let right_count = right
                    .report_overlap
                    .genes
                    .iter()
                    .map(|gene| gene.variant_count)
                    .sum::<u64>();
                right_count.cmp(&left_count)
            })
            .then_with(|| left.disease_name.cmp(&right.disease_name))
    });
    for (index, disease) in diseases.iter_mut().enumerate() {
        disease.order = index + 1;
    }
    let report_gene_count = diseases
        .iter()
        .flat_map(|disease| disease.report_overlap.genes.iter())
        .map(|gene| gene.symbol.to_ascii_uppercase())
        .collect::<HashSet<_>>()
        .len();
    let manifest = installed_asset_manifest(resources)?;
    Ok(ReportPhenotypeExploration {
        generated_at: super::annotation::current_timestamp(),
        hpo_release: manifest.release,
        sample_name,
        report_gene_count,
        associated_diseases: diseases.len(),
        diseases,
    })
}

fn monarch_gene_ranking(observed: &[PhenotypeTerm]) -> Result<MonarchGeneRanking, String> {
    let service = annocat_core::source_catalog::service("monarch-phenotype-gene-ranking")
        .ok_or("Monarch phenotype-ranking service is not configured")?;
    let payload = monarch_request_payload(observed, service.max_results);
    let payload = serde_json::to_vec(&payload)
        .map_err(|error| format!("cannot serialize Monarch gene-ranking request: {error}"))?;
    let response = super::http_client::source()?
        .post(&service.api_url)
        .timeout(Duration::from_secs(service.timeout_seconds))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Monarch gene ranking was unavailable: {error}"))?
        .bytes()
        .map_err(|error| format!("cannot read Monarch gene-ranking data: {error}"))?;
    let genes = parse_monarch_genes(&response)?;
    Ok(MonarchGeneRanking {
        provider: service.provider.clone(),
        provider_url: service.provider_url.clone(),
        metric: "Ancestor information content, bidirectional".into(),
        result_limit: service.max_results,
        genes,
    })
}

fn monarch_request_payload(observed: &[PhenotypeTerm], limit: usize) -> serde_json::Value {
    serde_json::json!({
        "termset": observed.iter().map(|term| term.id.as_str()).collect::<Vec<_>>(),
        "group": "Human Genes",
        "metric": "ancestor_information_content",
        "directionality": "bidirectional",
        "limit": limit
    })
}

fn parse_monarch_genes(response: &[u8]) -> Result<Vec<MonarchRankedGene>, String> {
    let response: serde_json::Value = serde_json::from_slice(&response)
        .map_err(|error| format!("Monarch returned invalid gene-ranking data: {error}"))?;
    let rows = response
        .as_array()
        .or_else(|| response.get("items").and_then(serde_json::Value::as_array))
        .ok_or("Monarch gene ranking did not contain a result list")?;
    let genes = rows
        .iter()
        .filter_map(|row| {
            let subject = row.get("subject")?.as_object()?;
            let gene_id = subject.get("id")?.as_str()?.trim();
            let symbol = subject
                .get("symbol")
                .and_then(serde_json::Value::as_str)
                .or_else(|| subject.get("name").and_then(serde_json::Value::as_str))?
                .trim();
            let score = row.get("score")?.as_f64()?;
            if gene_id.is_empty() || symbol.is_empty() || !score.is_finite() {
                return None;
            }
            Some(MonarchRankedGene {
                rank: 0,
                gene_id: gene_id.to_owned(),
                symbol: symbol.to_owned(),
                name: subject
                    .get("full_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(symbol)
                    .trim()
                    .to_owned(),
                score,
            })
        })
        .enumerate()
        .map(|(index, mut gene)| {
            gene.rank = index + 1;
            gene
        })
        .collect::<Vec<_>>();
    if genes.is_empty() {
        return Err("Monarch gene ranking did not contain usable human-gene results".into());
    }
    Ok(genes)
}

fn rank_disease(
    knowledge: &HpoKnowledge,
    disease: &DiseaseProfile,
    observed: &[usize],
    excluded: &[usize],
    report_overlap_by_gene: &HashMap<String, ReportGeneOverlap>,
) -> RankedDisease {
    let observed_best = observed
        .iter()
        .map(|&query| {
            disease
                .positive
                .iter()
                .map(|&term| (term, term_similarity(knowledge, query, term)))
                .max_by(|left, right| left.1.total_cmp(&right.1))
                .unwrap_or((query, 0.0))
        })
        .collect::<Vec<_>>();
    let query_average = mean(observed_best.iter().map(|(_, score)| *score));
    let absent_matches = excluded
        .iter()
        .filter_map(|&absent| {
            disease
                .positive
                .iter()
                .map(|&term| {
                    (
                        term_similarity(knowledge, absent, term),
                        disease
                            .annotations
                            .get(&term)
                            .and_then(|annotation| annotation.frequency_probability),
                    )
                })
                .max_by(|left, right| left.0.total_cmp(&right.0))
        })
        .collect::<Vec<_>>();
    let absent_conflict = mean(
        absent_matches
            .iter()
            .map(|(similarity, frequency)| similarity * frequency.unwrap_or(1.0)),
    );
    let conflict_frequency_complete = absent_matches
        .iter()
        .all(|(_, frequency)| frequency.is_some());
    let disease_negative_conflict = mean(disease.negative.iter().map(|&negative| {
        observed
            .iter()
            .map(|&query| term_similarity(knowledge, query, negative))
            .fold(0.0, f64::max)
    }));
    let conflict_score = match (excluded.is_empty(), disease.negative.is_empty()) {
        (true, true) => 0.0,
        (false, true) => absent_conflict,
        (true, false) => disease_negative_conflict,
        (false, false) => (absent_conflict + disease_negative_conflict) / 2.0,
    };
    let score = query_average.clamp(0.0, 1.0) * 100.0;
    let direct_match_percentage = if observed.is_empty() {
        0.0
    } else {
        observed
            .iter()
            .zip(&observed_best)
            .filter(|(query, (matched, _))| **query == *matched)
            .count() as f64
            / observed.len() as f64
            * 100.0
    };
    let mut matched_phenotypes = observed
        .iter()
        .zip(observed_best)
        .map(|(&query, (matched, similarity))| PhenotypeMatch {
            query: term_value(knowledge, query),
            disease_term: term_value(knowledge, matched),
            similarity,
            direct: query == matched,
            disease_annotation: disease.annotations.get(&matched).cloned(),
        })
        .collect::<Vec<_>>();
    matched_phenotypes.sort_by(|left, right| right.similarity.total_cmp(&left.similarity));
    matched_phenotypes.truncate(5);
    let report_overlap = disease_report_overlap(disease, report_overlap_by_gene);
    RankedDisease {
        phenotype_rank: 0,
        disease_id: disease.id.clone(),
        disease_name: disease.name.clone(),
        phenotype_score: score,
        query_coverage: direct_match_percentage,
        conflict_score: conflict_score * 100.0,
        conflict_frequency_complete,
        matched_phenotypes,
        genes: disease.genes.clone(),
        report_overlap,
    }
}

fn disease_report_overlap(
    disease: &DiseaseProfile,
    report_overlap_by_gene: &HashMap<String, ReportGeneOverlap>,
) -> DiseaseReportOverlap {
    let mut overlapping_genes = disease
        .genes
        .iter()
        .filter(|gene| gene.association_type.eq_ignore_ascii_case("MENDELIAN"))
        .filter_map(|gene| {
            report_overlap_by_gene
                .get(&gene.symbol.to_ascii_uppercase())
                .cloned()
        })
        .collect::<Vec<_>>();
    overlapping_genes.sort_by(|left, right| {
        right
            .tier
            .cmp(&left.tier)
            .then(right.variant_count.cmp(&left.variant_count))
            .then(left.symbol.cmp(&right.symbol))
    });
    overlapping_genes.dedup_by(|left, right| left.symbol.eq_ignore_ascii_case(&right.symbol));
    let tier = overlapping_genes.first().map(|gene| gene.tier).unwrap_or(0);
    DiseaseReportOverlap {
        has_overlap: !overlapping_genes.is_empty(),
        tier,
        label: report_overlap_label(tier).into(),
        genes: overlapping_genes,
    }
}

fn candidate_report_overlap_by_gene(
    report_overlaps: &[ReportGeneOverlap],
) -> HashMap<String, ReportGeneOverlap> {
    report_overlaps
        .iter()
        .filter(|gene| gene.tier >= 3)
        .cloned()
        .map(|gene| (gene.symbol.to_ascii_uppercase(), gene))
        .collect()
}

fn write_carried_allele_batch(
    writer: &mut ArrowWriter<File>,
    schema: &Arc<Schema>,
    allele_ids: &mut Vec<String>,
    passed: &mut Vec<bool>,
) -> Result<(), String> {
    if allele_ids.is_empty() {
        return Ok(());
    }
    let row_count = allele_ids.len();
    let columns = vec![
        Arc::new(Int32Array::from(vec![
            CARRIED_ALLELE_SCHEMA_VERSION;
            row_count
        ])) as ArrayRef,
        Arc::new(StringArray::from(std::mem::take(allele_ids))) as ArrayRef,
        Arc::new(BooleanArray::from(std::mem::take(passed))) as ArrayRef,
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|error| format!("cannot build carried-allele batch: {error}"))?;
    writer
        .write(&batch)
        .map_err(|error| format!("cannot write carried-allele batch: {error}"))
}

fn build_carried_allele_sidecar(
    variants: &Path,
    sample_name: &str,
    destination: &Path,
) -> Result<u64, String> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("schema_version", DataType::Int32, false),
        Field::new("allele_id", DataType::Utf8, false),
        Field::new("passed", DataType::Boolean, false),
    ]));
    let output = File::create(destination)
        .map_err(|error| format!("cannot create carried-allele sidecar: {error}"))?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .set_max_row_group_row_count(Some(100_000))
        .build();
    let mut writer = ArrowWriter::try_new(output, schema.clone(), Some(properties))
        .map_err(|error| format!("cannot initialize carried-allele sidecar: {error}"))?;
    let connection = Connection::open_in_memory()
        .map_err(|error| format!("cannot initialize carried-allele scan: {error}"))?;
    let query = if crate::results::parquet_has_column(variants, "alternate_count")? {
        "SELECT allele_id, filter, alt_index, alternate_count, format, samples_json
         FROM read_parquet(?)"
    } else {
        "SELECT allele_id, filter, alt_index, record_alternate_count, format, samples_json
         FROM (
             SELECT *,
                    max(alt_index) OVER (PARTITION BY record_number) AS record_alternate_count
             FROM read_parquet(?)
         )"
    };
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("cannot prepare carried-allele scan: {error}"))?;
    let mut rows = statement
        .query(params![variants.to_string_lossy().as_ref()])
        .map_err(|error| format!("cannot query carried alleles: {error}"))?;
    let mut allele_ids = Vec::with_capacity(65_536);
    let mut passed = Vec::with_capacity(65_536);
    let mut carried_count = 0_u64;
    let mut row_number = 0_u64;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot read carried-allele scan: {error}"))?
    {
        row_number = row_number.saturating_add(1);
        let allele_id: String = row.get(0).map_err(|error| error.to_string())?;
        let filter: Option<String> = row.get(1).map_err(|error| error.to_string())?;
        let alt_index: i32 = row.get(2).map_err(|error| error.to_string())?;
        let alternate_count: i32 = row.get(3).map_err(|error| error.to_string())?;
        let format: Option<String> = row.get(4).map_err(|error| error.to_string())?;
        let samples_json: String = row.get(5).map_err(|error| error.to_string())?;
        let alt_index = usize::try_from(alt_index)
            .ok()
            .filter(|index| *index > 0)
            .ok_or_else(|| format!("report row {row_number} has an invalid ALT index"))?;
        let alternate_count = usize::try_from(alternate_count)
            .ok()
            .filter(|count| *count >= alt_index)
            .ok_or_else(|| format!("report row {row_number} has an invalid ALT count"))?;
        let samples = serde_json::from_str::<Vec<RawSampleCall>>(&samples_json)
            .map_err(|error| format!("report row {row_number} has invalid sample data: {error}"))?;
        let sample = samples
            .iter()
            .find(|sample| sample.name == sample_name)
            .ok_or_else(|| {
                format!("sample {sample_name} is missing from report row {row_number}")
            })?;
        let call = annocat_core::sample_call::parse_sample_call(
            sample_name,
            format.as_deref(),
            &sample.value,
            alt_index,
            alternate_count,
        );
        if call.allele_presence != annocat_core::sample_call::AllelePresence::Carried {
            continue;
        }
        allele_ids.push(allele_id);
        passed.push(
            filter.as_deref() == Some("PASS")
                && call.genotype_filter_state
                    != annocat_core::sample_call::GenotypeFilterState::Failed,
        );
        carried_count = carried_count.saturating_add(1);
        if allele_ids.len() >= 65_536 {
            write_carried_allele_batch(&mut writer, &schema, &mut allele_ids, &mut passed)?;
        }
    }
    write_carried_allele_batch(&mut writer, &schema, &mut allele_ids, &mut passed)?;
    writer
        .close()
        .map_err(|error| format!("cannot finish carried-allele sidecar: {error}"))?;
    Ok(carried_count)
}

fn aggregate_report_gene_overlap(
    variants: &Path,
    carried: &Path,
) -> Result<Vec<ReportGeneOverlap>, String> {
    let connection = Connection::open_in_memory()
        .map_err(|error| format!("cannot initialize gene-overlap aggregation: {error}"))?;
    let query = "WITH effects AS (
             SELECT allele_id, gene_symbol, impact FROM read_parquet(?)
         ),
         allele_gene AS (
             SELECT min(trim(e.gene_symbol)) AS symbol,
                    upper(trim(e.gene_symbol)) AS symbol_key,
                    e.allele_id,
                    bool_or(c.passed) AS passed,
                    max(CASE upper(coalesce(e.impact, ''))
                        WHEN 'HIGH' THEN 2 WHEN 'MODERATE' THEN 1 ELSE 0 END) AS impact_rank
             FROM effects e
             JOIN read_parquet(?) c USING (allele_id)
             WHERE e.gene_symbol IS NOT NULL AND trim(e.gene_symbol) <> ''
             GROUP BY symbol_key, e.allele_id
         )
         SELECT min(symbol),
                count(*),
                sum(CASE WHEN passed THEN 1 ELSE 0 END),
                sum(CASE WHEN passed AND impact_rank = 2 THEN 1 ELSE 0 END),
                sum(CASE WHEN passed AND impact_rank = 1 THEN 1 ELSE 0 END)
         FROM allele_gene
         GROUP BY symbol_key
         ORDER BY symbol_key";
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("cannot prepare gene-overlap aggregation: {error}"))?;
    let variants_path = variants.to_string_lossy().into_owned();
    let carried_path = carried.to_string_lossy().into_owned();
    let mut rows = statement
        .query(params![variants_path, carried_path])
        .map_err(|error| format!("cannot query gene-overlap aggregation: {error}"))?;
    let mut genes = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot read gene-overlap aggregation: {error}"))?
    {
        let mut gene = ReportGeneOverlap {
            symbol: row.get(0).map_err(|error| error.to_string())?,
            variant_count: row.get(1).map_err(|error| error.to_string())?,
            pass_count: row.get(2).map_err(|error| error.to_string())?,
            high_impact_count: row.get(3).map_err(|error| error.to_string())?,
            moderate_impact_count: row.get(4).map_err(|error| error.to_string())?,
            tier: 0,
            tier_label: String::new(),
        };
        gene.tier = if gene.high_impact_count > 0 {
            4
        } else if gene.moderate_impact_count > 0 {
            3
        } else if gene.pass_count > 0 {
            2
        } else {
            1
        };
        gene.tier_label = report_overlap_label(gene.tier).into();
        genes.push(gene);
    }
    Ok(genes)
}

fn report_gene_overlap_summary(
    runs: &Path,
    run_id: &str,
    parquet: &Path,
    _consequences: Option<&Path>,
    sample_name: &str,
) -> Result<Vec<ReportGeneOverlap>, String> {
    let metadata =
        fs::metadata(parquet).map_err(|error| format!("cannot inspect report overlap: {error}"))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let cache_path = runs
        .join(".annocat-library")
        .join(run_id)
        .join("phenotype-gene-overlap.json");
    if let Ok(bytes) = fs::read(&cache_path)
        && bytes.len() <= 32 * 1024 * 1024
        && let Ok(cache) = serde_json::from_slice::<ReportOverlapCache>(&bytes)
        && cache.schema_version == REPORT_OVERLAP_CACHE_SCHEMA_VERSION
        && cache.parquet_bytes == metadata.len()
        && cache.parquet_modified_nanos == modified
        && cache.sample_name == sample_name
    {
        return Ok(cache.genes);
    }
    let cache_directory = cache_path
        .parent()
        .ok_or("phenotype overlap cache has no parent directory")?;
    fs::create_dir_all(cache_directory)
        .map_err(|error| format!("cannot create phenotype overlap cache: {error}"))?;
    let sidecar = cache_directory.join(format!(
        ".carried-alleles-{}-{}.parquet",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock cannot name phenotype sidecar: {error}"))?
            .as_nanos()
    ));
    let genes = (|| {
        build_carried_allele_sidecar(parquet, sample_name, &sidecar)?;
        aggregate_report_gene_overlap(parquet, &sidecar)
    })();
    let _ = fs::remove_file(&sidecar);
    let genes = genes?;
    let cache = ReportOverlapCache {
        schema_version: REPORT_OVERLAP_CACHE_SCHEMA_VERSION,
        parquet_bytes: metadata.len(),
        parquet_modified_nanos: modified,
        sample_name: sample_name.into(),
        genes: genes.clone(),
    };
    let bytes = serde_json::to_vec(&cache)
        .map_err(|error| format!("cannot serialize report overlap: {error}"))?;
    super::library_metadata::atomic_write(&cache_path, &bytes)?;
    Ok(genes)
}

fn report_overlap_label(tier: u8) -> &'static str {
    match tier {
        4 => "Carried ALT on a PASS row with HIGH VEP impact in an associated feature",
        3 => "Carried ALT on a PASS row with MODERATE VEP impact in an associated feature",
        2 => "Carried ALT on a PASS row with an associated gene symbol",
        1 => "Carried ALT has an associated gene symbol",
        _ => "No carried-ALT gene overlap",
    }
}

fn knowledge(resources: &Path) -> Result<Arc<HpoKnowledge>, String> {
    let root = release_root(resources)?;
    if installed_status_at(&root).is_none() {
        return Err(
            "Human Phenotype Ontology data is not installed. Install it from Data sources first."
                .into(),
        );
    }
    if let Some(cached) = knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&root)
        .cloned()
    {
        return Ok(cached);
    }
    let loaded = Arc::new(load_knowledge_from_root(&root)?);
    knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(root, loaded.clone());
    Ok(loaded)
}

fn load_knowledge_from_root(root: &Path) -> Result<HpoKnowledge, String> {
    let raw = root.join("raw");
    let (mut terms, term_index, _) = parse_ontology(&raw.join("hp.obo"))?;
    populate_ancestors(&mut terms)?;
    let phenotypic_abnormality_root = term_index
        .get(PHENOTYPIC_ABNORMALITY_ROOT)
        .copied()
        .ok_or("HPO ontology is missing the phenotypic abnormality root")?;
    let active_terms = terms
        .iter()
        .enumerate()
        .filter_map(|(index, term)| {
            (!term.obsolete
                && index != phenotypic_abnormality_root
                && term.ancestors.contains(&phenotypic_abnormality_root))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let (mut diseases, association_count) = parse_diseases(
        &raw.join("phenotype.hpoa"),
        &raw.join("genes_to_disease.txt"),
        &terms,
        &term_index,
        phenotypic_abnormality_root,
    )?;
    diseases.retain(|disease| !disease.positive.is_empty());
    let mut counts = vec![0_u64; terms.len()];
    for disease in &diseases {
        let mut propagated = HashSet::new();
        for &annotation in &disease.positive {
            propagated.extend(terms[annotation].ancestors.iter().copied());
        }
        for index in propagated {
            counts[index] = counts[index].saturating_add(1);
        }
    }
    let disease_count = diseases.len().max(1) as f64;
    let information_content = counts
        .iter()
        .map(|&count| {
            if count == 0 {
                disease_count.ln()
            } else {
                (disease_count / count as f64).ln().max(0.0)
            }
        })
        .collect();
    Ok(HpoKnowledge {
        terms,
        term_index,
        active_terms,
        phenotypic_abnormality_root,
        diseases,
        information_content,
        disease_gene_association_count: association_count,
    })
}

fn parse_ontology(path: &Path) -> Result<ParsedOntology, String> {
    let reader = BufReader::new(
        File::open(path).map_err(|error| format!("cannot read HPO ontology: {error}"))?,
    );
    let mut raw_terms = Vec::new();
    let mut current: Option<RawTerm> = None;
    for line in reader.lines() {
        let line = line.map_err(|error| format!("cannot parse HPO ontology: {error}"))?;
        if line == "[Term]" {
            if let Some(term) = current.take()
                && !term.id.is_empty()
            {
                raw_terms.push(term);
            }
            current = Some(RawTerm {
                id: String::new(),
                label: String::new(),
                synonyms: Vec::new(),
                parent_ids: Vec::new(),
                alt_ids: Vec::new(),
                obsolete: false,
                replacement: None,
            });
            continue;
        }
        if line.starts_with('[') {
            if let Some(term) = current.take()
                && !term.id.is_empty()
            {
                raw_terms.push(term);
            }
            continue;
        }
        let Some(term) = current.as_mut() else {
            continue;
        };
        if let Some(value) = line.strip_prefix("id: ") {
            term.id = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("name: ") {
            term.label = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("alt_id: ") {
            term.alt_ids.push(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("is_a: ") {
            if let Some(parent) = value.split_whitespace().next() {
                term.parent_ids.push(parent.to_owned());
            }
        } else if let Some(value) = line.strip_prefix("synonym: ") {
            if let Some(synonym) = quoted_value(value) {
                term.synonyms.push(synonym);
            }
        } else if line == "is_obsolete: true" {
            term.obsolete = true;
        } else if let Some(value) = line.strip_prefix("replaced_by: ") {
            term.replacement = Some(value.trim().to_owned());
        }
    }
    if let Some(term) = current
        && !term.id.is_empty()
    {
        raw_terms.push(term);
    }
    if raw_terms.is_empty() {
        return Err("HPO ontology contains no terms".into());
    }
    let mut term_index = HashMap::new();
    for (index, term) in raw_terms.iter().enumerate() {
        term_index.insert(term.id.clone(), index);
        for alt in &term.alt_ids {
            term_index.insert(alt.clone(), index);
        }
    }
    let terms = raw_terms
        .iter()
        .map(|term| {
            let mut search = vec![
                term.id.to_ascii_lowercase(),
                term.label.to_ascii_lowercase(),
            ];
            search.extend(term.synonyms.iter().map(|value| value.to_ascii_lowercase()));
            OntologyTerm {
                id: term.id.clone(),
                label: term.label.clone(),
                synonyms: term.synonyms.clone(),
                search_text: search.join("\n"),
                parents: term
                    .parent_ids
                    .iter()
                    .filter_map(|id| term_index.get(id).copied())
                    .collect(),
                ancestors: Vec::new(),
                obsolete: term.obsolete,
                replacement: term
                    .replacement
                    .as_ref()
                    .and_then(|id| term_index.get(id).copied()),
            }
        })
        .collect::<Vec<_>>();
    let active_terms = terms
        .iter()
        .enumerate()
        .filter_map(|(index, term)| (!term.obsolete).then_some(index))
        .collect();
    Ok((terms, term_index, active_terms))
}

fn populate_ancestors(terms: &mut [OntologyTerm]) -> Result<(), String> {
    fn visit(
        index: usize,
        parents: &[Vec<usize>],
        memo: &mut [Option<Vec<usize>>],
        visiting: &mut HashSet<usize>,
    ) -> Result<Vec<usize>, String> {
        if let Some(ancestors) = &memo[index] {
            return Ok(ancestors.clone());
        }
        if !visiting.insert(index) {
            return Err("HPO ontology contains a parent cycle".into());
        }
        let mut ancestors = vec![index];
        for &parent in &parents[index] {
            ancestors.extend(visit(parent, parents, memo, visiting)?);
        }
        ancestors.sort_unstable();
        ancestors.dedup();
        visiting.remove(&index);
        memo[index] = Some(ancestors.clone());
        Ok(ancestors)
    }
    let parents = terms
        .iter()
        .map(|term| term.parents.clone())
        .collect::<Vec<_>>();
    let mut memo = vec![None; terms.len()];
    for (index, term) in terms.iter_mut().enumerate() {
        term.ancestors = visit(index, &parents, &mut memo, &mut HashSet::new())?;
    }
    Ok(())
}

fn phenotype_frequency(raw: &str) -> (Option<f64>, Option<String>) {
    let raw = raw.trim();
    let coded = match raw {
        "HP:0040280" => Some((1.0, "Obligate (100%)")),
        "HP:0040281" => Some((0.895, "Very frequent (80-99%)")),
        "HP:0040282" => Some((0.545, "Frequent (30-79%)")),
        "HP:0040283" => Some((0.17, "Occasional (5-29%)")),
        "HP:0040284" => Some((0.025, "Very rare (1-4%)")),
        "HP:0040285" => Some((0.0, "Excluded (0%)")),
        _ => None,
    };
    if let Some((probability, label)) = coded {
        return (Some(probability), Some(label.into()));
    }
    if let Some((numerator, denominator)) = raw.split_once('/')
        && let (Ok(numerator), Ok(denominator)) =
            (numerator.parse::<f64>(), denominator.parse::<f64>())
        && denominator > 0.0
        && numerator >= 0.0
        && numerator <= denominator
    {
        let probability = numerator / denominator;
        return (
            Some(probability),
            Some(format!("{raw} ({:.1}%)", probability * 100.0)),
        );
    }
    if let Some(percent) = raw.strip_suffix('%')
        && let Ok(percent) = percent.trim().parse::<f64>()
        && (0.0..=100.0).contains(&percent)
    {
        return (Some(percent / 100.0), Some(raw.to_owned()));
    }
    (None, (!raw.is_empty()).then(|| raw.to_owned()))
}

fn append_unique(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

fn merge_disease_annotation(
    target: &mut DiseasePhenotypeContext,
    frequency_raw: &str,
    onset: &str,
    sex: &str,
    evidence: &str,
    reference: &str,
) {
    let (probability, label) = phenotype_frequency(frequency_raw);
    if probability > target.frequency_probability {
        target.frequency_probability = probability;
        target.frequency_label = label;
        target.frequency_raw = (!frequency_raw.is_empty()).then(|| frequency_raw.to_owned());
    } else if target.frequency_raw.is_none() && !frequency_raw.is_empty() {
        target.frequency_label = label;
        target.frequency_raw = Some(frequency_raw.to_owned());
    }
    append_unique(&mut target.onset, onset);
    append_unique(&mut target.sex, sex);
    append_unique(&mut target.evidence, evidence);
    append_unique(&mut target.references, reference);
}

fn parse_diseases(
    hpoa_path: &Path,
    genes_path: &Path,
    terms: &[OntologyTerm],
    term_index: &HashMap<String, usize>,
    phenotypic_abnormality_root: usize,
) -> Result<(Vec<DiseaseProfile>, usize), String> {
    let reader = BufReader::new(
        File::open(hpoa_path).map_err(|error| format!("cannot read HPO annotations: {error}"))?,
    );
    let mut header = None;
    let mut diseases: BTreeMap<String, DiseaseBuilder> = BTreeMap::new();
    for line in reader.lines() {
        let line = line.map_err(|error| format!("cannot parse HPO annotations: {error}"))?;
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if header.is_none() {
            header = Some(
                fields
                    .iter()
                    .enumerate()
                    .map(|(index, value)| ((*value).to_owned(), index))
                    .collect::<HashMap<_, _>>(),
            );
            continue;
        }
        let columns = header.as_ref().expect("HPOA header initialized");
        let value = |name: &str| {
            columns
                .get(name)
                .and_then(|&index| fields.get(index))
                .copied()
                .unwrap_or("")
                .trim()
        };
        if value("aspect") != "P" {
            continue;
        }
        let Some(index) = resolve_term_index(value("hpo_id"), terms, term_index) else {
            continue;
        };
        if index == phenotypic_abnormality_root
            || !terms[index]
                .ancestors
                .contains(&phenotypic_abnormality_root)
        {
            continue;
        }
        let disease_id = value("database_id");
        if disease_id.is_empty() {
            continue;
        }
        let disease = diseases
            .entry(disease_id.to_owned())
            .or_insert_with(|| DiseaseBuilder {
                id: disease_id.to_owned(),
                name: value("disease_name").to_owned(),
                positive: Vec::new(),
                negative: Vec::new(),
                annotations: HashMap::new(),
                genes: Vec::new(),
            });
        if value("qualifier") == "NOT" || value("frequency") == "HP:0040285" {
            disease.negative.push(index);
        } else {
            disease.positive.push(index);
            merge_disease_annotation(
                disease.annotations.entry(index).or_default(),
                value("frequency"),
                value("onset"),
                value("sex"),
                value("evidence"),
                value("reference"),
            );
        }
    }

    let reader = BufReader::new(
        File::open(genes_path)
            .map_err(|error| format!("cannot read HPO disease-gene associations: {error}"))?,
    );
    let mut gene_header = None;
    let mut association_count = 0_usize;
    for line in reader.lines() {
        let line =
            line.map_err(|error| format!("cannot parse HPO disease-gene associations: {error}"))?;
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if gene_header.is_none() {
            gene_header = Some(
                fields
                    .iter()
                    .enumerate()
                    .map(|(index, value)| ((*value).to_owned(), index))
                    .collect::<HashMap<_, _>>(),
            );
            continue;
        }
        let columns = gene_header.as_ref().expect("gene header initialized");
        let value = |name: &str| {
            columns
                .get(name)
                .and_then(|&index| fields.get(index))
                .copied()
                .unwrap_or("")
                .trim()
        };
        let Some(disease) = diseases.get_mut(value("disease_id")) else {
            continue;
        };
        let symbol = value("gene_symbol");
        if symbol.is_empty() {
            continue;
        }
        disease.genes.push(GeneAssociation {
            gene_id: value("ncbi_gene_id").to_owned(),
            symbol: symbol.to_owned(),
            association_type: value("association_type").to_owned(),
            source: value("source").to_owned(),
        });
        association_count += 1;
    }
    let profiles = diseases
        .into_values()
        .map(|mut disease| {
            disease.positive.sort_unstable();
            disease.positive.dedup();
            let positive = disease.positive.clone();
            disease.positive.retain(|index| {
                !positive
                    .iter()
                    .any(|other| index != other && terms[*other].ancestors.contains(index))
            });
            disease
                .annotations
                .retain(|index, _| disease.positive.contains(index));
            disease.negative.sort_unstable();
            disease.negative.dedup();
            let negative = disease.negative.clone();
            disease.negative.retain(|index| {
                !negative
                    .iter()
                    .any(|other| index != other && terms[*index].ancestors.contains(other))
            });
            disease.genes.sort_by(|left, right| {
                left.symbol
                    .cmp(&right.symbol)
                    .then(left.gene_id.cmp(&right.gene_id))
                    .then(left.association_type.cmp(&right.association_type))
                    .then(left.source.cmp(&right.source))
            });
            disease.genes.dedup_by(|left, right| {
                left.symbol == right.symbol
                    && left.gene_id == right.gene_id
                    && left.association_type == right.association_type
                    && left.source == right.source
            });
            DiseaseProfile {
                id: disease.id,
                name: disease.name,
                positive: disease.positive,
                negative: disease.negative,
                annotations: disease.annotations,
                genes: disease.genes,
            }
        })
        .collect();
    Ok((profiles, association_count))
}

fn term_similarity(knowledge: &HpoKnowledge, left: usize, right: usize) -> f64 {
    if left == right {
        return 1.0;
    }
    let left_ancestors = &knowledge.terms[left].ancestors;
    let right_ancestors = &knowledge.terms[right].ancestors;
    let mut left_position = 0;
    let mut right_position = 0;
    let mut mica = 0.0_f64;
    while left_position < left_ancestors.len() && right_position < right_ancestors.len() {
        match left_ancestors[left_position].cmp(&right_ancestors[right_position]) {
            std::cmp::Ordering::Less => left_position += 1,
            std::cmp::Ordering::Greater => right_position += 1,
            std::cmp::Ordering::Equal => {
                mica = mica.max(knowledge.information_content[left_ancestors[left_position]]);
                left_position += 1;
                right_position += 1;
            }
        }
    }
    let denominator = knowledge.information_content[left] + knowledge.information_content[right];
    if denominator <= f64::EPSILON {
        0.0
    } else {
        (2.0 * mica / denominator).clamp(0.0, 1.0)
    }
}

fn canonical_terms(
    knowledge: &HpoKnowledge,
    terms: &[PhenotypeTerm],
    allow_empty: bool,
) -> Result<Vec<PhenotypeTerm>, String> {
    if !allow_empty && terms.is_empty() {
        return Err("select at least one observed phenotype".into());
    }
    if terms.len() > MAX_PROFILE_TERMS {
        return Err(format!(
            "a phenotype profile can contain at most {MAX_PROFILE_TERMS} terms"
        ));
    }
    let mut seen = HashSet::new();
    terms
        .iter()
        .map(|term| {
            validate_hpo_id(&term.id)?;
            let index = resolve_term_index(&term.id, &knowledge.terms, &knowledge.term_index)
                .ok_or_else(|| {
                    format!("{} is not present in the installed HPO release", term.id)
                })?;
            if index == knowledge.phenotypic_abnormality_root
                || !knowledge.terms[index]
                    .ancestors
                    .contains(&knowledge.phenotypic_abnormality_root)
            {
                return Err(format!(
                    "{} is not a phenotypic abnormality term and cannot be used in a patient profile",
                    term.id
                ));
            }
            let canonical = &knowledge.terms[index];
            if !seen.insert(canonical.id.as_str()) {
                return Err(format!("duplicate HPO term: {}", canonical.id));
            }
            Ok(PhenotypeTerm {
                id: canonical.id.clone(),
                label: canonical.label.clone(),
            })
        })
        .collect()
}

fn normalize_terms(
    knowledge: &HpoKnowledge,
    terms: Vec<PhenotypeTerm>,
    keep_most_specific: bool,
) -> Result<Vec<PhenotypeTerm>, String> {
    let indexes = term_indexes(knowledge, &terms)?;
    Ok(terms
        .into_iter()
        .enumerate()
        .filter_map(|(position, term)| {
            let index = indexes[position];
            let redundant = indexes.iter().enumerate().any(|(other_position, &other)| {
                if position == other_position {
                    return false;
                }
                if keep_most_specific {
                    knowledge.terms[other].ancestors.contains(&index)
                } else {
                    knowledge.terms[index].ancestors.contains(&other)
                }
            });
            (!redundant).then_some(term)
        })
        .collect())
}

fn ensure_consistent_profile(
    knowledge: &HpoKnowledge,
    observed: &[PhenotypeTerm],
    excluded: &[PhenotypeTerm],
) -> Result<(), String> {
    if observed.len().saturating_add(excluded.len()) > MAX_PROFILE_TERMS {
        return Err(format!(
            "a phenotype profile can contain at most {MAX_PROFILE_TERMS} terms"
        ));
    }
    let observed_indexes = term_indexes(knowledge, observed)?;
    let excluded_indexes = term_indexes(knowledge, excluded)?;
    for (&observed_index, observed_term) in observed_indexes.iter().zip(observed) {
        if let Some((_, excluded_term)) =
            excluded_indexes
                .iter()
                .zip(excluded)
                .find(|(excluded_index, _)| {
                    knowledge.terms[observed_index]
                        .ancestors
                        .contains(excluded_index)
                })
        {
            return Err(format!(
                "{} ({}) is observed but is a descendant of explicitly absent {} ({})",
                observed_term.label, observed_term.id, excluded_term.label, excluded_term.id
            ));
        }
    }
    Ok(())
}

fn ensure_exactly_disjoint(
    observed: &[PhenotypeTerm],
    excluded: &[PhenotypeTerm],
) -> Result<(), String> {
    let observed = observed
        .iter()
        .map(|term| term.id.as_str())
        .collect::<HashSet<_>>();
    if let Some(term) = excluded
        .iter()
        .find(|term| observed.contains(term.id.as_str()))
    {
        return Err(format!(
            "{} cannot be both observed and explicitly absent",
            term.id
        ));
    }
    Ok(())
}

fn term_indexes(knowledge: &HpoKnowledge, terms: &[PhenotypeTerm]) -> Result<Vec<usize>, String> {
    terms
        .iter()
        .map(|term| {
            knowledge
                .term_index
                .get(&term.id)
                .copied()
                .ok_or_else(|| format!("HPO term {} is unavailable", term.id))
        })
        .collect()
}

fn resolve_term_index(
    id: &str,
    terms: &[OntologyTerm],
    term_index: &HashMap<String, usize>,
) -> Option<usize> {
    let mut index = *term_index.get(id)?;
    let mut visited = HashSet::new();
    while terms.get(index)?.obsolete {
        if !visited.insert(index) {
            return None;
        }
        index = terms.get(index)?.replacement?;
    }
    Some(index)
}

fn term_value(knowledge: &HpoKnowledge, index: usize) -> PhenotypeTerm {
    PhenotypeTerm {
        id: knowledge.terms[index].id.clone(),
        label: knowledge.terms[index].label.clone(),
    }
}

fn validate_profile(profile: &PhenotypeProfile, run_id: &str) -> Result<(), String> {
    if !(2..=PROFILE_SCHEMA_VERSION).contains(&profile.schema_version) || profile.run_id != run_id {
        return Err("phenotype profile identity is invalid".into());
    }
    for term in profile.observed.iter().chain(&profile.excluded) {
        validate_hpo_id(&term.id)?;
        if term.label.trim().is_empty()
            || term.label.len() > 300
            || term.label.chars().any(char::is_control)
        {
            return Err(format!("invalid label for {}", term.id));
        }
    }
    if profile
        .observed
        .len()
        .saturating_add(profile.excluded.len())
        > MAX_PROFILE_TERMS
    {
        return Err(format!(
            "a phenotype profile can contain at most {MAX_PROFILE_TERMS} terms"
        ));
    }
    ensure_exactly_disjoint(&profile.observed, &profile.excluded)
}

fn validate_hpo_id(id: &str) -> Result<(), String> {
    if id.len() != 10
        || !id.starts_with("HP:")
        || !id[3..].bytes().all(|byte| byte.is_ascii_digit())
    {
        Err(format!("invalid HPO identifier: {id}"))
    } else {
        Ok(())
    }
}

fn save(runs: &Path, profile: &PhenotypeProfile) -> Result<(), String> {
    validate_profile(profile, &profile.run_id)?;
    let bytes = serde_json::to_vec(profile)
        .map_err(|error| format!("cannot serialize phenotype profile: {error}"))?;
    super::library_metadata::atomic_write(&profile_path(runs, &profile.run_id), &bytes)
}

fn profile_path(runs: &Path, run_id: &str) -> PathBuf {
    runs.join(".annocat-library")
        .join(run_id)
        .join("phenotypes.json")
}

fn embedded_asset_manifest() -> Result<HpoAssetManifest, String> {
    let manifest: HpoAssetManifest =
        serde_json::from_str(annocat_core::source_catalog::resource_manifest_json("hpo")?)
            .map_err(|error| format!("invalid embedded HPO bootstrap manifest: {error}"))?;
    validate_asset_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_asset_manifest(manifest: &HpoAssetManifest) -> Result<(), String> {
    const REQUIRED_ASSETS: [(&str, &str); 3] = [
        ("ontology", "hp.obo"),
        ("disease-annotations", "phenotype.hpoa"),
        ("disease-genes", "genes_to_disease.txt"),
    ];
    let tag = format!("v{}", manifest.release);
    let release_url =
        format!("https://github.com/obophenotype/human-phenotype-ontology/releases/tag/{tag}");
    if manifest.schema_version != 1
        || !valid_hpo_release_version(&manifest.release)
        || manifest.release_url != release_url
        || manifest.assets.len() != 3
        || manifest.assets.iter().any(|asset| {
            asset.kind.trim().is_empty()
                || asset.filename.contains(['/', '\\'])
                || asset.bytes == 0
                || asset.sha256.len() != 64
                || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err("HPO asset manifest failed validation".into());
    }
    for (kind, filename) in REQUIRED_ASSETS {
        let matching = manifest
            .assets
            .iter()
            .filter(|asset| asset.kind == kind && asset.filename == filename)
            .collect::<Vec<_>>();
        let expected_url = format!(
            "https://github.com/obophenotype/human-phenotype-ontology/releases/download/{tag}/{filename}"
        );
        if matching.len() != 1 || matching[0].url != expected_url {
            return Err(format!(
                "HPO asset manifest is missing the official {filename} release asset"
            ));
        }
    }
    Ok(())
}

fn valid_hpo_release_version(value: &str) -> bool {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return false;
    }
    let month = value[5..7].parse::<u8>().ok();
    let day = value[8..10].parse::<u8>().ok();
    month.is_some_and(|month| (1..=12).contains(&month))
        && day.is_some_and(|day| (1..=31).contains(&day))
}

fn parse_github_release(bytes: &[u8]) -> Result<HpoAssetManifest, String> {
    const REQUIRED_ASSETS: [(&str, &str); 3] = [
        ("ontology", "hp.obo"),
        ("disease-annotations", "phenotype.hpoa"),
        ("disease-genes", "genes_to_disease.txt"),
    ];
    let release: GitHubRelease = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid HPO release metadata from GitHub: {error}"))?;
    if release.draft || release.prerelease {
        return Err("GitHub returned a draft or prerelease as the latest HPO release".into());
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .filter(|value| valid_hpo_release_version(value))
        .ok_or("the latest HPO release has an invalid version tag")?
        .to_owned();
    let expected_release_url = format!(
        "https://github.com/obophenotype/human-phenotype-ontology/releases/tag/{}",
        release.tag_name
    );
    if release.html_url != expected_release_url {
        return Err(
            "the latest HPO release metadata points outside the official repository".into(),
        );
    }
    let mut assets = Vec::with_capacity(REQUIRED_ASSETS.len());
    for (kind, filename) in REQUIRED_ASSETS {
        let matching = release
            .assets
            .iter()
            .filter(|asset| asset.name == filename)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(format!(
                "the latest HPO release did not contain exactly one {filename} asset"
            ));
        }
        let asset = matching[0];
        let expected_url = format!(
            "https://github.com/obophenotype/human-phenotype-ontology/releases/download/{}/{filename}",
            release.tag_name
        );
        if asset.browser_download_url != expected_url || asset.size == 0 {
            return Err(format!(
                "the latest HPO {filename} asset has invalid source metadata"
            ));
        }
        let sha256 = asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| {
                format!("the latest HPO {filename} asset has no valid GitHub SHA-256 digest")
            })?
            .to_ascii_lowercase();
        assets.push(HpoAsset {
            kind: kind.into(),
            filename: filename.into(),
            url: asset.browser_download_url.clone(),
            bytes: asset.size,
            sha256,
        });
    }
    let manifest = HpoAssetManifest {
        schema_version: 1,
        release: version,
        release_url: release.html_url,
        assets,
    };
    validate_asset_manifest(&manifest)?;
    Ok(manifest)
}

pub(crate) fn resolve_latest_asset_manifest() -> Result<HpoAssetManifest, String> {
    let url = annocat_core::source_catalog::resolver_api_url("hpo")
        .ok_or("HPO resolver API URL is missing from the source catalog")?;
    let client = super::http_client::source()
        .map_err(|error| format!("cannot create the HPO release resolver: {error}"))?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot discover the latest HPO release: {error}"))?;
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_RELEASE_METADATA_BYTES)
    {
        return Err("the HPO release metadata exceeded its safety limit".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RELEASE_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read the latest HPO release metadata: {error}"))?;
    if bytes.len() as u64 > MAX_RELEASE_METADATA_BYTES {
        return Err("the HPO release metadata exceeded its safety limit".into());
    }
    parse_github_release(&bytes)
}

fn download_asset(
    asset: &HpoAsset,
    final_path: &Path,
    completed_bytes: u64,
    expected_bytes: u64,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(InstallProgress),
) -> Result<(), String> {
    let partial = final_path.with_extension(format!(
        "{}.partial",
        final_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("asset")
    ));
    let mut existing = fs::metadata(&partial)
        .map(|metadata| metadata.len().min(asset.bytes))
        .unwrap_or(0);
    if existing == asset.bytes {
        if verify_sha256(&partial, &asset.sha256).is_ok() {
            if final_path.exists() {
                fs::remove_file(final_path)
                    .map_err(|error| format!("cannot replace {}: {error}", asset.filename))?;
            }
            fs::rename(&partial, final_path)
                .map_err(|error| format!("cannot publish {}: {error}", asset.filename))?;
            progress(InstallProgress {
                phase: "downloading".into(),
                detail: format!("Recovered verified {}", asset.filename),
                network_bytes: completed_bytes.saturating_add(asset.bytes),
                expected_network_bytes: expected_bytes,
                parsed_records: 0,
                prepared_bytes: completed_bytes.saturating_add(asset.bytes),
            });
            return Ok(());
        }
        fs::remove_file(&partial).map_err(|error| {
            format!("cannot discard corrupt {} partial: {error}", asset.filename)
        })?;
        existing = 0;
    }
    let client = super::http_client::source()?;
    let mut request = client.get(&asset.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut response = request
        .timeout(Duration::from_secs(15 * 60))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("HPO {} download failed: {error}", asset.filename))?;
    super::downloader::validate_response(&response, existing, asset.bytes, existing > 0)
        .map_err(|error| format!("HPO {} download failed: {error}", asset.filename))?;
    let append = existing > 0;
    let mut persisted = if append { existing } else { 0 };
    let mut output = if append {
        OpenOptions::new()
            .append(true)
            .open(&partial)
            .map_err(|error| format!("cannot resume {}: {error}", asset.filename))?
    } else {
        File::create(&partial)
            .map_err(|error| format!("cannot create {}: {error}", asset.filename))?
    };
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        ensure_not_cancelled(cancelled, "HPO installation")?;
        let read = response
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {} download: {error}", asset.filename))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot write {}: {error}", asset.filename))?;
        persisted = persisted.saturating_add(read as u64);
        if persisted > asset.bytes {
            return Err(format!(
                "{} exceeded its pinned size of {} bytes",
                asset.filename, asset.bytes
            ));
        }
        progress(InstallProgress {
            phase: "downloading".into(),
            detail: format!("Downloading {}", asset.filename),
            network_bytes: completed_bytes.saturating_add(persisted),
            expected_network_bytes: expected_bytes,
            parsed_records: 0,
            prepared_bytes: completed_bytes.saturating_add(persisted),
        });
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot flush {}: {error}", asset.filename))?;
    if persisted != asset.bytes {
        return Err(format!(
            "{} has {} bytes; expected {}",
            asset.filename, persisted, asset.bytes
        ));
    }
    verify_sha256(&partial, &asset.sha256)?;
    if final_path.exists() {
        fs::remove_file(final_path)
            .map_err(|error| format!("cannot replace {}: {error}", asset.filename))?;
    }
    fs::rename(&partial, final_path)
        .map_err(|error| format!("cannot publish {}: {error}", asset.filename))
}

fn verified_asset(path: &Path, asset: &HpoAsset) -> Result<bool, String> {
    if !fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() == asset.bytes)
    {
        return Ok(false);
    }
    if verify_sha256(path, &asset.sha256).is_ok() {
        return Ok(true);
    }
    fs::remove_file(path)
        .map_err(|error| format!("cannot discard corrupt {}: {error}", asset.filename))?;
    Ok(false)
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let actual = super::fastvep::sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "{} failed SHA-256 verification",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("HPO asset")
        ))
    }
}

fn directory_size(root: &Path) -> u64 {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_size(&path)
            } else {
                entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn ensure_not_cancelled(cancelled: &AtomicBool, operation: &str) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err(format!("{operation} was cancelled"))
    } else {
        Ok(())
    }
}

fn quoted_value(value: &str) -> Option<String> {
    let value = value.strip_prefix('"')?;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if character == '"' && !escaped {
            return Some(value[..index].replace("\\\"", "\""));
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    None
}

fn normalize_search(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (total, count) = values.fold((0.0, 0_usize), |(total, count), value| {
        (total + value, count + 1)
    });
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

fn round(value: f64, places: i32) -> f64 {
    let factor = 10_f64.powi(places);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_knowledge() -> HpoKnowledge {
        let terms = vec![
            OntologyTerm {
                id: PHENOTYPIC_ABNORMALITY_ROOT.into(),
                label: "Phenotypic abnormality".into(),
                synonyms: Vec::new(),
                search_text: "phenotypic abnormality".into(),
                parents: Vec::new(),
                ancestors: vec![0],
                obsolete: false,
                replacement: None,
            },
            OntologyTerm {
                id: "HP:0001250".into(),
                label: "Seizure".into(),
                synonyms: Vec::new(),
                search_text: "seizure".into(),
                parents: vec![0],
                ancestors: vec![0, 1],
                obsolete: false,
                replacement: None,
            },
            OntologyTerm {
                id: "HP:0001263".into(),
                label: "Global developmental delay".into(),
                synonyms: Vec::new(),
                search_text: "global developmental delay".into(),
                parents: vec![0],
                ancestors: vec![0, 2],
                obsolete: false,
                replacement: None,
            },
            OntologyTerm {
                id: "HP:0001252".into(),
                label: "Hypotonia".into(),
                synonyms: Vec::new(),
                search_text: "hypotonia".into(),
                parents: vec![0],
                ancestors: vec![0, 3],
                obsolete: false,
                replacement: None,
            },
            OntologyTerm {
                id: "HP:0002197".into(),
                label: "Generalized-onset seizure".into(),
                synonyms: Vec::new(),
                search_text: "generalized-onset seizure".into(),
                parents: vec![1],
                ancestors: vec![0, 1, 4],
                obsolete: false,
                replacement: None,
            },
            OntologyTerm {
                id: "HP:0000005".into(),
                label: "Mode of inheritance".into(),
                synonyms: Vec::new(),
                search_text: "mode of inheritance".into(),
                parents: Vec::new(),
                ancestors: vec![5],
                obsolete: false,
                replacement: None,
            },
        ];
        HpoKnowledge {
            term_index: terms
                .iter()
                .enumerate()
                .map(|(index, term)| (term.id.clone(), index))
                .collect(),
            active_terms: vec![1, 2, 3, 4],
            phenotypic_abnormality_root: 0,
            terms,
            diseases: Vec::new(),
            information_content: vec![0.0, 2.0, 2.0, 2.0, 3.0, 0.0],
            disease_gene_association_count: 0,
        }
    }

    fn disease(id: &str, positive: &[usize]) -> DiseaseProfile {
        DiseaseProfile {
            id: id.into(),
            name: id.into(),
            positive: positive.to_vec(),
            negative: Vec::new(),
            annotations: HashMap::new(),
            genes: Vec::new(),
        }
    }

    #[test]
    fn phenotype_terms_require_canonical_hpo_ids() {
        assert!(validate_hpo_id("HP:0001250").is_ok());
        assert!(validate_hpo_id("HP:1250").is_err());
        assert!(validate_hpo_id("MONDO:0001").is_err());
    }

    #[test]
    fn phenotype_frequencies_parse_hpo_bands_fractions_and_percentages() {
        assert_eq!(
            phenotype_frequency("HP:0040283"),
            (Some(0.17), Some("Occasional (5-29%)".into()))
        );
        assert_eq!(
            phenotype_frequency("3/4"),
            (Some(0.75), Some("3/4 (75.0%)".into()))
        );
        assert_eq!(
            phenotype_frequency("12.5%"),
            (Some(0.125), Some("12.5%".into()))
        );
        assert_eq!(
            phenotype_frequency("invalid"),
            (None, Some("invalid".into()))
        );
    }

    #[test]
    fn bootstrap_asset_manifest_is_pinned_and_complete() {
        let manifest = embedded_asset_manifest().unwrap();
        assert!(manifest.release_url.contains(&manifest.release));
        assert_eq!(manifest.assets.len(), 3);
        assert_eq!(
            manifest.assets.iter().map(|asset| asset.bytes).sum::<u64>(),
            48_372_406
        );
    }

    #[test]
    fn github_latest_release_metadata_becomes_a_verified_manifest() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let metadata = serde_json::to_vec(&serde_json::json!({
            "tag_name": "v2026-07-24",
            "html_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/tag/v2026-07-24",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "hp.obo",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/hp.obo",
                    "size": 11,
                    "digest": digest
                },
                {
                    "name": "phenotype.hpoa",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/phenotype.hpoa",
                    "size": 22,
                    "digest": format!("sha256:{}", "b".repeat(64))
                },
                {
                    "name": "genes_to_disease.txt",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/genes_to_disease.txt",
                    "size": 33,
                    "digest": format!("sha256:{}", "c".repeat(64))
                }
            ]
        }))
        .unwrap();
        let manifest = parse_github_release(&metadata).unwrap();
        assert_eq!(manifest.release, "2026-07-24");
        assert_eq!(manifest.expected_bytes(), 66);
        assert_eq!(manifest.assets[0].sha256, "a".repeat(64));
    }

    #[test]
    fn github_latest_release_requires_publisher_digests() {
        let metadata = serde_json::to_vec(&serde_json::json!({
            "tag_name": "v2026-07-24",
            "html_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/tag/v2026-07-24",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "hp.obo",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/hp.obo",
                    "size": 11,
                    "digest": null
                },
                {
                    "name": "phenotype.hpoa",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/phenotype.hpoa",
                    "size": 22,
                    "digest": format!("sha256:{}", "b".repeat(64))
                },
                {
                    "name": "genes_to_disease.txt",
                    "browser_download_url": "https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-07-24/genes_to_disease.txt",
                    "size": 33,
                    "digest": format!("sha256:{}", "c".repeat(64))
                }
            ]
        }))
        .unwrap();
        assert!(parse_github_release(&metadata).is_err());
    }

    #[test]
    fn newest_verified_installed_hpo_release_is_used_offline() {
        let resources = std::env::temp_dir().join(format!(
            "annocat-hpo-release-selection-test-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for release in ["2026-06-23", "2026-07-24"] {
            let root = resources.join("hpo").join(release);
            fs::create_dir_all(root.join("raw")).unwrap();
            let tag = format!("v{release}");
            let assets = [
                ("ontology", "hp.obo"),
                ("disease-annotations", "phenotype.hpoa"),
                ("disease-genes", "genes_to_disease.txt"),
            ]
            .into_iter()
            .map(|(kind, filename)| {
                fs::write(root.join("raw").join(filename), b"abc").unwrap();
                HpoAsset {
                    kind: kind.into(),
                    filename: filename.into(),
                    url: format!(
                        "https://github.com/obophenotype/human-phenotype-ontology/releases/download/{tag}/{filename}"
                    ),
                    bytes: 3,
                    sha256:
                        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                            .into(),
                }
            })
            .collect::<Vec<_>>();
            let manifest = HpoAssetManifest {
                schema_version: 1,
                release: release.into(),
                release_url: format!(
                    "https://github.com/obophenotype/human-phenotype-ontology/releases/tag/{tag}"
                ),
                assets,
            };
            fs::write(
                root.join(INSTALLED_ASSET_MANIFEST_FILENAME),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::write(
                root.join(READY_FILENAME),
                serde_json::to_vec(&HpoReadyManifest {
                    schema_version: INSTALL_SCHEMA_VERSION,
                    release: release.into(),
                    installed_at: "2026-07-24T00:00:00Z".into(),
                    asset_bytes: 9,
                    term_count: 1,
                    disease_count: 1,
                    disease_gene_association_count: 1,
                })
                .unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            installed_versions(&resources),
            vec!["2026-06-23".to_string(), "2026-07-24".to_string()]
        );
        assert_eq!(hpo_release(&resources).unwrap(), "2026-07-24");
        fs::remove_dir_all(resources).unwrap();
    }

    #[test]
    fn installed_assets_require_matching_sha256_not_only_matching_size() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-integrity-test-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("raw")).unwrap();
        fs::write(root.join("raw").join("asset.txt"), b"abd").unwrap();
        let manifest = HpoAssetManifest {
            schema_version: 1,
            release: "test".into(),
            release_url: "https://example.test".into(),
            assets: vec![HpoAsset {
                kind: "test".into(),
                filename: "asset.txt".into(),
                url: "https://example.test/asset.txt".into(),
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            }],
        };
        assert!(!verified_installation(&root, &manifest));
        fs::write(root.join("raw").join("asset.txt"), b"abc").unwrap();
        assert!(verified_installation(&root, &manifest));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn patient_to_disease_similarity_prefers_the_better_query_match() {
        let knowledge = test_knowledge();
        let report = HashMap::new();
        let matching = rank_disease(
            &knowledge,
            &disease("matching", &[1, 2]),
            &[1, 2],
            &[],
            &report,
        );
        let partial = rank_disease(
            &knowledge,
            &disease("partial", &[1, 3]),
            &[1, 2],
            &[],
            &report,
        );
        assert!(matching.phenotype_score > partial.phenotype_score);
        assert_eq!(matching.query_coverage, 100.0);
    }

    #[test]
    fn unrecorded_disease_findings_do_not_lower_an_exact_query_match() {
        let knowledge = test_knowledge();
        let report = HashMap::new();
        let ranked = rank_disease(
            &knowledge,
            &disease("exact-with-additional-findings", &[1, 2, 3]),
            &[1],
            &[],
            &report,
        );
        assert_eq!(ranked.phenotype_score, 100.0);
        assert_eq!(ranked.query_coverage, 100.0);
    }

    #[test]
    fn explicitly_absent_terms_are_reported_separately_from_similarity() {
        let knowledge = test_knowledge();
        let report = HashMap::new();
        let profile = disease("conflict", &[1, 2]);
        let without_absent = rank_disease(&knowledge, &profile, &[1], &[], &report);
        let with_absent = rank_disease(&knowledge, &profile, &[1], &[2], &report);
        assert_eq!(with_absent.phenotype_score, without_absent.phenotype_score);
        assert_eq!(with_absent.conflict_score, 100.0);
        assert!(!with_absent.conflict_frequency_complete);
    }

    #[test]
    fn absent_feature_conflict_respects_reported_disease_frequency() {
        let knowledge = test_knowledge();
        let report = HashMap::new();
        let mut profile = disease("frequency-weighted-conflict", &[1, 2]);
        profile.annotations.insert(
            2,
            DiseasePhenotypeContext {
                frequency_probability: Some(0.17),
                ..DiseasePhenotypeContext::default()
            },
        );
        let ranked = rank_disease(&knowledge, &profile, &[1], &[2], &report);
        assert!((ranked.conflict_score - 17.0).abs() < f64::EPSILON);
        assert!(ranked.conflict_frequency_complete);
    }

    #[test]
    fn report_overlap_requires_a_mendelian_gene_disease_association() {
        let overlap = ReportGeneOverlap {
            symbol: "GENE1".into(),
            variant_count: 1,
            pass_count: 1,
            high_impact_count: 1,
            moderate_impact_count: 0,
            tier: 4,
            tier_label: report_overlap_label(4).into(),
        };
        let report = HashMap::from([("GENE1".into(), overlap)]);
        let mut profile = disease("association-types", &[1]);
        profile.genes = vec![GeneAssociation {
            gene_id: "NCBIGene:1".into(),
            symbol: "GENE1".into(),
            association_type: "POLYGENIC".into(),
            source: "test".into(),
        }];
        assert!(!disease_report_overlap(&profile, &report).has_overlap);
        profile.genes[0].association_type = "MENDELIAN".into();
        assert!(disease_report_overlap(&profile, &report).has_overlap);
    }

    #[test]
    fn candidate_report_support_requires_pass_and_moderate_or_high_effect() {
        let overlap = |symbol: &str, tier: u8| ReportGeneOverlap {
            symbol: symbol.into(),
            variant_count: 1,
            pass_count: u64::from(tier >= 2),
            high_impact_count: u64::from(tier == 4),
            moderate_impact_count: u64::from(tier == 3),
            tier,
            tier_label: report_overlap_label(tier).into(),
        };
        let candidates = candidate_report_overlap_by_gene(&[
            overlap("FAILED", 1),
            overlap("PASS_ONLY", 2),
            overlap("MODERATE", 3),
            overlap("HIGH", 4),
        ]);
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains_key("MODERATE"));
        assert!(candidates.contains_key("HIGH"));
    }

    #[test]
    fn monarch_request_uses_the_cataloged_result_limit() {
        let terms = vec![PhenotypeTerm {
            id: "HP:0001250".into(),
            label: "Seizure".into(),
        }];
        let service =
            annocat_core::source_catalog::service("monarch-phenotype-gene-ranking").unwrap();
        let payload = monarch_request_payload(&terms, service.max_results);
        assert_eq!(payload["limit"], 50);
        assert_eq!(payload["termset"][0], "HP:0001250");
    }

    #[test]
    fn monarch_response_parser_keeps_gene_identity_and_order() {
        let genes = parse_monarch_genes(
            br#"[{"subject":{"id":"HGNC:1","symbol":"GENE1","full_name":"Gene one"},"score":0.91},
                 {"subject":{"id":"HGNC:2","symbol":"GENE2"},"score":0.72}]"#,
        )
        .unwrap();
        assert_eq!(genes.len(), 2);
        assert_eq!(genes[0].rank, 1);
        assert_eq!(genes[0].symbol, "GENE1");
        assert_eq!(genes[0].name, "Gene one");
        assert_eq!(genes[1].rank, 2);
        assert_eq!(genes[1].name, "GENE2");
    }

    #[test]
    fn patient_profiles_accept_only_phenotypic_abnormalities() {
        let knowledge = test_knowledge();
        let result = canonical_terms(
            &knowledge,
            &[PhenotypeTerm {
                id: "HP:0000005".into(),
                label: "Mode of inheritance".into(),
            }],
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn profile_normalization_keeps_the_most_specific_observed_term() {
        let knowledge = test_knowledge();
        let terms = canonical_terms(
            &knowledge,
            &[
                PhenotypeTerm {
                    id: "HP:0001250".into(),
                    label: "Seizure".into(),
                },
                PhenotypeTerm {
                    id: "HP:0002197".into(),
                    label: "Generalized-onset seizure".into(),
                },
            ],
            false,
        )
        .unwrap();
        let normalized = normalize_terms(&knowledge, terms, true).unwrap();
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].id, "HP:0002197");
    }

    #[test]
    fn an_absent_ancestor_conflicts_with_an_observed_descendant() {
        let knowledge = test_knowledge();
        let observed = vec![term_value(&knowledge, 4)];
        let excluded = vec![term_value(&knowledge, 1)];
        assert!(ensure_consistent_profile(&knowledge, &observed, &excluded).is_err());
    }

    #[test]
    fn report_overlap_uses_exact_carried_alt_literal_pass_and_representative_effect() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-filter-test-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let parquet = root.join("variants.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE variants(
                    allele_id VARCHAR,
                    gene_symbol VARCHAR,
                    filter VARCHAR,
                    impact VARCHAR,
                    alt_index INTEGER,
                    format VARCHAR,
                    samples_json VARCHAR,
                    record_number UBIGINT
                 );
                 INSERT INTO variants VALUES
                    ('allele-1', 'GENE1', 'PASS', 'HIGH', 1, 'GT', '[{"name":"CASE","value":"0/1"}]', 1),
                    ('allele-2', 'GENE1', 'PASS', 'HIGH', 2, 'GT', '[{"name":"CASE","value":"0/1"}]', 1),
                    ('allele-3', 'GENE1', '.', 'HIGH', 1, 'GT', '[{"name":"CASE","value":"0/1"}]', 2),
                    ('allele-4', 'GENE1', 'LowQual', 'MODERATE', 1, 'GT', '[{"name":"CASE","value":"0/1"}]', 3),
                    ('allele-5', 'GENE1', 'pass', 'MODERATE', 1, 'GT', '[{"name":"CASE","value":"0/1"}]', 4);"#,
            )
            .unwrap();
        let destination = parquet
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY variants TO '{destination}' (FORMAT PARQUET)"
            ))
            .unwrap();
        let consequences = root.join("consequences.parquet");
        connection
            .execute_batch(
                "CREATE TABLE consequences(allele_id VARCHAR, gene_symbol VARCHAR, impact VARCHAR);
                 INSERT INTO consequences VALUES ('allele-1', 'GENE2', 'MODERATE');",
            )
            .unwrap();
        let consequence_destination = consequences
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY consequences TO '{consequence_destination}' (FORMAT PARQUET)"
            ))
            .unwrap();
        let genes =
            report_gene_overlap_summary(&root, "run-1", &parquet, Some(&consequences), "CASE")
                .unwrap();
        assert_eq!(genes.len(), 1);
        let summary_gene = genes.iter().find(|gene| gene.symbol == "GENE1").unwrap();
        assert_eq!(summary_gene.variant_count, 4);
        assert_eq!(summary_gene.pass_count, 1);
        assert_eq!(summary_gene.high_impact_count, 1);
        assert_eq!(summary_gene.moderate_impact_count, 0);
        assert!(genes.iter().all(|gene| gene.symbol != "GENE2"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multisample_reports_require_an_explicit_patient_sample() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-sample-test-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let parquet = root.join("variants.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE variants(sample_names_json VARCHAR);
                   INSERT INTO variants VALUES ('["CASE","MOTHER"]');"#,
            )
            .unwrap();
        let destination = parquet
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY variants TO '{destination}' (FORMAT PARQUET)"
            ))
            .unwrap();

        assert_eq!(
            report_sample_names(&parquet).unwrap(),
            vec!["CASE".to_owned(), "MOTHER".to_owned()]
        );
        assert_eq!(resolve_sample_name(&parquet, None).unwrap(), None);
        assert_eq!(
            resolve_sample_name(&parquet, Some("CASE")).unwrap(),
            Some("CASE".to_owned())
        );
        assert!(resolve_sample_name(&parquet, Some("UNKNOWN")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_tables_parse_into_local_disease_profiles() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-test-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let raw = root.join("raw");
        fs::create_dir_all(&raw).unwrap();
        fs::write(
            raw.join("hp.obo"),
            "format-version: 1.2\n\n[Term]\nid: HP:0000001\nname: All\n\n[Term]\nid: HP:0000118\nname: Phenotypic abnormality\nis_a: HP:0000001 ! All\n\n[Term]\nid: HP:0001250\nname: Seizure\nsynonym: \"Convulsion\" EXACT []\nis_a: HP:0000118 ! Phenotypic abnormality\n\n[Term]\nid: HP:0002197\nname: Generalized-onset seizure\nis_a: HP:0001250 ! Seizure\n",
        )
        .unwrap();
        fs::write(
            raw.join("phenotype.hpoa"),
            "database_id\tdisease_name\tqualifier\thpo_id\treference\tevidence\tonset\tfrequency\tsex\tmodifier\taspect\tbiocuration\nOMIM:1\tExample disease\t\tHP:0001250\tPMID:1\tPCS\t\t1/1\t\t\tP\tHPO:test\nOMIM:1\tExample disease\t\tHP:0002197\tPMID:1\tPCS\t\t1/1\t\t\tP\tHPO:test\n",
        )
        .unwrap();
        fs::write(
            raw.join("genes_to_disease.txt"),
            "ncbi_gene_id\tgene_symbol\tassociation_type\tdisease_id\tsource\nNCBIGene:1\tGENE1\tMENDELIAN\tOMIM:1\ttest\n",
        )
        .unwrap();
        let knowledge = load_knowledge_from_root(&root).unwrap();
        assert_eq!(knowledge.active_terms.len(), 2);
        assert_eq!(knowledge.diseases.len(), 1);
        assert_eq!(knowledge.diseases[0].positive.len(), 1);
        assert_eq!(
            knowledge.terms[knowledge.diseases[0].positive[0]].id,
            "HP:0002197"
        );
        assert_eq!(knowledge.diseases[0].genes[0].symbol, "GENE1");
        let _ = fs::remove_dir_all(root);
    }
}
