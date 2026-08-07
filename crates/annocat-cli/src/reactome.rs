use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

const READY_FILENAME: &str = "reactome-ready.json";
const PATHWAYS_FILENAME: &str = "reactome-pathways.gmt";
const MAX_GMT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ResolvedRelease {
    pub version: String,
    pub url: String,
    pub bytes: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadyManifest {
    schema_version: u16,
    pub release: String,
    pub installed_at: String,
    pub asset_bytes: u64,
    pub prepared_bytes: u64,
    pub pathway_count: usize,
    pub gene_count: usize,
    asset_sha256: String,
    prepared_sha256: String,
    source_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Pathway {
    pub id: String,
    pub label: String,
    pub genes: Vec<String>,
}

#[derive(Debug)]
pub struct Knowledge {
    pathways: Vec<Pathway>,
    by_id: HashMap<String, usize>,
}

fn knowledge_cache() -> &'static Mutex<HashMap<PathBuf, Arc<Knowledge>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Knowledge>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn resolve_latest_release() -> Result<ResolvedRelease, String> {
    let resource = annocat_core::source_catalog::resource("reactome")
        .ok_or("Reactome is missing from the source catalog")?;
    let response = super::http_client::source()?
        .head(&resource.release.primary_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot resolve the current Reactome release: {error}"))?;
    let bytes = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or("Reactome did not report the pathway asset size")?;
    let header = |name: reqwest::header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let last_modified = header(reqwest::header::LAST_MODIFIED);
    let etag = header(reqwest::header::ETAG);
    let version = last_modified
        .as_deref()
        .and_then(http_date_version)
        .or_else(|| etag.as_deref().map(safe_version))
        .ok_or("Reactome did not report a stable release identity")?;
    Ok(ResolvedRelease {
        version,
        url: resource.release.primary_url.clone(),
        bytes,
        etag,
        last_modified,
    })
}

fn http_date_version(value: &str) -> Option<String> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 6 {
        return None;
    }
    let month = match parts[2] {
        "Jan" => "01",
        "Feb" => "02",
        "Mar" => "03",
        "Apr" => "04",
        "May" => "05",
        "Jun" => "06",
        "Jul" => "07",
        "Aug" => "08",
        "Sep" => "09",
        "Oct" => "10",
        "Nov" => "11",
        "Dec" => "12",
        _ => return None,
    };
    let day = parts[1].parse::<u8>().ok()?;
    let year = parts[3].parse::<u16>().ok()?;
    let time = parts[4].replace(':', "");
    (time.len() == 6).then(|| format!("{year:04}{month}{day:02}-{time}"))
}

fn safe_version(value: &str) -> String {
    let version = value
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() {
                value
            } else {
                '-'
            }
        })
        .collect::<String>();
    version.trim_matches('-').to_owned()
}

pub fn installed_versions(resources: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(resources.join("reactome")) else {
        return Vec::new();
    };
    let mut versions = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| verified_status_at(&entry.path()).map(|ready| ready.release))
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    versions
}

pub fn installed_status(resources: &Path) -> Option<ReadyManifest> {
    installed_release(resources).map(|(_, ready)| ready)
}

fn installed_release(resources: &Path) -> Option<(PathBuf, ReadyManifest)> {
    fs::read_dir(resources.join("reactome"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| verified_status_at(&entry.path()).map(|ready| (entry.path(), ready)))
        .max_by(|left, right| left.1.release.cmp(&right.1.release))
}

fn verified_status_at(root: &Path) -> Option<ReadyManifest> {
    let bytes = fs::read(root.join(READY_FILENAME)).ok()?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return None;
    }
    let ready: ReadyManifest = serde_json::from_slice(&bytes).ok()?;
    if ready.schema_version != 1
        || root.file_name().and_then(|name| name.to_str()) != Some(&ready.release)
    {
        return None;
    }
    let asset = root.join("raw").join("ReactomePathways.gmt.zip");
    let prepared = root.join(PATHWAYS_FILENAME);
    let asset_size = fs::metadata(&asset).ok()?.len();
    let prepared_size = fs::metadata(&prepared).ok()?.len();
    if asset_size != ready.asset_bytes || prepared_size != ready.prepared_bytes {
        return None;
    }
    let asset_sha = super::fastvep::sha256_file(&asset).ok()?;
    let prepared_sha = super::fastvep::sha256_file(&prepared).ok()?;
    (asset_sha == ready.asset_sha256 && prepared_sha == ready.prepared_sha256).then_some(ready)
}

