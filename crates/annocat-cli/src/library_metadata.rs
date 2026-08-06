use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

const MAX_NAME_BYTES: usize = 256;
const MAX_NOTES_BYTES: usize = 1_000_000;
static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub use crate::report_import::{CandidateEntry, CandidateOverlay};

fn atomic_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalMetadata {
    schema_version: u16,
    run_id: String,
    display_name: String,
    renamed_at: String,
}

pub fn display_name(runs: &Path, run_id: &str) -> Option<String> {
    let metadata: LocalMetadata =
        serde_json::from_slice(&fs::read(metadata_path(runs, run_id)).ok()?).ok()?;
    (metadata.schema_version == 1 && metadata.run_id == run_id).then_some(metadata.display_name)
}

pub fn rename(runs: &Path, run_id: &str, name: &str) -> Result<String, String> {
    validate_run_id(run_id)?;
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err("result name must be 1–256 characters without control characters".into());
    }
    for entry in fs::read_dir(runs)
        .map_err(|error| format!("cannot inspect result names: {error}"))?
        .flatten()
    {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir())
            || entry.file_name() == ".annocat-library"
        {
            continue;
        }
        let Ok(manifest) = fs::read(entry.path().join("manifest.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&manifest) else {
            continue;
        };
        let Some(other_id) = manifest["runId"].as_str() else {
            continue;
        };
        if other_id == run_id {
            continue;
        }
        let other_name =
            display_name(runs, other_id).or_else(|| manifest["name"].as_str().map(str::to_owned));
        if other_name
            .as_deref()
            .is_some_and(|other| other.eq_ignore_ascii_case(name))
        {
            return Err("another result already uses that name".into());
        }
    }
    let value = LocalMetadata {
        schema_version: 1,
        run_id: run_id.into(),
        display_name: name.into(),
        renamed_at: super::annotation::current_timestamp(),
    };
    atomic_write(
        &metadata_path(runs, run_id),
        &serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
    )?;
    Ok(name.into())
}

pub fn notes(runs: &Path, run_id: &str) -> Result<String, String> {
    validate_run_id(run_id)?;
    let path = notes_path(runs, run_id);
    if !path.exists() {
        return Ok(String::new());
    }
    let bytes = fs::read(&path).map_err(|error| format!("cannot read notes: {error}"))?;
    if bytes.len() > MAX_NOTES_BYTES {
        return Err("notes exceed the 1 MB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "notes are not valid UTF-8".into())
}

pub fn save_notes(runs: &Path, run_id: &str, notes: &str) -> Result<(), String> {
    validate_run_id(run_id)?;
    if notes.len() > MAX_NOTES_BYTES {
        return Err("notes exceed the 1 MB limit".into());
    }
    atomic_write(&notes_path(runs, run_id), notes.as_bytes())
}

pub fn candidates(runs: &Path, run_id: &str) -> Result<Vec<CandidateEntry>, String> {
    Ok(read_candidates(runs, run_id)?
        .candidates
        .into_values()
        .collect())
}

pub fn update_candidates(
    runs: &Path,
    run_id: &str,
    allele_ids: &[String],
    add: bool,
) -> Result<Vec<CandidateEntry>, String> {
    validate_run_id(run_id)?;
    if allele_ids.is_empty() || allele_ids.len() > 1_000 {
        return Err("candidate update needs between 1 and 1,000 allele IDs".into());
    }
    for allele_id in allele_ids {
        validate_allele_id(allele_id)?;
    }
    let mut overlay = read_candidates(runs, run_id)?;
    if add
        && overlay.candidates.len().saturating_add(allele_ids.len())
            > crate::report_import::MAX_CANDIDATES
    {
        return Err("an AnnoCAT result can contain at most 10,000 candidates".into());
    }
    let now = super::annotation::current_timestamp();
    for allele_id in allele_ids {
        if add {
            overlay
                .candidates
                .entry(allele_id.clone())
                .or_insert_with(|| CandidateEntry {
                    allele_id: allele_id.clone(),
                    added_at: now.clone(),
                    reason: "Added manually".into(),
                });
        } else {
            overlay.candidates.remove(allele_id);
        }
    }
    overlay.revision = overlay.revision.saturating_add(1);
    overlay.updated_at = now;
    write_candidate_snapshot(runs, &overlay)?;
    Ok(overlay.candidates.into_values().collect())
}

pub fn candidate_snapshot(runs: &Path, run_id: &str) -> Result<CandidateOverlay, String> {
    validate_run_id(run_id)?;
    let path = candidates_path(runs, run_id);
    if !path.exists() {
        return Ok(CandidateOverlay::empty(run_id));
    }
    let bytes = fs::read(&path).map_err(|error| format!("cannot read candidates: {error}"))?;
    crate::report_import::validate_candidate_bytes(&bytes, run_id)
}

pub fn write_candidate_snapshot(runs: &Path, overlay: &CandidateOverlay) -> Result<(), String> {
    let bytes = candidate_snapshot_bytes(overlay)?;
    atomic_write(&candidates_path(runs, &overlay.run_id), &bytes)
}

pub fn merge_candidate_snapshot(
    runs: &Path,
    imported: &CandidateOverlay,
    updated_at: &str,
) -> Result<bool, String> {
    crate::report_import::validate_candidate_overlay(imported, &imported.run_id)?;
    let mut local = candidate_snapshot(runs, &imported.run_id)?;
    let original_count = local.candidates.len();
    for (allele_id, entry) in &imported.candidates {
        local
            .candidates
            .entry(allele_id.clone())
            .or_insert_with(|| entry.clone());
    }
    if local.candidates.len() == original_count {
        return Ok(false);
    }
    local.revision = local.revision.saturating_add(1);
    local.updated_at = updated_at.into();
    write_candidate_snapshot(runs, &local)?;
    Ok(true)
}

pub fn remove_candidate_snapshot(runs: &Path, run_id: &str) -> Result<(), String> {
    validate_run_id(run_id)?;
    let path = candidates_path(runs, run_id);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|error| format!("cannot remove candidate bookmarks: {error}"))?;
    }
    if let Some(parent) = path.parent() {
        if parent
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(parent);
        }
    }
    Ok(())
}

