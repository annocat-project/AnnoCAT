use serde::Deserialize;
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
        .map_err(|error| format!("cannot open report archive {}: {error}", path.display()))?;
    validate_archive_file(file)
}

pub fn validate_archive_file(file: File) -> Result<ReportInspection, String> {
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("invalid report ZIP: {error}"))?;
    let entries = inspect_entries(&mut archive)?;
    let manifest = read_manifest(&mut archive)?;
    validate_manifest(&manifest, &entries)?;
    verify_checksums(&mut archive, &manifest)?;
    Ok(ReportInspection {
        run_id: manifest.run_id,
        schema_version: manifest.schema_version,
        file_count: manifest.files.len() + 1,
        uncompressed_bytes: entries.values().try_fold(0_u64, |total, entry| {
            total
                .checked_add(entry.size)
                .ok_or_else(|| "report archive size overflows supported range".to_string())
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
            "report ZIP must contain between 1 and {MAX_ARCHIVE_ENTRIES} entries"
        ));
    }
    let mut entries = BTreeMap::new();
    let mut folded_names = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect ZIP entry {index}: {error}"))?;
        if file.is_dir() {
            return Err("report ZIP cannot contain directories".into());
        }
        let name = safe_top_level_name(file.name())?;
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("report ZIP contains a symbolic link: {name}"));
        }
        let folded = name.to_ascii_lowercase();
        if !folded_names.insert(folded) || entries.contains_key(&name) {
            return Err(format!("report ZIP contains a duplicate filename: {name}"));
        }
        let compressed = file.compressed_size();
        let size = file.size();
        if compressed == 0 && size != 0
            || compressed != 0 && size > compressed.saturating_mul(MAX_COMPRESSION_RATIO)
        {
            return Err(format!(
                "report ZIP entry has an unsafe compression ratio: {name}"
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
        return Err("report ZIP contains an invalid filename".into());
    }
    let path = Path::new(name);
    let mut components = path.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(format!("report ZIP contains an unsafe path: {name}"));
    };
    if components.next().is_some() || component.to_string_lossy() != name {
        return Err(format!(
            "report ZIP files must use safe top-level names: {name}"
        ));
    }
    Ok(name.to_owned())
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<ReportManifest, String> {
    let file = archive
        .by_name(MANIFEST_NAME)
        .map_err(|_| format!("report ZIP is missing {MANIFEST_NAME}"))?;
    if file.size() == 0 || file.size() > MAX_MANIFEST_BYTES {
        return Err("report manifest has an invalid size".into());
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read report manifest: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid report manifest: {error}"))
}

fn validate_manifest(
    manifest: &ReportManifest,
    entries: &BTreeMap<String, ArchiveEntry>,
) -> Result<(), String> {
    if manifest.package_format != "annocat-report" || manifest.package_version != 1 {
        return Err("unsupported AnnoCAT report package format".into());
    }
    if !(1..=annocat_core::RESULT_SCHEMA_VERSION as u32).contains(&manifest.schema_version) {
        return Err(format!(
            "unsupported AnnoCAT result schema version {}",
            manifest.schema_version
        ));
    }
    let report_kind = manifest.report_kind.as_deref().unwrap_or("annotation");
    if !matches!(report_kind, "annotation" | "core-consequences" | "vcf-only") {
        return Err(format!("unsupported AnnoCAT report kind: {report_kind}"));
    }
    validate_identifier(&manifest.run_id, "run ID")?;
    if manifest.display_name.trim().is_empty()
        || manifest.display_name.len() > 256
        || manifest.completed_at.is_empty()
        || manifest.completed_at.len() > 64
        || manifest.assembly != "GRCh38"
        || manifest.variant_count == 0
    {
        return Err("report manifest has invalid run metadata".into());
    }
    if manifest.files.is_empty() || manifest.files.len() + 1 != entries.len() {
        return Err("report manifest must declare every ZIP file exactly once".into());
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
            return Err(format!("duplicate report file role: {}", declared.role));
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
            return Err(format!("report manifest is missing required role: {role}"));
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("report {label} is invalid"));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture_archive() -> std::path::PathBuf {
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
        let files = [
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
        let path = fixture_archive();
        let inspection = validate_archive(&path).unwrap();
        assert_eq!(inspection.run_id, "run-fixture");
        assert_eq!(inspection.file_count, 5);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
