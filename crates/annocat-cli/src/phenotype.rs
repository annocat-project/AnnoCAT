use duckdb::types::Value as SqlValue;
use duckdb::{Connection, appender_params_from_iter, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

const PROFILE_SCHEMA_VERSION: u16 = 6;
const ACTIVE_QUERY_CONTRACT_VERSION: &str = "gene-profile-live-v1";
const GENE_SET_ALGORITHM_VERSION: &str = "hpo-association-query-v5";
const PHENOTYPE_RANKING_ALGORITHM_VERSION: &str = "resnik-query-disease-v1";
const INSTALL_SCHEMA_VERSION: u16 = 1;
const PHENOTYPIC_ABNORMALITY_ROOT: &str = "HP:0000118";
const MAX_PROFILE_TERMS: usize = 500;
const MAX_PROFILE_GENES: usize = 30_000;
const READY_FILENAME: &str = "hpo-ready.json";
const INSTALLED_ASSET_MANIFEST_FILENAME: &str = "hpo-assets.json";
const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PORTABLE_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
const SAVED_GENE_LISTS_SCHEMA_VERSION: u16 = 1;
const MAX_SAVED_GENE_LISTS: usize = 100;
const MAX_SAVED_GENE_LISTS_BYTES: u64 = 64 * 1024 * 1024;

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
    pub conditions: Vec<PhenotypeTerm>,
    pub pathways: Vec<PhenotypeTerm>,
    pub genes: Vec<super::gene_identity::ResolvedGene>,
    pub show_matches_only: bool,
    pub active_generation: Option<PhenotypeGeneration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhenotypeGeneration {
    pub fingerprint: String,
    pub matched_gene_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceAsset {
    name: String,
    release: String,
    sha256: String,
}

#[derive(Debug, Clone)]
struct PhenotypeRanking {
    hpo_release: String,
    query_count: usize,
    disease_profile_count: usize,
    denominator: usize,
    genes: BTreeMap<String, RankedGene>,
}

#[derive(Debug, Clone)]
struct RankedGene {
    rank: usize,
    tie_count: usize,
    raw_score: f64,
    score_key: u64,
    best_disease_id: String,
    best_disease_name: String,
    matched_terms: Vec<RankedTermMatch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RankedTermMatch {
    query: PhenotypeTerm,
    disease_term: PhenotypeTerm,
    mica: Option<PhenotypeTerm>,
    eligible_disease_profiles: usize,
    mica_disease_profiles: Option<u64>,
    resnik_similarity: f64,
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
    pub biocuration: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneAssociation {
    pub gene_id: String,
    pub symbol: String,
    pub association_type: String,
    pub source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileUpdate {
    pub action: String,
    #[serde(default)]
    pub observed: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub conditions: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub pathways: Vec<PhenotypeTerm>,
    #[serde(default)]
    pub genes: Vec<super::gene_identity::ResolvedGene>,
    #[serde(default)]
    pub show_matches_only: bool,
    #[serde(default)]
    pub preview_fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenePreviewRow {
    pub symbol: String,
    pub canonical_gene_id: Option<String>,
    pub result_gene_id: Option<String>,
    pub identity_status: String,
    pub sources: Vec<String>,
    pub in_result: bool,
    pub included: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenePreviewResponse {
    pub fingerprint: String,
    pub total_genes: usize,
    pub genes_in_result: usize,
    pub included_genes: usize,
    pub included_genes_in_result: usize,
    pub offset: usize,
    pub has_more: bool,
    pub rows: Vec<GenePreviewRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_included_genes: Option<Vec<super::gene_identity::ResolvedGene>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedGeneList {
    pub name: String,
    pub genes: Vec<PhenotypeTerm>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedGeneListUpdate {
    pub action: String,
    pub name: String,
    #[serde(default)]
    pub genes: Vec<PhenotypeTerm>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedGeneListsFile {
    schema_version: u16,
    lists: Vec<SavedGeneList>,
}

impl Default for SavedGeneListsFile {
    fn default() -> Self {
        Self {
            schema_version: SAVED_GENE_LISTS_SCHEMA_VERSION,
            lists: Vec::new(),
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gene_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_gene_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_gene_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TermResolutionRequest {
    pub entries: Vec<String>,
    #[serde(default)]
    pub run_id: Option<String>,
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

fn saved_gene_lists_path(config: &Path) -> PathBuf {
    config.join("gene-lists.json")
}

pub fn saved_gene_lists(config: &Path) -> Result<Vec<SavedGeneList>, String> {
    let path = saved_gene_lists_path(config);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata =
        fs::metadata(&path).map_err(|error| format!("cannot inspect saved gene lists: {error}"))?;
    if metadata.len() > MAX_SAVED_GENE_LISTS_BYTES {
        return Err("saved gene lists are too large".into());
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read saved gene lists: {error}"))?;
    let file: SavedGeneListsFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("saved gene lists are invalid: {error}"))?;
    if file.schema_version != SAVED_GENE_LISTS_SCHEMA_VERSION {
        return Err("saved gene lists use an unsupported format".into());
    }
    Ok(file.lists)
}

pub fn update_saved_gene_lists(
    config: &Path,
    request: SavedGeneListUpdate,
) -> Result<Vec<SavedGeneList>, String> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("gene list name must contain 1 to 80 printable characters".into());
    }
    let mut lists = saved_gene_lists(config)?;
    match request.action.as_str() {
        "save" => {
            if request.genes.is_empty() {
                return Err("gene list must contain at least one gene".into());
            }
            if request.genes.len() > MAX_PROFILE_GENES {
                return Err(format!(
                    "gene list cannot contain more than {MAX_PROFILE_GENES} genes"
                ));
            }
            let mut genes = BTreeMap::<String, PhenotypeTerm>::new();
            for (index, gene) in request.genes.into_iter().enumerate() {
                let id = gene.id.trim();
                let label = gene.label.trim();
                if id.is_empty()
                    || label.is_empty()
                    || id.chars().count() > 128
                    || label.chars().count() > 64
                    || id.chars().any(char::is_control)
                    || label.chars().any(char::is_control)
                {
                    return Err(format!(
                        "gene list entry {} ({}) has an invalid gene identifier: {}",
                        index + 1,
                        if label.is_empty() {
                            "missing label"
                        } else {
                            label
                        },
                        if id.is_empty() { "missing" } else { id },
                    ));
                }
                genes.insert(
                    label.to_ascii_uppercase(),
                    PhenotypeTerm {
                        id: id.to_string(),
                        label: label.to_string(),
                    },
                );
            }
            let saved = SavedGeneList {
                name: name.to_string(),
                genes: genes.into_values().collect(),
                updated_at: super::annotation::current_timestamp(),
            };
            if let Some(existing) = lists
                .iter_mut()
                .find(|list| list.name.eq_ignore_ascii_case(name))
            {
                *existing = saved;
            } else {
                if lists.len() >= MAX_SAVED_GENE_LISTS {
                    return Err(format!(
                        "no more than {MAX_SAVED_GENE_LISTS} gene lists can be saved"
                    ));
                }
                lists.push(saved);
            }
        }
        "delete" => lists.retain(|list| !list.name.eq_ignore_ascii_case(name)),
        _ => return Err("gene list action must be save or delete".into()),
    }
    lists.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then(left.name.cmp(&right.name))
    });
    let file = SavedGeneListsFile {
        schema_version: SAVED_GENE_LISTS_SCHEMA_VERSION,
        lists: lists.clone(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| format!("cannot serialize saved gene lists: {error}"))?;
    bytes.push(b'\n');
    super::library_metadata::atomic_write(&saved_gene_lists_path(config), &bytes)?;
    Ok(lists)
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
    #[serde(default)]
    pub hgnc_release: Option<String>,
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
    #[serde(default)]
    hgnc_release: Option<String>,
    #[serde(default)]
    hgnc_release_url: Option<String>,
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
        let mut version = self.release.clone();
        if let Some(mondo) = &self.mondo_release {
            version.push_str(&format!("+mondo-{mondo}"));
        }
        if let Some(hgnc) = &self.hgnc_release {
            version.push_str(&format!("+hgnc-{hgnc}"));
        }
        version
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HpoAsset {
    kind: String,
    filename: String,
    url: String,
    bytes: u64,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    md5: Option<String>,
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
    annotations: HashMap<usize, DiseasePhenotypeContext>,
    genes: Vec<GeneAssociation>,
}

#[derive(Debug)]
struct DiseaseBuilder {
    id: String,
    name: String,
    positive: Vec<usize>,
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

pub(crate) fn gene_identity_files(resources: &Path) -> Option<(PathBuf, PathBuf, String)> {
    let (root, _, manifest) = installed_release(resources)?;
    let release = manifest.hgnc_release?;
    let complete = manifest
        .assets
        .iter()
        .find(|asset| asset.kind == "gene-identities")?;
    let withdrawn = manifest
        .assets
        .iter()
        .find(|asset| asset.kind == "withdrawn-gene-identities")?;
    Some((
        root.join("raw").join(&complete.filename),
        root.join("raw").join(&withdrawn.filename),
        release,
    ))
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
        || ready.hgnc_release != manifest.hgnc_release
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
        verify_asset_checksum(&path, asset)?;
    }
    Ok(serde_json::json!({
        "sourceId": "hpo",
        "verified": true,
        "scope": "size-and-checksum",
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
        || ready.hgnc_release != manifest.hgnc_release
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
        .any(|asset| verify_asset_checksum(&root.join("raw").join(&asset.filename), asset).is_err())
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
    let hgnc_gene_count = if manifest.hgnc_release.is_some() {
        Some(super::gene_identity::validate_files(
            &raw_root.join("hgnc_complete_set.txt"),
            &raw_root.join("withdrawn.txt"),
        )?)
    } else {
        None
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
        hgnc_release: manifest.hgnc_release.clone(),
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
        detail: if let Some(hgnc_gene_count) = hgnc_gene_count {
            format!(
                "Validated {} phenotype terms, {} conditions, {} disease profiles, and {} gene identities",
                ready.term_count, ready.mondo_term_count, ready.disease_count, hgnc_gene_count
            )
        } else {
            format!(
                "Validated {} phenotype terms, {} conditions, and {} disease profiles",
                ready.term_count, ready.mondo_term_count, ready.disease_count
            )
        },
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
                    gene_count: None,
                    synonyms: term
                        .synonyms
                        .iter()
                        .filter(|synonym| synonym.to_ascii_lowercase().contains(&query))
                        .take(3)
                        .cloned()
                        .collect(),
                    symbol: None,
                    canonical_gene_id: None,
                    result_gene_id: None,
                    identity_status: None,
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
                    gene_count: None,
                    synonyms: Vec::new(),
                    symbol: None,
                    canonical_gene_id: None,
                    result_gene_id: None,
                    identity_status: None,
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
    parquet: Option<&Path>,
    request: TermResolutionRequest,
) -> Result<TermResolutionResponse, String> {
    if request.entries.len() > MAX_PROFILE_GENES {
        return Err(format!(
            "A pasted gene list can contain at most {MAX_PROFILE_GENES} entries"
        ));
    }
    let mut recognized = Vec::new();
    let mut ambiguous = Vec::new();
    let mut not_recognized = Vec::new();
    let report = parquet
        .map(super::results::report_gene_identities)
        .transpose()?
        .unwrap_or_default();
    let gene_resolver = super::gene_identity::Resolver::new(resources, &report);
    for entry in request.entries {
        let entry = entry.trim().to_owned();
        if entry.is_empty() {
            continue;
        }
        let matches = exact_gene_matches(&gene_resolver, &entry);
        let mut matches = matches
            .into_iter()
            .filter(|item| exact_term_match(item, &entry))
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        matches.retain(|item| seen.insert((item.term_type.clone(), item.id.clone())));
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

fn exact_term_match(item: &TermSearchResult, entry: &str) -> bool {
    matches!(
        item.match_kind.as_str(),
        "identifier" | "externalIdentifier" | "label" | "synonym" | "geneIdentifier" | "geneSymbol"
    ) && (item.id.eq_ignore_ascii_case(entry) || item.matched_text.eq_ignore_ascii_case(entry))
}

fn exact_gene_matches(
    resolver: &super::gene_identity::Resolver,
    entry: &str,
) -> Vec<TermSearchResult> {
    let to_result = |matched: super::gene_identity::SearchMatch| TermSearchResult {
        id: matched.gene.result_id(),
        label: matched.gene.symbol.clone(),
        term_type: "gene".into(),
        matched_text: matched.matched_text,
        match_kind: matched.match_kind.into(),
        synonym_scope: None,
        subtype_count: None,
        gene_count: None,
        synonyms: Vec::new(),
        symbol: Some(matched.gene.symbol),
        canonical_gene_id: matched.gene.canonical_gene_id,
        result_gene_id: matched.gene.result_gene_id,
        identity_status: Some(matched.gene.identity_status),
    };
    match resolver.resolve(entry) {
        super::gene_identity::Resolution::Resolved(gene) => vec![TermSearchResult {
            id: gene.result_id(),
            label: gene.symbol.clone(),
            term_type: "gene".into(),
            matched_text: entry.to_owned(),
            match_kind: "geneSymbol".into(),
            synonym_scope: None,
            subtype_count: None,
            gene_count: None,
            synonyms: Vec::new(),
            symbol: Some(gene.symbol),
            canonical_gene_id: gene.canonical_gene_id,
            result_gene_id: gene.result_gene_id,
            identity_status: Some(gene.identity_status),
        }],
        super::gene_identity::Resolution::Ambiguous => resolver
            .search(entry, 100)
            .into_iter()
            .map(to_result)
            .filter(|item| exact_term_match(item, entry))
            .collect(),
        super::gene_identity::Resolution::Unknown => Vec::new(),
    }
}

pub fn search_gene_terms(
    resources: &Path,
    parquet: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<TermSearchResult>, String> {
    let report = super::results::report_gene_identities(parquet)?;
    let resolver = super::gene_identity::Resolver::new(resources, &report);
    Ok(resolver
        .search(query, limit)
        .into_iter()
        .map(|matched| TermSearchResult {
            id: matched.gene.result_id(),
            label: matched.gene.symbol.clone(),
            term_type: "gene".into(),
            matched_text: matched.matched_text,
            match_kind: matched.match_kind.into(),
            synonym_scope: None,
            subtype_count: None,
            gene_count: None,
            synonyms: Vec::new(),
            symbol: Some(matched.gene.symbol),
            canonical_gene_id: matched.gene.canonical_gene_id,
            result_gene_id: matched.gene.result_gene_id,
            identity_status: Some(matched.gene.identity_status),
        })
        .collect())
}

pub fn empty_profile(run_id: &str) -> PhenotypeProfile {
    PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: String::new(),
        observed: Vec::new(),
        conditions: Vec::new(),
        pathways: Vec::new(),
        genes: Vec::new(),
        show_matches_only: false,
        active_generation: None,
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
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    let schema = value["schemaVersion"].as_u64().unwrap_or_default();
    if schema != u64::from(PROFILE_SCHEMA_VERSION) {
        return Err(format!(
            "This result uses unsupported Genes query schema {schema}. Clear it before applying a new Genes query."
        ));
    }
    let profile: PhenotypeProfile = serde_json::from_value(value)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    validate_profile(&profile, run_id)?;
    Ok(profile)
}

fn current_profile_fingerprint(
    resources: &Path,
    profile: &PhenotypeProfile,
) -> Result<String, String> {
    let identity = super::gene_identity::Resolver::new(resources, &[]);
    let source_assets = profile_source_assets(
        resources,
        !profile.observed.is_empty(),
        !profile.conditions.is_empty(),
        !profile.pathways.is_empty(),
        identity.identity_release().is_some(),
    )?;
    Ok(active_query_fingerprint(
        &profile.observed,
        &profile.conditions,
        &profile.pathways,
        &profile.genes,
        &source_assets,
    ))
}

pub fn load_current(
    resources: &Path,
    runs: &Path,
    run_id: &str,
) -> Result<PhenotypeProfile, String> {
    if let Some(root) = profile_path(runs, run_id).parent() {
        remove_legacy_query_files(root);
    }
    let mut profile = load(runs, run_id)?;
    let stale = profile.active_generation.as_ref().is_some_and(|active| {
        current_profile_fingerprint(resources, &profile)
            .map_or(true, |fingerprint| fingerprint != active.fingerprint)
    });
    if stale {
        clear_cached_active_query(run_id);
        profile.active_generation = None;
        profile.show_matches_only = false;
        profile.updated_at = super::annotation::current_timestamp();
        save(runs, &profile)?;
    } else if let Some(active) = profile.active_generation.as_mut()
        && (active.evidence_file.is_some() || active.catalog_file.is_some())
    {
        active.evidence_file = None;
        active.catalog_file = None;
        profile.updated_at = super::annotation::current_timestamp();
        save(runs, &profile)?;
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
            let needs_hpo = !request.observed.is_empty() || !request.conditions.is_empty();
            let hpo = needs_hpo.then(|| knowledge(resources)).transpose()?;
            let observed = hpo
                .as_ref()
                .map(|knowledge| {
                    normalize_terms(
                        knowledge,
                        canonical_terms(knowledge, &request.observed, true)?,
                        true,
                    )
                })
                .transpose()?
                .unwrap_or_default();
            let root = needs_hpo.then(|| release_root(resources)).transpose()?;
            let conditions = if request.conditions.is_empty() {
                Vec::new()
            } else {
                crate::mondo::knowledge(root.as_ref().expect("conditions require HPO data"))?
                    .canonical_conditions(&request.conditions)?
                    .into_iter()
                    .map(|condition| PhenotypeTerm {
                        id: condition.id,
                        label: condition.label,
                    })
                    .collect()
            };
            let pathways = if request.pathways.is_empty() {
                Vec::new()
            } else {
                crate::reactome::knowledge(resources)?
                    .canonical_pathways(&request.pathways)?
                    .into_iter()
                    .map(|pathway| PhenotypeTerm {
                        id: pathway.id,
                        label: pathway.label,
                    })
                    .collect()
            };
            let identity = super::gene_identity::Resolver::new(resources, &[]);
            let genes = canonical_manual_genes(&identity, &request.genes)?;
            let existing = load(runs, run_id).unwrap_or_else(|_| empty_profile(run_id));
            let same_profile = existing.observed == observed
                && existing.conditions == conditions
                && existing.pathways == pathways
                && existing.genes == genes
                && existing.show_matches_only == request.show_matches_only;
            let profile = PhenotypeProfile {
                schema_version: PROFILE_SCHEMA_VERSION,
                run_id: run_id.to_owned(),
                updated_at: super::annotation::current_timestamp(),
                observed,
                conditions,
                pathways,
                genes,
                show_matches_only: request.show_matches_only,
                active_generation: same_profile.then_some(existing.active_generation).flatten(),
            };
            save(runs, &profile)?;
            Ok(profile)
        }
        "clear" => {
            let path = profile_path(runs, run_id);
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|error| format!("cannot clear phenotype profile: {error}"))?;
            }
            clear_cached_active_query(run_id);
            if let Some(root) = path.parent() {
                remove_legacy_query_files(root);
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
    if !request.show_matches_only {
        return Err("Applied Genes queries must show matching variants only".into());
    }
    super::library_metadata::validate_run_id(run_id)?;
    let prepared = prepare_gene_profile(resources, parquet, &request, true)?;
    let expected_fingerprint = active_query_fingerprint(
        &prepared.observed,
        &prepared.conditions,
        &prepared.pathways,
        &prepared.genes,
        &prepared.source_assets,
    );
    if request.preview_fingerprint.as_deref() != Some(expected_fingerprint.as_str()) {
        return Err("The resolved gene list changed. Review it again before you apply it.".into());
    }
    let resolved = &prepared.resolved;
    let matched_gene_count = resolved
        .included
        .iter()
        .filter(|key| resolved.result_identities.contains(*key))
        .count();
    require_result_overlap(matched_gene_count)?;
    let query = build_active_gene_query(parquet, &prepared, expected_fingerprint.clone())?;
    let profile = PhenotypeProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        updated_at: super::annotation::current_timestamp(),
        observed: prepared.observed,
        conditions: prepared.conditions,
        pathways: prepared.pathways,
        genes: prepared.genes,
        show_matches_only: true,
        active_generation: Some(PhenotypeGeneration {
            fingerprint: expected_fingerprint,
            matched_gene_count,
            evidence_file: None,
            catalog_file: None,
        }),
    };
    save(runs, &profile)?;
    cache_active_query(run_id, query);
    if let Some(root) = profile_path(runs, run_id).parent() {
        remove_legacy_query_files(root);
    }
    Ok(profile)
}

fn require_result_overlap(count: usize) -> Result<(), String> {
    (count > 0)
        .then_some(())
        .ok_or_else(|| "No resolved genes have variants in this result.".into())
}

struct PreparedGeneProfile {
    observed: Vec<PhenotypeTerm>,
    conditions: Vec<PhenotypeTerm>,
    pathways: Vec<PhenotypeTerm>,
    genes: Vec<super::gene_identity::ResolvedGene>,
    ranking: PhenotypeRanking,
    source_assets: Vec<SourceAsset>,
    resolved: ResolvedGeneSet,
    identity: super::gene_identity::Resolver,
}

fn prepare_gene_profile(
    resources: &Path,
    parquet: &Path,
    request: &ProfileUpdate,
    compute_ranking: bool,
) -> Result<PreparedGeneProfile, String> {
    let needs_hpo = !request.observed.is_empty() || !request.conditions.is_empty();
    let knowledge = needs_hpo.then(|| knowledge(resources)).transpose()?;
    let observed = if let Some(knowledge) = &knowledge {
        normalize_terms(
            knowledge,
            canonical_terms(knowledge, &request.observed, true)?,
            true,
        )?
    } else {
        Vec::new()
    };
    let root = needs_hpo.then(|| release_root(resources)).transpose()?;
    let mondo = (!request.conditions.is_empty())
        .then(|| crate::mondo::knowledge(root.as_ref().expect("conditions require HPO data")))
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
    let report_identities = super::results::report_gene_identities(parquet)?;
    let identity = super::gene_identity::Resolver::new(resources, &report_identities);
    let report_genes = report_identities
        .into_iter()
        .map(|(gene_symbol, gene_id)| {
            let gene = identity.canonicalize(&gene_symbol, &gene_id);
            let gene_id = gene.result_id();
            super::results::ReportGeneOccurrence {
                allele_id: String::new(),
                gene_symbol: gene.symbol,
                gene_id,
            }
        })
        .collect::<Vec<_>>();
    let genes = canonical_manual_genes(&identity, &request.genes)?;
    let selected_pathways = if request.pathways.is_empty() {
        Vec::new()
    } else {
        crate::reactome::knowledge(resources)?.canonical_pathways(&request.pathways)?
    };
    let pathways = selected_pathways
        .iter()
        .map(|pathway| PhenotypeTerm {
            id: pathway.id.clone(),
            label: pathway.label.clone(),
        })
        .collect::<Vec<_>>();
    if observed.is_empty() && conditions.is_empty() && pathways.is_empty() && genes.is_empty() {
        return Err("Add at least one feature, condition, pathway, or gene".into());
    }
    let observed_indexes = knowledge
        .as_ref()
        .map(|knowledge| term_indexes(knowledge, &observed))
        .transpose()?
        .unwrap_or_default();
    let condition_matches = if let Some(mondo) = &mondo {
        knowledge
            .as_ref()
            .expect("conditions require HPO data")
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
    let manifest = needs_hpo
        .then(|| installed_asset_manifest(resources))
        .transpose()?;
    let hpo_release = manifest
        .as_ref()
        .map(|manifest| manifest.release.clone())
        .unwrap_or_default();
    let ranking = knowledge.as_ref().filter(|_| compute_ranking).map_or_else(
        || PhenotypeRanking {
            hpo_release: String::new(),
            query_count: 0,
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        },
        |knowledge| {
            build_phenotype_ranking(knowledge, &identity, &observed_indexes, hpo_release.clone())
        },
    );
    let source_assets = profile_source_assets(
        resources,
        !observed.is_empty(),
        !conditions.is_empty(),
        !pathways.is_empty(),
        identity.identity_release().is_some(),
    )?;
    let resolved = resolve_gene_set(
        &identity,
        &report_genes,
        knowledge.as_deref(),
        &observed,
        &conditions,
        &selected_pathways,
        &genes,
        &condition_matches,
        &ranking,
    )?;
    Ok(PreparedGeneProfile {
        observed,
        conditions,
        pathways,
        genes,
        ranking,
        source_assets,
        resolved,
        identity,
    })
}

pub fn preview(
    resources: &Path,
    parquet: &Path,
    request: ProfileUpdate,
    offset: usize,
    limit: usize,
    query: &str,
    presence: &str,
    include_all_symbols: bool,
) -> Result<GenePreviewResponse, String> {
    if request.action != "preview" {
        return Err("gene preview action must be preview".into());
    }
    let prepared = prepare_gene_profile(resources, parquet, &request, false)?;
    let fingerprint = active_query_fingerprint(
        &prepared.observed,
        &prepared.conditions,
        &prepared.pathways,
        &prepared.genes,
        &prepared.source_assets,
    );
    let resolved = &prepared.resolved;
    let total_genes = resolved.genes.len();
    let genes_in_result = resolved
        .genes
        .keys()
        .filter(|key| resolved.result_identities.contains(*key))
        .count();
    let included_genes = resolved.included.len();
    let included_genes_in_result = resolved
        .included
        .iter()
        .filter(|key| resolved.result_identities.contains(*key))
        .count();
    let all_included_genes = include_all_symbols.then(|| {
        let mut genes = resolved
            .genes
            .values()
            .filter(|gene| resolved.included.contains(&gene.identity.comparison_key()))
            .map(|gene| gene.identity.clone())
            .collect::<Vec<_>>();
        genes.sort_by(|left, right| {
            left.symbol
                .cmp(&right.symbol)
                .then(left.comparison_key().cmp(&right.comparison_key()))
        });
        genes
    });
    let query = query.trim().to_ascii_uppercase();
    let mut rows = resolved
        .genes
        .values()
        .filter(|gene| {
            (query.is_empty()
                || gene.identity.symbol.contains(&query)
                || gene
                    .identity
                    .result_gene_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_uppercase().contains(&query))
                || gene
                    .identity
                    .canonical_gene_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_uppercase().contains(&query)))
                && match presence {
                    "in-result" => resolved
                        .result_identities
                        .contains(&gene.identity.comparison_key()),
                    "not-in-result" => {
                        resolved.included.contains(&gene.identity.comparison_key())
                            && !resolved
                                .result_identities
                                .contains(&gene.identity.comparison_key())
                    }
                    _ => true,
                }
        })
        .map(|gene| {
            let mut matches = gene.selected_matches.values().collect::<Vec<_>>();
            matches.sort_by_key(|matched| matched.order);
            GenePreviewRow {
                symbol: gene.identity.symbol.clone(),
                canonical_gene_id: gene.identity.canonical_gene_id.clone(),
                result_gene_id: gene.identity.result_gene_id.clone(),
                identity_status: gene.identity.identity_status.clone(),
                sources: matches
                    .into_iter()
                    .map(|matched| format!("{}: {}", matched.item_type, matched.label))
                    .collect(),
                in_result: resolved
                    .result_identities
                    .contains(&gene.identity.comparison_key()),
                included: resolved.included.contains(&gene.identity.comparison_key()),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.symbol
            .cmp(&right.symbol)
            .then(left.canonical_gene_id.cmp(&right.canonical_gene_id))
    });
    let offset = offset.min(rows.len());
    let limit = limit.clamp(1, 250);
    let has_more = offset.saturating_add(limit) < rows.len();
    let rows = rows.into_iter().skip(offset).take(limit).collect();
    Ok(GenePreviewResponse {
        fingerprint,
        total_genes,
        genes_in_result,
        included_genes,
        included_genes_in_result,
        offset,
        has_more,
        rows,
        all_included_genes,
    })
}

fn canonical_manual_genes(
    identity: &super::gene_identity::Resolver,
    requested: &[super::gene_identity::ResolvedGene],
) -> Result<Vec<super::gene_identity::ResolvedGene>, String> {
    if requested.len() > MAX_PROFILE_GENES {
        return Err(format!(
            "A gene list can contain at most {MAX_PROFILE_GENES} genes"
        ));
    }
    let mut genes = BTreeMap::<String, super::gene_identity::ResolvedGene>::new();
    for submitted in requested {
        let id = submitted
            .canonical_gene_id
            .as_deref()
            .or(submitted.result_gene_id.as_deref())
            .unwrap_or(&submitted.symbol);
        let resolved = match identity.resolve_pair(id, &submitted.symbol) {
            super::gene_identity::Resolution::Resolved(gene) => gene,
            super::gene_identity::Resolution::Ambiguous => {
                return Err(format!("Gene {} is ambiguous", submitted.symbol));
            }
            super::gene_identity::Resolution::Unknown => {
                return Err(format!("Gene {} is not recognized", submitted.symbol));
            }
        };
        if submitted
            .canonical_gene_id
            .as_ref()
            .zip(resolved.canonical_gene_id.as_ref())
            .is_some_and(|(submitted, resolved)| !submitted.eq_ignore_ascii_case(resolved))
        {
            return Err(format!("Gene {} changed identity", submitted.symbol));
        }
        genes.insert(resolved.comparison_key(), resolved);
    }
    Ok(genes.into_values().collect())
}

#[derive(Clone)]
struct GenePhenotypeSummary {
    identity: super::gene_identity::ResolvedGene,
    observed_feature_linked: bool,
    hpo_links: Vec<HpoAssociationLink>,
    condition_links: BTreeMap<String, GeneConditionLink>,
    condition_evidence_links: Vec<GeneConditionLink>,
    selected_matches: BTreeMap<String, GeneSelectedMatch>,
    rank: Option<RankedGene>,
}

#[derive(Clone)]
struct ResolvedGeneSet {
    genes: BTreeMap<String, GenePhenotypeSummary>,
    included: HashSet<String>,
    result_identities: HashSet<String>,
}

#[derive(Clone)]
struct GeneSelectedMatch {
    id: String,
    label: String,
    item_type: &'static str,
    relation: String,
    order: usize,
}

#[derive(Clone)]
struct ActiveGeneField {
    gene_key: String,
    gene_id: String,
    gene_symbol: String,
    canonical_gene_id: Option<String>,
    result_gene_id: Option<String>,
    identity_status: String,
    field_path: &'static str,
    value_type: &'static str,
    string_value: Option<String>,
    integer_value: Option<i64>,
    number_value: Option<f64>,
    boolean_value: Option<bool>,
    json_value: Option<String>,
}

#[derive(Clone)]
struct ActiveGeneSummary {
    gene_key: String,
    gene_id: String,
    gene_symbol: String,
    canonical_gene_id: Option<String>,
    result_gene_id: Option<String>,
    identity_status: String,
    phenotype_rank: Option<i64>,
    phenotype_rank_details: Option<String>,
}

#[derive(Clone)]
struct ActiveGeneAlias {
    gene_symbol: String,
    gene_id: String,
    gene_key: String,
}

#[derive(Clone)]
struct ActiveGeneMatch {
    gene_key: String,
    item_order: i64,
    item_type: &'static str,
    item_id: String,
    item_label: String,
    relation: String,
}

#[derive(Clone)]
pub(crate) struct ActiveGeneQuery {
    fingerprint: String,
    fields: Vec<ActiveGeneField>,
    genes: Vec<ActiveGeneSummary>,
    aliases: Vec<ActiveGeneAlias>,
    matches: Vec<ActiveGeneMatch>,
}

impl ActiveGeneQuery {
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

struct CachedActiveGeneQuery {
    run_id: String,
    fingerprint: String,
    query: Arc<ActiveGeneQuery>,
}

fn active_query_cache() -> &'static Mutex<Option<CachedActiveGeneQuery>> {
    static CACHE: OnceLock<Mutex<Option<CachedActiveGeneQuery>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Clone, PartialEq, Eq)]
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

#[derive(Clone)]
struct HpoAssociationLink {
    selected_id: String,
    selected_label: String,
    annotated_id: String,
    annotated_label: String,
    relation: String,
    disease_id: String,
    disease_name: String,
    association_type: String,
    association_source: String,
    annotation: DiseasePhenotypeContext,
}

fn profile_source_assets(
    resources: &Path,
    use_hpo: bool,
    use_mondo: bool,
    use_reactome: bool,
    use_hgnc: bool,
) -> Result<Vec<SourceAsset>, String> {
    let mut sources = Vec::new();
    if use_hpo || use_mondo || use_hgnc {
        let (root, _, manifest) = installed_release(resources).ok_or_else(|| {
            "Human Phenotype Ontology data is not installed. Install it from Data sources first."
                .to_owned()
        })?;
        for asset in &manifest.assets {
            let include = source_asset_used(&asset.kind, use_hpo, use_mondo, use_hgnc);
            if !include {
                continue;
            }
            let sha256 = if asset.sha256.len() == 64
                && asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                asset.sha256.to_ascii_lowercase()
            } else {
                super::fastvep::sha256_file(&root.join("raw").join(&asset.filename))?
            };
            let release = if asset.kind.contains("gene-identit") {
                manifest.hgnc_release.clone()
            } else if asset.kind.contains("mondo") {
                manifest.mondo_release.clone()
            } else {
                Some(manifest.release.clone())
            }
            .ok_or_else(|| format!("{} has no source release", asset.filename))?;
            sources.push(SourceAsset {
                name: asset.filename.clone(),
                release,
                sha256,
            });
        }
    }
    if use_reactome {
        let (name, release, sha256) = crate::reactome::source_asset(resources)
            .ok_or("Reactome pathway data is not installed")?;
        sources.push(SourceAsset {
            name,
            release,
            sha256: sha256.to_ascii_lowercase(),
        });
    }
    sources.sort_by(|left, right| left.name.cmp(&right.name));
    sources.dedup_by(|left, right| left.name == right.name);
    Ok(sources)
}

fn source_asset_used(kind: &str, use_hpo: bool, use_mondo: bool, use_hgnc: bool) -> bool {
    (use_hpo || use_mondo) && matches!(kind, "ontology" | "disease-annotations" | "disease-genes")
        || use_mondo && kind.contains("mondo")
        || use_hgnc && matches!(kind, "gene-identities" | "withdrawn-gene-identities")
}

fn resolved_association_gene(
    identity: &super::gene_identity::Resolver,
    association: &GeneAssociation,
) -> Option<super::gene_identity::ResolvedGene> {
    match identity.resolve_pair(&association.gene_id, &association.symbol) {
        super::gene_identity::Resolution::Resolved(gene) => Some(gene),
        super::gene_identity::Resolution::Ambiguous => None,
        super::gene_identity::Resolution::Unknown if valid_gene_symbol(&association.symbol) => {
            Some(super::gene_identity::ResolvedGene {
                symbol: association.symbol.trim().to_ascii_uppercase(),
                canonical_gene_id: None,
                result_gene_id: None,
                identity_status: "symbol-only".into(),
            })
        }
        super::gene_identity::Resolution::Unknown => None,
    }
}

fn resnik_similarity(
    knowledge: &HpoKnowledge,
    information_content: &[Option<f64>],
    left: usize,
    right: usize,
) -> (f64, Option<usize>) {
    let mut best: (f64, Option<usize>) = (0.0, None);
    let mut left_position = 0;
    let mut right_position = 0;
    let left_ancestors = &knowledge.terms[left].ancestors;
    let right_ancestors = &knowledge.terms[right].ancestors;
    while left_position < left_ancestors.len() && right_position < right_ancestors.len() {
        match left_ancestors[left_position].cmp(&right_ancestors[right_position]) {
            std::cmp::Ordering::Less => left_position += 1,
            std::cmp::Ordering::Greater => right_position += 1,
            std::cmp::Ordering::Equal => {
                let common = left_ancestors[left_position];
                if let Some(value) = information_content[common]
                    && (value > best.0
                        || value == best.0
                            && best.1.is_none_or(|current| {
                                knowledge.terms[common].id < knowledge.terms[current].id
                            }))
                {
                    best = (value, Some(common));
                }
                left_position += 1;
                right_position += 1;
            }
        }
    }
    best
}

fn build_phenotype_ranking(
    knowledge: &HpoKnowledge,
    identity: &super::gene_identity::Resolver,
    observed: &[usize],
    hpo_release: String,
) -> PhenotypeRanking {
    if observed.is_empty() {
        return PhenotypeRanking {
            hpo_release,
            query_count: 0,
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        };
    }
    let mut diseases = knowledge
        .diseases
        .iter()
        .filter_map(|disease| {
            let mut genes = BTreeMap::new();
            for association in &disease.genes {
                if association
                    .association_type
                    .eq_ignore_ascii_case("MENDELIAN")
                    && let Some(gene) = resolved_association_gene(identity, association)
                {
                    genes.entry(gene.comparison_key()).or_insert(gene);
                }
            }
            (!disease.positive.is_empty() && !genes.is_empty()).then_some((disease, genes))
        })
        .collect::<Vec<_>>();
    diseases.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    let disease_count = diseases.len();
    if disease_count == 0 {
        return PhenotypeRanking {
            hpo_release,
            query_count: observed.len(),
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        };
    }
    let mut counts = vec![0_u64; knowledge.terms.len()];
    for (disease, _) in &diseases {
        let mut propagated = BTreeSet::new();
        for &annotation in &disease.positive {
            propagated.extend(knowledge.terms[annotation].ancestors.iter().copied());
        }
        for term in propagated {
            counts[term] = counts[term].saturating_add(1);
        }
    }
    let information_content = counts
        .iter()
        .map(|&count| (count > 0).then(|| (disease_count as f64 / count as f64).ln().max(0.0)))
        .collect::<Vec<_>>();
    let mut query = observed.to_vec();
    query.sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
    query.dedup();
    let mut best_by_gene =
        BTreeMap::<String, (super::gene_identity::ResolvedGene, RankedGene)>::new();
    for (disease, disease_genes) in diseases {
        let mut disease_terms = disease.positive.clone();
        disease_terms
            .sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
        let mut matched_terms = Vec::with_capacity(query.len());
        let mut score_sum = 0.0;
        for &query_term in &query {
            let mut best: Option<(f64, usize, Option<usize>)> = None;
            for &disease_term in &disease_terms {
                let (score, mica) =
                    resnik_similarity(knowledge, &information_content, query_term, disease_term);
                if best.as_ref().is_none_or(|current| {
                    score > current.0
                        || score == current.0
                            && knowledge.terms[disease_term].id < knowledge.terms[current.1].id
                }) {
                    best = Some((score, disease_term, mica));
                }
            }
            let best = best.expect("eligible disease profiles contain a positive HPO term");
            score_sum += best.0;
            matched_terms.push(RankedTermMatch {
                query: term_value(knowledge, query_term),
                disease_term: term_value(knowledge, best.1),
                mica: best.2.map(|index| term_value(knowledge, index)),
                eligible_disease_profiles: disease_count,
                mica_disease_profiles: best.2.map(|index| counts[index]),
                resnik_similarity: best.0,
            });
        }
        let raw_score = score_sum / query.len() as f64;
        let score_key = (raw_score * 1_000_000_000_000.0 + 0.5).floor() as u64;
        for (key, gene) in disease_genes {
            let candidate = RankedGene {
                rank: 0,
                tie_count: 0,
                raw_score,
                score_key,
                best_disease_id: disease.id.clone(),
                best_disease_name: disease.name.clone(),
                matched_terms: matched_terms.clone(),
            };
            best_by_gene
                .entry(key)
                .and_modify(|(_, current)| {
                    if candidate.raw_score > current.raw_score
                        || candidate.raw_score == current.raw_score
                            && candidate.best_disease_id < current.best_disease_id
                    {
                        *current = candidate.clone();
                    }
                })
                .or_insert((gene, candidate));
        }
    }
    let denominator = best_by_gene.len();
    let mut tie_counts = HashMap::<u64, usize>::new();
    for (_, rank) in best_by_gene.values() {
        *tie_counts.entry(rank.score_key).or_default() += 1;
    }
    let mut ranked = best_by_gene.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .1
            .score_key
            .cmp(&left.1.1.score_key)
            .then(left.1.0.symbol.cmp(&right.1.0.symbol))
            .then(left.0.cmp(&right.0))
    });
    let mut previous_score = None;
    let mut competition_rank = 0;
    let mut genes = BTreeMap::new();
    for (position, (key, (_, mut rank))) in ranked.into_iter().enumerate() {
        if previous_score != Some(rank.score_key) {
            competition_rank = position + 1;
            previous_score = Some(rank.score_key);
        }
        rank.rank = competition_rank;
        rank.tie_count = tie_counts[&rank.score_key];
        genes.insert(key, rank);
    }
    PhenotypeRanking {
        hpo_release,
        query_count: query.len(),
        disease_profile_count: disease_count,
        denominator,
        genes,
    }
}

fn new_gene_summary(
    identity: super::gene_identity::ResolvedGene,
    ranking: &PhenotypeRanking,
) -> GenePhenotypeSummary {
    let rank = ranking.genes.get(&identity.comparison_key()).cloned();
    GenePhenotypeSummary {
        identity,
        observed_feature_linked: false,
        hpo_links: Vec::new(),
        condition_links: BTreeMap::new(),
        condition_evidence_links: Vec::new(),
        selected_matches: BTreeMap::new(),
        rank,
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_gene_set(
    identity: &super::gene_identity::Resolver,
    report_genes: &[super::results::ReportGeneOccurrence],
    knowledge: Option<&HpoKnowledge>,
    observed: &[PhenotypeTerm],
    conditions: &[PhenotypeTerm],
    selected_pathways: &[crate::reactome::Pathway],
    manual_genes: &[super::gene_identity::ResolvedGene],
    condition_matches: &HashMap<String, Vec<crate::mondo::DiseaseConditionMatch>>,
    ranking: &PhenotypeRanking,
) -> Result<ResolvedGeneSet, String> {
    let result_identities = report_genes
        .iter()
        .map(|gene| {
            identity
                .canonicalize(&gene.gene_symbol, &gene.gene_id)
                .comparison_key()
        })
        .collect::<HashSet<_>>();
    let mut genes = BTreeMap::<String, GenePhenotypeSummary>::new();
    if let Some(knowledge) = knowledge {
        let observed_indexes = term_indexes(knowledge, observed)?;
        for (order, (&selected_index, selected)) in
            observed_indexes.iter().zip(observed).enumerate()
        {
            for disease in &knowledge.diseases {
                for &annotated_index in &disease.positive {
                    let relation = if annotated_index == selected_index {
                        Some("HPO link via exact disease annotation")
                    } else if knowledge.terms[annotated_index]
                        .ancestors
                        .contains(&selected_index)
                    {
                        Some("HPO link via more-specific disease annotation")
                    } else {
                        None
                    };
                    let Some(relation) = relation else {
                        continue;
                    };
                    for association in &disease.genes {
                        if !association
                            .association_type
                            .eq_ignore_ascii_case("MENDELIAN")
                        {
                            continue;
                        }
                        let Some(resolved) = resolved_association_gene(identity, association)
                        else {
                            continue;
                        };
                        let key = resolved.comparison_key();
                        let summary = genes
                            .entry(key)
                            .or_insert_with(|| new_gene_summary(resolved, ranking));
                        summary.observed_feature_linked |= annotated_index == selected_index;
                        summary
                            .selected_matches
                            .entry(format!("Feature:{}", selected.id))
                            .and_modify(|current| {
                                if current.relation != "HPO link via exact disease annotation"
                                    && relation == "HPO link via exact disease annotation"
                                {
                                    current.relation = relation.into();
                                }
                            })
                            .or_insert(GeneSelectedMatch {
                                id: selected.id.clone(),
                                label: selected.label.clone(),
                                item_type: "Feature",
                                relation: relation.into(),
                                order,
                            });
                        let link = HpoAssociationLink {
                            selected_id: selected.id.clone(),
                            selected_label: selected.label.clone(),
                            annotated_id: knowledge.terms[annotated_index].id.clone(),
                            annotated_label: knowledge.terms[annotated_index].label.clone(),
                            relation: relation.into(),
                            disease_id: disease.id.clone(),
                            disease_name: disease.name.clone(),
                            association_type: association.association_type.clone(),
                            association_source: association.source.clone(),
                            annotation: disease
                                .annotations
                                .get(&annotated_index)
                                .cloned()
                                .unwrap_or_default(),
                        };
                        if !summary.hpo_links.iter().any(|current| {
                            current.selected_id == link.selected_id
                                && current.annotated_id == link.annotated_id
                                && current.disease_id == link.disease_id
                                && current.association_source == link.association_source
                        }) {
                            summary.hpo_links.push(link);
                        }
                    }
                }
            }
        }
        for association in &knowledge.condition_associations {
            let Some(matches) = condition_matches.get(&association.id) else {
                continue;
            };
            for gene_association in &association.genes {
                if !matches!(
                    gene_association
                        .association_type
                        .to_ascii_uppercase()
                        .as_str(),
                    "MENDELIAN" | "POLYGENIC"
                ) {
                    continue;
                }
                let Some(resolved) = resolved_association_gene(identity, gene_association) else {
                    continue;
                };
                let key = resolved.comparison_key();
                let summary = genes
                    .entry(key)
                    .or_insert_with(|| new_gene_summary(resolved, ranking));
                for matched in matches {
                    let candidate = GeneConditionLink {
                        selected_id: matched.selected_id.clone(),
                        selected_label: matched.selected_label.clone(),
                        matched_id: matched.matched_id.clone(),
                        matched_label: matched.matched_label.clone(),
                        relation: matched.relation.into(),
                        source_disease_id: association.id.clone(),
                        source_disease_name: association.name.clone(),
                        association_type: gene_association.association_type.clone(),
                        association_source: gene_association.source.clone(),
                    };
                    if !summary.condition_evidence_links.contains(&candidate) {
                        summary.condition_evidence_links.push(candidate.clone());
                    }
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
                    let selected_relation = summary
                        .condition_links
                        .get(&matched.selected_id)
                        .map(|link| {
                            format!(
                                "{} · {}",
                                link.relation,
                                link.association_type.to_ascii_uppercase()
                            )
                        })
                        .unwrap();
                    let order = observed.len()
                        + conditions
                            .iter()
                            .position(|term| term.id == matched.selected_id)
                            .unwrap_or(usize::MAX - observed.len());
                    summary.selected_matches.insert(
                        format!("Condition:{}", matched.selected_id),
                        GeneSelectedMatch {
                            id: matched.selected_id.clone(),
                            label: matched.selected_label.clone(),
                            item_type: "Condition",
                            relation: selected_relation,
                            order,
                        },
                    );
                }
            }
        }
    }
    for (pathway_order, pathway) in selected_pathways.iter().enumerate() {
        for source_symbol in &pathway.genes {
            let resolved = identity.canonicalize(source_symbol, "");
            let key = resolved.comparison_key();
            let summary = genes
                .entry(key)
                .or_insert_with(|| new_gene_summary(resolved, ranking));
            summary.selected_matches.insert(
                format!("Pathway:{}", pathway.id),
                GeneSelectedMatch {
                    id: pathway.id.clone(),
                    label: pathway.label.clone(),
                    item_type: "Pathway",
                    relation: "Listed in Reactome pathway gene set".into(),
                    order: observed.len() + conditions.len() + pathway_order,
                },
            );
        }
    }
    for (manual_order, resolved) in manual_genes.iter().enumerate() {
        let key = resolved.comparison_key();
        let summary = genes
            .entry(key.clone())
            .or_insert_with(|| new_gene_summary(resolved.clone(), ranking));
        summary.selected_matches.insert(
            format!("Gene:{key}"),
            GeneSelectedMatch {
                id: resolved
                    .canonical_gene_id
                    .clone()
                    .or_else(|| resolved.result_gene_id.clone())
                    .unwrap_or_else(|| resolved.symbol.clone()),
                label: resolved.symbol.clone(),
                item_type: "Gene",
                relation: "Entered gene".into(),
                order: observed.len() + conditions.len() + selected_pathways.len() + manual_order,
            },
        );
    }
    let included = genes.keys().cloned().collect();
    Ok(ResolvedGeneSet {
        genes,
        included,
        result_identities,
    })
}

fn active_query_fingerprint(
    observed: &[PhenotypeTerm],
    conditions: &[PhenotypeTerm],
    pathways: &[PhenotypeTerm],
    genes: &[super::gene_identity::ResolvedGene],
    source_assets: &[SourceAsset],
) -> String {
    let value = json!({
        "profileSchemaVersion": PROFILE_SCHEMA_VERSION,
        "activeQueryContractVersion": ACTIVE_QUERY_CONTRACT_VERSION,
        "identityContractVersion": super::gene_identity::CONTRACT_VERSION,
        "geneSetAlgorithmVersion": GENE_SET_ALGORITHM_VERSION,
        "phenotypeRankingAlgorithmVersion": PHENOTYPE_RANKING_ALGORITHM_VERSION,
        "observed": observed.iter().map(|term| term.id.as_str()).collect::<Vec<_>>(),
        "conditions": conditions.iter().map(|term| term.id.as_str()).collect::<Vec<_>>(),
        "pathways": pathways.iter().map(|term| term.id.as_str()).collect::<Vec<_>>(),
        "genes": genes,
        "sourceAssets": source_assets,
    });
    format!("{:x}", Sha256::digest(serde_json::to_vec(&value).unwrap()))
}

fn phenotype_rank_details(
    gene: &super::gene_identity::ResolvedGene,
    rank: Option<&RankedGene>,
    ranking: &PhenotypeRanking,
    source_assets: &[SourceAsset],
) -> serde_json::Value {
    json!({
        "geneSymbol": gene.symbol,
        "canonicalGeneId": gene.canonical_gene_id,
        "resultGeneId": gene.result_gene_id,
        "identityStatus": gene.identity_status,
        "rank": rank.map(|value| value.rank),
        "denominator": ranking.denominator,
        "tieCount": rank.map(|value| value.tie_count).unwrap_or(0),
        "bestDiseaseId": rank.map(|value| value.best_disease_id.as_str()),
        "bestDisease": rank.map(|value| value.best_disease_name.as_str()),
        "rawResnikScore": rank.map(|value| value.raw_score),
        "scoreKey": rank.map(|value| value.score_key),
        "algorithmVersion": PHENOTYPE_RANKING_ALGORITHM_VERSION,
        "queryTermCount": ranking.query_count,
        "eligibleDiseaseProfileCount": ranking.disease_profile_count,
        "hpoRelease": ranking.hpo_release,
        "sourceAssets": source_assets,
        "matchedTerms": rank.map(|value| value.matched_terms.as_slice()).unwrap_or(&[]),
    })
}

fn build_active_gene_query(
    parquet: &Path,
    prepared: &PreparedGeneProfile,
    fingerprint: String,
) -> Result<ActiveGeneQuery, String> {
    let resolved = &prepared.resolved;
    let mut fields = Vec::with_capacity(resolved.genes.len() * 12);
    let mut genes = Vec::with_capacity(resolved.genes.len());
    let mut matches = Vec::new();
    for gene in resolved.genes.values() {
        let identity = &gene.identity;
        let key = identity.comparison_key();
        if !resolved.result_identities.contains(&key) {
            continue;
        }
        let base = |field_path, value_type| ActiveGeneField {
            gene_key: key.clone(),
            gene_id: identity.result_id(),
            gene_symbol: identity.symbol.clone(),
            canonical_gene_id: identity.canonical_gene_id.clone(),
            result_gene_id: identity.result_gene_id.clone(),
            identity_status: identity.identity_status.clone(),
            field_path,
            value_type,
            string_value: None,
            integer_value: None,
            number_value: None,
            boolean_value: None,
            json_value: None,
        };
        let direct_matches = gene
            .hpo_links
            .iter()
            .filter(|link| link.selected_id == link.annotated_id)
            .map(|link| link.selected_id.as_str())
            .collect::<HashSet<_>>()
            .len() as i64;
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
        let evidence_details = serde_json::to_string(&json!({
            "activeQueryContractVersion": ACTIVE_QUERY_CONTRACT_VERSION,
            "identity": identity,
            "hpoLinks": gene.hpo_links.iter().map(|link| json!({
                "selectedFeatureId": link.selected_id,
                "selectedFeature": link.selected_label,
                "annotatedFeatureId": link.annotated_id,
                "annotatedFeature": link.annotated_label,
                "relation": link.relation,
                "sourceDiseaseId": link.disease_id,
                "sourceDisease": link.disease_name,
                "associationType": link.association_type,
                "associationSource": link.association_source,
                "annotation": link.annotation,
            })).collect::<Vec<_>>(),
            "conditionLinks": gene.condition_evidence_links.iter().map(|link| json!({
                "selectedConditionId": link.selected_id,
                "selectedCondition": link.selected_label,
                "matchedConditionId": link.matched_id,
                "matchedCondition": link.matched_label,
                "relation": link.relation,
                "sourceDiseaseId": link.source_disease_id,
                "sourceDisease": link.source_disease_name,
                "associationType": link.association_type,
                "associationSource": link.association_source,
            })).collect::<Vec<_>>(),
            "selectedMatches": gene.selected_matches.values().map(|matched| json!({
                "selectedItemId": matched.id,
                "selectedItem": matched.label,
                "itemType": matched.item_type,
                "relation": matched.relation,
            })).collect::<Vec<_>>(),
        }))
        .map_err(|error| format!("cannot serialize phenotype evidence details: {error}"))?;
        let rank_details = (prepared.ranking.query_count > 0)
            .then(|| {
                serde_json::to_string(&phenotype_rank_details(
                    identity,
                    gene.rank.as_ref(),
                    &prepared.ranking,
                    &prepared.source_assets,
                ))
                .map_err(|error| format!("cannot serialize phenotype rank details: {error}"))
            })
            .transpose()?;
        if prepared.ranking.query_count > 0 {
            fields.push(ActiveGeneField {
                integer_value: gene.rank.as_ref().map(|rank| rank.rank as i64),
                ..base("phenotypeRank", "integer")
            });
            fields.push(ActiveGeneField {
                json_value: rank_details.clone(),
                ..base("phenotypeRankDetails", "json")
            });
        }
        fields.push(ActiveGeneField {
            boolean_value: Some(!gene.hpo_links.is_empty() || !gene.condition_links.is_empty()),
            ..base("profileLinked", "boolean")
        });
        fields.push(ActiveGeneField {
            boolean_value: Some(resolved.included.contains(&key)),
            ..base("includedGene", "boolean")
        });
        fields.push(ActiveGeneField {
            boolean_value: Some(gene.observed_feature_linked),
            ..base("observedFeatureLinked", "boolean")
        });
        fields.push(ActiveGeneField {
            string_value: gene
                .rank
                .as_ref()
                .map(|rank| rank.best_disease_name.clone()),
            ..base("bestMatchingCondition", "text")
        });
        fields.push(ActiveGeneField {
            integer_value: Some(direct_matches),
            ..base("directFeatureMatches", "integer")
        });
        fields.push(ActiveGeneField {
            integer_value: Some(condition_count),
            ..base("selectedConditionMatches", "integer")
        });
        fields.push(ActiveGeneField {
            string_value: Some(matched_conditions),
            ..base("matchedSelectedConditions", "text")
        });
        fields.push(ActiveGeneField {
            string_value: Some(condition_relation.into()),
            ..base("selectedConditionRelation", "text")
        });
        fields.push(ActiveGeneField {
            json_value: Some(evidence_details),
            ..base("phenotypeEvidenceDetails", "json")
        });
        genes.push(ActiveGeneSummary {
            gene_key: key.clone(),
            gene_id: identity.result_id(),
            gene_symbol: identity.symbol.clone(),
            canonical_gene_id: identity.canonical_gene_id.clone(),
            result_gene_id: identity.result_gene_id.clone(),
            identity_status: identity.identity_status.clone(),
            phenotype_rank: gene.rank.as_ref().map(|rank| rank.rank as i64),
            phenotype_rank_details: rank_details,
        });
        if resolved.included.contains(&key) {
            matches.extend(
                gene.selected_matches
                    .values()
                    .map(|matched| ActiveGeneMatch {
                        gene_key: key.clone(),
                        item_order: matched.order as i64,
                        item_type: matched.item_type,
                        item_id: matched.id.clone(),
                        item_label: matched.label.clone(),
                        relation: matched.relation.clone(),
                    }),
            );
        }
    }

    let mut aliases = BTreeSet::new();
    for (gene_symbol, gene_id) in super::results::report_gene_identities(parquet)? {
        let canonical = prepared.identity.canonicalize(&gene_symbol, &gene_id);
        let key = canonical.comparison_key();
        if resolved.included.contains(&key) {
            aliases.insert((
                gene_symbol.trim().to_ascii_uppercase(),
                gene_id.trim().to_ascii_uppercase(),
                key,
            ));
        }
    }
    let aliases = aliases
        .into_iter()
        .map(|(gene_symbol, gene_id, gene_key)| ActiveGeneAlias {
            gene_symbol,
            gene_id,
            gene_key,
        })
        .collect::<Vec<_>>();
    Ok(ActiveGeneQuery {
        fingerprint,
        fields,
        genes,
        aliases,
        matches,
    })
}

fn cache_active_query(run_id: &str, query: ActiveGeneQuery) -> Arc<ActiveGeneQuery> {
    let fingerprint = query.fingerprint.clone();
    let query = Arc::new(query);
    *active_query_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(CachedActiveGeneQuery {
        run_id: run_id.into(),
        fingerprint,
        query: query.clone(),
    });
    query
}

fn clear_cached_active_query(run_id: &str) {
    let mut cache = active_query_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if cache.as_ref().is_some_and(|cached| cached.run_id == run_id) {
        *cache = None;
    }
}

pub(crate) fn active_query(
    resources: &Path,
    runs: &Path,
    run_id: &str,
    parquet: &Path,
) -> Result<Option<Arc<ActiveGeneQuery>>, String> {
    let profile = match load_current(resources, runs, run_id) {
        Ok(profile) => profile,
        Err(_) => {
            clear_cached_active_query(run_id);
            return Ok(None);
        }
    };
    let Some(active) = profile.active_generation.as_ref() else {
        return Ok(None);
    };
    if let Some(query) = active_query_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .filter(|cached| cached.run_id == run_id && cached.fingerprint == active.fingerprint)
        .map(|cached| cached.query.clone())
    {
        return Ok(Some(query));
    }
    let request = ProfileUpdate {
        action: "preview".into(),
        observed: profile.observed.clone(),
        conditions: profile.conditions.clone(),
        pathways: profile.pathways.clone(),
        genes: profile.genes.clone(),
        show_matches_only: true,
        preview_fingerprint: None,
    };
    let prepared = prepare_gene_profile(resources, parquet, &request, true)?;
    let fingerprint = active_query_fingerprint(
        &prepared.observed,
        &prepared.conditions,
        &prepared.pathways,
        &prepared.genes,
        &prepared.source_assets,
    );
    if fingerprint != active.fingerprint {
        return Ok(None);
    }
    let query = build_active_gene_query(parquet, &prepared, fingerprint)?;
    Ok(Some(cache_active_query(run_id, query)))
}

pub(crate) fn active_query_catalog(profile: &PhenotypeProfile) -> Option<serde_json::Value> {
    let active = profile.active_generation.as_ref()?;
    Some(json!({
        "activeGeneQueryContractVersion": ACTIVE_QUERY_CONTRACT_VERSION,
        "fingerprint": active.fingerprint,
        "fields": super::report_import::gene_catalog_fields(profile.observed.len()),
    }))
}

pub(crate) fn register_active_query(
    connection: &Connection,
    variants: &Path,
    query: &ActiveGeneQuery,
) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TEMP TABLE annocat_active_gene_fields(
                 gene_key VARCHAR NOT NULL,
                 gene_id VARCHAR NOT NULL,
                 gene_symbol VARCHAR NOT NULL,
                 canonical_gene_id VARCHAR,
                 result_gene_id VARCHAR,
                 identity_status VARCHAR NOT NULL,
                 scope VARCHAR NOT NULL,
                 source_id VARCHAR NOT NULL,
                 field_path VARCHAR NOT NULL,
                 value_type VARCHAR NOT NULL,
                 string_value VARCHAR,
                 integer_value BIGINT,
                 number_value DOUBLE,
                 boolean_value BOOLEAN,
                 json_value VARCHAR
             );
             CREATE TEMP TABLE annocat_active_genes(
                 gene_key VARCHAR PRIMARY KEY,
                 gene_id VARCHAR NOT NULL,
                 gene_symbol VARCHAR NOT NULL,
                 canonical_gene_id VARCHAR,
                 result_gene_id VARCHAR,
                 identity_status VARCHAR NOT NULL,
                 phenotype_rank BIGINT,
                 phenotype_rank_details VARCHAR
             );
             CREATE TEMP TABLE annocat_active_gene_aliases(
                 gene_symbol VARCHAR NOT NULL,
                 gene_id VARCHAR NOT NULL,
                 gene_key VARCHAR NOT NULL,
                 PRIMARY KEY(gene_symbol, gene_id)
             );
             CREATE TEMP TABLE annocat_active_gene_matches(
                 gene_key VARCHAR NOT NULL,
                 item_order BIGINT NOT NULL,
                 item_type VARCHAR NOT NULL,
                 item_id VARCHAR NOT NULL,
                 item_label VARCHAR NOT NULL,
                 relation VARCHAR NOT NULL,
                 PRIMARY KEY(gene_key, item_type, item_id)
             );",
        )
        .map_err(|error| format!("cannot prepare active Genes query: {error}"))?;
    {
        let mut appender = connection
            .appender("annocat_active_gene_fields")
            .map_err(|error| format!("cannot prepare active gene fields: {error}"))?;
        for field in &query.fields {
            let values = vec![
                field.gene_key.clone().into(),
                field.gene_id.clone().into(),
                field.gene_symbol.clone().into(),
                field
                    .canonical_gene_id
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
                field
                    .result_gene_id
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
                field.identity_status.clone().into(),
                String::from("gene").into(),
                String::from("gene-profile").into(),
                field.field_path.to_owned().into(),
                field.value_type.to_owned().into(),
                field
                    .string_value
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
                field.integer_value.map_or(SqlValue::Null, SqlValue::BigInt),
                field.number_value.map_or(SqlValue::Null, SqlValue::Double),
                field
                    .boolean_value
                    .map_or(SqlValue::Null, SqlValue::Boolean),
                field.json_value.clone().map_or(SqlValue::Null, Into::into),
            ];
            appender
                .append_row(appender_params_from_iter(values))
                .map_err(|error| format!("cannot add an active gene field: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot finish active gene fields: {error}"))?;
    }
    {
        let mut appender = connection
            .appender("annocat_active_genes")
            .map_err(|error| format!("cannot prepare active genes: {error}"))?;
        for gene in &query.genes {
            let values = vec![
                gene.gene_key.clone().into(),
                gene.gene_id.clone().into(),
                gene.gene_symbol.clone().into(),
                gene.canonical_gene_id
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
                gene.result_gene_id
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
                gene.identity_status.clone().into(),
                gene.phenotype_rank.map_or(SqlValue::Null, SqlValue::BigInt),
                gene.phenotype_rank_details
                    .clone()
                    .map_or(SqlValue::Null, Into::into),
            ];
            appender
                .append_row(appender_params_from_iter(values))
                .map_err(|error| format!("cannot add an active gene: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot finish active genes: {error}"))?;
    }
    {
        let mut appender = connection
            .appender("annocat_active_gene_aliases")
            .map_err(|error| format!("cannot prepare active gene aliases: {error}"))?;
        for alias in &query.aliases {
            appender
                .append_row(params![alias.gene_symbol, alias.gene_id, alias.gene_key])
                .map_err(|error| format!("cannot add an active gene alias: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot finish active gene aliases: {error}"))?;
    }
    {
        let mut appender = connection
            .appender("annocat_active_gene_matches")
            .map_err(|error| format!("cannot prepare active gene matches: {error}"))?;
        for matched in &query.matches {
            appender
                .append_row(params![
                    matched.gene_key,
                    matched.item_order,
                    matched.item_type,
                    matched.item_id,
                    matched.item_label,
                    matched.relation
                ])
                .map_err(|error| format!("cannot add an active gene match: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot finish active gene matches: {error}"))?;
    }
    let consequences = variants.with_file_name("consequences.parquet");
    if !consequences.is_file() {
        return Err("Genes queries require the result consequence table".into());
    }
    let path = consequences.to_string_lossy().replace('\'', "''");
    connection
        .execute_batch(&format!(
            "CREATE TEMP VIEW annocat_active_allele_genes AS
             SELECT DISTINCT c.allele_id, alias.gene_key
             FROM read_parquet('{path}') c
             JOIN annocat_active_gene_aliases alias
               ON alias.gene_symbol=upper(trim(c.gene_symbol))
              AND alias.gene_id=upper(trim(coalesce(c.gene_id, '')))
             WHERE c.allele_id IS NOT NULL AND trim(c.allele_id) <> ''"
        ))
        .map_err(|error| format!("cannot prepare active allele genes: {error}"))?;
    Ok(())
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
    Ok(HpoKnowledge {
        terms,
        term_index,
        active_terms,
        phenotypic_abnormality_root,
        diseases,
        condition_associations,
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
    biocuration: &str,
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
    append_unique(&mut target.biocuration, biocuration);
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
                annotations: HashMap::new(),
                genes: Vec::new(),
            });
        if value("qualifier") == "NOT" || value("frequency") == "HP:0040285" {
            continue;
        } else {
            disease.positive.push(index);
            merge_disease_annotation(
                disease.annotations.entry(index).or_default(),
                value("frequency"),
                value("onset"),
                value("sex"),
                value("evidence"),
                value("reference"),
                value("biocuration"),
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
                annotations: disease.annotations,
                genes: disease.genes,
            }
        })
        .collect();
    Ok((profiles, association_count))
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
    if profile.schema_version != PROFILE_SCHEMA_VERSION || profile.run_id != run_id {
        return Err("phenotype profile identity is invalid".into());
    }
    for term in &profile.observed {
        validate_hpo_id(&term.id)?;
        if invalid_profile_label(&term.label) {
            return Err(format!("invalid label for {}", term.id));
        }
    }
    for term in &profile.conditions {
        if !term.id.starts_with("MONDO:")
            || term.id[6..].is_empty()
            || !term.id[6..].bytes().all(|byte| byte.is_ascii_digit())
            || invalid_profile_label(&term.label)
        {
            return Err("phenotype profile contains an invalid condition".into());
        }
    }
    for term in &profile.pathways {
        if !term.id.starts_with("R-HSA-")
            || term.id[6..].is_empty()
            || !term.id[6..].bytes().all(|byte| byte.is_ascii_digit())
            || invalid_profile_label(&term.label)
        {
            return Err("gene profile contains an invalid pathway".into());
        }
    }
    let mut gene_keys = HashSet::new();
    for gene in &profile.genes {
        let valid_hgnc = gene.canonical_gene_id.as_deref().is_some_and(|id| {
            id.starts_with("HGNC:")
                && !id[5..].is_empty()
                && id[5..].bytes().all(|byte| byte.is_ascii_digit())
        });
        let valid_result = gene.result_gene_id.as_deref().is_none_or(|id| {
            !id.trim().is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
        });
        let valid_identity = match gene.identity_status.as_str() {
            "hgnc" => valid_hgnc,
            "symbol-only" => gene.canonical_gene_id.is_none(),
            _ => false,
        };
        if !valid_gene_symbol(&gene.symbol)
            || !valid_result
            || !valid_identity
            || !gene_keys.insert(gene.comparison_key())
        {
            return Err("gene profile contains an invalid gene identity".into());
        }
    }
    if let Some(active) = &profile.active_generation {
        if !profile.show_matches_only {
            return Err("an active Genes query must show matching variants only".into());
        }
        if profile.observed.is_empty()
            && profile.conditions.is_empty()
            && profile.pathways.is_empty()
            && profile.genes.is_empty()
        {
            return Err("an active Genes query must contain a selection".into());
        }
        if active.matched_gene_count == 0
            || active.fingerprint.len() != 64
            || !active
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err("phenotype profile contains an invalid active generation".into());
        }
        let short = &active.fingerprint[..16];
        for (name, expected) in [
            (
                active.evidence_file.as_deref(),
                format!("phenotype-gene-evidence.{short}.parquet"),
            ),
            (
                active.catalog_file.as_deref(),
                format!("phenotype-field-catalog.{short}.json"),
            ),
        ] {
            if name.is_some_and(|name| name != expected) {
                return Err("phenotype profile contains an invalid active generation".into());
            }
        }
    }
    if profile
        .observed
        .len()
        .saturating_add(profile.conditions.len())
        .saturating_add(profile.pathways.len())
        > MAX_PROFILE_TERMS
        || profile.genes.len() > MAX_PROFILE_GENES
    {
        return Err("gene profile contains too many selections".into());
    }
    Ok(())
}

fn invalid_profile_label(label: &str) -> bool {
    label.trim().is_empty() || label.len() > 300 || label.chars().any(char::is_control)
}

fn valid_gene_symbol(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
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

fn remove_legacy_query_files(root: &Path) {
    if let Ok(entries) = fs::read_dir(root) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let legacy = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    (name.starts_with("phenotype-gene-evidence.")
                        && (name.ends_with(".parquet") || name.ends_with(".parquet.part")))
                        || (name.starts_with("phenotype-field-catalog.") && name.ends_with(".json"))
                });
            if legacy {
                let _ = fs::remove_file(path);
            }
        }
    }
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
    let profile_value: serde_json::Value = match serde_json::from_slice(&profile_bytes) {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    if profile_value["schemaVersion"].as_u64() != Some(u64::from(PROFILE_SCHEMA_VERSION)) {
        return Ok(Vec::new());
    }
    let _ = load(runs, run_id)?;
    Ok(vec![(
        "phenotypes.json".into(),
        "phenotype-profile",
        profile_file,
    )])
}

pub(crate) fn validate_portable_metadata(
    profile_bytes: &[u8],
    run_id: &str,
) -> Result<PhenotypeProfile, String> {
    if profile_bytes.is_empty() || profile_bytes.len() > MAX_PORTABLE_PROFILE_BYTES as usize {
        return Err("phenotype profile has an invalid size".into());
    }
    let profile: PhenotypeProfile = serde_json::from_slice(profile_bytes)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    validate_profile(&profile, run_id)?;
    Ok(profile)
}

pub(crate) fn install_portable_group(
    runs: &Path,
    run_id: &str,
    profile_file: &Path,
) -> Result<(), String> {
    let profile_bytes = fs::read(profile_file)
        .map_err(|error| format!("cannot read phenotype profile: {error}"))?;
    let profile_bytes = match validate_portable_metadata(&profile_bytes, run_id) {
        Ok(mut profile) => {
            if let Some(active) = profile.active_generation.as_mut() {
                active.evidence_file = None;
                active.catalog_file = None;
            }
            serde_json::to_vec(&profile)
                .map_err(|error| format!("cannot serialize imported phenotype profile: {error}"))?
        }
        Err(_) => serde_json::from_slice::<serde_json::Value>(&profile_bytes)
            .ok()
            .and_then(|value| value["schemaVersion"].as_u64())
            .map_or_else(
                || b"invalid imported Genes query".to_vec(),
                |schema| format!(r#"{{"schemaVersion":{schema}}}"#).into_bytes(),
            ),
    };
    let root = profile_path(runs, run_id)
        .parent()
        .ok_or("phenotype profile has no directory")?
        .to_path_buf();
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create phenotype result directory: {error}"))?;
    super::library_metadata::atomic_write(&profile_path(runs, run_id), &profile_bytes)?;
    let _ = fs::remove_file(profile_file);
    Ok(())
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
        || !(3..=6).contains(&manifest.assets.len())
        || manifest.assets.iter().any(|asset| {
            asset.kind.trim().is_empty()
                || asset.filename.contains(['/', '\\'])
                || asset.bytes == 0
                || !valid_asset_checksum(asset)
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
    let mut expected_assets = 3;
    match (
        manifest.mondo_release.as_deref(),
        manifest.mondo_release_url.as_deref(),
    ) {
        (None, None) => {}
        (Some(release), Some(release_url)) if valid_hpo_release_version(release) => {
            expected_assets += 1;
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
    match (
        manifest.hgnc_release.as_deref(),
        manifest.hgnc_release_url.as_deref(),
    ) {
        (None, None) => {}
        (Some(release), Some("https://www.genenames.org/download/"))
            if valid_hpo_release_version(release) =>
        {
            expected_assets += 2;
            for (kind, filename) in [
                ("gene-identities", "hgnc_complete_set.txt"),
                ("withdrawn-gene-identities", "withdrawn.txt"),
            ] {
                let matching = manifest
                    .assets
                    .iter()
                    .filter(|asset| asset.kind == kind && asset.filename == filename)
                    .collect::<Vec<_>>();
                let base = format!(
                    "https://storage.googleapis.com/public-download-files/hgnc/tsv/tsv/{filename}"
                );
                if matching.len() != 1 || !valid_hgnc_asset_url(&matching[0].url, &base) {
                    return Err(format!(
                        "phenotype knowledge is missing the official HGNC {filename} asset"
                    ));
                }
            }
        }
        _ => return Err("HGNC release metadata is incomplete".into()),
    }
    if manifest.assets.len() != expected_assets {
        return Err("phenotype knowledge contains unexpected assets".into());
    }
    Ok(())
}

fn valid_asset_checksum(asset: &HpoAsset) -> bool {
    let sha256 =
        asset.sha256.len() == 64 && asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
    let md5 = asset.md5.as_deref().is_some_and(|value| {
        value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    sha256 ^ md5
}

fn valid_hgnc_asset_url(url: &str, base: &str) -> bool {
    if url == base
        || url
            .strip_prefix(&format!("{base}?generation="))
            .is_some_and(|generation| {
                !generation.is_empty() && generation.bytes().all(|byte| byte.is_ascii_digit())
            })
    {
        return true;
    }
    let Some(stem) = base
        .rsplit('/')
        .next()
        .and_then(|filename| filename.strip_suffix(".txt"))
    else {
        return false;
    };
    let prefix = format!(
        "https://storage.googleapis.com/public-download-files/hgnc/archive/archive/monthly/tsv/{stem}_"
    );
    url.strip_prefix(&prefix)
        .and_then(|suffix| suffix.split_once(".txt?generation="))
        .is_some_and(|(release, generation)| {
            valid_hpo_release_version(release)
                && !generation.is_empty()
                && generation.bytes().all(|byte| byte.is_ascii_digit())
        })
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

fn hgnc_release_from_http_date(value: &str) -> Option<String> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    let month = match parts[2] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day = parts[1].parse::<u8>().ok()?;
    let year = parts[3].parse::<u16>().ok()?;
    let release = format!("{year:04}-{month:02}-{day:02}");
    valid_hpo_release_version(&release).then_some(release)
}

fn resolve_latest_hgnc_assets() -> Result<(String, Vec<HpoAsset>), String> {
    let client = super::http_client::source()
        .map_err(|error| format!("cannot create the HGNC release resolver: {error}"))?;
    let mut release = None;
    let mut assets = Vec::with_capacity(2);
    for (kind, filename) in [
        ("gene-identities", "hgnc_complete_set.txt"),
        ("withdrawn-gene-identities", "withdrawn.txt"),
    ] {
        let base_url =
            format!("https://storage.googleapis.com/public-download-files/hgnc/tsv/tsv/{filename}");
        let response = client
            .head(&base_url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| format!("cannot discover the current HGNC {filename}: {error}"))?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let bytes = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("HGNC did not report the {filename} size"))?;
        let generation = header("x-goog-generation")
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| format!("HGNC did not report a stable {filename} generation"))?;
        let md5 = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim_matches('"').to_ascii_lowercase())
            .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| format!("HGNC did not report a valid {filename} checksum"))?;
        let current_release = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .and_then(hgnc_release_from_http_date)
            .ok_or_else(|| format!("HGNC did not report a valid {filename} release date"))?;
        if release
            .as_ref()
            .is_some_and(|release| release != &current_release)
        {
            return Err("HGNC complete and withdrawn files are from different releases".into());
        }
        release = Some(current_release);
        assets.push(HpoAsset {
            kind: kind.into(),
            filename: filename.into(),
            url: format!("{base_url}?generation={generation}"),
            bytes,
            sha256: String::new(),
            md5: Some(md5),
        });
    }
    Ok((release.ok_or("HGNC release metadata is empty")?, assets))
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
            md5: None,
        });
    }
    let manifest = HpoAssetManifest {
        schema_version: 1,
        release: version,
        release_url: release.html_url,
        mondo_release: None,
        mondo_release_url: None,
        hgnc_release: None,
        hgnc_release_url: None,
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
        md5: None,
    });
    let (hgnc_release, hgnc_assets) = resolve_latest_hgnc_assets()?;
    manifest.hgnc_release = Some(hgnc_release);
    manifest.hgnc_release_url = Some("https://www.genenames.org/download/".into());
    manifest.assets.extend(hgnc_assets);
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
    verify_asset_checksum(&partial, asset)?;
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
    Ok(verify_asset_checksum(path, asset).is_ok())
}

fn verify_asset_checksum(path: &Path, asset: &HpoAsset) -> Result<(), String> {
    if !asset.sha256.is_empty() {
        return verify_sha256(path, &asset.sha256);
    }
    let expected = asset.md5.as_deref().ok_or("asset has no checksum")?;
    let actual = format!(
        "{:x}",
        md5::compute(fs::read(path).map_err(|error| {
            format!(
                "cannot read {} for checksum verification: {error}",
                asset.filename
            )
        })?)
    );
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!("{} failed MD5 verification", asset.filename))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_requires_at_least_one_gene_in_the_result() {
        assert!(require_result_overlap(1).is_ok());
        assert_eq!(
            require_result_overlap(0).unwrap_err(),
            "No resolved genes have variants in this result."
        );
    }

    #[test]
    fn condition_provenance_includes_the_disease_gene_source() {
        assert!(source_asset_used("disease-genes", false, true, false));
        assert!(source_asset_used("ontology", false, true, false));
        assert!(source_asset_used("disease-annotations", false, true, false));
        assert!(source_asset_used("mondo-ontology", false, true, false));
        assert!(source_asset_used("gene-identities", false, false, true));
    }

    #[test]
    fn source_checksum_changes_the_active_query_fingerprint() {
        let source = |sha256: String| SourceAsset {
            name: "hp.obo".into(),
            release: "same-release".into(),
            sha256,
        };
        let first = active_query_fingerprint(&[], &[], &[], &[], &[source("a".repeat(64))]);
        let second = active_query_fingerprint(&[], &[], &[], &[], &[source("b".repeat(64))]);
        assert_ne!(first, second);
    }

    #[test]
    fn unsupported_or_invalid_saved_queries_are_rejected() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-unsupported-phenotype-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = runs.join(".annocat-library").join("run-1");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("phenotypes.json"), br#"{"schemaVersion":5}"#).unwrap();
        assert!(load(&runs, "run-1").is_err());
        fs::write(root.join("phenotypes.json"), b"not json").unwrap();
        assert!(load(&runs, "run-1").is_err());
        fs::remove_dir_all(runs).unwrap();
    }

    #[test]
    fn imported_unsupported_query_remains_inactive_and_clearable() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-imported-unsupported-phenotype-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&runs).unwrap();
        let imported = runs.join("imported-phenotypes.json");
        fs::write(&imported, br#"{"schemaVersion":5}"#).unwrap();

        install_portable_group(&runs, "run-1", &imported).unwrap();

        assert!(!imported.exists());
        assert!(profile_path(&runs, "run-1").is_file());
        assert_eq!(
            fs::read(profile_path(&runs, "run-1")).unwrap(),
            br#"{"schemaVersion":5}"#
        );
        assert!(
            load(&runs, "run-1")
                .unwrap_err()
                .contains("unsupported Genes query schema 5")
        );
        fs::remove_dir_all(runs).unwrap();
    }

    #[test]
    fn large_pasted_gene_lists_use_one_exact_resolver() {
        let report = (0..=MAX_PROFILE_TERMS)
            .map(|index| (format!("GENE{index}"), format!("ENSG{index:011}")))
            .collect::<Vec<_>>();
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        for (symbol, gene_id) in report {
            let matches = exact_gene_matches(&resolver, &symbol);
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].label, symbol);
            assert_eq!(matches[0].id, gene_id);
        }
    }

    #[test]
    fn pasted_gene_lists_can_exceed_the_ontology_term_limit() {
        let entries = (0..=MAX_PROFILE_TERMS)
            .map(|index| format!("UNKNOWN{index}"))
            .collect::<Vec<_>>();
        let response = resolve_terms(
            Path::new("missing"),
            None,
            TermResolutionRequest {
                entries,
                run_id: None,
            },
        )
        .unwrap();
        assert_eq!(response.not_recognized.len(), MAX_PROFILE_TERMS + 1);
    }

    #[test]
    fn pasted_gene_symbols_and_identifiers_are_exact_matches() {
        let mut gene = TermSearchResult {
            id: "ENSG00000141510".into(),
            label: "TP53".into(),
            term_type: "gene".into(),
            matched_text: "TP53".into(),
            match_kind: "geneSymbol".into(),
            synonym_scope: None,
            subtype_count: None,
            gene_count: None,
            synonyms: Vec::new(),
            symbol: Some("TP53".into()),
            canonical_gene_id: Some("HGNC:11998".into()),
            result_gene_id: Some("ENSG00000141510".into()),
            identity_status: Some("hgnc".into()),
        };
        assert!(exact_term_match(&gene, "tp53"));
        gene.match_kind = "geneIdentifier".into();
        assert!(exact_term_match(&gene, "ENSG00000141510"));
        assert!(!exact_term_match(&gene, "TP5"));
    }

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
            disease_gene_association_count: 0,
        }
    }

    fn disease(
        id: &str,
        positive: &[usize],
        symbol: &str,
        association_type: &str,
    ) -> DiseaseProfile {
        DiseaseProfile {
            id: id.into(),
            name: id.into(),
            positive: positive.to_vec(),
            annotations: HashMap::new(),
            genes: vec![GeneAssociation {
                gene_id: format!("NCBIGene:{}", &id[id.find(':').unwrap_or(0) + 1..]),
                symbol: symbol.into(),
                association_type: association_type.into(),
                source: "test".into(),
            }],
        }
    }

    #[test]
    fn hpo_membership_is_exact_or_descendant_while_resnik_ranks_the_global_universe() {
        let mut knowledge = test_knowledge();
        knowledge.diseases = vec![
            disease("OMIM:1", &[4, 1], "GENE1", "MENDELIAN"),
            disease("OMIM:2", &[2], "GENE2", "MENDELIAN"),
            disease("OMIM:3", &[3], "GENE3", "MENDELIAN"),
            disease("OMIM:4", &[1], "POLY1", "POLYGENIC"),
        ];
        let report = ["GENE1", "GENE2", "POLY1"]
            .into_iter()
            .enumerate()
            .map(|(index, symbol)| (symbol.into(), format!("ENSG{index:011}")))
            .collect::<Vec<_>>();
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        let ranking = build_phenotype_ranking(&knowledge, &resolver, &[1], "test".into());
        assert_eq!(ranking.denominator, 3);
        assert_eq!(ranking.genes["SYMBOL:GENE1"].rank, 1);
        assert!(ranking.genes["SYMBOL:GENE1"].raw_score > 0.0);
        assert_eq!(ranking.genes["SYMBOL:GENE2"].rank, 2);
        assert_eq!(ranking.genes["SYMBOL:GENE2"].tie_count, 2);
        assert!(ranking.genes.contains_key("SYMBOL:GENE3"));
        assert!(!ranking.genes.contains_key("SYMBOL:POLY1"));

        let occurrences = report
            .iter()
            .map(
                |(symbol, gene_id)| super::super::results::ReportGeneOccurrence {
                    allele_id: String::new(),
                    gene_symbol: symbol.clone(),
                    gene_id: gene_id.clone(),
                },
            )
            .collect::<Vec<_>>();
        let resolved = resolve_gene_set(
            &resolver,
            &occurrences,
            Some(&knowledge),
            &[PhenotypeTerm {
                id: "HP:0001250".into(),
                label: "Seizure".into(),
            }],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &ranking,
        )
        .unwrap();
        assert_eq!(
            resolved
                .genes
                .values()
                .map(|gene| gene.identity.symbol.as_str())
                .collect::<Vec<_>>(),
            ["GENE1"]
        );
        let gene = &resolved.genes["SYMBOL:GENE1"];
        assert_eq!(gene.hpo_links.len(), 2);
        assert_eq!(
            gene.selected_matches["Feature:HP:0001250"].relation,
            "HPO link via exact disease annotation"
        );

        knowledge.diseases = vec![
            disease("OMIM:1", &[1], "GENE1", "MENDELIAN"),
            disease("OMIM:2", &[4], "GENE2", "MENDELIAN"),
            disease("OMIM:3", &[2], "GENE3", "MENDELIAN"),
        ];
        let exact_only = resolve_gene_set(
            &resolver,
            &occurrences,
            Some(&knowledge),
            &[PhenotypeTerm {
                id: "HP:0002197".into(),
                label: "Generalized-onset seizure".into(),
            }],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &PhenotypeRanking {
                hpo_release: "test".into(),
                query_count: 0,
                disease_profile_count: 0,
                denominator: 0,
                genes: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            exact_only
                .genes
                .values()
                .map(|gene| gene.identity.symbol.as_str())
                .collect::<Vec<_>>(),
            ["GENE2"]
        );
    }

    #[test]
    fn mondo_condition_evidence_retains_each_source_association() {
        let mut knowledge = test_knowledge();
        let association = |id: &str, association_type: &str| ConditionAssociation {
            id: id.into(),
            name: id.into(),
            genes: vec![GeneAssociation {
                gene_id: "ENSG00000000001".into(),
                symbol: "GENE1".into(),
                association_type: association_type.into(),
                source: format!("{id}-source"),
            }],
        };
        knowledge.condition_associations = vec![
            association("OMIM:1", "MENDELIAN"),
            association("OMIM:2", "POLYGENIC"),
        ];
        let report = vec![("GENE1".into(), "ENSG00000000001".into())];
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        let occurrence = super::super::results::ReportGeneOccurrence {
            allele_id: "allele-1".into(),
            gene_symbol: report[0].0.clone(),
            gene_id: report[0].1.clone(),
        };
        let matched = |matched_id: &str, relation| crate::mondo::DiseaseConditionMatch {
            selected_id: "MONDO:0000001".into(),
            selected_label: "Selected condition".into(),
            matched_id: matched_id.into(),
            matched_label: matched_id.into(),
            relation,
        };
        let condition_matches = HashMap::from([
            (
                "OMIM:1".into(),
                vec![matched("MONDO:0000001", "Exact condition")],
            ),
            (
                "OMIM:2".into(),
                vec![matched("MONDO:0000002", "Condition subtype")],
            ),
        ]);
        let ranking = PhenotypeRanking {
            hpo_release: String::new(),
            query_count: 0,
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        };
        let resolved = resolve_gene_set(
            &resolver,
            &[occurrence],
            Some(&knowledge),
            &[],
            &[PhenotypeTerm {
                id: "MONDO:0000001".into(),
                label: "Selected condition".into(),
            }],
            &[],
            &[],
            &condition_matches,
            &ranking,
        )
        .unwrap();
        let gene = &resolved.genes["SYMBOL:GENE1"];
        assert_eq!(gene.condition_links.len(), 1);
        assert_eq!(gene.condition_evidence_links.len(), 2);
        assert_eq!(
            gene.selected_matches["Condition:MONDO:0000001"].relation,
            "Exact condition · MENDELIAN"
        );
    }

    #[test]
    fn resnik_ranking_uses_fixed_ic_best_match_averaging_and_competition_ties() {
        let mut knowledge = test_knowledge();
        let mut second = disease("OMIM:2", &[1], "GENE2", "MENDELIAN");
        second.genes.push(GeneAssociation {
            gene_id: "".into(),
            symbol: "GENE1".into(),
            association_type: "MENDELIAN".into(),
            source: "test".into(),
        });
        let mut conflicting = disease("OMIM:5", &[4], "GENE2", "MENDELIAN");
        conflicting.genes[0].gene_id = "ENSG00000000000".into();
        knowledge.diseases = vec![
            disease("OMIM:1", &[4], "GENE1", "MENDELIAN"),
            second,
            disease("OMIM:3", &[2], "GENE3", "MENDELIAN"),
            disease("OMIM:4", &[3], "GENE4", "MENDELIAN"),
            conflicting,
        ];
        let report = ["GENE1", "GENE2", "GENE3", "GENE4"]
            .into_iter()
            .enumerate()
            .map(|(index, symbol)| (symbol.into(), format!("ENSG{index:011}")))
            .collect::<Vec<_>>();
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        let ranking = build_phenotype_ranking(&knowledge, &resolver, &[2, 4], "test".into());

        assert_eq!(ranking.query_count, 2);
        assert_eq!(ranking.denominator, 4);
        let expected_top = std::f64::consts::LN_2;
        let expected_key = (expected_top * 1_000_000_000_000.0 + 0.5).floor() as u64;
        for symbol in ["GENE1", "GENE3"] {
            let rank = &ranking.genes[&format!("SYMBOL:{symbol}")];
            assert!((rank.raw_score - expected_top).abs() < 1e-12);
            assert_eq!(rank.score_key, expected_key);
            assert_eq!(rank.rank, 1);
            assert_eq!(rank.tie_count, 2);
        }
        assert_eq!(ranking.genes["SYMBOL:GENE1"].best_disease_id, "OMIM:1");
        assert_eq!(ranking.genes["SYMBOL:GENE2"].rank, 3);
        assert!((ranking.genes["SYMBOL:GENE2"].raw_score - expected_top / 2.0).abs() < 1e-12);
        assert_eq!(ranking.genes["SYMBOL:GENE4"].rank, 4);
        assert_eq!(ranking.genes["SYMBOL:GENE4"].raw_score, 0.0);
        assert_eq!(
            ranking.genes["SYMBOL:GENE1"].matched_terms[0].query.id,
            "HP:0001263"
        );

        let unsupported = knowledge.terms.len();
        knowledge.terms.push(OntologyTerm {
            id: "HP:9999999".into(),
            label: "Unannotated feature".into(),
            synonyms: Vec::new(),
            search_text: "unannotated feature".into(),
            parents: vec![0],
            ancestors: vec![0, unsupported],
            obsolete: false,
            replacement: None,
        });
        knowledge
            .term_index
            .insert("HP:9999999".into(), unsupported);
        let zero = build_phenotype_ranking(&knowledge, &resolver, &[unsupported], "test".into());
        assert_eq!(zero.denominator, 4);
        assert!(zero.genes.values().all(|rank| rank.raw_score == 0.0));
        assert!(zero.genes.values().all(|rank| rank.raw_score.is_finite()));
        assert!(
            zero.genes
                .values()
                .all(|rank| rank.rank == 1 && rank.tie_count == 4)
        );
    }

    #[test]
    fn allele_gene_matches_deduplicate_selected_items_but_keep_gene_rows() {
        let root = std::env::temp_dir().join(format!(
            "annocat-gene-match-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let variants = root.join("variants.parquet");
        let consequences = root.join("consequences.parquet");
        let report = vec![
            ("GENE1".into(), "ENSG00000000001".into()),
            ("GENE2".into(), "ENSG00000000002".into()),
        ];
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        let occurrences = report
            .iter()
            .map(
                |(symbol, gene_id)| super::super::results::ReportGeneOccurrence {
                    allele_id: "allele-1".into(),
                    gene_symbol: symbol.clone(),
                    gene_id: gene_id.clone(),
                },
            )
            .collect::<Vec<_>>();
        let pathways = vec![
            crate::reactome::Pathway {
                id: "R-HSA-1".into(),
                label: "First pathway".into(),
                genes: vec!["GENE1".into(), "GENE2".into()],
            },
            crate::reactome::Pathway {
                id: "R-HSA-2".into(),
                label: "Second pathway".into(),
                genes: vec!["GENE1".into()],
            },
        ];
        let ranking = PhenotypeRanking {
            hpo_release: String::new(),
            query_count: 0,
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        };
        let resolved = resolve_gene_set(
            &resolver,
            &occurrences,
            None,
            &[],
            &[],
            &pathways,
            &[],
            &HashMap::new(),
            &ranking,
        )
        .unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let destination = consequences.to_string_lossy().replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY (SELECT * FROM (VALUES
                    ('allele-1', 'GENE1', 'ENSG00000000001'),
                    ('allele-1', 'GENE2', 'ENSG00000000002')
                 ) AS t(allele_id, gene_symbol, gene_id))
                 TO '{destination}' (FORMAT PARQUET)"
            ))
            .unwrap();
        let prepared = PreparedGeneProfile {
            observed: Vec::new(),
            conditions: Vec::new(),
            pathways: vec![
                PhenotypeTerm {
                    id: "R-HSA-1".into(),
                    label: "First pathway".into(),
                },
                PhenotypeTerm {
                    id: "R-HSA-2".into(),
                    label: "Second pathway".into(),
                },
            ],
            genes: Vec::new(),
            ranking,
            source_assets: Vec::new(),
            resolved,
            identity: resolver,
        };
        let active = build_active_gene_query(&variants, &prepared, "a".repeat(64)).unwrap();
        register_active_query(&connection, &variants, &active).unwrap();
        let gene_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM annocat_active_allele_genes matched
                 JOIN annocat_active_gene_matches item USING(gene_key)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let selected_items: i64 = connection
            .query_row(
                "SELECT count(*) FROM (
                    SELECT item_type, item_id
                    FROM annocat_active_allele_genes matched
                    JOIN annocat_active_gene_matches item USING(gene_key)
                    GROUP BY item_type, item_id
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gene_rows, 3);
        assert_eq!(selected_items, 2);
        assert!(!root.join("phenotype-gene-evidence.test.parquet").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_query_matches_nonrepresentative_consequences_and_exports_the_same_value() {
        let root = std::env::temp_dir().join(format!(
            "annocat-live-gene-query-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        let variants = root.join("variants.parquet");
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG00000000001|ENST1|A/G|YES\n1\t200\t.\tC\tT\t50\tPASS\tCSQ=T|missense_variant|MODERATE|GENE3|ENSG00000000003|ENST3|C/T|YES\n",
        )
        .unwrap();
        crate::results::convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let mut statement = connection
            .prepare("SELECT allele_id FROM read_parquet(?) ORDER BY position")
            .unwrap();
        let allele_ids = statement
            .query_map(params![variants.to_string_lossy().as_ref()], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let consequences_path = consequences.to_string_lossy().replace('\'', "''");
        let first_allele_sql = allele_ids[0].replace('\'', "''");
        let second_allele_sql = allele_ids[1].replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY (SELECT * FROM (VALUES
                    ('{first_allele_sql}', 0, 'GENE1', 'ENSG00000000001',
                     '{{\"SYMBOL\":\"GENE1\",\"Gene\":\"ENSG00000000001\",\"Feature\":\"ENST1\",\"Consequence\":\"missense_variant\",\"IMPACT\":\"MODERATE\",\"CANONICAL\":\"YES\"}}'),
                    ('{first_allele_sql}', 1, 'GENE2', 'ENSG00000000002',
                     '{{\"SYMBOL\":\"GENE2\",\"Gene\":\"ENSG00000000002\",\"Feature\":\"ENST2\",\"Consequence\":\"intron_variant\",\"IMPACT\":\"MODIFIER\"}}'),
                    ('{second_allele_sql}', 0, 'GENE3', 'ENSG00000000003',
                     '{{\"SYMBOL\":\"GENE3\",\"Gene\":\"ENSG00000000003\",\"Feature\":\"ENST3\",\"Consequence\":\"missense_variant\",\"IMPACT\":\"MODERATE\",\"CANONICAL\":\"YES\"}}')
                 ) AS t(allele_id, ordinal, gene_symbol, gene_id, consequence_json))
                 TO '{consequences_path}' (FORMAT PARQUET)"
            ))
            .unwrap();
        let evidence_path = evidence.to_string_lossy().replace('\'', "''");
        connection
            .execute_batch(&format!(
                "COPY (
                    SELECT NULL::VARCHAR AS allele_id, NULL::VARCHAR AS consequence_id,
                           NULL::VARCHAR AS scope, NULL::VARCHAR AS source_id,
                           NULL::VARCHAR AS field_path, NULL::VARCHAR AS value_type,
                           NULL::VARCHAR AS string_value, NULL::BIGINT AS integer_value,
                           NULL::DOUBLE AS number_value, NULL::BOOLEAN AS boolean_value,
                           NULL::VARCHAR AS json_value WHERE false
                 ) TO '{evidence_path}' (FORMAT PARQUET)"
            ))
            .unwrap();

        let report = vec![
            ("GENE1".into(), "ENSG00000000001".into()),
            ("GENE2".into(), "ENSG00000000002".into()),
            ("GENE3".into(), "ENSG00000000003".into()),
        ];
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &report);
        let occurrences = report
            .iter()
            .map(
                |(gene_symbol, gene_id)| crate::results::ReportGeneOccurrence {
                    allele_id: String::new(),
                    gene_symbol: gene_symbol.clone(),
                    gene_id: gene_id.clone(),
                },
            )
            .collect::<Vec<_>>();
        let selected_pathway = crate::reactome::Pathway {
            id: "R-HSA-1".into(),
            label: "Only GENE2 pathway".into(),
            genes: vec!["GENE2".into(), "GENE3".into()],
        };
        let ranking = PhenotypeRanking {
            hpo_release: "test".into(),
            query_count: 1,
            disease_profile_count: 2,
            denominator: 2,
            genes: BTreeMap::from([
                (
                    "SYMBOL:GENE2".into(),
                    RankedGene {
                        rank: 2,
                        tie_count: 1,
                        raw_score: 1.0,
                        score_key: 1_000_000_000_000,
                        best_disease_id: "OMIM:2".into(),
                        best_disease_name: "Second profile".into(),
                        matched_terms: Vec::new(),
                    },
                ),
                (
                    "SYMBOL:GENE3".into(),
                    RankedGene {
                        rank: 1,
                        tie_count: 1,
                        raw_score: 2.0,
                        score_key: 2_000_000_000_000,
                        best_disease_id: "OMIM:1".into(),
                        best_disease_name: "First profile".into(),
                        matched_terms: Vec::new(),
                    },
                ),
            ]),
        };
        let resolved = resolve_gene_set(
            &resolver,
            &occurrences,
            None,
            &[],
            &[],
            std::slice::from_ref(&selected_pathway),
            &[],
            &HashMap::new(),
            &ranking,
        )
        .unwrap();
        let prepared = PreparedGeneProfile {
            observed: Vec::new(),
            conditions: Vec::new(),
            pathways: vec![PhenotypeTerm {
                id: selected_pathway.id,
                label: selected_pathway.label,
            }],
            genes: Vec::new(),
            ranking,
            source_assets: Vec::new(),
            resolved,
            identity: resolver,
        };
        let active = build_active_gene_query(&variants, &prepared, "b".repeat(64)).unwrap();
        let fields = crate::report_import::gene_catalog_fields(1);
        let gene_matches_index = fields
            .iter()
            .position(|field| field["fieldPath"] == "geneMatches")
            .unwrap();
        let gene_match_index = fields
            .iter()
            .position(|field| field["fieldPath"] == "geneMatch")
            .unwrap();
        let phenotype_rank_index = fields
            .iter()
            .position(|field| field["fieldPath"] == "phenotypeRank")
            .unwrap();
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({"schemaVersion": 1, "fields": fields})).unwrap(),
        )
        .unwrap();
        let request = crate::results::PageRequest {
            evidence_columns: vec![gene_matches_index],
            evidence_filters: vec![crate::results::EvidenceFilterRequest {
                index: gene_match_index,
                operator: "equals".into(),
                value: "true".into(),
                value2: String::new(),
                values: None,
                include_missing: None,
            }],
            known_total: Some(2),
            exact_total: true,
            ..crate::results::PageRequest::default()
        };
        let page: serde_json::Value = serde_json::from_str(
            &crate::results::page_json_with_active_gene_query(
                "live-nonrepresentative-test",
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &request,
                Some(&active),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["total"], 2);
        assert_eq!(page["rows"][0]["geneSymbol"], "GENE1");
        assert_eq!(
            page["rows"][0]["evidence"][gene_matches_index.to_string()],
            "Only GENE2 pathway"
        );

        let mut changed = active.clone();
        changed.fingerprint = "c".repeat(64);
        let gene3_key = changed
            .aliases
            .iter()
            .find(|alias| alias.gene_symbol == "GENE3")
            .unwrap()
            .gene_key
            .clone();
        changed.aliases.retain(|alias| alias.gene_key == gene3_key);
        changed.genes.retain(|gene| gene.gene_key == gene3_key);
        changed.fields.retain(|field| field.gene_key == gene3_key);
        changed
            .matches
            .retain(|matched| matched.gene_key == gene3_key);
        let changed_page: serde_json::Value = serde_json::from_str(
            &crate::results::page_json_with_active_gene_query(
                "live-nonrepresentative-test",
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &request,
                Some(&changed),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(changed_page["rows"].as_array().unwrap().len(), 1);
        assert_eq!(changed_page["rows"][0]["position"], 200);

        let ranked_request = crate::results::PageRequest {
            evidence_columns: vec![gene_matches_index, phenotype_rank_index],
            sort_evidence: Some(phenotype_rank_index),
            direction: "asc".into(),
            evidence_filters: request.evidence_filters.clone(),
            exact_total: true,
            ..crate::results::PageRequest::default()
        };
        let ranked: serde_json::Value = serde_json::from_str(
            &crate::results::page_json_with_active_gene_query(
                "live-rank-sort-test",
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &ranked_request,
                Some(&active),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(ranked["rows"][0]["position"], 200);
        assert_eq!(
            ranked["rows"][0]["evidence"][phenotype_rank_index.to_string()],
            "1"
        );
        assert_eq!(ranked["rows"][1]["position"], 100);
        assert_eq!(
            ranked["rows"][1]["evidence"][phenotype_rank_index.to_string()],
            "2"
        );

        let export = root.join("filtered.csv");
        let columns = vec!["gene".into(), format!("evidence:{gene_matches_index}")];
        crate::results::export_filtered_rows_with_active_gene_query_and_labels(
            &variants,
            Some(&evidence),
            Some(&catalog),
            &export,
            &request,
            &columns,
            &[],
            Some(&active),
        )
        .unwrap();
        let exported = String::from_utf8(fs::read(export).unwrap()).unwrap();
        assert!(exported.contains("GENE1"));
        assert!(exported.contains("Only GENE2 pathway"));
        assert!(!root.join("phenotype-gene-evidence.parquet").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "set ANNOCAT_HPO_FIXTURE_ROOT to the root containing raw/hp.obo and HPO tables"]
    fn official_hpo_known_cases_rank_within_top_twenty() {
        let root = PathBuf::from(std::env::var("ANNOCAT_HPO_FIXTURE_ROOT").unwrap());
        let knowledge = load_knowledge_from_root(&root).unwrap();
        let resolver = crate::gene_identity::Resolver::new(Path::new("missing"), &[]);
        let mut global_denominator = None;
        for (disease_id, target_symbol) in [("OMIM:607208", "SCN1A"), ("OMIM:108500", "CACNA1A")] {
            let disease = knowledge
                .diseases
                .iter()
                .find(|disease| disease.id == disease_id)
                .unwrap_or_else(|| panic!("official HPO fixture has no {disease_id} profile"));
            let ranking = build_phenotype_ranking(
                &knowledge,
                &resolver,
                &disease.positive,
                "official-fixture".into(),
            );
            let target = &ranking.genes[&format!("SYMBOL:{target_symbol}")];
            global_denominator.get_or_insert(ranking.denominator);
            let target_group_end = ranking
                .genes
                .values()
                .filter(|rank| rank.score_key >= target.score_key)
                .count();
            eprintln!(
                "{disease_id} {target_symbol}: rank {}, tie {}, group end {}, denominator {}",
                target.rank, target.tie_count, target_group_end, ranking.denominator
            );
            assert!(
                target_group_end <= 20,
                "{target_symbol} ended at {target_group_end} for {disease_id}"
            );
        }

        let empty_ranking = PhenotypeRanking {
            hpo_release: "official-fixture".into(),
            query_count: 0,
            disease_profile_count: 0,
            denominator: 0,
            genes: BTreeMap::new(),
        };
        let mut counts = Vec::new();
        for (id, label) in [
            ("HP:0001250", "Seizure"),
            ("HP:0004322", "Short stature"),
            ("HP:0001631", "Atrial septal defect"),
        ] {
            let resolved = resolve_gene_set(
                &resolver,
                &[],
                Some(&knowledge),
                &[PhenotypeTerm {
                    id: id.into(),
                    label: label.into(),
                }],
                &[],
                &[],
                &[],
                &HashMap::new(),
                &empty_ranking,
            )
            .unwrap();
            counts.push(resolved.included.len());
        }
        eprintln!("official HPO association counts: {counts:?}");
        assert_eq!(counts.iter().copied().collect::<HashSet<_>>().len(), 3);
        assert!(
            counts
                .iter()
                .all(|count| *count > 0 && *count < global_denominator.unwrap())
        );
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
        assert_eq!(manifest.assets.len(), 5);
        assert_eq!(
            manifest.assets.iter().map(|asset| asset.bytes).sum::<u64>(),
            65_562_828
        );
        assert_eq!(manifest.hgnc_release.as_deref(), Some("2026-08-07"));
        assert!(manifest.version_key().contains("+hgnc-2026-08-07"));
        assert!(manifest.assets.iter().any(|asset| {
            asset.kind == "gene-identities" && asset.filename == "hgnc_complete_set.txt"
        }));
    }

    #[test]
    fn hgnc_archive_asset_urls_are_accepted() {
        let base = "https://storage.googleapis.com/public-download-files/hgnc/tsv/tsv/hgnc_complete_set.txt";
        assert!(valid_hgnc_asset_url(
            "https://storage.googleapis.com/public-download-files/hgnc/archive/archive/monthly/tsv/hgnc_complete_set_2026-08-07.txt?generation=1786106235498772",
            base,
        ));
        assert!(!valid_hgnc_asset_url(
            "https://example.org/hgnc_complete_set_2026-08-07.txt?generation=1786106235498772",
            base,
        ));
        assert!(!valid_hgnc_asset_url(
            "https://storage.googleapis.com/public-download-files/hgnc/archive/archive/monthly/tsv/hgnc_complete_set_2026-08-07.txt?generation=current",
            base,
        ));
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
                    md5: None,
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
                hgnc_release: None,
                hgnc_release_url: None,
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
                    hgnc_release: None,
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
            hgnc_release: None,
            hgnc_release_url: None,
            assets: vec![HpoAsset {
                kind: "test".into(),
                filename: "asset.txt".into(),
                url: "https://example.test/asset.txt".into(),
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                md5: None,
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
        let md5_asset = HpoAsset {
            sha256: String::new(),
            md5: Some("900150983cd24fb0d6963f7d28e17f72".into()),
            ..manifest.assets[0].clone()
        };
        assert!(verified_asset(&root.join("raw").join("asset.txt"), &md5_asset).unwrap());
        fs::remove_dir_all(root).unwrap();
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
        let evidence = root.join("phenotype-gene-evidence.aaaaaaaaaaaaaaaa.parquet");
        let catalog = root.join("phenotype-field-catalog.aaaaaaaaaaaaaaaa.json");
        fs::write(&evidence, b"evidence").unwrap();
        fs::write(&catalog, b"catalog").unwrap();
        let mut profile = empty_profile("run-1");
        profile.observed.push(PhenotypeTerm {
            id: "HP:0001250".into(),
            label: "Seizure".into(),
        });
        profile.show_matches_only = true;
        profile.active_generation = Some(PhenotypeGeneration {
            fingerprint: "a".repeat(64),
            evidence_file: Some(evidence.file_name().unwrap().to_string_lossy().into_owned()),
            catalog_file: Some(catalog.file_name().unwrap().to_string_lossy().into_owned()),
            matched_gene_count: 1,
        });
        save(&runs, &profile).unwrap();

        update(
            &runs,
            &runs,
            "run-1",
            ProfileUpdate {
                action: "clear".into(),
                observed: Vec::new(),
                conditions: Vec::new(),
                pathways: Vec::new(),
                genes: Vec::new(),
                show_matches_only: false,
                preview_fingerprint: None,
            },
        )
        .unwrap();

        assert!(!profile_path(&runs, "run-1").exists());
        assert!(!evidence.exists());
        assert!(!catalog.exists());
        fs::remove_dir_all(runs).unwrap();
    }

    #[test]
    fn opening_a_result_removes_orphaned_legacy_query_files() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-hpo-orphaned-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = runs.join(".annocat-library").join("run-1");
        fs::create_dir_all(&root).unwrap();
        let evidence = root.join("phenotype-gene-evidence.aaaaaaaaaaaaaaaa.parquet");
        let partial = root.join("phenotype-gene-evidence.aaaaaaaaaaaaaaaa.parquet.part");
        let catalog = root.join("phenotype-field-catalog.aaaaaaaaaaaaaaaa.json");
        let canonical = root.join("evidence.parquet");
        for path in [&evidence, &partial, &catalog, &canonical] {
            fs::write(path, b"test").unwrap();
        }

        assert!(load_current(&runs, &runs, "run-1").is_ok());
        assert!(!evidence.exists());
        assert!(!partial.exists());
        assert!(!catalog.exists());
        assert!(canonical.exists());
        fs::remove_dir_all(runs).unwrap();
    }

    #[test]
    fn unsupported_profile_does_not_block_the_base_result_query() {
        let runs = std::env::temp_dir().join(format!(
            "annocat-hpo-unsupported-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = profile_path(&runs, "run-1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"schemaVersion":5}"#).unwrap();

        assert!(load_current(&runs, &runs, "run-1").is_err());
        assert!(
            active_query(&runs, &runs, "run-1", &runs.join("variants.parquet"))
                .unwrap()
                .is_none()
        );
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
    fn active_query_catalog_recommendations_follow_the_hpo_feature_count() {
        for (feature_count, expected) in [
            (0, vec!["geneMatches"]),
            (1, vec!["geneMatches"]),
            (2, vec!["phenotypeRank", "geneMatches"]),
        ] {
            let fields = crate::report_import::gene_catalog_fields(feature_count);
            assert_eq!(
                fields
                    .iter()
                    .filter(|field| field["recommended"] == true)
                    .map(|field| field["fieldPath"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                fields
                    .iter()
                    .any(|field| field["fieldPath"] == "phenotypeRank"),
                feature_count > 0
            );
            assert!(
                fields
                    .iter()
                    .filter(|field| field["selectable"] == false)
                    .all(|field| !field["fieldPath"]
                        .as_str()
                        .is_some_and(|path| path == "geneMatches" || path == "phenotypeRank"))
            );
        }
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
        assert_eq!(knowledge.diseases[0].positive.len(), 2);
        assert_eq!(
            knowledge.terms[knowledge.diseases[0].positive[0]].id,
            "HP:0001250"
        );
        assert_eq!(
            knowledge.terms[knowledge.diseases[0].positive[1]].id,
            "HP:0002197"
        );
        assert_eq!(knowledge.diseases[0].genes[0].symbol, "GENE1");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saved_gene_lists_replace_by_name_and_delete_atomically() {
        let root = std::env::temp_dir().join(format!(
            "annocat-gene-lists-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gene = |id: &str, label: &str| PhenotypeTerm {
            id: id.into(),
            label: label.into(),
        };
        let lists = update_saved_gene_lists(
            &root,
            SavedGeneListUpdate {
                action: "save".into(),
                name: "Migraine genes".into(),
                genes: vec![gene("ENSG2", "ATP1A2"), gene("ENSG1", "CACNA1A")],
            },
        )
        .unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].genes[0].label, "ATP1A2");

        let lists = update_saved_gene_lists(
            &root,
            SavedGeneListUpdate {
                action: "save".into(),
                name: "migraine genes".into(),
                genes: vec![gene("ENSG1", "CACNA1A")],
            },
        )
        .unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(saved_gene_lists(&root).unwrap()[0].genes.len(), 1);

        let lists = update_saved_gene_lists(
            &root,
            SavedGeneListUpdate {
                action: "delete".into(),
                name: "Migraine genes".into(),
                genes: Vec::new(),
            },
        )
        .unwrap();
        assert!(lists.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
