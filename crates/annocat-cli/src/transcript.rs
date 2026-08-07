use flate2::read::MultiGzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static CANCEL: AtomicBool = AtomicBool::new(false);
const GENE_DICTIONARY_FILENAME: &str = "gene-dictionary.tsv";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneIdentity {
    pub symbol: String,
    pub gene_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptStatus {
    pub state: &'static str,
    pub phase: &'static str,
    pub detail: String,
    pub error: Option<String>,
}

fn state() -> &'static Mutex<TranscriptStatus> {
    static STATE: OnceLock<Mutex<TranscriptStatus>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(idle_state()))
}

fn idle_state() -> TranscriptStatus {
    TranscriptStatus {
        state: "idle",
        phase: "waiting",
        detail: "Matching Ensembl transcript cache is not installed".into(),
        error: None,
    }
}

pub fn is_ready(resources: &Path) -> bool {
    validate_installation(resources).is_ok()
}

pub(crate) fn verify(fastvep: &Path, resources: &Path) -> Result<serde_json::Value, String> {
    validate_installation(resources)?;
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(resources.join("transcript-cache").join("manifest.json"))
            .map_err(|error| format!("cannot read transcript cache manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid transcript cache manifest: {error}"))?;
    let cache = cache_path(resources);
    if !crate::fastvep::sha256_file(&cache)?.eq_ignore_ascii_case(&manifest.cache_sha256) {
        return Err("transcript cache SHA-256 mismatch".into());
    }
    let verification = verify_staged_cache(fastvep, &cache)?;
    Ok(serde_json::json!({
        "sourceId": "ensembl-gff3",
        "verified": true,
        "scope": "size-sha256-and-structure",
        "transcriptCount": verification.transcript_count,
        "cacheBytes": verification.cache_bytes
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptCacheVerification {
    schema_version: u64,
    cache_format: String,
    cache_bytes: u64,
    transcript_count: u64,
    coding_transcript_count: u64,
    coding_with_sequence_count: u64,
    primary_coding_missing_sequence_count: u64,
    non_primary_coding_missing_sequence_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u64,
    resource_id: String,
    assembly: String,
    ensembl_release: String,
    cache: String,
    cache_bytes: u64,
    cache_sha256: String,
    verification: TranscriptCacheVerification,
    builder_provenance: crate::cache_contract::BuilderProvenance,
}

fn validate_installation(resources: &Path) -> Result<(), String> {
    let manifest_path = resources.join("transcript-cache").join("manifest.json");
    let bytes = fs::read(&manifest_path)
        .map_err(|error| format!("transcript cache manifest is missing: {error}"))?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err("transcript cache manifest has an invalid size".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("transcript cache manifest is invalid: {error}"))?;
    if manifest.schema_version != 2
        || manifest.resource_id != "transcript-cache"
        || manifest.assembly != "GRCh38"
        || manifest.ensembl_release != "115"
        || manifest.cache != "ensembl-115.cache"
        || manifest.cache_bytes == 0
        || manifest.cache_sha256.len() != 64
        || !manifest
            .cache_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.verification.schema_version != 1
        || manifest.verification.cache_format != "FSTVEP02"
        || manifest.verification.cache_bytes != manifest.cache_bytes
        || manifest.verification.transcript_count == 0
        || manifest.verification.coding_transcript_count == 0
        || manifest.verification.coding_with_sequence_count == 0
        || manifest.verification.primary_coding_missing_sequence_count != 0
        || manifest.builder_provenance.repository.is_empty()
        || manifest.builder_provenance.commit.is_empty()
        || manifest.builder_provenance.binary_sha256.len() != 64
    {
        return Err("transcript cache manifest does not match Ensembl 115 on GRCh38".into());
    }
    let actual = fs::metadata(cache_path(resources))
        .map_err(|error| format!("transcript cache file is missing: {error}"))?
        .len();
    if actual != manifest.cache_bytes {
        return Err(format!(
            "transcript cache size differs from its build manifest ({actual} != {})",
            manifest.cache_bytes
        ));
    }
    Ok(())
}

pub fn cache_path(resources: &Path) -> PathBuf {
    resources.join("transcript-cache").join("ensembl-115.cache")
}

pub fn gene_dictionary(resources: &Path) -> Result<Vec<GeneIdentity>, String> {
    let path = resources
        .join("transcript-cache")
        .join(GENE_DICTIONARY_FILENAME);
    let metadata = fs::metadata(&path)
        .map_err(|_| "the installed transcript cache has no gene dictionary".to_string())?;
    if metadata.len() == 0 || metadata.len() > 16 * 1024 * 1024 {
        return Err("the transcript gene dictionary has an invalid size".into());
    }
    let reader = BufReader::new(
        File::open(&path).map_err(|error| format!("cannot open the gene dictionary: {error}"))?,
    );
    parse_gene_dictionary(reader)
}

fn parse_gene_dictionary(reader: impl BufRead) -> Result<Vec<GeneIdentity>, String> {
    let mut genes = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|error| format!("cannot read the gene dictionary: {error}"))?;
        let Some((symbol, gene_id)) = line.split_once('\t') else {
            return Err("the transcript gene dictionary is invalid".into());
        };
        if symbol.is_empty()
            || gene_id.is_empty()
            || symbol.chars().any(char::is_control)
            || !gene_id.starts_with("ENSG")
        {
            return Err("the transcript gene dictionary is invalid".into());
        }
        genes.push(GeneIdentity {
            symbol: symbol.to_owned(),
            gene_id: gene_id.to_owned(),
        });
    }
    if genes.is_empty() {
        return Err("the transcript gene dictionary is empty".into());
    }
    Ok(genes)
}

fn write_gene_dictionary(gff3: &Path, destination: &Path) -> Result<(), String> {
    let reader = BufReader::new(MultiGzDecoder::new(
        File::open(gff3).map_err(|error| format!("cannot open the Ensembl GFF3: {error}"))?,
    ));
    let mut genes = BTreeSet::new();
    for line in reader.lines() {
        let line = line.map_err(|error| format!("cannot read the Ensembl GFF3: {error}"))?;
        if line.starts_with('#') {
            continue;
        }
        let columns = line.split('\t').collect::<Vec<_>>();
        if columns.len() != 9 || columns[2] != "gene" {
            continue;
        }
        let attributes = columns[8]
            .split(';')
            .filter_map(|value| value.split_once('='))
            .collect::<std::collections::HashMap<_, _>>();
        let Some(gene_id) = attributes
            .get("ID")
            .and_then(|value| value.strip_prefix("gene:"))
            .filter(|value| value.starts_with("ENSG"))
        else {
            continue;
        };
        let symbol = attributes.get("Name").copied().unwrap_or(gene_id);
        if !symbol.is_empty()
            && !symbol.contains(['\t', '\n', '\r'])
            && !gene_id.contains(['\t', '\n', '\r'])
        {
            genes.insert((symbol.to_ascii_uppercase(), gene_id.to_owned()));
        }
    }
    if genes.is_empty() {
        return Err("the Ensembl GFF3 did not contain a usable gene dictionary".into());
    }
    let mut output = File::create(destination)
        .map_err(|error| format!("cannot create the transcript gene dictionary: {error}"))?;
    for (symbol, gene_id) in genes {
        writeln!(output, "{symbol}\t{gene_id}")
            .map_err(|error| format!("cannot write the transcript gene dictionary: {error}"))?;
    }
    output
        .flush()
        .map_err(|error| format!("cannot flush the transcript gene dictionary: {error}"))?;
    let dictionary = gene_dictionary_at(destination)?;
    if dictionary.is_empty() {
        return Err("the transcript gene dictionary did not pass verification".into());
    }
    Ok(())
}

fn gene_dictionary_at(path: &Path) -> Result<Vec<GeneIdentity>, String> {
    let reader = BufReader::new(
        File::open(path).map_err(|error| format!("cannot open the gene dictionary: {error}"))?,
    );
    parse_gene_dictionary(reader)
}

pub fn is_running() -> bool {
    state()
        .lock()
        .is_ok_and(|current| current.state == "running")
}

pub fn cancel_background() -> bool {
    if is_running() {
        CANCEL.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub fn status(resources: &Path) -> TranscriptStatus {
    let current = state()
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| idle_state());
    if current.state == "running" {
        return current;
    }
    if is_ready(resources) {
        return TranscriptStatus {
            state: "ready",
            phase: "ready",
            detail: "Ensembl 115 transcript cache ready".into(),
            error: None,
        };
    }
    if matches!(current.state, "failed" | "cancelled") {
        return current;
    }
    let manifest = resources.join("transcript-cache").join("manifest.json");
    if manifest.exists() {
        let error = validate_installation(resources)
            .err()
            .unwrap_or_else(|| "transcript cache validation failed".into());
        return TranscriptStatus {
            state: "failed",
            phase: "failed",
            detail: "The Ensembl transcript cache is incomplete or inconsistent".into(),
            error: Some(error),
        };
    }
    current
}

pub fn forget() {
    if let Ok(mut current) = state().lock()
        && current.state != "running"
    {
        *current = idle_state();
    }
}

pub fn start_background(
    fastvep: PathBuf,
    gff3: PathBuf,
    fasta: PathBuf,
    resources: PathBuf,
) -> Result<(), String> {
    if is_ready(&resources) {
        return Err("the transcript cache is already installed".into());
    }
    {
        let mut current = state().lock().map_err(|_| "transcript state lock failed")?;
        if current.state == "running" {
            return Err("transcript cache preparation is already running".into());
        }
        *current = TranscriptStatus {
            state: "running",
            phase: "building-cache",
            detail: "Building the Ensembl 115 binary transcript cache".into(),
            error: None,
        };
    }
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let result = build(&fastvep, &gff3, &fasta, &resources);
        if let Ok(mut current) = state().lock() {
            *current = match result {
                Ok(()) => TranscriptStatus {
                    state: "ready",
                    phase: "ready",
                    detail: "Ensembl 115 transcript cache ready".into(),
                    error: None,
                },
                Err(error) if error == "cancelled" => TranscriptStatus {
                    state: "cancelled",
                    phase: "cancelled",
                    detail: "Transcript cache installation was cancelled".into(),
                    error: None,
                },
                Err(error) => TranscriptStatus {
                    state: "failed",
                    phase: "failed",
                    detail: "Transcript cache preparation failed".into(),
                    error: Some(error),
                },
            };
        }
    });
    Ok(())
}

fn build(fastvep: &Path, gff3: &Path, fasta: &Path, resources: &Path) -> Result<(), String> {
    if !gff3.is_file() {
        return Err(format!(
            "downloaded Ensembl GFF3 is missing: {}",
            gff3.display()
        ));
    }
    if !fasta.is_file() {
        return Err(format!(
            "prepared GRCh38 FASTA is missing: {}",
            fasta.display()
        ));
    }
    let target = resources.join("transcript-cache");
    let staging = resources.join("transcript-cache.partial");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let cache = staging.join("ensembl-115.cache");
    let log_path = staging.join("fastvep.log");
    let log = fs::File::create(&log_path)
        .map_err(|error| format!("cannot create fastVEP transcript-cache log: {error}"))?;
    let mut child = Command::new(fastvep)
        .args(["cache", "--gff3"])
        .arg(format!("Ensembl={}", gff3.display()))
        .arg("--fasta")
        .arg(fasta)
        .arg("--output")
        .arg(&cache)
        .arg("--no-progress")
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("cannot start fastVEP cache builder: {error}"))?;
    let status = loop {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_dir_all(&staging);
            return Err("cancelled".into());
        }
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for fastVEP cache builder: {error}"))?
        {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        let detail = fs::read_to_string(&log_path).unwrap_or_default();
        return Err(if detail.trim().is_empty() {
            format!("fastVEP cache builder exited with {status}")
        } else {
            format!(
                "fastVEP cache builder exited with {status}: {}",
                detail.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        });
    }
    fs::remove_file(&log_path)
        .map_err(|error| format!("cannot remove fastVEP transcript-cache log: {error}"))?;
    let bytes = fs::metadata(&cache)
        .map_err(|error| error.to_string())?
        .len();
    if bytes == 0 {
        return Err("fastVEP produced an empty transcript cache".into());
    }
    if let Ok(mut current) = state().lock() {
        current.phase = "verifying-cache";
        current.detail = "Verifying the completed Ensembl 115 transcript cache".into();
    }
    let verification = verify_staged_cache(fastvep, &cache)?;
    if verification.cache_bytes != bytes {
        return Err(format!(
            "fastVEP verified {verified} cache bytes but the staged file contains {bytes}",
            verified = verification.cache_bytes
        ));
    }
    let cache_sha256 = crate::fastvep::sha256_file(&cache)?;
    write_gene_dictionary(gff3, &staging.join(GENE_DICTIONARY_FILENAME))?;
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "resourceId": "transcript-cache",
        "assembly": "GRCh38",
        "ensemblRelease": "115",
        "cache": "ensembl-115.cache",
        "cacheBytes": bytes,
        "cacheSha256": cache_sha256,
        "verification": verification,
        "builderProvenance": crate::fastvep::pinned_builder_provenance(),
        "validation": "fastVEP cache build succeeded; the staged cache was fully decoded and structurally verified before promotion"
    });
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    publish_replacement(resources, &staging, &target)
}