pub fn install(
    root: &Path,
    release: &ResolvedRelease,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(super::phenotype::InstallProgress),
) -> Result<ReadyManifest, String> {
    if root.file_name().and_then(|name| name.to_str()) != Some(&release.version) {
        return Err("Reactome installation directory does not match its release".into());
    }
    let raw = root.join("raw");
    fs::create_dir_all(&raw)
        .map_err(|error| format!("cannot create the Reactome resource directory: {error}"))?;
    let asset = raw.join("ReactomePathways.gmt.zip");
    if fs::metadata(&asset).map(|value| value.len()).ok() != Some(release.bytes) {
        download(release, &asset, cancelled, &mut progress)?;
    }
    ensure_running(cancelled)?;
    progress(super::phenotype::InstallProgress {
        phase: "preparing".into(),
        detail: "Validating Reactome pathways and gene members".into(),
        network_bytes: release.bytes,
        expected_network_bytes: release.bytes,
        parsed_records: 0,
        prepared_bytes: release.bytes,
    });
    let pathways = read_archive(&asset)?;
    let prepared = root.join(PATHWAYS_FILENAME);
    write_gmt(&prepared, &pathways)?;
    let verified = load_gmt(&prepared)?;
    if verified.pathways.len() != pathways.len() {
        return Err("Reactome pathway verification changed the pathway count".into());
    }
    let prepared_bytes = fs::metadata(&prepared)
        .map_err(|error| format!("cannot read the Reactome pathway cache: {error}"))?
        .len();
    let gene_count = pathways
        .iter()
        .flat_map(|pathway| pathway.genes.iter())
        .collect::<BTreeSet<_>>()
        .len();
    let ready = ReadyManifest {
        schema_version: 1,
        release: release.version.clone(),
        installed_at: super::annotation::current_timestamp(),
        asset_bytes: release.bytes,
        prepared_bytes,
        pathway_count: pathways.len(),
        gene_count,
        asset_sha256: super::fastvep::sha256_file(&asset)?,
        prepared_sha256: super::fastvep::sha256_file(&prepared)?,
        source_url: release.url.clone(),
        etag: release.etag.clone(),
        last_modified: release.last_modified.clone(),
    };
    super::library_metadata::atomic_write(
        &root.join(READY_FILENAME),
        &serde_json::to_vec_pretty(&ready)
            .map_err(|error| format!("cannot serialize the Reactome ready marker: {error}"))?,
    )?;
    knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(root.to_path_buf(), Arc::new(verified));
    progress(super::phenotype::InstallProgress {
        phase: "ready".into(),
        detail: format!(
            "Indexed {} pathways and {} gene symbols",
            ready.pathway_count, ready.gene_count
        ),
        network_bytes: release.bytes,
        expected_network_bytes: release.bytes,
        parsed_records: ready.pathway_count as u64,
        prepared_bytes,
    });
    Ok(ready)
}

