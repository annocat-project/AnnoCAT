use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static CANCEL: AtomicBool = AtomicBool::new(false);

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
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "resourceId": "transcript-cache",
        "assembly": "GRCh38",
        "ensemblRelease": "115",
        "gff3": gff3,
        "fasta": fasta,
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
