use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct State {
    state: &'static str,
    phase: &'static str,
    detail: String,
    error: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(idle_state()))
}

fn idle_state() -> State {
    State {
        state: "idle",
        phase: "Waiting",
        detail: "Matching Ensembl transcript cache is not installed".into(),
        error: None,
    }
}

pub fn is_ready(resources: &Path) -> bool {
    validate_installation(resources).is_ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u64,
    resource_id: String,
    assembly: String,
    ensembl_release: String,
    gff3: PathBuf,
    fasta: PathBuf,
    cache: String,
    cache_bytes: u64,
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
    if manifest.schema_version != 1
        || manifest.resource_id != "transcript-cache"
        || manifest.assembly != "GRCh38"
        || manifest.ensembl_release != "115"
        || manifest.cache != "ensembl-115.cache"
        || manifest.cache_bytes == 0
    {
        return Err("transcript cache manifest does not match Ensembl 115 on GRCh38".into());
    }
    if !manifest.gff3.is_file() {
        return Err(format!(
            "managed Ensembl GFF3 is missing: {}",
            manifest.gff3.display()
        ));
    }
    if !manifest.fasta.is_file() {
        return Err(format!(
            "managed GRCh38 FASTA is missing: {}",
            manifest.fasta.display()
        ));
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

pub fn status_json(resources: &Path) -> String {
    if is_ready(resources) {
        return r#"{"state":"ready","phase":"Ready","detail":"Ensembl 115 transcript cache ready","error":null}"#.into();
    }
    let manifest = resources.join("transcript-cache").join("manifest.json");
    if manifest.exists() && !is_running() {
        let error = validate_installation(resources)
            .err()
            .unwrap_or_else(|| "transcript cache validation failed".into());
        return serde_json::to_string(&State {
            state: "failed",
            phase: "Needs attention",
            detail: "The Ensembl transcript cache is incomplete or inconsistent".into(),
            error: Some(error),
        }).unwrap_or_else(|_| r#"{"state":"failed","phase":"Failed","detail":"State unavailable","error":"serialization failed"}"#.into());
    }
    serde_json::to_string(&state().lock().map(|value| value.clone()).unwrap_or_else(|_| idle_state()))
        .unwrap_or_else(|_| r#"{"state":"failed","phase":"Failed","detail":"State unavailable","error":"state lock failed"}"#.into())
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
        *current = State {
            state: "running",
            phase: "Building",
            detail: "Building the Ensembl 115 binary transcript cache".into(),
            error: None,
        };
    }
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let result = build(&fastvep, &gff3, &fasta, &resources);
        if let Ok(mut current) = state().lock() {
            *current = match result {
                Ok(()) => State {
                    state: "ready",
                    phase: "Ready",
                    detail: "Ensembl 115 transcript cache ready".into(),
                    error: None,
                },
                Err(error) if error == "cancelled" => State {
                    state: "cancelled",
                    phase: "Cancelled",
                    detail: "Transcript cache installation was cancelled".into(),
                    error: None,
                },
                Err(error) => State {
                    state: "failed",
                    phase: "Failed",
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
    let mut child = Command::new(fastvep)
        .args(["cache", "--gff3"])
        .arg(format!("Ensembl={}", gff3.display()))
        .arg("--fasta")
        .arg(fasta)
        .arg("--output")
        .arg(&cache)
        .arg("--no-progress")
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
        return Err(format!("fastVEP cache builder exited with {status}"));
    }
    let bytes = fs::metadata(&cache)
        .map_err(|error| error.to_string())?
        .len();
    if bytes == 0 {
        return Err("fastVEP produced an empty transcript cache".into());
    }
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "resourceId": "transcript-cache",
        "assembly": "GRCh38",
        "ensemblRelease": "115",
        "gff3": gff3,
        "fasta": fasta,
        "cache": "ensembl-115.cache",
        "cacheBytes": bytes,
        "validation": "fastVEP cache build succeeded and produced a non-empty binary cache"
    });
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if target.exists() {
        return Err("an existing transcript cache must be removed before replacement".into());
    }
    fs::rename(&staging, &target).map_err(|error| error.to_string())
}
