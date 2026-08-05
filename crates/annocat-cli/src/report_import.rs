use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path};
use zip::ZipArchive;

const MANIFEST_NAME: &str = "annocat-manifest.json";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 128;
const MAX_ENTRY_NAME_BYTES: usize = 240;
const MAX_COMPRESSION_RATIO: u64 = 10_000;
pub const MAX_CANDIDATE_BYTES: usize = 4_000_000;
pub const MAX_CANDIDATES: usize = 10_000;
const MAX_PHENOTYPE_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PHENOTYPE_CATALOG_BYTES: u64 = 4 * 1024 * 1024;
const PHENOTYPE_ROLES: [&str; 3] = [
    "phenotype-profile",
    "phenotype-gene-evidence",
    "phenotype-field-catalog",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportManifest {
    package_format: String,
    package_version: u32,
    schema_version: u32,
    run_id: String,
    display_name: String,
    completed_at: String,
    assembly: String,
    variant_count: u64,
    #[serde(default)]
    report_kind: Option<String>,
    #[serde(default)]
    result_kind: Option<String>,
    #[serde(skip)]
    report_kind_present: bool,
    #[serde(skip)]
    result_kind_present: bool,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    path: String,
    role: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateOverlay {
    pub schema_version: u16,
    pub run_id: String,
    pub revision: u64,
    pub updated_at: String,
    pub candidates: BTreeMap<String, CandidateEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateEntry {
    pub allele_id: String,
    pub added_at: String,
    pub reason: String,
}

impl CandidateOverlay {
    #[allow(dead_code)]
    pub fn empty(run_id: &str) -> Self {
        Self {
            schema_version: 1,
            run_id: run_id.into(),
            revision: 0,
            updated_at: String::new(),
            candidates: BTreeMap::new(),
        }
    }
}

pub fn validate_candidate_bytes(
    bytes: &[u8],
    expected_run_id: &str,
) -> Result<CandidateOverlay, String> {
    if bytes.is_empty() || bytes.len() > MAX_CANDIDATE_BYTES {
        return Err("candidate bookmark data has an invalid size".into());
    }
    let overlay: CandidateOverlay = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid candidate bookmark data: {error}"))?;
    validate_candidate_overlay(&overlay, expected_run_id)?;
    Ok(overlay)
}

pub fn validate_candidate_overlay(
    overlay: &CandidateOverlay,
    expected_run_id: &str,
) -> Result<(), String> {
    validate_identifier(expected_run_id, "result ID")?;
    if overlay.schema_version != 1
        || overlay.run_id != expected_run_id
        || overlay.candidates.len() > MAX_CANDIDATES
        || overlay.candidates.iter().any(|(id, entry)| {
            id != &entry.allele_id || validate_allele_id(&entry.allele_id).is_err()
        })
    {
        return Err("candidate bookmark identity or contents are invalid".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportInspection {
    pub run_id: String,
    pub schema_version: u32,
    pub file_count: usize,
    pub uncompressed_bytes: u64,
}

#[allow(dead_code)]
pub fn validate_archive(path: &Path) -> Result<ReportInspection, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot open AnnoCAT result {}: {error}", path.display()))?;
    validate_archive_file(file)
}

pub fn validate_archive_file(file: File) -> Result<ReportInspection, String> {
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("invalid AnnoCAT result: {error}"))?;
    let entries = inspect_entries(&mut archive)?;
    let manifest = read_manifest(&mut archive)?;
    validate_manifest(&manifest, &entries)?;
    verify_checksums(&mut archive, &manifest)?;
    verify_candidates(&mut archive, &manifest)?;
    verify_phenotypes(&mut archive, &manifest)?;
    Ok(ReportInspection {
        run_id: manifest.run_id,
        schema_version: manifest.schema_version,
        file_count: manifest.files.len() + 1,
        uncompressed_bytes: entries.values().try_fold(0_u64, |total, entry| {
            total
                .checked_add(entry.size)
                .ok_or_else(|| "AnnoCAT result size exceeds the supported range".to_string())
        })?,
    })
}

#[derive(Debug, Clone)]
struct ArchiveEntry {
    size: u64,
}

fn inspect_entries<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<BTreeMap<String, ArchiveEntry>, String> {
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "AnnoCAT result must contain between 1 and {MAX_ARCHIVE_ENTRIES} entries"
        ));
    }
    let mut entries = BTreeMap::new();
    let mut folded_names = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect ZIP entry {index}: {error}"))?;
        if file.is_dir() {
            return Err("AnnoCAT result cannot contain directories".into());
        }
        let name = safe_top_level_name(file.name())?;
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("AnnoCAT result contains a symbolic link: {name}"));
        }
        let folded = name.to_ascii_lowercase();
        if !folded_names.insert(folded) || entries.contains_key(&name) {
            return Err(format!(
                "AnnoCAT result contains a duplicate filename: {name}"
            ));
        }
        let compressed = file.compressed_size();
        let size = file.size();
        if compressed == 0 && size != 0
            || compressed != 0 && size > compressed.saturating_mul(MAX_COMPRESSION_RATIO)
        {
            return Err(format!(
                "AnnoCAT result entry has an unsafe compression ratio: {name}"
            ));
        }
        entries.insert(name, ArchiveEntry { size });
    }
    Ok(entries)
}

