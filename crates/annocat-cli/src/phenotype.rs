use duckdb::arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::{Connection, params};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
#[cfg(test)]
use std::time::SystemTime;
use std::time::{Duration, UNIX_EPOCH};

const PROFILE_SCHEMA_VERSION: u16 = 4;
const INSTALL_SCHEMA_VERSION: u16 = 1;
const PHENOTYPIC_ABNORMALITY_ROOT: &str = "HP:0000118";
const MAX_PROFILE_TERMS: usize = 500;
const READY_FILENAME: &str = "hpo-ready.json";
const INSTALLED_ASSET_MANIFEST_FILENAME: &str = "hpo-assets.json";
const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PORTABLE_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PORTABLE_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

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
    #[serde(default)]
    pub conditions: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub limit_to_linked_genes: bool,
    #[serde(default)]
    pub active_generation: Option<PhenotypeGeneration>,
    #[serde(default)]
    pub ranking: Option<PhenotypeRanking>,
    #[serde(default)]
    pub monarch_suggestions: Option<MonarchGeneRanking>,
    #[serde(default)]
    pub monarch_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeGeneration {
    pub fingerprint: String,
    pub evidence_file: String,
    pub catalog_file: String,
    #[serde(default)]
    pub matched_gene_count: Option<usize>,
    #[serde(default, skip_serializing)]
    pub candidate_evidence_file: Option<String>,
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
    "hpo-lin-query-v4".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonarchGeneRanking {
    pub provider: String,
    pub provider_url: String,
    pub metric: String,
    #[serde(default)]
    pub generated_at: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileUpdate {
    pub action: String,
    #[serde(default)]
    pub observed: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub excluded: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub conditions: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub limit_to_linked_genes: bool,
    #[serde(default)]
    pub request_monarch_suggestions: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermSearchResult {
    pub id: String,
    pub label: String,
    pub term_type: String,
    pub matched_text: String,
    pub match_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synonym_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TermResolutionRequest {
    pub entries: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermResolution {
    pub entry: String,
    pub matches: Vec<TermSearchResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermResolutionResponse {
    pub recognized: Vec<TermResolution>,
    pub ambiguous: Vec<TermResolution>,
    pub not_recognized: Vec<String>,
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
    #[serde(default)]
    pub mondo_release: Option<String>,
    #[serde(default)]
    pub mondo_term_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HpoAssetManifest {
    schema_version: u16,
    release: String,
    release_url: String,
    #[serde(default)]
    mondo_release: Option<String>,
    #[serde(default)]
    mondo_release_url: Option<String>,
    assets: Vec<HpoAsset>,
}

impl HpoAssetManifest {
    pub(crate) fn release(&self) -> &str {
        &self.release
    }

    pub(crate) fn expected_bytes(&self) -> u64 {
        self.assets.iter().map(|asset| asset.bytes).sum()
    }

    pub(crate) fn version_key(&self) -> String {
        self.mondo_release
            .as_ref()
            .map(|mondo| format!("{}+mondo-{mondo}", self.release))
            .unwrap_or_else(|| self.release.clone())
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
struct ConditionAssociation {
    id: String,
    name: String,
    genes: Vec<GeneAssociation>,
}

#[derive(Debug)]
struct HpoKnowledge {
    terms: Vec<OntologyTerm>,
    term_index: HashMap<String, usize>,
    active_terms: Vec<usize>,
    phenotypic_abnormality_root: usize,
    diseases: Vec<DiseaseProfile>,
    condition_associations: Vec<ConditionAssociation>,
    information_content: Vec<f64>,
    disease_gene_association_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetIntegrityStamp {
    filename: String,
    bytes: u64,
    modified_nanos: u128,
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

pub fn mondo_release(resources: &Path) -> Option<String> {
    installed_asset_manifest(resources)
        .ok()
        .and_then(|manifest| manifest.mondo_release)
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
            installed_status_and_manifest_at(&entry.path())
                .map(|(_, manifest)| manifest.version_key())
        })
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    versions
}

pub fn installed_status(resources: &Path) -> Option<HpoReadyManifest> {
    installed_release(resources).map(|(_, ready, _)| ready)
}

pub(crate) fn verify_assets(resources: &Path) -> Result<serde_json::Value, String> {
    let mut installations = fs::read_dir(resources.join("hpo"))
        .map_err(|error| format!("cannot inspect installed HPO data: {error}"))?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let root = entry.path();
            let manifest = asset_manifest_at(&root).ok()?;
            Some((root, manifest))
        })
        .collect::<Vec<_>>();
    installations.sort_by(|left, right| left.1.version_key().cmp(&right.1.version_key()));
    let (root, manifest) = installations
        .pop()
        .ok_or("Human Phenotype Ontology data is not installed")?;
    let ready: HpoReadyManifest = serde_json::from_slice(
        &fs::read(root.join(READY_FILENAME))
            .map_err(|error| format!("cannot read the HPO ready marker: {error}"))?,
    )
    .map_err(|error| format!("invalid HPO ready marker: {error}"))?;
    if ready.schema_version != INSTALL_SCHEMA_VERSION
        || ready.release != manifest.release
        || ready.asset_bytes != manifest.expected_bytes()
        || ready.mondo_release != manifest.mondo_release
    {
        return Err("HPO ready marker does not match its asset manifest".into());
    }
    for asset in &manifest.assets {
        let path = root.join("raw").join(&asset.filename);
        let actual_bytes = fs::metadata(&path)
            .map_err(|error| format!("cannot read {} metadata: {error}", asset.filename))?
            .len();
        if actual_bytes != asset.bytes {
            return Err(format!(
                "{} size differs from its manifest ({actual_bytes} != {})",
                asset.filename, asset.bytes
            ));
        }
        verify_sha256(&path, &asset.sha256)?;
    }
    Ok(serde_json::json!({
        "sourceId": "hpo",
        "verified": true,
        "scope": "size-and-sha256",
        "release": ready.release,
        "assetCount": manifest.assets.len(),
        "assetBytes": ready.asset_bytes
    }))
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
        || ready.mondo_release != manifest.mondo_release
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
    let mondo_term_count = if manifest.mondo_release.is_some() {
        crate::mondo::validate_file(&raw_root.join("mondo.json"))?
    } else {
        0
    };
    let ready = HpoReadyManifest {
        schema_version: INSTALL_SCHEMA_VERSION,
        release: manifest.release.clone(),
        installed_at: super::annotation::current_timestamp(),
        asset_bytes: expected,
        term_count: knowledge.active_terms.len(),
        disease_count: knowledge.diseases.len(),
        disease_gene_association_count: knowledge.disease_gene_association_count,
        mondo_release: manifest.mondo_release.clone(),
        mondo_term_count,
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
            "Validated {} phenotype terms, {} conditions, and {} disease profiles",
            ready.term_count, ready.mondo_term_count, ready.disease_count
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
            let (score, matched_text, match_kind) = if id == query {
                (0, term.id.clone(), "identifier")
            } else if label == query {
                (2, term.label.clone(), "label")
            } else if label.starts_with(&query) {
                (3, term.label.clone(), "label")
            } else if let Some(synonym) = term
                .synonyms
                .iter()
                .find(|synonym| synonym.eq_ignore_ascii_case(&query))
            {
                (4, synonym.clone(), "synonym")
            } else if label.contains(&query) {
                (5, term.label.clone(), "label")
            } else if term.search_text.contains(&query) {
                let synonym = term
                    .synonyms
                    .iter()
                    .find(|synonym| synonym.to_ascii_lowercase().contains(&query))
                    .cloned()
                    .unwrap_or_else(|| term.label.clone());
                (6, synonym, "synonym")
            } else {
                return None;
            };
            Some((
                score,
                term.label.len(),
                TermSearchResult {
                    id: term.id.clone(),
                    label: term.label.clone(),
                    term_type: "feature".into(),
                    matched_text,
                    match_kind: match_kind.into(),
                    synonym_scope: None,
                    subtype_count: None,
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
    if let Ok(mondo) = crate::mondo::knowledge(&release_root(resources)?) {
        matches.extend(mondo.search(&query, limit).into_iter().map(|item| {
            (
                item.score,
                item.label.len(),
                TermSearchResult {
                    id: item.id,
                    label: item.label,
                    term_type: "condition".into(),
                    matched_text: item.matched_text,
                    match_kind: item.match_kind,
                    synonym_scope: item.synonym_scope,
                    subtype_count: Some(item.subtype_count),
                    synonyms: Vec::new(),
                },
            )
        }));
    }
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

pub fn resolve_terms(
    resources: &Path,
    request: TermResolutionRequest,
) -> Result<TermResolutionResponse, String> {
    if request.entries.len() > MAX_PROFILE_TERMS {
        return Err(format!(
            "A pasted list can contain at most {MAX_PROFILE_TERMS} entries"
        ));
    }
    let mut recognized = Vec::new();
    let mut ambiguous = Vec::new();
    let mut not_recognized = Vec::new();
    for entry in request.entries {
        let entry = entry.trim().to_owned();
        if entry.is_empty() {
            continue;
        }
        let mut matches = search_terms(resources, &entry, 100)?
            .into_iter()
            .filter(|item| {
                matches!(
                    item.match_kind.as_str(),
                    "identifier" | "externalIdentifier" | "label" | "synonym"
                ) && (item.id.eq_ignore_ascii_case(&entry)
                    || item.matched_text.eq_ignore_ascii_case(&entry))
            })
            .collect::<Vec<_>>();
        matches.dedup_by(|left, right| left.id == right.id && left.term_type == right.term_type);
        let resolution = TermResolution {
            entry: entry.clone(),
            matches,
        };
        match resolution.matches.len() {
            0 => not_recognized.push(entry),
            1 => recognized.push(resolution),
            _ => ambiguous.push(resolution),
        }
    }
    Ok(TermResolutionResponse {
        recognized,
        ambiguous,
        not_recognized,
    })
}

pub fn empty_profile(run_id: &str) -> PhenotypeProfile {
    PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: String::new(),
        observed: Vec::new(),
        excluded: Vec::new(),
        conditions: Vec::new(),
        limit_to_linked_genes: false,
        active_generation: None,
        ranking: None,
        monarch_suggestions: None,
        monarch_error: None,
    }
}

pub fn load(runs: &Path, run_id: &str) -> Result<PhenotypeProfile, String> {
    super::library_metadata::validate_run_id(run_id)?;
    let path = profile_path(runs, run_id);
    if !path.exists() {
        return Ok(empty_profile(run_id));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read phenotype profile: {error}"))?;
    if bytes.len() > MAX_PORTABLE_PROFILE_BYTES as usize {
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
    if profile
        .active_generation
        .as_ref()
        .is_some_and(|active| active.candidate_evidence_file.is_some())
    {
        profile.active_generation = None;
    }
    Ok(profile)
}

pub fn profile_json(resources: &Path, profile: &PhenotypeProfile) -> Result<String, String> {
    let manifest = installed_asset_manifest(resources).ok();
    let mut value = serde_json::to_value(profile)
        .map_err(|error| format!("cannot serialize phenotype profile: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or("phenotype profile did not serialize as an object")?;
    object.insert(
        "hpoRelease".into(),
        manifest
            .as_ref()
            .map(|manifest| serde_json::Value::String(manifest.release.clone()))
            .or_else(|| {
                profile
                    .ranking
                    .as_ref()
                    .map(|ranking| serde_json::Value::String(ranking.hpo_release.clone()))
            })
            .unwrap_or(serde_json::Value::Null),
    );
    object.insert(
        "hpoReleaseUrl".into(),
        manifest
            .as_ref()
            .map(|manifest| serde_json::Value::String(manifest.release_url.clone()))
            .unwrap_or(serde_json::Value::Null),
    );
    object.insert(
        "mondoRelease".into(),
        manifest
            .as_ref()
            .and_then(|manifest| manifest.mondo_release.clone())
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    object.insert(
        "mondoReleaseUrl".into(),
        manifest
            .and_then(|manifest| manifest.mondo_release_url)
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    if let Ok(root) = release_root(resources)
        && let Ok(mondo) = crate::mondo::knowledge(&root)
        && let Some(conditions) = object
            .get_mut("conditions")
            .and_then(serde_json::Value::as_array_mut)
    {
        for condition in conditions {
            if let Some(entry) = condition.as_object_mut()
                && let Some(id) = entry.get("id").and_then(serde_json::Value::as_str)
                && let Some(count) = mondo.subtype_count(id)
            {
                entry.insert("subtypeCount".into(), count.into());
            }
        }
    }
    serde_json::to_string(&value).map_err(|error| error.to_string())
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
            let same_profile = existing.observed == observed
                && existing.excluded == excluded
                && existing.conditions == request.conditions
                && existing.limit_to_linked_genes == request.limit_to_linked_genes;
            let profile = PhenotypeProfile {
                schema_version: PROFILE_SCHEMA_VERSION,
                run_id: run_id.to_owned(),
                updated_at: super::annotation::current_timestamp(),
                observed,
                excluded,
                conditions: request.conditions,
                limit_to_linked_genes: request.limit_to_linked_genes,
                active_generation: same_profile.then_some(existing.active_generation).flatten(),
                ranking: same_profile.then_some(existing.ranking).flatten(),
                monarch_suggestions: same_profile
                    .then_some(existing.monarch_suggestions)
                    .flatten(),
                monarch_error: same_profile.then_some(existing.monarch_error).flatten(),
            };
            save(runs, &profile)?;
            Ok(profile)
        }
        "clear" => {
            let path = profile_path(runs, run_id);
            let active = load(runs, run_id)
                .ok()
                .and_then(|profile| profile.active_generation);
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|error| format!("cannot clear phenotype profile: {error}"))?;
            }
            if let Some(active) = active
                && let Some(root) = path.parent()
            {
                let mut names = vec![active.evidence_file, active.catalog_file];
                names.extend(active.candidate_evidence_file);
                for name in names {
                    let _ = fs::remove_file(root.join(name));
                }
            }
            Ok(empty_profile(run_id))
        }
        _ => Err("phenotype action must be save or clear".into()),
    }
}

pub fn apply(
    resources: &Path,
    runs: &Path,
    run_id: &str,
    parquet: &Path,
    request: ProfileUpdate,
) -> Result<PhenotypeProfile, String> {
    if request.action != "apply" {
        return Err("phenotype action must be apply".into());
    }
    super::library_metadata::validate_run_id(run_id)?;
    let knowledge = knowledge(resources)?;
    let observed = normalize_terms(
        &knowledge,
        canonical_terms(
            &knowledge,
            &request.observed,
            !request.conditions.is_empty(),
        )?,
        true,
    )?;
    let excluded = normalize_terms(
        &knowledge,
        canonical_terms(&knowledge, &request.excluded, true)?,
        false,
    )?;
    ensure_consistent_profile(&knowledge, &observed, &excluded)?;
    let root = release_root(resources)?;
    let mondo = (!request.conditions.is_empty())
        .then(|| crate::mondo::knowledge(&root))
        .transpose()?;
    let canonical_conditions = mondo
        .as_ref()
        .map(|knowledge| knowledge.canonical_conditions(&request.conditions))
        .transpose()?
        .unwrap_or_default();
    let conditions = canonical_conditions
        .iter()
        .map(|condition| PhenotypeTerm {
            id: condition.id.clone(),
            label: condition.label.clone(),
        })
        .collect::<Vec<_>>();
    if observed.is_empty() && conditions.is_empty() {
        return Err("Add at least one observed feature or known condition".into());
    }
    if request.request_monarch_suggestions && observed.is_empty() {
        return Err("Add an observed feature before you request Monarch suggestions".into());
    }
    let existing = load(runs, run_id)?;
    let same_observed = existing.observed == observed;
    let observed_indexes = term_indexes(&knowledge, &observed)?;
    let excluded_indexes = term_indexes(&knowledge, &excluded)?;
    let mut diseases = knowledge
        .diseases
        .par_iter()
        .map(|disease| rank_disease(&knowledge, disease, &observed_indexes, &excluded_indexes))
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
    let condition_matches = if let Some(mondo) = &mondo {
        knowledge
            .condition_associations
            .iter()
            .filter_map(|association| {
                let matches = mondo.disease_matches(&canonical_conditions, &association.id);
                (!matches.is_empty()).then(|| (association.id.clone(), matches))
            })
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    if !condition_matches.is_empty() {
        let ranked = diseases
            .iter()
            .map(|disease| disease.disease_id.clone())
            .collect::<HashSet<_>>();
        diseases.extend(
            knowledge
                .condition_associations
                .iter()
                .filter(|association| {
                    condition_matches.contains_key(&association.id)
                        && !ranked.contains(&association.id)
                })
                .map(|association| RankedDisease {
                    phenotype_rank: 0,
                    disease_id: association.id.clone(),
                    disease_name: association.name.clone(),
                    phenotype_score: 0.0,
                    query_coverage: 0.0,
                    conflict_score: 0.0,
                    conflict_frequency_complete: true,
                    matched_phenotypes: Vec::new(),
                    genes: association.genes.clone(),
                    report_overlap: DiseaseReportOverlap::default(),
                }),
        );
    }
    let manifest = installed_asset_manifest(resources)?;
    let mondo_release = manifest.mondo_release.clone();
    let ranking = PhenotypeRanking {
        algorithm_version: default_phenotype_algorithm_version(),
        provider: "Human Phenotype Ontology".into(),
        provider_url: manifest.release_url.clone(),
        hpo_release: manifest.release.clone(),
        metric: "Query-to-disease best-match Lin semantic similarity".into(),
        score_interpretation: "A relative match between the recorded features and each disease profile. This is not a diagnosis or a disease probability.".into(),
        generated_at: super::annotation::current_timestamp(),
        evaluated_diseases: diseases.len(),
        sample_name: None,
        report_gene_count: 0,
        online_enrichment: None,
        online_error: None,
        diseases,
    };
    let generation = publish_gene_evidence(
        runs,
        run_id,
        &super::results::report_gene_identities(parquet)?,
        &observed,
        &excluded,
        &conditions,
        &condition_matches,
        mondo_release.as_deref(),
        &ranking,
    )?;
    let (monarch_suggestions, monarch_error) = if request.request_monarch_suggestions {
        match monarch_gene_ranking(&observed) {
            Ok(suggestions) => (Some(suggestions), None),
            Err(error) => (None, Some(error)),
        }
    } else if same_observed {
        (existing.monarch_suggestions, existing.monarch_error)
    } else {
        (None, None)
    };
    let profile = PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: super::annotation::current_timestamp(),
        observed,
        excluded,
        conditions,
        limit_to_linked_genes: request.limit_to_linked_genes,
        active_generation: Some(generation),
        ranking: None,
        monarch_suggestions,
        monarch_error,
    };
    save(runs, &profile)?;
    Ok(profile)
}

#[derive(Default)]
struct GenePhenotypeSummary {
    hpo_gene_id: String,
    result_gene_id: String,
    symbol: String,
    has_profile: bool,
    observed_feature_linked: bool,
    relevance: f64,
    direct_matches: i64,
    absent_conflict: f64,
    best_condition: String,
    details: String,
    condition_links: BTreeMap<String, GeneConditionLink>,
}

#[derive(Clone)]
struct GeneConditionLink {
    selected_id: String,
    selected_label: String,
    matched_id: String,
    matched_label: String,
    relation: String,
    source_disease_id: String,
    source_disease_name: String,
    association_type: String,
    association_source: String,
}

struct GeneEvidenceRow {
    gene_id: String,
    gene_symbol: String,
    field_path: &'static str,
    value_type: &'static str,
    string_value: Option<String>,
    integer_value: Option<i64>,
    number_value: Option<f64>,
    boolean_value: Option<bool>,
    json_value: Option<String>,
}

fn publish_gene_evidence(
    runs: &Path,
    run_id: &str,
    report_genes: &[(String, String)],
    observed: &[PhenotypeTerm],
    excluded: &[PhenotypeTerm],
    conditions: &[PhenotypeTerm],
    condition_matches: &HashMap<String, Vec<crate::mondo::DiseaseConditionMatch>>,
    mondo_release: Option<&str>,
    ranking: &PhenotypeRanking,
) -> Result<PhenotypeGeneration, String> {
    let mut fingerprint_input = observed
        .iter()
        .map(|term| format!("O:{}", term.id))
        .chain(excluded.iter().map(|term| format!("A:{}", term.id)))
        .chain(conditions.iter().map(|term| format!("C:{}", term.id)))
        .collect::<Vec<_>>();
    fingerprint_input.sort();
    fingerprint_input.push(format!("H:{}", ranking.hpo_release));
    if let Some(release) = mondo_release {
        fingerprint_input.push(format!("M:{release}"));
    }
    fingerprint_input.push(format!("V:{}", ranking.algorithm_version));
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(fingerprint_input.join("\n").as_bytes())
    );
    let short = &fingerprint[..16];
    let evidence_file = format!("phenotype-gene-evidence.{short}.parquet");
    let catalog_file = format!("phenotype-field-catalog.{short}.json");
    let root = profile_path(runs, run_id)
        .parent()
        .ok_or("phenotype profile has no directory")?
        .to_path_buf();
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create phenotype result directory: {error}"))?;
    let evidence_path = root.join(&evidence_file);
    let catalog_path = root.join(&catalog_file);
    if !evidence_path.is_file() {
        write_gene_evidence(
            &evidence_path,
            report_genes,
            !observed.is_empty(),
            condition_matches,
            ranking,
        )?;
    }
    // The catalog is small and can gain presentation dependencies without rebuilding evidence.
    write_gene_catalog(
        &catalog_path,
        &evidence_file,
        &fingerprint,
        &ranking.hpo_release,
        mondo_release,
        &ranking.algorithm_version,
        ranking.provider_url.as_str(),
    )?;
    let matched_gene_count = phenotype_matched_gene_count(&evidence_path)?;
    Ok(PhenotypeGeneration {
        fingerprint,
        evidence_file,
        catalog_file,
        matched_gene_count: Some(matched_gene_count),
        candidate_evidence_file: None,
    })
}

fn phenotype_matched_gene_count(path: &Path) -> Result<usize, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let count: i64 = connection
        .query_row(
            "SELECT count(DISTINCT upper(gene_symbol))
             FROM read_parquet(?)
             WHERE field_path='profileLinked' AND boolean_value=true",
            params![path.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot count phenotype-linked genes: {error}"))?;
    usize::try_from(count).map_err(|_| "phenotype-linked gene count is invalid".into())
}

fn write_gene_evidence(
    path: &Path,
    report_genes: &[(String, String)],
    has_observed_features: bool,
    condition_matches: &HashMap<String, Vec<crate::mondo::DiseaseConditionMatch>>,
    ranking: &PhenotypeRanking,
) -> Result<(), String> {
    let result_gene_ids = report_genes
        .iter()
        .map(|(symbol, gene_id)| (symbol.as_str(), gene_id.as_str()))
        .collect::<HashMap<_, _>>();
    let mut hpo_gene_ids = HashMap::<String, HashSet<String>>::new();
    for gene in ranking
        .diseases
        .iter()
        .flat_map(|disease| disease.genes.iter())
    {
        let symbol = gene.symbol.trim().to_ascii_uppercase();
        let gene_id = gene.gene_id.trim();
        if result_gene_ids.contains_key(symbol.as_str()) && !gene_id.is_empty() {
            hpo_gene_ids
                .entry(symbol)
                .or_default()
                .insert(gene_id.to_owned());
        }
    }
    let mut genes = BTreeMap::<String, GenePhenotypeSummary>::new();
    for disease in &ranking.diseases {
        let direct_matches = disease
            .matched_phenotypes
            .iter()
            .filter(|item| item.direct)
            .count() as i64;
        let details = serde_json::to_string(&json!({
            "conditionId": disease.disease_id,
            "condition": disease.disease_name,
            "matches": disease.matched_phenotypes,
        }))
        .map_err(|error| format!("cannot serialize phenotype evidence details: {error}"))?;
        for gene in &disease.genes {
            let symbol = gene.symbol.trim().to_ascii_uppercase();
            let Some(&result_gene_id) = result_gene_ids.get(symbol.as_str()) else {
                continue;
            };
            let Some(unique_hpo_ids) = hpo_gene_ids
                .get(&symbol)
                .filter(|gene_ids| gene_ids.len() == 1)
            else {
                continue;
            };
            let hpo_gene_id = unique_hpo_ids.iter().next().unwrap();
            let association_type = gene.association_type.to_ascii_uppercase();
            if has_observed_features
                && association_type == "MENDELIAN"
                && !disease.matched_phenotypes.is_empty()
            {
                let observed_feature_linked = genes
                    .get(&symbol)
                    .is_some_and(|current| current.observed_feature_linked)
                    || direct_matches > 0;
                let replace = genes
                    .get(&symbol)
                    .is_none_or(|current| disease.phenotype_score > current.relevance);
                if replace {
                    let condition_links = genes
                        .get(&symbol)
                        .map(|current| current.condition_links.clone())
                        .unwrap_or_default();
                    genes.insert(
                        symbol.clone(),
                        GenePhenotypeSummary {
                            hpo_gene_id: hpo_gene_id.clone(),
                            result_gene_id: result_gene_id.to_owned(),
                            symbol: symbol.clone(),
                            has_profile: true,
                            observed_feature_linked,
                            relevance: disease.phenotype_score,
                            direct_matches,
                            absent_conflict: disease.conflict_score,
                            best_condition: disease.disease_name.clone(),
                            details: details.clone(),
                            condition_links,
                        },
                    );
                } else if direct_matches > 0
                    && let Some(summary) = genes.get_mut(&symbol)
                {
                    summary.observed_feature_linked = true;
                }
            }
            if !matches!(association_type.as_str(), "MENDELIAN" | "POLYGENIC") {
                continue;
            }
            for matched in condition_matches
                .get(&disease.disease_id)
                .into_iter()
                .flatten()
            {
                let summary = genes
                    .entry(symbol.clone())
                    .or_insert_with(|| GenePhenotypeSummary {
                        hpo_gene_id: hpo_gene_id.clone(),
                        result_gene_id: result_gene_id.to_owned(),
                        symbol: symbol.clone(),
                        ..GenePhenotypeSummary::default()
                    });
                let candidate = GeneConditionLink {
                    selected_id: matched.selected_id.clone(),
                    selected_label: matched.selected_label.clone(),
                    matched_id: matched.matched_id.clone(),
                    matched_label: matched.matched_label.clone(),
                    relation: matched.relation.into(),
                    source_disease_id: disease.disease_id.clone(),
                    source_disease_name: disease.disease_name.clone(),
                    association_type: gene.association_type.clone(),
                    association_source: gene.source.clone(),
                };
                summary
                    .condition_links
                    .entry(matched.selected_id.clone())
                    .and_modify(|current| {
                        if current.relation != "Exact condition"
                            && candidate.relation == "Exact condition"
                        {
                            *current = candidate.clone();
                        }
                    })
                    .or_insert(candidate);
            }
        }
    }
    for (symbol, result_gene_id) in report_genes {
        let Some(hpo_gene_id) = hpo_gene_ids
            .get(symbol)
            .filter(|gene_ids| gene_ids.len() == 1)
            .and_then(|gene_ids| gene_ids.iter().next())
        else {
            continue;
        };
        genes
            .entry(symbol.clone())
            .or_insert_with(|| GenePhenotypeSummary {
                hpo_gene_id: hpo_gene_id.clone(),
                result_gene_id: result_gene_id.clone(),
                symbol: symbol.clone(),
                ..GenePhenotypeSummary::default()
            });
    }
    let mut rows = Vec::with_capacity(genes.len() * 10);
    for gene in genes.values() {
        let base = |field_path, value_type| GeneEvidenceRow {
            gene_id: gene.result_gene_id.clone(),
            gene_symbol: gene.symbol.clone(),
            field_path,
            value_type,
            string_value: None,
            integer_value: None,
            number_value: None,
            boolean_value: None,
            json_value: None,
        };
        let condition_count = gene.condition_links.len() as i64;
        let matched_conditions = gene
            .condition_links
            .values()
            .map(|link| format!("{} {}", link.selected_id, link.selected_label))
            .collect::<Vec<_>>()
            .join("; ");
        let condition_relation = if gene
            .condition_links
            .values()
            .any(|link| link.relation == "Exact condition")
        {
            "Exact condition"
        } else if condition_count > 0 {
            "Condition subtype"
        } else {
            ""
        };
        let condition_details = gene
            .condition_links
            .values()
            .map(|link| {
                json!({
                    "selectedConditionId": link.selected_id,
                    "selectedCondition": link.selected_label,
                    "matchedConditionId": link.matched_id,
                    "matchedCondition": link.matched_label,
                    "relation": link.relation,
                    "sourceDiseaseId": link.source_disease_id,
                    "sourceDisease": link.source_disease_name,
                    "associationType": link.association_type,
                    "associationSource": link.association_source,
                })
            })
            .collect::<Vec<_>>();
        let combined_details = serde_json::to_string(&json!({
            "geneResolution": "uniqueGeneSymbol",
            "resultGeneId": gene.result_gene_id,
            "hpoGeneId": gene.hpo_gene_id,
            "bestPhenotypeMatch": serde_json::from_str::<serde_json::Value>(&gene.details)
                .unwrap_or(serde_json::Value::Null),
            "conditionLinks": condition_details,
        }))
        .map_err(|error| format!("cannot serialize phenotype and condition details: {error}"))?;
        rows.push(GeneEvidenceRow {
            number_value: (has_observed_features && gene.has_profile).then_some(gene.relevance),
            ..base("phenotypeRelevance", "number")
        });
        rows.push(GeneEvidenceRow {
            boolean_value: Some(gene.observed_feature_linked || condition_count > 0),
            ..base("profileLinked", "boolean")
        });
        rows.push(GeneEvidenceRow {
            boolean_value: Some(gene.observed_feature_linked),
            ..base("observedFeatureLinked", "boolean")
        });
        rows.push(GeneEvidenceRow {
            string_value: (has_observed_features && gene.has_profile)
                .then(|| gene.best_condition.clone()),
            ..base("bestMatchingCondition", "text")
        });
        rows.push(GeneEvidenceRow {
            integer_value: Some(gene.direct_matches),
            ..base("directFeatureMatches", "integer")
        });
        rows.push(GeneEvidenceRow {
            number_value: (has_observed_features && gene.has_profile)
                .then_some(gene.absent_conflict),
            ..base("absentFeatureConflict", "number")
        });
        rows.push(GeneEvidenceRow {
            integer_value: Some(condition_count),
            ..base("selectedConditionMatches", "integer")
        });
        rows.push(GeneEvidenceRow {
            string_value: Some(matched_conditions),
            ..base("matchedSelectedConditions", "text")
        });
        rows.push(GeneEvidenceRow {
            string_value: Some(condition_relation.into()),
            ..base("selectedConditionRelation", "text")
        });
        rows.push(GeneEvidenceRow {
            json_value: Some(combined_details),
            ..base("phenotypeEvidenceDetails", "json")
        });
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("gene_id", DataType::Utf8, false),
        Field::new("gene_symbol", DataType::Utf8, false),
        Field::new("scope", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("field_path", DataType::Utf8, false),
        Field::new("value_type", DataType::Utf8, false),
        Field::new("string_value", DataType::Utf8, true),
        Field::new("integer_value", DataType::Int64, true),
        Field::new("number_value", DataType::Float64, true),
        Field::new("boolean_value", DataType::Boolean, true),
        Field::new("json_value", DataType::Utf8, true),
    ]));
    let len = rows.len();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.gene_id.as_str())
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.gene_symbol.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec!["gene"; len])),
            Arc::new(StringArray::from(vec!["hpo"; len])),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.field_path).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.value_type).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.string_value.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                rows.iter().map(|row| row.integer_value).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.number_value).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|row| row.boolean_value).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.json_value.as_deref())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|error| format!("cannot build phenotype evidence batch: {error}"))?;
    let temporary = path.with_extension("parquet.part");
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();
    let mut writer = ArrowWriter::try_new(
        File::create(&temporary)
            .map_err(|error| format!("cannot create phenotype evidence: {error}"))?,
        schema,
        Some(properties),
    )
    .map_err(|error| format!("cannot create phenotype evidence writer: {error}"))?;
    writer
        .write(&batch)
        .and_then(|_| writer.close())
        .map_err(|error| format!("cannot write phenotype evidence: {error}"))?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let written: i64 = connection
        .query_row(
            "SELECT count(*) FROM read_parquet(?)",
            params![temporary.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot verify phenotype evidence: {error}"))?;
    if written != len as i64 {
        let _ = fs::remove_file(&temporary);
        return Err("phenotype evidence row count changed while writing".into());
    }
    super::library_metadata::publish_atomic_file(&temporary, path)
}

fn write_gene_catalog(
    path: &Path,
    evidence_file: &str,
    fingerprint: &str,
    hpo_release: &str,
    mondo_release: Option<&str>,
    algorithm_version: &str,
    provider_url: &str,
) -> Result<(), String> {
    let definitions = [
        ("phenotypeRelevance", "number", "Phenotype relevance", true),
        ("profileLinked", "boolean", "Profile link", false),
        (
            "observedFeatureLinked",
            "boolean",
            "Observed feature link",
            false,
        ),
        (
            "bestMatchingCondition",
            "text",
            "Best matching condition",
            false,
        ),
        (
            "directFeatureMatches",
            "integer",
            "Direct feature matches",
            false,
        ),
        (
            "absentFeatureConflict",
            "number",
            "Absent feature conflict",
            false,
        ),
        (
            "selectedConditionMatches",
            "integer",
            "Condition matches",
            false,
        ),
        (
            "matchedSelectedConditions",
            "text",
            "Matched selected conditions",
            false,
        ),
        (
            "selectedConditionRelation",
            "text",
            "Selected condition relation",
            false,
        ),
        (
            "phenotypeEvidenceDetails",
            "json",
            "Phenotype evidence details",
            false,
        ),
    ];
    let fields = definitions
        .iter()
        .map(|(field, value_type, label, recommended)| {
            json!({
                "scope": "gene",
                "physicalScope": "gene",
                "biologicalScope": "gene",
                "sourceId": "hpo",
                "fieldPath": field,
                "valueType": value_type,
                "label": label,
                "recommended": recommended,
                "selectable": *field == "phenotypeRelevance",
                "storageRelation": "geneEvidence",
                "resolutionPolicy": "geneDirect",
                "columnPresentation": (*field == "phenotypeRelevance").then_some("profileEvidence"),
                "presentationDependencies": (*field == "phenotypeRelevance").then_some([
                    "selectedConditionMatches",
                    "matchedSelectedConditions",
                    "selectedConditionRelation",
                    "directFeatureMatches",
                    "absentFeatureConflict",
                    "phenotypeEvidenceDetails"
                ]),
            })
        })
        .collect::<Vec<_>>();
    let catalog = json!({
        "schemaVersion": 1,
        "geneEvidenceFile": evidence_file,
        "profileFingerprint": fingerprint,
        "hpoRelease": hpo_release,
        "mondoRelease": mondo_release,
        "algorithmVersion": algorithm_version,
        "sources": [{
            "id": "hpo",
            "name": "Phenotype and condition knowledge",
            "providerUrl": provider_url
        }],
        "fields": fields,
    });
    super::library_metadata::atomic_write(
        path,
        &serde_json::to_vec_pretty(&catalog).map_err(|error| error.to_string())?,
    )
}

pub fn active_query_assets(
    runs: &Path,
    run_id: &str,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let profile = load(runs, run_id)?;
    let Some(active) = profile.active_generation else {
        return Ok(None);
    };
    let profile = profile_path(runs, run_id);
    let root = profile
        .parent()
        .ok_or("phenotype profile has no directory")?;
    let evidence = root.join(active.evidence_file);
    let catalog = root.join(active.catalog_file);
    if !evidence.is_file() || !catalog.is_file() {
        return Err("active phenotype evidence is incomplete".into());
    }
    Ok(Some((evidence, catalog)))
}

fn monarch_gene_ranking(observed: &[PhenotypeTerm]) -> Result<MonarchGeneRanking, String> {
    let service = annocat_core::source_catalog::service("monarch-phenotype-gene-ranking")
        .ok_or("Monarch gene-suggestion service is not configured")?;
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
        generated_at: super::annotation::current_timestamp(),
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
    let response: serde_json::Value = serde_json::from_slice(response)
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
        report_overlap: DiseaseReportOverlap::default(),
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
    let condition_associations = diseases
        .iter()
        .filter(|disease| !disease.genes.is_empty())
        .map(|disease| ConditionAssociation {
            id: disease.id.clone(),
            name: disease.name.clone(),
            genes: disease.genes.clone(),
        })
        .collect();
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
        condition_associations,
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
        let disease_id = value("disease_id");
        if disease_id.is_empty() {
            continue;
        }
        let symbol = value("gene_symbol");
        if symbol.is_empty() {
            continue;
        }
        let disease = diseases
            .entry(disease_id.to_owned())
            .or_insert_with(|| DiseaseBuilder {
                id: disease_id.to_owned(),
                name: disease_id.to_owned(),
                positive: Vec::new(),
                negative: Vec::new(),
                annotations: HashMap::new(),
                genes: Vec::new(),
            });
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
    for term in &profile.conditions {
        if !term.id.starts_with("MONDO:")
            || term.id[6..].is_empty()
            || !term.id[6..].bytes().all(|byte| byte.is_ascii_digit())
            || term.label.trim().is_empty()
            || term.label.len() > 300
            || term.label.chars().any(char::is_control)
        {
            return Err("phenotype profile contains an invalid condition".into());
        }
    }
    if let Some(active) = &profile.active_generation {
        for name in [&active.evidence_file, &active.catalog_file] {
            if name.is_empty()
                || name.len() > 180
                || name.contains(['/', '\\'])
                || name.chars().any(char::is_control)
            {
                return Err("phenotype profile contains an invalid active generation".into());
            }
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
    if let Some(suggestions) = &profile.monarch_suggestions {
        validate_monarch_suggestions(suggestions)?;
    }
    if profile.monarch_error.as_ref().is_some_and(|error| {
        error.len() > 2_000 || error.chars().any(|character| character.is_control())
    }) {
        return Err("phenotype profile contains an invalid Monarch error".into());
    }
    ensure_exactly_disjoint(&profile.observed, &profile.excluded)
}

fn validate_monarch_suggestions(suggestions: &MonarchGeneRanking) -> Result<(), String> {
    if suggestions.genes.len() > 100
        || suggestions.provider.trim().is_empty()
        || suggestions.provider.len() > 200
        || !suggestions.provider_url.starts_with("https://")
        || suggestions.provider_url.len() > 1_000
        || suggestions.metric.len() > 300
        || suggestions.generated_at.len() > 100
        || suggestions
            .provider
            .chars()
            .chain(suggestions.provider_url.chars())
            .chain(suggestions.metric.chars())
            .chain(suggestions.generated_at.chars())
            .any(char::is_control)
    {
        return Err("phenotype profile contains invalid Monarch suggestions".into());
    }
    for (index, gene) in suggestions.genes.iter().enumerate() {
        if gene.rank != index + 1
            || gene.gene_id.trim().is_empty()
            || gene.gene_id.len() > 100
            || gene.symbol.trim().is_empty()
            || gene.symbol.len() > 100
            || gene.name.len() > 300
            || !gene.score.is_finite()
            || gene
                .gene_id
                .chars()
                .chain(gene.symbol.chars())
                .chain(gene.name.chars())
                .any(char::is_control)
        {
            return Err("phenotype profile contains invalid Monarch suggestions".into());
        }
    }
    Ok(())
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

pub(crate) fn packaged_assets(
    runs: &Path,
    run_id: &str,
) -> Result<Vec<(String, &'static str, PathBuf)>, String> {
    let profile_file = profile_path(runs, run_id);
    if !profile_file.is_file() {
        return Ok(Vec::new());
    }
    let profile_bytes = fs::read(&profile_file)
        .map_err(|error| format!("cannot read phenotype profile: {error}"))?;
    let profile = load(runs, run_id)?;
    let Some(active) = profile.active_generation.as_ref() else {
        return Ok(Vec::new());
    };
    let root = profile_file
        .parent()
        .ok_or("phenotype profile has no directory")?;
    let evidence = root.join(&active.evidence_file);
    let catalog = root.join(&active.catalog_file);
    let catalog_bytes = fs::read(&catalog)
        .map_err(|error| format!("cannot read phenotype field catalog: {error}"))?;
    validate_portable_metadata(
        &profile_bytes,
        &catalog_bytes,
        run_id,
        &active.evidence_file,
        &active.catalog_file,
        None,
    )?;
    if !evidence.is_file() {
        return Err("active phenotype evidence is missing".into());
    }
    let assets = vec![
        ("phenotypes.json".into(), "phenotype-profile", profile_file),
        (
            active.evidence_file.clone(),
            "phenotype-gene-evidence",
            evidence,
        ),
        (
            active.catalog_file.clone(),
            "phenotype-field-catalog",
            catalog,
        ),
    ];
    Ok(assets)
}

pub(crate) fn validate_portable_metadata(
    profile_bytes: &[u8],
    catalog_bytes: &[u8],
    run_id: &str,
    evidence_file: &str,
    catalog_file: &str,
    candidate_file: Option<&str>,
) -> Result<PhenotypeProfile, String> {
    if candidate_file.is_some() {
        return Err("phenotype candidate ranking data is no longer supported".into());
    }
    if profile_bytes.is_empty() || profile_bytes.len() > MAX_PORTABLE_PROFILE_BYTES as usize {
        return Err("phenotype profile has an invalid size".into());
    }
    if catalog_bytes.is_empty() || catalog_bytes.len() > MAX_PORTABLE_CATALOG_BYTES as usize {
        return Err("phenotype field catalog has an invalid size".into());
    }
    let profile: PhenotypeProfile = serde_json::from_slice(profile_bytes)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    validate_profile(&profile, run_id)?;
    let active = profile
        .active_generation
        .as_ref()
        .ok_or("portable phenotype profile has no active evidence")?;
    if active.evidence_file != evidence_file
        || active.catalog_file != catalog_file
        || active.candidate_evidence_file.is_some()
    {
        return Err("phenotype profile does not name the packaged evidence files".into());
    }
    if active.fingerprint.len() != 64
        || !active
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("phenotype profile has an invalid fingerprint".into());
    }
    let short = &active.fingerprint[..16];
    if active.evidence_file != format!("phenotype-gene-evidence.{short}.parquet")
        || active.catalog_file != format!("phenotype-field-catalog.{short}.json")
    {
        return Err("phenotype generation filenames do not match its fingerprint".into());
    }
    let catalog: serde_json::Value = serde_json::from_slice(catalog_bytes)
        .map_err(|error| format!("invalid phenotype field catalog: {error}"))?;
    if catalog["schemaVersion"] != 1
        || catalog["geneEvidenceFile"].as_str() != Some(evidence_file)
        || !catalog["candidateEvidenceFile"].is_null()
        || catalog["profileFingerprint"].as_str() != Some(&active.fingerprint)
        || catalog["algorithmVersion"].as_str() != Some("hpo-lin-query-v4")
        || catalog["hpoRelease"]
            .as_str()
            .is_none_or(|release| release.is_empty() || release.len() > 100)
        || (!profile.conditions.is_empty()
            && catalog["mondoRelease"]
                .as_str()
                .is_none_or(|release| release.is_empty() || release.len() > 100))
    {
        return Err("phenotype field catalog does not match its profile".into());
    }
    Ok(profile)
}

pub(crate) fn install_portable_group(
    runs: &Path,
    run_id: &str,
    profile_file: &Path,
    evidence_file: &Path,
    catalog_file: &Path,
    candidate_file: Option<&Path>,
) -> Result<(), String> {
    let profile_bytes = fs::read(profile_file)
        .map_err(|error| format!("cannot read phenotype profile: {error}"))?;
    let catalog_bytes = fs::read(catalog_file)
        .map_err(|error| format!("cannot read phenotype field catalog: {error}"))?;
    let profile = validate_portable_metadata(
        &profile_bytes,
        &catalog_bytes,
        run_id,
        evidence_file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("phenotype evidence has an invalid filename")?,
        catalog_file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("phenotype field catalog has an invalid filename")?,
        candidate_file
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("phenotype candidate evidence has an invalid filename")
            })
            .transpose()?,
    )?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .prepare(
            "SELECT gene_id, gene_symbol, scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?) LIMIT 0",
        )
        .and_then(|mut statement| {
            statement
                .query(params![evidence_file.to_string_lossy().as_ref()])
                .map(|_| ())
        })
        .map_err(|error| format!("invalid phenotype gene evidence: {error}"))?;
    let root = profile_path(runs, run_id)
        .parent()
        .ok_or("phenotype profile has no directory")?
        .to_path_buf();
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create phenotype result directory: {error}"))?;
    let active = profile.active_generation.as_ref().unwrap();
    let evidence_target = root.join(&active.evidence_file);
    let catalog_target = root.join(&active.catalog_file);
    copy_portable_file(evidence_file, &evidence_target)?;
    if let Err(error) = copy_portable_file(catalog_file, &catalog_target) {
        let _ = fs::remove_file(&evidence_target);
        return Err(error);
    }
    if let Err(error) =
        super::library_metadata::atomic_write(&profile_path(runs, run_id), &profile_bytes)
    {
        let _ = fs::remove_file(&evidence_target);
        let _ = fs::remove_file(&catalog_target);
        return Err(error);
    }
    let _ = fs::remove_file(profile_file);
    Ok(())
}

fn copy_portable_file(source: &Path, destination: &Path) -> Result<(), String> {
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("portable phenotype file has an invalid filename")?;
    let temporary =
        destination.with_file_name(format!(".{filename}.import-part-{}", std::process::id()));
    let result = (|| {
        let mut input = BufReader::new(
            File::open(source)
                .map_err(|error| format!("cannot open imported phenotype file: {error}"))?,
        );
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("cannot stage imported phenotype file: {error}"))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("cannot copy imported phenotype file: {error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("cannot flush imported phenotype file: {error}"))?;
        drop(output);
        super::library_metadata::publish_atomic_file(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    } else {
        let _ = fs::remove_file(source);
    }
    result
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
        || !matches!(manifest.assets.len(), 3 | 4)
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
    match (
        manifest.mondo_release.as_deref(),
        manifest.mondo_release_url.as_deref(),
    ) {
        (None, None) if manifest.assets.len() == 3 => {}
        (Some(release), Some(release_url))
            if valid_hpo_release_version(release) && manifest.assets.len() == 4 =>
        {
            let tag = format!("v{release}");
            if release_url
                != format!("https://github.com/monarch-initiative/mondo/releases/tag/{tag}")
            {
                return Err("MONDO release metadata failed validation".into());
            }
            let matching = manifest
                .assets
                .iter()
                .filter(|asset| {
                    asset.kind == "condition-ontology" && asset.filename == "mondo.json"
                })
                .collect::<Vec<_>>();
            let expected_url = format!(
                "https://github.com/monarch-initiative/mondo/releases/download/{tag}/mondo.json"
            );
            if matching.len() != 1 || matching[0].url != expected_url {
                return Err("phenotype knowledge is missing the official MONDO asset".into());
            }
        }
        _ => return Err("MONDO release metadata is incomplete".into()),
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
        mondo_release: None,
        mondo_release_url: None,
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
    let mut manifest = parse_github_release(&bytes)?;
    let mondo = crate::mondo::resolve_latest_asset_manifest()?;
    let asset = mondo.asset();
    manifest.mondo_release = Some(mondo.release().to_owned());
    manifest.mondo_release_url = Some(mondo.release_url().to_owned());
    manifest.assets.push(HpoAsset {
        kind: asset.kind.clone(),
        filename: asset.filename.clone(),
        url: asset.url.clone(),
        bytes: asset.bytes,
        sha256: asset.sha256.clone(),
    });
    validate_asset_manifest(&manifest)?;
    Ok(manifest)
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
    Ok(verify_sha256(path, &asset.sha256).is_ok())
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
            condition_associations: Vec::new(),
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
                mondo_release: None,
                mondo_release_url: None,
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
                    mondo_release: None,
                    mondo_term_count: 0,
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
            mondo_release: None,
            mondo_release_url: None,
            assets: vec![HpoAsset {
                kind: "test".into(),
                filename: "asset.txt".into(),
                url: "https://example.test/asset.txt".into(),
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            }],
        };
        assert!(!verified_installation(&root, &manifest));
        assert!(!verified_asset(&root.join("raw").join("asset.txt"), &manifest.assets[0]).unwrap());
        assert_eq!(
            fs::read(root.join("raw").join("asset.txt")).unwrap(),
            b"abd",
            "a rolling update must keep the installed asset until its replacement is verified"
        );
        fs::write(root.join("raw").join("asset.txt"), b"abc").unwrap();
        assert!(verified_installation(&root, &manifest));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn patient_to_disease_similarity_prefers_the_better_query_match() {
        let knowledge = test_knowledge();
        let matching = rank_disease(&knowledge, &disease("matching", &[1, 2]), &[1, 2], &[]);
        let partial = rank_disease(&knowledge, &disease("partial", &[1, 3]), &[1, 2], &[]);
        assert!(matching.phenotype_score > partial.phenotype_score);
        assert_eq!(matching.query_coverage, 100.0);
    }

    #[test]
    fn unrecorded_disease_findings_do_not_lower_an_exact_query_match() {
        let knowledge = test_knowledge();
        let ranked = rank_disease(
            &knowledge,
            &disease("exact-with-additional-findings", &[1, 2, 3]),
            &[1],
            &[],
        );
        assert_eq!(ranked.phenotype_score, 100.0);
        assert_eq!(ranked.query_coverage, 100.0);
    }

    #[test]
    fn explicitly_absent_terms_are_reported_separately_from_similarity() {
        let knowledge = test_knowledge();
        let profile = disease("conflict", &[1, 2]);
        let without_absent = rank_disease(&knowledge, &profile, &[1], &[]);
        let with_absent = rank_disease(&knowledge, &profile, &[1], &[2]);
        assert_eq!(with_absent.phenotype_score, without_absent.phenotype_score);
        assert_eq!(with_absent.conflict_score, 100.0);
        assert!(!with_absent.conflict_frequency_complete);
    }

    #[test]
    fn absent_feature_conflict_respects_reported_disease_frequency() {
        let knowledge = test_knowledge();
        let mut profile = disease("frequency-weighted-conflict", &[1, 2]);
        profile.annotations.insert(
            2,
            DiseasePhenotypeContext {
                frequency_probability: Some(0.17),
                ..DiseasePhenotypeContext::default()
            },
        );
        let ranked = rank_disease(&knowledge, &profile, &[1], &[2]);
        assert!((ranked.conflict_score - 17.0).abs() < f64::EPSILON);
        assert!(ranked.conflict_frequency_complete);
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
    fn monarch_suggestions_require_safe_ordered_provider_data() {
        let mut suggestions = MonarchGeneRanking {
            provider: "Monarch Initiative".into(),
            provider_url: "https://monarchinitiative.org/".into(),
            metric: "Ancestor information content, bidirectional".into(),
            generated_at: "2026-07-31T00:00:00Z".into(),
            result_limit: 50,
            genes: vec![MonarchRankedGene {
                rank: 1,
                gene_id: "HGNC:1".into(),
                symbol: "GENE1".into(),
                name: "Gene one".into(),
                score: 0.91,
            }],
        };
        assert!(validate_monarch_suggestions(&suggestions).is_ok());
        suggestions.provider_url = "javascript:alert(1)".into();
        assert!(validate_monarch_suggestions(&suggestions).is_err());
    }

    #[test]
    fn clearing_a_profile_removes_its_active_generation() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-hpo-clear-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = runs.join(".annocat-library").join("run-1");
        fs::create_dir_all(&root).unwrap();
        let evidence = root.join("phenotype-gene-evidence.test.parquet");
        let catalog = root.join("phenotype-field-catalog.test.json");
        fs::write(&evidence, b"evidence").unwrap();
        fs::write(&catalog, b"catalog").unwrap();
        let mut profile = empty_profile("run-1");
        profile.active_generation = Some(PhenotypeGeneration {
            fingerprint: "test".into(),
            evidence_file: evidence.file_name().unwrap().to_string_lossy().into_owned(),
            catalog_file: catalog.file_name().unwrap().to_string_lossy().into_owned(),
            matched_gene_count: None,
            candidate_evidence_file: None,
        });
        save(&runs, &profile).unwrap();

        update(
            &runs,
            &runs,
            "run-1",
            ProfileUpdate {
                action: "clear".into(),
                observed: Vec::new(),
                excluded: Vec::new(),
                conditions: Vec::new(),
                limit_to_linked_genes: false,
                request_monarch_suggestions: false,
            },
        )
        .unwrap();

        assert!(!profile_path(&runs, "run-1").exists());
        assert!(!evidence.exists());
        assert!(!catalog.exists());
        fs::remove_dir_all(runs).unwrap();
    }

    #[test]
    fn loading_a_profile_invalidates_legacy_candidate_evidence() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-hpo-stale-candidates-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = runs.join(".annocat-library").join("run-1");
        fs::create_dir_all(&root).unwrap();
        let mut profile = empty_profile("run-1");
        profile.active_generation = Some(PhenotypeGeneration {
            fingerprint: "test".into(),
            evidence_file: "phenotype-gene-evidence.test.parquet".into(),
            catalog_file: "phenotype-field-catalog.test.json".into(),
            matched_gene_count: None,
            candidate_evidence_file: None,
        });
        let mut value = serde_json::to_value(profile).unwrap();
        value["activeGeneration"]["candidateEvidenceFile"] =
            "phenotype-candidate-evidence.test.parquet".into();
        fs::write(
            profile_path(&runs, "run-1"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        assert!(load(&runs, "run-1").unwrap().active_generation.is_none());
        fs::remove_dir_all(runs).unwrap();
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
    fn phenotype_catalog_exposes_only_the_composite_column() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-catalog-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("phenotype-field-catalog.test.json");
        write_gene_catalog(
            &path,
            "phenotype-gene-evidence.test.parquet",
            "test",
            "hpo-test",
            Some("mondo-test"),
            "test",
            "https://example.test",
        )
        .unwrap();
        let catalog: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let fields = catalog["fields"].as_array().unwrap();
        assert_eq!(
            fields
                .iter()
                .filter(|field| field["selectable"] == true)
                .map(|field| field["fieldPath"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["phenotypeRelevance"]
        );
        assert!(
            fields
                .iter()
                .filter(|field| field["fieldPath"] != "phenotypeRelevance")
                .all(|field| field["selectable"] == false)
        );
        let composite = fields
            .iter()
            .find(|field| field["fieldPath"] == "phenotypeRelevance")
            .unwrap();
        assert!(
            composite["presentationDependencies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "phenotypeEvidenceDetails")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn condition_only_links_do_not_create_a_similarity_score() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-gene-evidence-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("gene-evidence.parquet");
        let ranking = PhenotypeRanking {
            algorithm_version: default_phenotype_algorithm_version(),
            provider: "Human Phenotype Ontology".into(),
            provider_url: "https://hpo.jax.org/".into(),
            hpo_release: "test".into(),
            metric: "test".into(),
            score_interpretation: String::new(),
            generated_at: String::new(),
            evaluated_diseases: 1,
            sample_name: None,
            report_gene_count: 1,
            online_enrichment: None,
            online_error: None,
            diseases: vec![RankedDisease {
                phenotype_rank: 1,
                disease_id: "OMIM:1".into(),
                disease_name: "Example condition".into(),
                phenotype_score: 0.0,
                query_coverage: 0.0,
                conflict_score: 0.0,
                conflict_frequency_complete: true,
                matched_phenotypes: Vec::new(),
                genes: vec![GeneAssociation {
                    gene_id: "1".into(),
                    symbol: "GENE1".into(),
                    association_type: "MENDELIAN".into(),
                    source: "HPO".into(),
                }],
                report_overlap: DiseaseReportOverlap::default(),
            }],
        };
        let condition_matches = HashMap::from([(
            "OMIM:1".into(),
            vec![crate::mondo::DiseaseConditionMatch {
                selected_id: "MONDO:0000001".into(),
                selected_label: "Selected condition".into(),
                matched_id: "MONDO:0000001".into(),
                matched_label: "Selected condition".into(),
                relation: "Exact condition",
            }],
        )]);
        write_gene_evidence(
            &path,
            &[("GENE1".into(), "ENSG1".into())],
            false,
            &condition_matches,
            &ranking,
        )
        .unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let parquet = path.to_string_lossy();
        let profile_link: bool = connection
            .query_row(
                "SELECT boolean_value FROM read_parquet(?) WHERE field_path='profileLinked'",
                params![parquet.as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        let condition_matches: i64 = connection
            .query_row(
                "SELECT integer_value FROM read_parquet(?) WHERE field_path='selectedConditionMatches'",
                params![parquet.as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        let condition_label: String = connection
            .query_row(
                "SELECT string_value FROM read_parquet(?) WHERE field_path='matchedSelectedConditions'",
                params![parquet.as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        let relevance_count: i64 = connection
            .query_row(
                "SELECT count(number_value) FROM read_parquet(?) WHERE field_path='phenotypeRelevance'",
                params![parquet.as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(profile_link);
        assert_eq!(condition_matches, 1);
        assert_eq!(condition_label, "MONDO:0000001 Selected condition");
        assert_eq!(relevance_count, 0);
        assert_eq!(phenotype_matched_gene_count(&path).unwrap(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gene_evidence_uses_association_types_for_their_documented_purpose() {
        let root = std::env::temp_dir().join(format!(
            "annocat-hpo-association-types-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("gene-evidence.parquet");
        let ranking = PhenotypeRanking {
            algorithm_version: default_phenotype_algorithm_version(),
            provider: "Human Phenotype Ontology".into(),
            provider_url: "https://hpo.jax.org/".into(),
            hpo_release: "test".into(),
            metric: "test".into(),
            score_interpretation: String::new(),
            generated_at: String::new(),
            evaluated_diseases: 1,
            sample_name: None,
            report_gene_count: 3,
            online_enrichment: None,
            online_error: None,
            diseases: vec![RankedDisease {
                phenotype_rank: 1,
                disease_id: "OMIM:1".into(),
                disease_name: "Example condition".into(),
                phenotype_score: 80.0,
                query_coverage: 100.0,
                conflict_score: 0.0,
                conflict_frequency_complete: true,
                matched_phenotypes: vec![PhenotypeMatch {
                    query: PhenotypeTerm {
                        id: "HP:0001250".into(),
                        label: "Seizure".into(),
                    },
                    disease_term: PhenotypeTerm {
                        id: "HP:0001250".into(),
                        label: "Seizure".into(),
                    },
                    similarity: 1.0,
                    direct: true,
                    disease_annotation: None,
                }],
                genes: vec![
                    GeneAssociation {
                        gene_id: "1".into(),
                        symbol: "MENDEL".into(),
                        association_type: "MENDELIAN".into(),
                        source: "HPO".into(),
                    },
                    GeneAssociation {
                        gene_id: "2".into(),
                        symbol: "POLY".into(),
                        association_type: "POLYGENIC".into(),
                        source: "HPO".into(),
                    },
                    GeneAssociation {
                        gene_id: "3".into(),
                        symbol: "UNKNOWN".into(),
                        association_type: "UNKNOWN".into(),
                        source: "HPO".into(),
                    },
                ],
                report_overlap: DiseaseReportOverlap::default(),
            }],
        };
        let condition_matches = HashMap::from([(
            "OMIM:1".into(),
            vec![crate::mondo::DiseaseConditionMatch {
                selected_id: "MONDO:0000001".into(),
                selected_label: "Selected condition".into(),
                matched_id: "MONDO:0000001".into(),
                matched_label: "Selected condition".into(),
                relation: "Exact condition",
            }],
        )]);
        write_gene_evidence(
            &path,
            &[
                ("MENDEL".into(), "ENSG1".into()),
                ("POLY".into(), "ENSG2".into()),
                ("UNKNOWN".into(), "ENSG3".into()),
            ],
            true,
            &condition_matches,
            &ranking,
        )
        .unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let parquet = path.to_string_lossy();
        let boolean = |symbol: &str, field: &str| -> bool {
            connection
                .query_row(
                    "SELECT boolean_value FROM read_parquet(?)
                     WHERE gene_symbol=? AND field_path=?",
                    params![parquet.as_ref(), symbol, field],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let integer = |symbol: &str, field: &str| -> i64 {
            connection
                .query_row(
                    "SELECT integer_value FROM read_parquet(?)
                     WHERE gene_symbol=? AND field_path=?",
                    params![parquet.as_ref(), symbol, field],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert!(boolean("MENDEL", "observedFeatureLinked"));
        assert!(!boolean("POLY", "observedFeatureLinked"));
        assert!(!boolean("UNKNOWN", "observedFeatureLinked"));
        assert_eq!(integer("MENDEL", "selectedConditionMatches"), 1);
        assert_eq!(integer("POLY", "selectedConditionMatches"), 1);
        assert_eq!(integer("UNKNOWN", "selectedConditionMatches"), 0);
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
            "ncbi_gene_id\tgene_symbol\tassociation_type\tdisease_id\tsource\nNCBIGene:1\tGENE1\tMENDELIAN\tOMIM:1\ttest\nNCBIGene:2\tGENE2\tMENDELIAN\tOMIM:2\ttest\n",
        )
        .unwrap();
        let knowledge = load_knowledge_from_root(&root).unwrap();
        assert_eq!(knowledge.active_terms.len(), 2);
        assert_eq!(knowledge.diseases.len(), 1);
        assert_eq!(knowledge.condition_associations.len(), 2);
        assert_eq!(knowledge.diseases[0].positive.len(), 1);
        assert_eq!(
            knowledge.terms[knowledge.diseases[0].positive[0]].id,
            "HP:0002197"
        );
        assert_eq!(knowledge.diseases[0].genes[0].symbol, "GENE1");
        let _ = fs::remove_dir_all(root);
    }
}