pub fn remove_result_metadata(runs: &Path, run_id: &str) -> Result<(), String> {
    validate_run_id(run_id)?;
    let directory = library_directory(runs, run_id);
    if directory.exists() {
        fs::remove_dir_all(&directory)
            .map_err(|error| format!("cannot remove local result metadata: {error}"))?;
    }
    let library = runs.join(".annocat-library");
    if library
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_none())
    {
        let _ = fs::remove_dir(library);
    }
    Ok(())
}

fn candidate_snapshot_bytes(overlay: &CandidateOverlay) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec_pretty(overlay)
        .map_err(|error| format!("cannot serialize candidate bookmarks: {error}"))?;
    crate::report_import::validate_candidate_bytes(&bytes, &overlay.run_id)?;
    Ok(bytes)
}

fn read_candidates(runs: &Path, run_id: &str) -> Result<CandidateOverlay, String> {
    candidate_snapshot(runs, run_id)
}

fn library_directory(runs: &Path, run_id: &str) -> PathBuf {
    runs.join(".annocat-library").join(run_id)
}
fn metadata_path(runs: &Path, run_id: &str) -> PathBuf {
    library_directory(runs, run_id).join("metadata.json")
}
fn notes_path(runs: &Path, run_id: &str) -> PathBuf {
    library_directory(runs, run_id).join("case-notes.md")
}
fn candidates_path(runs: &Path, run_id: &str) -> PathBuf {
    library_directory(runs, run_id).join("candidates.json")
}
fn validate_allele_id(allele_id: &str) -> Result<(), String> {
    if allele_id.len() > 64
        || !allele_id.starts_with("allele-")
        || !allele_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err("invalid stable allele identifier".into())
    } else {
        Ok(())
    }
}

pub(crate) fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err("invalid run identifier".into())
    } else {
        Ok(())
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let _publish_guard = atomic_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let parent = path.parent().ok_or("local metadata path has no parent")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create local metadata directory: {error}"))?;
    let temporary = unique_temporary_path(path)?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write local metadata: {error}"))?;
    if let Err(error) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn unique_temporary_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("local metadata path has no parent")?;
    Ok(parent.join(format!(
        ".{}.{}.{}.partial",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("metadata"),
        std::process::id(),
        ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )))
}