fn safe_top_level_name(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.len() > MAX_ENTRY_NAME_BYTES
        || name
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err("AnnoCAT result contains an invalid filename".into());
    }
    let path = Path::new(name);
    let mut components = path.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(format!("AnnoCAT result contains an unsafe path: {name}"));
    };
    if components.next().is_some() || component.to_string_lossy() != name {
        return Err(format!(
            "AnnoCAT result files must use safe top-level names: {name}"
        ));
    }
    Ok(name.to_owned())
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<ReportManifest, String> {
    let file = archive
        .by_name(MANIFEST_NAME)
        .map_err(|_| format!("AnnoCAT result is missing {MANIFEST_NAME}"))?;
    if file.size() == 0 || file.size() > MAX_MANIFEST_BYTES {
        return Err("AnnoCAT result manifest has an invalid size".into());
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read AnnoCAT result manifest: {error}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid result manifest: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err("result manifest must be a JSON object".into());
    };
    let report_kind_present = object.contains_key("reportKind");
    let result_kind_present = object.contains_key("resultKind");
    let mut manifest: ReportManifest = serde_json::from_value(value)
        .map_err(|error| format!("invalid result manifest: {error}"))?;
    manifest.report_kind_present = report_kind_present;
    manifest.result_kind_present = result_kind_present;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &ReportManifest,
    entries: &BTreeMap<String, ArchiveEntry>,
) -> Result<(), String> {
    let result_kind = package_result_kind(
        &manifest.package_format,
        manifest.package_version,
        manifest.report_kind.as_deref(),
        manifest.result_kind.as_deref(),
        manifest.report_kind_present,
        manifest.result_kind_present,
    )?;
    if !(1..=annocat_core::RESULT_SCHEMA_VERSION as u32).contains(&manifest.schema_version) {
        return Err(format!(
            "unsupported AnnoCAT result schema version {}",
            manifest.schema_version
        ));
    }
    debug_assert!(matches!(
        result_kind,
        "annotation" | "core-consequences" | "vcf-only"
    ));
    validate_identifier(&manifest.run_id, "run ID")?;
    if manifest.display_name.trim().is_empty()
        || manifest.display_name.len() > 256
        || manifest.completed_at.is_empty()
        || manifest.completed_at.len() > 64
        || manifest.assembly != "GRCh38"
        || manifest.variant_count == 0
    {
        return Err("AnnoCAT result manifest has invalid result metadata".into());
    }
    if manifest.files.is_empty() || manifest.files.len() + 1 != entries.len() {
        return Err("AnnoCAT result manifest must declare every archive file exactly once".into());
    }
    let required_roles = ["variants", "consequences", "evidence", "field-catalog"];
    let mut roles = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for declared in &manifest.files {
        let path = safe_top_level_name(&declared.path)?;
        if path == MANIFEST_NAME || !paths.insert(path.clone()) {
            return Err(format!("duplicate or reserved manifest path: {path}"));
        }
        if !roles.insert(declared.role.as_str()) {
            return Err(format!("duplicate result file role: {}", declared.role));
        }
        if declared.sha256.len() != 64
            || !declared.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("invalid SHA-256 for {path}"));
        }
        let entry = entries
            .get(&path)
            .ok_or_else(|| format!("manifest file is missing from ZIP: {path}"))?;
        if entry.size != declared.bytes {
            return Err(format!("declared size does not match ZIP entry: {path}"));
        }
    }
    for role in required_roles {
        if !roles.contains(role) {
            return Err(format!("result manifest is missing required role: {role}"));
        }
    }
    if manifest.package_format == "annocat-result" && !roles.contains("candidate-bookmarks") {
        return Err("result manifest is missing required role: candidate-bookmarks".into());
    }
    let phenotype_role_count = PHENOTYPE_ROLES
        .iter()
        .filter(|role| roles.contains(**role))
        .count();
    if phenotype_role_count != 0 && phenotype_role_count != PHENOTYPE_ROLES.len() {
        return Err("result contains an incomplete phenotype evidence group".into());
    }
    if roles.contains("phenotype-candidate-evidence")
        && phenotype_role_count != PHENOTYPE_ROLES.len()
    {
        return Err("result contains phenotype ranks without their profile".into());
    }
    if phenotype_role_count == PHENOTYPE_ROLES.len()
        && manifest
            .files
            .iter()
            .find(|file| file.role == "phenotype-profile")
            .is_none_or(|file| file.path != "phenotypes.json")
    {
        return Err("result phenotype profile has an invalid filename".into());
    }
    Ok(())
}