fn verify_staged_cache(
    fastvep: &Path,
    cache: &Path,
) -> Result<TranscriptCacheVerification, String> {
    let output = Command::new(fastvep)
        .args(["cache-verify", "--input"])
        .arg(cache)
        .arg("--require-primary-coding-sequences")
        .output()
        .map_err(|error| format!("cannot start fastVEP transcript-cache verifier: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            format!(
                "fastVEP transcript-cache verification exited with {}",
                output.status
            )
        } else {
            format!("fastVEP transcript-cache verification failed: {error}")
        });
    }
    if output.stdout.len() > 64 * 1024 {
        return Err("fastVEP transcript-cache verification report is too large".into());
    }
    let report: TranscriptCacheVerification = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("fastVEP returned an invalid verification report: {error}"))?;
    if report.schema_version != 1
        || report.cache_format != "FSTVEP02"
        || report.cache_bytes == 0
        || report.transcript_count == 0
        || report.coding_transcript_count == 0
        || report.coding_with_sequence_count == 0
        || report.primary_coding_missing_sequence_count != 0
    {
        return Err(
            "fastVEP transcript-cache verification report failed AnnoCAT's contract".into(),
        );
    }
    Ok(report)
}

fn publish_replacement(resources: &Path, staging: &Path, target: &Path) -> Result<(), String> {
    let backup = resources.join("transcript-cache.replaced");
    if backup.exists() {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("cannot remove stale transcript cache backup: {error}"))?;
    }
    let had_target = target.exists();
    if had_target {
        fs::rename(target, &backup).map_err(|error| {
            format!("cannot stage the existing transcript cache for replacement: {error}")
        })?;
    }
    if let Err(error) = fs::rename(staging, target) {
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        return Err(format!(
            "cannot publish the rebuilt transcript cache: {error}"
        ));
    }
    if let Err(error) = validate_installation(resources) {
        let _ = fs::remove_dir_all(target);
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        return Err(format!(
            "rebuilt transcript cache failed validation: {error}"
        ));
    }
    if had_target {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("cannot remove replaced transcript cache: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn gene_dictionary_requires_symbols_and_stable_ensembl_ids() {
        let genes = parse_gene_dictionary(Cursor::new(
            b"BRCA1\tENSG00000012048\nTP53\tENSG00000141510\n",
        ))
        .unwrap();
        assert_eq!(genes.len(), 2);
        assert_eq!(genes[0].symbol, "BRCA1");
        assert!(parse_gene_dictionary(Cursor::new(b"BRCA1\tbad-id\n")).is_err());
    }

    #[test]
    fn verified_rebuild_replaces_an_invalid_installed_cache() {
        let resources = std::env::temp_dir().join(format!(
            "annocat-transcript-replacement-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = resources.join("transcript-cache");
        let staging = resources.join("transcript-cache.partial");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&staging).unwrap();
        let gff3 = PathBuf::from(r"C:\original-machine\genes.gff3.gz");
        let fasta = PathBuf::from(r"C:\original-machine\reference.fna");
        fs::write(target.join("ensembl-115.cache"), b"bad").unwrap();
        fs::write(target.join("manifest.json"), b"{}").unwrap();
        let rebuilt = b"verified rebuilt cache";
        fs::write(staging.join("ensembl-115.cache"), rebuilt).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "resourceId": "transcript-cache",
            "assembly": "GRCh38",
            "ensemblRelease": "115",
            "gff3": gff3,
            "fasta": fasta,
            "cache": "ensembl-115.cache",
            "cacheBytes": rebuilt.len(),
            "cacheSha256": "a".repeat(64),
            "verification": {
                "schemaVersion": 1,
                "cacheFormat": "FSTVEP02",
                "cacheBytes": rebuilt.len(),
                "transcriptCount": 1,
                "codingTranscriptCount": 1,
                "codingWithSequenceCount": 1,
                "primaryCodingMissingSequenceCount": 0,
                "nonPrimaryCodingMissingSequenceCount": 0
            },
            "builderProvenance": {
                "repository": "https://example.invalid/fastvep",
                "commit": "test",
                "binarySha256": "b".repeat(64)
            }
        });
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        publish_replacement(&resources, &staging, &target).unwrap();

        assert_eq!(fs::read(cache_path(&resources)).unwrap(), rebuilt);
        assert!(!gff3.exists());
        assert!(!fasta.exists());
        assert!(validate_installation(&resources).is_ok());
        assert!(!resources.join("transcript-cache.replaced").exists());
        fs::remove_dir_all(resources).unwrap();
    }
}
