use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

const LOCAL_MANIFEST_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunManifest {
    schema_version: u32,
    canonical_schema_version: u32,
    #[serde(default)]
    representative_selection_contract: Option<String>,
    state: String,
    run_id: String,
    name: String,
    completed_at: String,
    assembly: String,
    variant_count: u64,
    #[serde(default)]
    report_kind: Option<String>,
    result_file: String,
    consequences_file: String,
    evidence_file: String,
    field_catalog_file: String,
    fastvep_version: Option<String>,
    fastvep_sha256: Option<String>,
    source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub run_id: String,
}

pub fn create(
    run_directory: &Path,
    destination: &Path,
    candidates: &crate::report_import::CandidateOverlay,
) -> Result<PackageSummary, String> {
    create_with_display_name(run_directory, destination, None, candidates)
}

pub fn create_with_display_name(
    run_directory: &Path,
    destination: &Path,
    display_name: Option<&str>,
    candidates: &crate::report_import::CandidateOverlay,
) -> Result<PackageSummary, String> {
    let destination = result_destination(destination)?;
    if destination.exists() {
        return Err(format!(
            "result destination already exists: {}",
            destination.display()
        ));
    }
    let manifest = read_run_manifest(run_directory)?;
    let result_kind = manifest.report_kind.as_deref().unwrap_or("annotation");
    if !matches!(result_kind, "annotation" | "core-consequences" | "vcf-only") {
        return Err(format!(
            "completed result has unsupported result kind: {result_kind}"
        ));
    }
    crate::report_import::validate_candidate_overlay(candidates, &manifest.run_id)?;
    let candidate_bytes = serde_json::to_vec_pretty(candidates)
        .map_err(|error| format!("cannot serialize candidate bookmarks: {error}"))?;
    crate::report_import::validate_candidate_bytes(&candidate_bytes, &manifest.run_id)?;
    let candidate_sha256 = format!("{:x}", Sha256::digest(&candidate_bytes));
    let display_name = display_name.unwrap_or(&manifest.name).trim();
    if display_name.is_empty()
        || display_name.len() > 256
        || display_name.chars().any(char::is_control)
    {
        return Err("result display name is invalid".into());
    }
    let mut files = vec![
        (manifest.result_file.clone(), "variants"),
        (manifest.consequences_file.clone(), "consequences"),
        (manifest.evidence_file.clone(), "evidence"),
        (manifest.field_catalog_file.clone(), "field-catalog"),
    ];
    files.extend(
        crate::favor::packaged_assets(run_directory)?
            .into_iter()
            .map(|(name, role)| (name.to_owned(), role)),
    );
    let run_root = run_directory
        .canonicalize()
        .map_err(|error| format!("cannot resolve the AnnoCAT result: {error}"))?;
    let mut entries = Vec::with_capacity(files.len() + 3);
    for (declared, role) in files {
        let path = contained_file(&run_root, &declared)?;
        let bytes = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .len();
        let sha256 = sha256_file(&path)?;
        entries.push((declared, role, path, bytes, sha256));
    }
    let runs = run_directory
        .parent()
        .ok_or("AnnoCAT result has no results directory")?;
    for (name, role, path) in crate::phenotype::packaged_assets(runs, &manifest.run_id)? {
        if entries
            .iter()
            .any(|(existing, _, _, _, _)| existing == &name)
        {
            return Err(format!("phenotype result filename conflicts with {name}"));
        }
        let bytes = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .len();
        let sha256 = sha256_file(&path)?;
        entries.push((name, role, path, bytes, sha256));
    }
    let mut source_ids = manifest.source_ids;
    if entries
        .iter()
        .any(|(_, role, _, _, _)| *role == "favor-evidence")
        && !source_ids
            .iter()
            .any(|source| source == crate::favor::SOURCE_ID)
    {
        source_ids.push(crate::favor::SOURCE_ID.into());
    }
    if entries
        .iter()
        .any(|(_, role, _, _, _)| *role == "phenotype-profile")
        && !source_ids.iter().any(|source| source == "hpo")
    {
        source_ids.push("hpo".into());
    }
    let mut file_declarations = entries
        .iter()
        .map(|(name, role, _, bytes, sha256)| {
            serde_json::json!({
                "path": name, "role": role, "bytes": bytes, "sha256": sha256
            })
        })
        .collect::<Vec<_>>();
    file_declarations.push(serde_json::json!({
        "path": "candidates.json",
        "role": "candidate-bookmarks",
        "bytes": candidate_bytes.len(),
        "sha256": candidate_sha256
    }));
    let package_manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "packageFormat": "annocat-result",
        "packageVersion": 1,
        "schemaVersion": manifest.canonical_schema_version,
        "representativeSelectionContract": manifest.representative_selection_contract,
        "runId": manifest.run_id,
        "displayName": display_name,
        "originalDisplayName": manifest.name,
        "completedAt": manifest.completed_at,
        "assembly": manifest.assembly,
        "variantCount": manifest.variant_count,
        "resultKind": result_kind,
        "createdBy": {"application": "AnnoCAT", "version": env!("CARGO_PKG_VERSION")},
        "annotationEngine": (result_kind != "vcf-only").then(|| serde_json::json!({
            "name": "fastVEP",
            "version": manifest.fastvep_version,
            "sha256": manifest.fastvep_sha256
        })),
        "sourceIds": source_ids,
        "files": file_declarations
    }))
    .map_err(|error| format!("cannot serialize result manifest: {error}"))?;

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create result destination: {error}"))?;
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("result destination has an invalid filename")?;
    let temporary = parent.join(format!(".{file_name}.partial-{}", std::process::id()));
    let cleanup = CleanupPath(temporary.clone());
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("cannot create temporary result file: {error}"))?;
    let mut zip = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    zip.start_file("annocat-manifest.json", options)
        .map_err(|error| format!("cannot start result manifest entry: {error}"))?;
    zip.write_all(&package_manifest)
        .map_err(|error| format!("cannot write result manifest: {error}"))?;
    for (name, _, path, expected_bytes, _) in &entries {
        zip.start_file(name, options)
            .map_err(|error| format!("cannot start result entry {name}: {error}"))?;
        let source = File::open(path)
            .map_err(|error| format!("cannot reopen result file {}: {error}", path.display()))?;
        let copied = std::io::copy(&mut BufReader::new(source), &mut zip)
            .map_err(|error| format!("cannot write result entry {name}: {error}"))?;
        if copied != *expected_bytes {
            return Err(format!("result file changed while packaging: {name}"));
        }
    }
    zip.start_file("candidates.json", options)
        .map_err(|error| format!("cannot start candidate bookmark entry: {error}"))?;
    zip.write_all(&candidate_bytes)
        .map_err(|error| format!("cannot write candidate bookmarks: {error}"))?;
    let output = zip
        .finish()
        .map_err(|error| format!("cannot finalize result file: {error}"))?;
    output
        .sync_all()
        .map_err(|error| format!("cannot flush result file: {error}"))?;
    drop(output);
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("cannot publish result file: {error}"))?;
    std::mem::forget(cleanup);
    let bytes = fs::metadata(&destination)
        .map_err(|error| format!("cannot inspect published result file: {error}"))?
        .len();
    Ok(PackageSummary {
        path: destination,
        bytes,
        run_id: manifest.run_id,
    })
}