pub fn package_result_kind<'a>(
    package_format: &str,
    package_version: u32,
    report_kind: Option<&'a str>,
    result_kind: Option<&'a str>,
    report_kind_present: bool,
    result_kind_present: bool,
) -> Result<&'a str, String> {
    if package_version != 1 {
        return Err("unsupported AnnoCAT result package version".into());
    }
    let kind = match package_format {
        "annocat-result" => {
            if report_kind_present || !result_kind_present {
                return Err("new AnnoCAT results require resultKind and reject reportKind".into());
            }
            result_kind.ok_or("new AnnoCAT result has an invalid resultKind")?
        }
        "annocat-report" => {
            if result_kind_present {
                return Err("legacy AnnoCAT results reject resultKind".into());
            }
            if report_kind_present && report_kind.is_none() {
                return Err("legacy AnnoCAT result has an invalid reportKind".into());
            }
            report_kind.unwrap_or("annotation")
        }
        _ => return Err("unsupported AnnoCAT result package format".into()),
    };
    if !matches!(kind, "annotation" | "core-consequences" | "vcf-only") {
        return Err(format!("unsupported AnnoCAT result kind: {kind}"));
    }
    Ok(kind)
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("result {label} is invalid"));
    }
    Ok(())
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

fn verify_checksums<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &ReportManifest,
) -> Result<(), String> {
    for declared in &manifest.files {
        let file = archive
            .by_name(&declared.path)
            .map_err(|error| format!("cannot open {}: {error}", declared.path))?;
        let mut hasher = Sha256::new();
        let copied = std::io::copy(
            &mut file.take(declared.bytes.saturating_add(1)),
            &mut hasher,
        )
        .map_err(|error| format!("cannot verify {}: {error}", declared.path))?;
        if copied != declared.bytes {
            return Err(format!("size changed while reading {}", declared.path));
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(&declared.sha256) {
            return Err(format!("checksum mismatch for {}", declared.path));
        }
    }
    Ok(())
}

fn verify_candidates<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &ReportManifest,
) -> Result<(), String> {
    let Some(declared) = manifest
        .files
        .iter()
        .find(|file| file.role == "candidate-bookmarks")
    else {
        return Ok(());
    };
    if declared.bytes == 0 || declared.bytes > MAX_CANDIDATE_BYTES as u64 {
        return Err("candidate bookmark data has an invalid size".into());
    }
    let file = archive
        .by_name(&declared.path)
        .map_err(|error| format!("cannot open candidate bookmarks: {error}"))?;
    let mut bytes = Vec::with_capacity(declared.bytes as usize);
    file.take(MAX_CANDIDATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read candidate bookmarks: {error}"))?;
    validate_candidate_bytes(&bytes, &manifest.run_id)?;
    Ok(())
}

fn verify_phenotypes<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &ReportManifest,
) -> Result<(), String> {
    let Some(profile) = manifest
        .files
        .iter()
        .find(|file| file.role == "phenotype-profile")
    else {
        return Ok(());
    };
    let evidence = manifest
        .files
        .iter()
        .find(|file| file.role == "phenotype-gene-evidence")
        .ok_or("result contains an incomplete phenotype evidence group")?;
    let catalog = manifest
        .files
        .iter()
        .find(|file| file.role == "phenotype-field-catalog")
        .ok_or("result contains an incomplete phenotype evidence group")?;
    let candidate = manifest
        .files
        .iter()
        .find(|file| file.role == "phenotype-candidate-evidence");
    let read = |archive: &mut ZipArchive<R>,
                file: &ManifestFile,
                limit: u64,
                label: &str|
     -> Result<Vec<u8>, String> {
        if file.bytes == 0 || file.bytes > limit {
            return Err(format!("{label} has an invalid size"));
        }
        let entry = archive
            .by_name(&file.path)
            .map_err(|error| format!("cannot open {label}: {error}"))?;
        let mut bytes = Vec::with_capacity(file.bytes as usize);
        entry
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {label}: {error}"))?;
        Ok(bytes)
    };
    let profile_bytes = read(
        archive,
        profile,
        MAX_PHENOTYPE_PROFILE_BYTES,
        "phenotype profile",
    )?;
    let catalog_bytes = read(
        archive,
        catalog,
        MAX_PHENOTYPE_CATALOG_BYTES,
        "phenotype field catalog",
    )?;
    validate_portable_phenotype_metadata(
        &profile_bytes,
        &catalog_bytes,
        &manifest.run_id,
        &evidence.path,
        &catalog.path,
        candidate.map(|file| file.path.as_str()),
    )?;
    Ok(())
}

