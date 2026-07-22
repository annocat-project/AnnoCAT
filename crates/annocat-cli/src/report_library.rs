use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifest {
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
    files: Vec<PackageFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageFile {
    path: String,
    role: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedReport {
    pub run_id: String,
    pub directory: PathBuf,
    pub name: String,
}

pub fn import(path: &Path, runs: &Path) -> Result<ImportedReport, String> {
    crate::report_import::validate_archive(path)?;
    fs::create_dir_all(runs).map_err(|error| format!("cannot create runs directory: {error}"))?;
    let file = File::open(path)
        .map_err(|error| format!("cannot reopen report ZIP {}: {error}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("cannot reopen validated report ZIP: {error}"))?;
    let manifest: PackageManifest = {
        let entry = archive
            .by_name("annocat-manifest.json")
            .map_err(|_| "validated report manifest disappeared")?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot reread report manifest: {error}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse validated report manifest: {error}"))?
    };
    if manifest.package_format != "annocat-report"
        || manifest.package_version != 1
        || manifest.schema_version != 1
        || manifest.display_name.trim().is_empty()
        || manifest.display_name.len() > 256
        || manifest.completed_at.is_empty()
        || manifest.completed_at.len() > 64
        || manifest.assembly != "GRCh38"
        || manifest.variant_count == 0
    {
        return Err("validated report package identity changed".into());
    }
    let report_kind = manifest.report_kind.as_deref().unwrap_or("annotation");
    if !matches!(report_kind, "annotation" | "core-consequences" | "vcf-only") {
        return Err("validated report package has an unsupported report kind".into());
    }
    if let Some(existing) = existing_run(runs, &manifest)? {
        return Ok(existing);
    }

    let name = manifest.display_name.clone();
    let basename = safe_basename(
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&manifest.run_id),
    );
    let final_directory = unique_destination(runs, &basename);
    let staging = runs.join(format!(".import-{}.partial", manifest.run_id));
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| format!("cannot clear interrupted import: {error}"))?;
    }
    fs::create_dir(&staging).map_err(|error| format!("cannot create import staging: {error}"))?;
    let cleanup = CleanupDirectory(staging.clone());

    let mut roles = BTreeMap::new();
    for declared in &manifest.files {
        let mut entry = archive
            .by_name(&declared.path)
            .map_err(|_| format!("validated report file disappeared: {}", declared.path))?;
        let destination = staging.join(&declared.path);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| format!("cannot create imported file {}: {error}", declared.path))?;
        let mut hasher = Sha256::new();
        let copied = copy_and_hash(&mut entry, &mut output, &mut hasher, declared.bytes)?;
        output
            .sync_all()
            .map_err(|error| format!("cannot flush imported file {}: {error}", declared.path))?;
        if copied != declared.bytes
            || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&declared.sha256)
        {
            return Err(format!(
                "report file changed during import: {}",
                declared.path
            ));
        }
        roles.insert(declared.role.as_str(), destination);
    }
    let variants = role(&roles, "variants")?;
    let consequences = role(&roles, "consequences")?;
    let evidence = role(&roles, "evidence")?;
    let catalog = role(&roles, "field-catalog")?;
    let variant_count = manifest.variant_count;
    if report_kind == "vcf-only" {
        crate::results::validate_report_tables_allow_empty_consequences(
            variants,
            consequences,
            evidence,
            catalog,
            variant_count,
        )?;
    } else {
        crate::results::validate_report_tables(
            variants,
            consequences,
            evidence,
            catalog,
            variant_count,
        )?;
    }

    let file_for_role = |role_name: &str| {
        manifest
            .files
            .iter()
            .find(|file| file.role == role_name)
            .expect("required role was validated")
    };
    let variants_file = file_for_role("variants");
    let consequences_file = file_for_role("consequences");
    let evidence_file = file_for_role("evidence");
    let catalog_file = file_for_role("field-catalog");
    let local_manifest = serde_json::json!({
        "schemaVersion": 1,
        "canonicalSchemaVersion": manifest.schema_version,
        "state": "completed",
        "runId": manifest.run_id,
        "name": name,
        "completedAt": manifest.completed_at,
        "assembly": manifest.assembly,
        "variantCount": variant_count,
        "reportKind": report_kind,
        "resultFile": variants_file.path,
        "resultSha256": variants_file.sha256,
        "consequencesFile": consequences_file.path,
        "consequencesSha256": consequences_file.sha256,
        "evidenceFile": evidence_file.path,
        "evidenceSha256": evidence_file.sha256,
        "fieldCatalogFile": catalog_file.path,
        "fieldCatalogSha256": catalog_file.sha256,
        "canonicalResultBytes": manifest.files.iter().map(|file| file.bytes).sum::<u64>(),
        "importedFrom": path,
        "importedAt": crate::annotation::current_timestamp()
    });
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&local_manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write imported run manifest: {error}"))?;
    fs::rename(&staging, &final_directory)
        .map_err(|error| format!("cannot publish imported report: {error}"))?;
    std::mem::forget(cleanup);
    Ok(ImportedReport {
        run_id: manifest.run_id,
        directory: final_directory,
        name,
    })
}