pub(crate) fn publish_atomic_file(source: &Path, destination: &Path) -> Result<(), String> {
    let _publish_guard = atomic_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if source.parent() != destination.parent() {
        return Err("atomic file publication requires paths in the same directory".into());
    }
    replace_file(source, destination)
}

pub(crate) fn publish_cache_file(
    source: &Path,
    destination: &Path,
    is_valid: impl Fn(&Path) -> bool,
) -> Result<(), String> {
    match publish_atomic_file(source, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            for _ in 0..5 {
                if is_valid(destination) {
                    let _ = fs::remove_file(source);
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = fs::remove_file(source);
            Err(error)
        }
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(format!(
            "cannot publish local metadata: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("cannot publish local metadata: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rename_and_notes_are_separate_from_the_run() {
        let root = std::env::temp_dir().join(format!(
            "annocat-library-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = root.join("original");
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            br#"{"runId":"run-1","name":"Original"}"#,
        )
        .unwrap();
        assert_eq!(rename(&root, "run-1", "Renamed").unwrap(), "Renamed");
        save_notes(&root, "run-1", "private note").unwrap();
        let allele = "allele-0123456789abcdef".to_string();
        assert_eq!(
            update_candidates(&root, "run-1", std::slice::from_ref(&allele), true)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(candidates(&root, "run-1").unwrap()[0].allele_id, allele);
        assert!(
            update_candidates(&root, "run-1", &["allele-0123456789abcdef".into()], false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(display_name(&root, "run-1").as_deref(), Some("Renamed"));
        assert_eq!(notes(&root, "run-1").unwrap(), "private note");
        assert!(!run.join("case-notes.md").exists());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(run.join("manifest.json")).unwrap()
            )
            .unwrap()["name"],
            "Original"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_atomic_writes_use_distinct_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "annocat-atomic-write-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("profile.json");
        let barrier = Arc::new(Barrier::new(12));
        let handles = (0..12)
            .map(|index| {
                let target = target.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let value = format!("value-{index}");
                    barrier.wait();
                    atomic_write(&target, value.as_bytes()).unwrap();
                    value
                })
            })
            .collect::<Vec<_>>();
        let expected = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        let actual = fs::read_to_string(&target).unwrap();
        assert!(expected.contains(&actual));
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_publish_accepts_a_valid_concurrent_winner() {
        let root = std::env::temp_dir().join(format!(
            "annocat-cache-publish-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("cache.parquet");
        fs::write(&destination, b"winner").unwrap();
        let missing_source = root.join("loser.partial");
        publish_cache_file(&missing_source, &destination, |path| {
            fs::read(path).is_ok_and(|bytes| bytes == b"winner")
        })
        .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_candidates_merge_without_replacing_local_entries() {
        let root = std::env::temp_dir().join(format!(
            "annocat-candidate-merge-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut local = CandidateOverlay::empty("run-merge");
        local.revision = 3;
        local.candidates.insert(
            "allele-local".into(),
            CandidateEntry {
                allele_id: "allele-local".into(),
                added_at: "local-time".into(),
                reason: "Local reason".into(),
            },
        );
        write_candidate_snapshot(&root, &local).unwrap();

        let mut imported = CandidateOverlay::empty("run-merge");
        imported.candidates.insert(
            "allele-local".into(),
            CandidateEntry {
                allele_id: "allele-local".into(),
                added_at: "import-time".into(),
                reason: "Imported reason".into(),
            },
        );
        imported.candidates.insert(
            "allele-imported".into(),
            CandidateEntry {
                allele_id: "allele-imported".into(),
                added_at: "import-time".into(),
                reason: "Imported reason".into(),
            },
        );
        assert!(merge_candidate_snapshot(&root, &imported, "merge-time").unwrap());
        let merged = candidate_snapshot(&root, "run-merge").unwrap();
        assert_eq!(merged.revision, 4);
        assert_eq!(merged.updated_at, "merge-time");
        assert_eq!(merged.candidates["allele-local"].reason, "Local reason");
        assert_eq!(
            merged.candidates["allele-imported"].reason,
            "Imported reason"
        );

        let before = fs::read(candidates_path(&root, "run-merge")).unwrap();
        assert!(!merge_candidate_snapshot(&root, &imported, "later").unwrap());
        assert_eq!(
            fs::read(candidates_path(&root, "run-merge")).unwrap(),
            before
        );
        fs::remove_dir_all(root).unwrap();
    }
}