fn validate_portable_phenotype_metadata(
    profile_bytes: &[u8],
    catalog_bytes: &[u8],
    run_id: &str,
    evidence_file: &str,
    catalog_file: &str,
    candidate_file: Option<&str>,
) -> Result<(), String> {
    let profile: serde_json::Value = serde_json::from_slice(profile_bytes)
        .map_err(|error| format!("invalid phenotype profile: {error}"))?;
    let active = profile["activeGeneration"]
        .as_object()
        .ok_or("portable phenotype profile has no active evidence")?;
    let fingerprint = active
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or("phenotype profile has no generation fingerprint")?;
    if profile["schemaVersion"] != 4
        || profile["runId"].as_str() != Some(run_id)
        || fingerprint.len() != 64
        || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        || active
            .get("evidenceFile")
            .and_then(serde_json::Value::as_str)
            != Some(evidence_file)
        || active
            .get("catalogFile")
            .and_then(serde_json::Value::as_str)
            != Some(catalog_file)
        || active
            .get("candidateEvidenceFile")
            .and_then(serde_json::Value::as_str)
            != candidate_file
    {
        return Err("phenotype profile identity or generation is invalid".into());
    }
    let short = &fingerprint[..16];
    if evidence_file != format!("phenotype-gene-evidence.{short}.parquet")
        || catalog_file != format!("phenotype-field-catalog.{short}.json")
        || candidate_file
            .is_some_and(|file| file != format!("phenotype-candidate-evidence.{short}.parquet"))
    {
        return Err("phenotype generation filenames do not match its fingerprint".into());
    }
    let catalog: serde_json::Value = serde_json::from_slice(catalog_bytes)
        .map_err(|error| format!("invalid phenotype field catalog: {error}"))?;
    if catalog["schemaVersion"] != 1
        || catalog["geneEvidenceFile"].as_str() != Some(evidence_file)
        || catalog["candidateEvidenceFile"].as_str() != candidate_file
        || catalog["profileFingerprint"].as_str() != Some(fingerprint)
        || catalog["algorithmVersion"].as_str() != Some("hpo-lin-query-v4")
        || (candidate_file.is_some()
            && catalog["candidateAlgorithmVersion"].as_str() != Some("phenotype-candidate-v1"))
    {
        return Err("phenotype field catalog does not match its profile".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture_archive(extra_files: &[(&str, &str, &[u8])]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "annocat-report-import-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("report.zip");
        let mut files = vec![
            ("variants.parquet", "variants", b"PAR1variants".as_slice()),
            (
                "consequences.parquet",
                "consequences",
                b"PAR1consequences".as_slice(),
            ),
            ("evidence.parquet", "evidence", b"PAR1evidence".as_slice()),
            (
                "field-catalog.json",
                "field-catalog",
                br#"{"schemaVersion":1,"fields":[]}"#.as_slice(),
            ),
        ];
        files.extend_from_slice(extra_files);
        let declarations = files
            .iter()
            .map(|(path, role, bytes)| {
                serde_json::json!({
                    "path": path,
                    "role": role,
                    "bytes": bytes.len(),
                    "sha256": sha256(bytes)
                })
            })
            .collect::<Vec<_>>();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "packageFormat": "annocat-report",
            "packageVersion": 1,
            "schemaVersion": 1,
            "runId": "run-fixture",
            "displayName": "Report fixture",
            "completedAt": "2026-07-16T00:00:00Z",
            "assembly": "GRCh38",
            "variantCount": 1,
            "files": declarations,
            "futureMetadata": {"isAllowed": true}
        }))
        .unwrap();
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(MANIFEST_NAME, options).unwrap();
        writer.write_all(&manifest).unwrap();
        for (name, _, bytes) in files {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[test]
    fn paths_must_be_single_safe_top_level_names() {
        assert_eq!(
            safe_top_level_name("variants.parquet").unwrap(),
            "variants.parquet"
        );
        for malicious in [
            "../variants.parquet",
            "folder/variants.parquet",
            "folder\\variants.parquet",
            "C:\\variants.parquet",
            "/variants.parquet",
            "variants.parquet\0.exe",
        ] {
            assert!(
                safe_top_level_name(malicious).is_err(),
                "accepted {malicious:?}"
            );
        }
    }

    #[test]
    fn identifiers_cannot_contain_paths_or_commands() {
        assert!(validate_identifier("run-6bf8153e93e5", "run ID").is_ok());
        assert!(validate_identifier("../run", "run ID").is_err());
        assert!(validate_identifier("run; DROP TABLE", "run ID").is_err());
    }

    #[test]
    fn complete_report_is_checked_without_rejecting_future_metadata() {
        let path = fixture_archive(&[]);
        let inspection = validate_archive(&path).unwrap();
        assert_eq!(inspection.run_id, "run-fixture");
        assert_eq!(inspection.file_count, 5);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn phenotype_archive_group_must_be_complete() {
        let path = fixture_archive(&[(
            "phenotypes.json",
            "phenotype-profile",
            br#"{"schemaVersion":4}"#,
        )]);
        let error = validate_archive(&path).err().unwrap();
        assert!(error.contains("incomplete phenotype evidence group"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn package_identity_fields_cannot_mix() {
        assert_eq!(
            package_result_kind("annocat-result", 1, None, Some("annotation"), false, true)
                .unwrap(),
            "annotation"
        );
        assert_eq!(
            package_result_kind("annocat-report", 1, None, None, false, false).unwrap(),
            "annotation"
        );
        assert!(
            package_result_kind(
                "annocat-result",
                1,
                Some("annotation"),
                Some("annotation"),
                true,
                true
            )
            .is_err()
        );
        assert!(
            package_result_kind("annocat-report", 1, None, Some("annotation"), false, true)
                .is_err()
        );
        assert!(package_result_kind("annocat-result", 1, None, None, false, false).is_err());
    }

    #[test]
    fn candidate_schema_checks_result_and_allele_identity() {
        let mut candidates = CandidateOverlay::empty("run-fixture");
        candidates.candidates.insert(
            "allele-0123456789abcdef".into(),
            CandidateEntry {
                allele_id: "allele-0123456789abcdef".into(),
                added_at: "2026-07-29T00:00:00Z".into(),
                reason: "Added manually".into(),
            },
        );
        let bytes = serde_json::to_vec(&candidates).unwrap();
        assert_eq!(
            validate_candidate_bytes(&bytes, "run-fixture")
                .unwrap()
                .candidates
                .len(),
            1
        );
        assert!(validate_candidate_bytes(&bytes, "other-run").is_err());
        candidates
            .candidates
            .get_mut("allele-0123456789abcdef")
            .unwrap()
            .allele_id = "different".into();
        assert!(
            validate_candidate_bytes(&serde_json::to_vec(&candidates).unwrap(), "run-fixture")
                .is_err()
        );
    }
}