fn result_destination(destination: &Path) -> Result<PathBuf, String> {
    match destination.extension().and_then(|value| value.to_str()) {
        None => Ok(destination.with_extension("zip")),
        Some(extension) if extension.eq_ignore_ascii_case("zip") => Ok(destination.to_path_buf()),
        Some(_) => Err("AnnoCAT result files must use the .zip extension".into()),
    }
}

fn read_run_manifest(run_directory: &Path) -> Result<RunManifest, String> {
    let path = run_directory.join("manifest.json");
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("AnnoCAT result manifest is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > LOCAL_MANIFEST_LIMIT {
        return Err("AnnoCAT result manifest has an invalid size".into());
    }
    let manifest: RunManifest = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("cannot read run manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid AnnoCAT result manifest: {error}"))?;
    if manifest.schema_version != 1
        || !(1..=annocat_core::RESULT_SCHEMA_VERSION as u32)
            .contains(&manifest.canonical_schema_version)
        || manifest.state != "completed"
    {
        return Err("run is not a supported completed AnnoCAT result".into());
    }
    Ok(manifest)
}

fn contained_file(root: &Path, declared: &str) -> Result<PathBuf, String> {
    let relative = Path::new(declared);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().count() != 1
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "run manifest contains an unsafe result path: {declared}"
        ));
    }
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("run result is missing ({declared}): {error}"))?;
    if !path.is_file() || !path.starts_with(root) {
        return Err(format!(
            "run result failed containment validation: {declared}"
        ));
    }
    Ok(path)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = BufReader::new(
        File::open(path).map_err(|error| format!("cannot hash {}: {error}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

struct CleanupPath(PathBuf);

impl Drop for CleanupPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_run_packages_without_duplicate_vcf() {
        let root = std::env::temp_dir().join(format!(
            "annocat-report-package-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = root.join("run");
        fs::create_dir_all(&run).unwrap();
        for (name, bytes) in [
            ("variants.parquet", b"PAR1variants".as_slice()),
            ("consequences.parquet", b"PAR1consequences".as_slice()),
            ("evidence.parquet", b"PAR1evidence".as_slice()),
            ("field-catalog.json", br#"{"fields":[]}"#.as_slice()),
            ("annotated.vcf", b"duplicate export".as_slice()),
        ] {
            fs::write(run.join(name), bytes).unwrap();
        }
        fs::write(
            run.join("manifest.json"),
            br#"{"schemaVersion":1,"canonicalSchemaVersion":1,"representativeSelectionContract":"allele-gene-severity-v1","state":"completed","runId":"run-package","name":"Package fixture","completedAt":"2026-07-16T00:00:00Z","assembly":"GRCh38","variantCount":2,"resultFile":"variants.parquet","consequencesFile":"consequences.parquet","evidenceFile":"evidence.parquet","fieldCatalogFile":"field-catalog.json","fastvepVersion":"0.2.0","fastvepSha256":"fixture","sourceIds":["clinvar"]}"#,
        )
        .unwrap();
        let destination = root.join("shared.zip");
        let summary = create(
            &run,
            &destination,
            &crate::report_import::CandidateOverlay::empty("run-package"),
        )
        .unwrap();
        assert_eq!(summary.run_id, "run-package");
        let inspection = crate::report_import::validate_archive(&destination).unwrap();
        assert_eq!(inspection.file_count, 6);
        let mut archive = zip::ZipArchive::new(File::open(&destination).unwrap()).unwrap();
        assert!(archive.by_name("annotated.vcf").is_err());
        assert!(archive.by_name("candidates.json").is_ok());
        let mut manifest = String::new();
        archive
            .by_name("annocat-manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&manifest).unwrap()["representativeSelectionContract"],
            "allele-gene-severity-v1"
        );
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["packageFormat"], "annocat-result");
        assert_eq!(manifest["resultKind"], "annotation");
        assert!(manifest.get("reportKind").is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