fn copy_and_hash(
    input: &mut impl Read,
    output: &mut impl Write,
    hasher: &mut Sha256,
    expected: u64,
) -> Result<u64, String> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    while total <= expected {
        let remaining = expected.saturating_add(1).saturating_sub(total);
        if remaining == 0 {
            break;
        }
        let capacity = buffer.len().min(remaining as usize);
        let read = input
            .read(&mut buffer[..capacity])
            .map_err(|error| format!("cannot read report entry: {error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot write imported report entry: {error}"))?;
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok(total)
}

fn role<'a>(roles: &'a BTreeMap<&str, PathBuf>, name: &str) -> Result<&'a Path, String> {
    roles
        .get(name)
        .map(PathBuf::as_path)
        .ok_or_else(|| format!("validated report is missing {name}"))
}

fn existing_run(runs: &Path, package: &PackageManifest) -> Result<Option<ImportedReport>, String> {
    let Ok(entries) = fs::read_dir(runs) else {
        return Ok(None);
    };
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        let path = entry.path().join("manifest.json");
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(existing) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if existing["runId"] != package.run_id {
            continue;
        }
        for file in &package.files {
            let key = match file.role.as_str() {
                "variants" => "resultSha256",
                "consequences" => "consequencesSha256",
                "evidence" => "evidenceSha256",
                "field-catalog" => "fieldCatalogSha256",
                _ => continue,
            };
            if existing[key].as_str() != Some(&file.sha256) {
                return Err(format!(
                    "run ID {} already exists with different report content",
                    package.run_id
                ));
            }
        }
        return Ok(Some(ImportedReport {
            run_id: package.run_id.clone(),
            directory: entry.path(),
            name: existing["name"]
                .as_str()
                .unwrap_or(&package.run_id)
                .to_owned(),
        }));
    }
    Ok(None)
}

fn unique_destination(runs: &Path, basename: &str) -> PathBuf {
    let first = runs.join(basename);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|number| runs.join(format!("{basename}-{number}")))
        .find(|path| !path.exists())
        .expect("a free imported report directory exists")
}

fn safe_basename(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('-');
    if value.is_empty() {
        "imported-report".into()
    } else {
        value.chars().take(120).collect()
    }
}

struct CleanupDirectory(PathBuf);

impl Drop for CleanupDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validated_report_imports_atomically_and_keeps_source_zip() {
        let root = std::env::temp_dir().join(format!(
            "annocat-report-library-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = root.join("source-run");
        let library = root.join("library");
        fs::create_dir_all(&run).unwrap();
        let vcf = run.join("annotated.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|YES|NM_1\n",
        )
        .unwrap();
        crate::results::convert_vcf(
            &vcf,
            &run.join("variants.parquet"),
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        let structured = run.join("fastvep.ndjson");
        fs::write(
            &structured,
            concat!(
                r#"{"allele_string":"A/G","start":100,"end":100,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST1","canonical":1,"clinvar":"Pathogenic"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        crate::results::convert_structured(
            &structured,
            &run.join("consequences.parquet"),
            &run.join("evidence.parquet"),
            &run.join("field-catalog.json"),
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        fs::write(
            run.join("manifest.json"),
            br#"{"schemaVersion":1,"canonicalSchemaVersion":1,"state":"completed","runId":"run-import","name":"Import fixture","completedAt":"2026-07-16T00:00:00Z","assembly":"GRCh38","variantCount":1,"resultFile":"variants.parquet","consequencesFile":"consequences.parquet","evidenceFile":"evidence.parquet","fieldCatalogFile":"field-catalog.json","fastvepVersion":"0.2.0","fastvepSha256":"fixture","sourceIds":["clinvar"]}"#,
        )
        .unwrap();
        let package_path = root.join("Import-fixture--20260716--import.zip");
        crate::report_package::create(&run, &package_path).unwrap();
        let imported = import(&package_path, &library).unwrap();
        assert_eq!(imported.run_id, "run-import");
        assert!(imported.directory.join("manifest.json").is_file());
        assert!(imported.directory.join("variants.parquet").is_file());
        assert!(package_path.is_file(), "source ZIP must remain untouched");
        assert!(!library.join(".import-run-import.partial").exists());
        let repeated = import(&package_path, &library).unwrap();
        assert_eq!(repeated.directory, imported.directory);
        assert_eq!(fs::read_dir(&library).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