fn download(
    release: &ResolvedRelease,
    target: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(super::phenotype::InstallProgress),
) -> Result<(), String> {
    let partial = target.with_extension("zip.part");
    let mut existing = fs::metadata(&partial).map(|value| value.len()).unwrap_or(0);
    if existing > release.bytes {
        fs::remove_file(&partial)
            .map_err(|error| format!("cannot reset the Reactome partial download: {error}"))?;
        existing = 0;
    }
    let client = super::http_client::source()?;
    let mut request = client.get(&release.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut response = request
        .send()
        .map_err(|error| format!("cannot download Reactome pathways: {error}"))?;
    if existing > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        existing = 0;
        response = client
            .get(&release.url)
            .send()
            .map_err(|error| format!("cannot restart the Reactome download: {error}"))?;
    }
    let mut response = response
        .error_for_status()
        .map_err(|error| format!("cannot download Reactome pathways: {error}"))?;
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .append(existing > 0)
        .truncate(existing == 0)
        .open(&partial)
        .map_err(|error| format!("cannot open the Reactome partial download: {error}"))?;
    let mut downloaded = existing;
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        ensure_running(cancelled)?;
        let count = response
            .read(&mut buffer)
            .map_err(|error| format!("cannot read the Reactome download: {error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write the Reactome download: {error}"))?;
        downloaded = downloaded.saturating_add(count as u64);
        progress(super::phenotype::InstallProgress {
            phase: "downloading".into(),
            detail: "Downloading Reactome pathway data".into(),
            network_bytes: downloaded,
            expected_network_bytes: release.bytes,
            parsed_records: 0,
            prepared_bytes: 0,
        });
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot flush the Reactome download: {error}"))?;
    drop(output);
    if downloaded != release.bytes {
        return Err(format!(
            "Reactome download size differs from the resolved release ({downloaded} != {})",
            release.bytes
        ));
    }
    super::library_metadata::publish_atomic_file(&partial, target)
        .map_err(|error| format!("cannot publish the Reactome download: {error}"))
}

fn ensure_running(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}

fn read_archive(path: &Path) -> Result<Vec<Pathway>, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot open the Reactome pathway archive: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("invalid Reactome pathway archive: {error}"))?;
    let index = (0..archive.len())
        .find(|index| {
            archive
                .by_index(*index)
                .ok()
                .is_some_and(|entry| entry.name().ends_with(".gmt"))
        })
        .ok_or("Reactome pathway archive has no GMT file")?;
    let entry = archive
        .by_index(index)
        .map_err(|error| format!("cannot open the Reactome GMT file: {error}"))?;
    if entry.size() > MAX_GMT_BYTES {
        return Err("Reactome GMT file exceeds its safety limit".into());
    }
    parse_gmt(BufReader::new(entry))
}

fn load_gmt(path: &Path) -> Result<Knowledge, String> {
    parse_gmt(BufReader::new(File::open(path).map_err(|error| {
        format!("cannot open Reactome pathways: {error}")
    })?))
    .map(Knowledge::new)
}

