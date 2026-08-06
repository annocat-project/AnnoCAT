use flate2::read::GzDecoder;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

const ARCHIVE: &str = "GRCh38_no_alt_analysis_set.fna.gz";
const EXPECTED_BYTES: u64 = 872_949_833;
const VERSION: &str = "GCA_000001405.15";
const FASTA: &str = "GRCh38_no_alt_analysis_set.fna";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceStatus {
    pub state: String,
    pub phase: String,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
    pub detail: String,
    pub error: Option<String>,
}

#[derive(Clone)]
struct State {
    state: &'static str,
    completed: u64,
    total: u64,
    detail: String,
    error: Option<String>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static CANCEL: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            state: "idle",
            completed: 0,
            total: EXPECTED_BYTES,
            detail: String::new(),
            error: None,
        })
    })
}
fn archive(downloads: &Path) -> PathBuf {
    downloads.join(ARCHIVE)
}
fn install(resources: &Path) -> PathBuf {
    resources.join("reference").join("grch38").join(VERSION)
}
pub fn fasta_path(resources: &Path) -> PathBuf {
    install(resources).join(FASTA)
}
fn manifest(resources: &Path) -> PathBuf {
    install(resources).join("resource-manifest.json")
}
pub fn is_ready(resources: &Path) -> bool {
    let fasta = fasta_path(resources);
    let fai = fasta.with_extension("fna.fai");
    let valid_file = |path: &Path| path.metadata().is_ok_and(|value| value.len() > 0);
    fs::read(manifest(resources))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            value["resource_id"] == "grch38-reference"
                && value["version"] == VERSION
                && value["assembly"] == "GRCh38"
                && valid_file(&fasta)
                && valid_file(&fai)
        })
}

pub(crate) fn verify(resources: &Path) -> Result<serde_json::Value, String> {
    if !is_ready(resources) {
        return Err("GRCh38 reference is not ready".into());
    }
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(manifest(resources))
            .map_err(|error| format!("cannot read the GRCh38 reference manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid GRCh38 reference manifest: {error}"))?;
    let fasta = fasta_path(resources);
    let fai = fasta.with_extension("fna.fai");
    let stored = [
        ("FASTA", &fasta, "fasta_bytes", "fasta_sha256"),
        ("FAI", &fai, "fai_bytes", "fai_sha256"),
    ];
    let hash_field_count = ["fasta_bytes", "fasta_sha256", "fai_bytes", "fai_sha256"]
        .into_iter()
        .filter(|key| value.get(*key).is_some_and(|value| !value.is_null()))
        .count();
    if !matches!(hash_field_count, 0 | 4) {
        return Err("GRCh38 reference manifest has incomplete size or hash metadata".into());
    }
    let has_hashes = hash_field_count == 4;
    if has_hashes {
        for (label, path, bytes_key, hash_key) in stored {
            let expected_bytes = value[bytes_key].as_u64().unwrap();
            let actual_bytes = fs::metadata(path)
                .map_err(|error| format!("cannot read {label} metadata: {error}"))?
                .len();
            if actual_bytes != expected_bytes {
                return Err(format!(
                    "GRCh38 {label} size differs from its manifest ({actual_bytes} != {expected_bytes})"
                ));
            }
            let expected_hash = value[hash_key].as_str().unwrap();
            if !crate::fastvep::sha256_file(path)?.eq_ignore_ascii_case(expected_hash) {
                return Err(format!("GRCh38 {label} SHA-256 mismatch"));
            }
        }
    }
    Ok(serde_json::json!({
        "sourceId": "grch38-reference",
        "verified": true,
        "scope": if has_hashes { "size-and-sha256" } else { "portable-metadata" }
    }))
}
pub fn is_running() -> bool {
    state()
        .lock()
        .map(|s| s.state == "running")
        .unwrap_or(false)
}
pub fn should_prepare(downloads: &Path, resources: &Path) -> bool {
    archive(downloads).exists() && !is_ready(resources) && !is_running()
}

pub fn start_background(downloads: PathBuf, resources: PathBuf) -> Result<(), String> {
    let input = archive(&downloads);
    let size = input
        .metadata()
        .map_err(|e| format!("cannot read {}: {e}", input.display()))?
        .len();
    if size != EXPECTED_BYTES {
        return Err(format!(
            "reference archive size mismatch: expected {EXPECTED_BYTES}, found {size}"
        ));
    }
    if is_ready(&resources) {
        return Err("GRCh38 reference is already prepared".into());
    }
    {
        let mut s = state().lock().map_err(|_| "reference state lock failed")?;
        if s.state == "running" {
            return Err("GRCh38 preparation is already running".into());
        }
        *s = State {
            state: "running",
            completed: 0,
            total: EXPECTED_BYTES,
            detail: "Decompressing FASTA and building index".into(),
            error: None,
        };
    }
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let result = prepare(&input, &resources);
        if let Ok(mut s) = state().lock() {
            match result {
                Ok(()) => {
                    s.state = "ready";
                    s.completed = s.total;
                    s.detail = "Indexed GRCh38 reference ready".into();
                    crate::terminal_log("resources", "GRCh38 reference installed");
                }
                Err(e) if e == "cancelled" => {
                    s.state = "cancelled";
                    s.detail = "Preparation can be resumed by restarting it".into();
                }
                Err(e) => {
                    s.state = "failed";
                    s.error = Some(e.clone());
                    crate::terminal_log("resources", format!("GRCh38 installation failed: {e}"));
                }
            }
        }
    });
    Ok(())
}

pub fn cancel_background() -> bool {
    let running = is_running();
    if running {
        CANCEL.store(true, Ordering::SeqCst);
    }
    running
}

pub fn forget() {
    if !is_running() {
        CANCEL.store(false, Ordering::SeqCst);
        if let Ok(mut current) = state().lock() {
            *current = State {
                state: "idle",
                completed: 0,
                total: EXPECTED_BYTES,
                detail: String::new(),
                error: None,
            };
        }
    }
}

pub fn status(downloads: &Path, resources: &Path) -> ReferenceStatus {
    if is_ready(resources) {
        return ReferenceStatus {
            state: "ready".into(),
            phase: "ready".into(),
            completed_bytes: 1,
            total_bytes: 1,
            percent: 100.0,
            detail: "Indexed GRCh38 reference ready".into(),
            error: None,
        };
    }
    let downloaded = archive(downloads).metadata().map(|m| m.len()).unwrap_or(0);
    let s = state().lock().map(|s| s.clone()).unwrap_or(State {
        state: "failed",
        completed: 0,
        total: EXPECTED_BYTES,
        detail: String::new(),
        error: Some("state lock failed".into()),
    });
    let effective = if s.state == "idle" && downloaded == EXPECTED_BYTES {
        "downloaded"
    } else {
        s.state
    };
    let percent = if s.total == 0 {
        0.0
    } else {
        s.completed as f64 * 100.0 / s.total as f64
    };
    ReferenceStatus {
        state: effective.into(),
        phase: "preparing-reference".into(),
        completed_bytes: s.completed,
        total_bytes: s.total,
        percent,
        detail: s.detail,
        error: s.error,
    }
}

struct CountingReader {
    inner: File,
    count: u64,
}
impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        self.count += n as u64;
        if let Ok(mut s) = state().lock() {
            s.completed = self.count;
        }
        Ok(n)
    }
}

#[derive(Serialize)]
struct Manifest {
    resource_id: &'static str,
    version: &'static str,
    assembly: &'static str,
    source_archive: &'static str,
    fasta: &'static str,
    fai: &'static str,
    fasta_bytes: u64,
    fasta_sha256: String,
    fai_bytes: u64,
    fai_sha256: String,
    validation: &'static str,
}

fn prepare(input: &Path, resources: &Path) -> Result<(), String> {
    let target = install(resources);
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let fasta = fasta_path(resources);
    let partial = fasta.with_extension("fna.partial");
    let fai_partial = fasta.with_extension("fna.fai.partial");
    let source = CountingReader {
        inner: File::open(input).map_err(|e| e.to_string())?,
        count: 0,
    };
    let mut reader = BufReader::with_capacity(1024 * 1024, GzDecoder::new(source));
    let mut output = BufWriter::with_capacity(
        1024 * 1024,
        File::create(&partial).map_err(|e| e.to_string())?,
    );
    let mut index = BufWriter::new(File::create(&fai_partial).map_err(|e| e.to_string())?);
    let mut line = Vec::with_capacity(128);
    let mut output_offset = 0_u64;
    let mut name: Option<String> = None;
    let mut length = 0_u64;
    let mut sequence_offset = 0_u64;
    let mut line_bases = 0_u64;
    let mut line_width = 0_u64;
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.starts_with(b">") {
            if let Some(previous) = name.take() {
                writeln!(
                    index,
                    "{previous}\t{length}\t{sequence_offset}\t{line_bases}\t{line_width}"
                )
                .map_err(|e| e.to_string())?;
            }
            name = Some(
                String::from_utf8_lossy(&line[1..])
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
            length = 0;
            output
                .write_all(&line)
                .and_then(|_| output.write_all(b"\n"))
                .map_err(|e| e.to_string())?;
            output_offset += line.len() as u64 + 1;
            sequence_offset = output_offset;
            line_bases = 0;
            line_width = 0;
        } else {
            if name.is_none() || line.is_empty() {
                return Err("invalid FASTA structure".into());
            }
            if line_bases == 0 {
                line_bases = line.len() as u64;
                line_width = line_bases + 1;
            }
            length += line.len() as u64;
            output
                .write_all(&line)
                .and_then(|_| output.write_all(b"\n"))
                .map_err(|e| e.to_string())?;
            output_offset += line.len() as u64 + 1;
        }
    }
    if let Some(previous) = name {
        writeln!(
            index,
            "{previous}\t{length}\t{sequence_offset}\t{line_bases}\t{line_width}"
        )
        .map_err(|e| e.to_string())?;
    }
    output.flush().map_err(|e| e.to_string())?;
    index.flush().map_err(|e| e.to_string())?;
    fs::rename(&partial, &fasta).map_err(|e| e.to_string())?;
    let fai = fasta.with_extension("fna.fai");
    fs::rename(&fai_partial, &fai).map_err(|e| e.to_string())?;
    let value = Manifest {
        resource_id: "grch38-reference",
        version: VERSION,
        assembly: "GRCh38",
        source_archive: ARCHIVE,
        fasta: FASTA,
        fai: "GRCh38_no_alt_analysis_set.fna.fai",
        fasta_bytes: fs::metadata(&fasta).map_err(|e| e.to_string())?.len(),
        fasta_sha256: crate::fastvep::sha256_file(&fasta)?,
        fai_bytes: fs::metadata(&fai).map_err(|e| e.to_string())?.len(),
        fai_sha256: crate::fastvep::sha256_file(&fai)?,
        validation: "expected compressed size + gzip decode + FASTA structure + generated FAI",
    };
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    crate::library_metadata::atomic_write(&manifest(resources), &bytes)
}