fn parse_gmt(reader: impl BufRead) -> Result<Vec<Pathway>, String> {
    let mut pathways = BTreeMap::<String, Pathway>::new();
    for (index, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|error| format!("cannot read Reactome GMT line {}: {error}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 3 {
            return Err(format!(
                "Reactome GMT line {} has fewer than three fields",
                index + 1
            ));
        }
        let label = fields[0].trim();
        let id = fields[1].trim();
        if label.is_empty() || !id.starts_with("R-HSA-") {
            continue;
        }
        let genes = fields[2..]
            .iter()
            .map(|gene| gene.trim().to_ascii_uppercase())
            .filter(|gene| !gene.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if genes.is_empty() {
            continue;
        }
        if pathways
            .insert(
                id.into(),
                Pathway {
                    id: id.into(),
                    label: label.into(),
                    genes,
                },
            )
            .is_some()
        {
            return Err(format!("Reactome GMT repeats pathway {id}"));
        }
    }
    if pathways.is_empty() {
        return Err("Reactome GMT contains no human pathways".into());
    }
    Ok(pathways.into_values().collect())
}

fn write_gmt(path: &Path, pathways: &[Pathway]) -> Result<(), String> {
    let mut bytes = Vec::new();
    for pathway in pathways {
        writeln!(
            bytes,
            "{}\t{}\t{}",
            pathway.label,
            pathway.id,
            pathway.genes.join("\t")
        )
        .map_err(|error| format!("cannot prepare Reactome pathways: {error}"))?;
    }
    super::library_metadata::atomic_write(path, &bytes)
}

impl Knowledge {
    fn new(pathways: Vec<Pathway>) -> Self {
        let by_id = pathways
            .iter()
            .enumerate()
            .map(|(index, pathway)| (pathway.id.to_ascii_uppercase(), index))
            .collect();
        Self { pathways, by_id }
    }

    pub fn canonical_pathways(
        &self,
        requested: &[super::phenotype::PhenotypeTerm],
    ) -> Result<Vec<Pathway>, String> {
        let mut selected = BTreeMap::new();
        for term in requested {
            let Some(&index) = self.by_id.get(&term.id.trim().to_ascii_uppercase()) else {
                return Err(format!("Reactome pathway {} is not available", term.id));
            };
            let pathway = self.pathways[index].clone();
            selected.insert(pathway.id.clone(), pathway);
        }
        Ok(selected.into_values().collect())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<super::phenotype::TermSearchResult> {
        let query = query.trim().to_ascii_lowercase();
        if query.len() < 2 {
            return Vec::new();
        }
        let mut matches = self
            .pathways
            .iter()
            .filter_map(|pathway| {
                let id = pathway.id.to_ascii_lowercase();
                let label = pathway.label.to_ascii_lowercase();
                let (score, kind) = if id == query {
                    (0, "identifier")
                } else if label == query {
                    (1, "label")
                } else if label.starts_with(&query) {
                    (2, "label")
                } else if label.contains(&query) {
                    (3, "label")
                } else {
                    return None;
                };
                Some((
                    score,
                    pathway.label.len(),
                    super::phenotype::TermSearchResult {
                        id: pathway.id.clone(),
                        label: pathway.label.clone(),
                        term_type: "pathway".into(),
                        matched_text: if kind == "identifier" {
                            pathway.id.clone()
                        } else {
                            pathway.label.clone()
                        },
                        match_kind: kind.into(),
                        synonym_scope: None,
                        subtype_count: None,
                        gene_count: Some(pathway.genes.len()),
                        synonyms: Vec::new(),
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
        matches
            .into_iter()
            .take(limit)
            .map(|(_, _, result)| result)
            .collect()
    }
}

pub fn knowledge(resources: &Path) -> Result<Arc<Knowledge>, String> {
    let (root, _) = installed_release(resources).ok_or_else(|| {
        "Reactome pathways are not installed. Install them from Data sources first.".to_string()
    })?;
    if let Some(cached) = knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&root)
        .cloned()
    {
        return Ok(cached);
    }
    let knowledge = Arc::new(load_gmt(&root.join(PATHWAYS_FILENAME))?);
    knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(root, knowledge.clone());
    Ok(knowledge)
}

pub fn search(
    resources: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<super::phenotype::TermSearchResult>, String> {
    Ok(knowledge(resources)?.search(query, limit.clamp(1, 100)))
}

pub fn verify_assets(resources: &Path) -> Result<serde_json::Value, String> {
    let (_, ready) = installed_release(resources)
        .ok_or("Reactome pathways are not installed or failed verification")?;
    Ok(serde_json::json!({
        "sourceId": "reactome",
        "verified": true,
        "scope": "size-and-sha256",
        "release": ready.release,
        "assetBytes": ready.asset_bytes,
        "pathwayCount": ready.pathway_count,
        "geneCount": ready.gene_count
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_reactome_gmt_and_deduplicates_genes() {
        let input =
            "Signaling by EGFR\tR-HSA-177929\tEGFR\tGRB2\tEGFR\nMouse pathway\tR-MMU-1\tA\tB\n";
        let pathways = parse_gmt(BufReader::new(input.as_bytes())).unwrap();
        assert_eq!(pathways.len(), 1);
        assert_eq!(pathways[0].id, "R-HSA-177929");
        assert_eq!(pathways[0].genes, ["EGFR", "GRB2"]);
    }

    #[test]
    fn converts_http_dates_to_ordered_release_keys() {
        assert_eq!(
            http_date_version("Sun, 21 Jun 2026 06:23:59 GMT").as_deref(),
            Some("20260621-062359")
        );
    }
}
