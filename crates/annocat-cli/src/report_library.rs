use duckdb::{Connection, params};
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
    #[serde(default)]
    representative_selection_contract: Option<String>,
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
    #[serde(default)]
    annotation_engine: Option<crate::report_import::PackageAnnotationEngine>,
    #[serde(default)]
    source_ids: Vec<String>,
    #[serde(default)]
    annotation_provenance: Option<crate::report_import::PackageAnnotationProvenance>,
    #[serde(default)]
    input_name: Option<String>,
    #[serde(default)]
    input_bytes: Option<u64>,
    #[serde(default)]
    input_content_sha256: Option<String>,
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

fn parse_package_manifest(bytes: &[u8]) -> Result<PackageManifest, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("cannot parse validated result manifest: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err("validated result manifest must be a JSON object".into());
    };
    let report_kind_present = object.contains_key("reportKind");
    let result_kind_present = object.contains_key("resultKind");
    let mut manifest: PackageManifest = serde_json::from_value(value)
        .map_err(|error| format!("cannot parse validated result manifest: {error}"))?;
    manifest.report_kind_present = report_kind_present;
    manifest.result_kind_present = result_kind_present;
    Ok(manifest)
}

pub fn import(path: &Path, runs: &Path) -> Result<ImportedReport, String> {
    crate::report_import::validate_archive(path)?;
    fs::create_dir_all(runs).map_err(|error| format!("cannot create runs directory: {error}"))?;
    let file = File::open(path)
        .map_err(|error| format!("cannot reopen AnnoCAT result {}: {error}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("cannot reopen the validated AnnoCAT result: {error}"))?;
    let manifest: PackageManifest = {
        let entry = archive
            .by_name("annocat-manifest.json")
            .map_err(|_| "validated result manifest disappeared")?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot reread result manifest: {error}"))?;
        parse_package_manifest(&bytes)?
    };
    let result_kind = crate::report_import::package_result_kind(
        &manifest.package_format,
        manifest.package_version,
        manifest.report_kind.as_deref(),
        manifest.result_kind.as_deref(),
        manifest.report_kind_present,
        manifest.result_kind_present,
    )?;
    if !(1..=annocat_core::RESULT_SCHEMA_VERSION as u32).contains(&manifest.schema_version)
        || manifest.display_name.trim().is_empty()
        || manifest.display_name.len() > 256
        || manifest.completed_at.is_empty()
        || manifest.completed_at.len() > 64
        || manifest.assembly != "GRCh38"
        || manifest.variant_count == 0
    {
        return Err("validated result package identity changed".into());
    }
    let imported_candidates = read_candidate_snapshot(&mut archive, &manifest)?;
    if let Some((existing, variants)) = existing_run(runs, &manifest)? {
        validate_candidate_alleles(&variants, &imported_candidates)?;
        crate::library_metadata::merge_candidate_snapshot(
            runs,
            &imported_candidates,
            &crate::annotation::current_timestamp(),
        )?;
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
        if declared.role == "candidate-bookmarks" {
            continue;
        }
        let mut entry = archive
            .by_name(&declared.path)
            .map_err(|_| format!("validated result file disappeared: {}", declared.path))?;
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
                "result file changed during import: {}",
                declared.path
            ));
        }
        roles.insert(declared.role.as_str(), destination);
    }
    let variants = role(&roles, "variants")?;
    let consequences = role(&roles, "consequences")?;
    let evidence = role(&roles, "evidence")?;
    let catalog = role(&roles, "field-catalog")?;
    let favor_roles = [
        "favor-evidence",
        "favor-status",
        "favor-field-catalog",
        "favor-provenance",
    ];
    let favor_role_count = favor_roles
        .iter()
        .filter(|role_name| roles.contains_key(**role_name))
        .count();
    if favor_role_count != 0 && favor_role_count != favor_roles.len() {
        return Err("imported online annotation files are incomplete".into());
    }
    let phenotype_roles = [
        "phenotype-profile",
        "phenotype-gene-evidence",
        "phenotype-field-catalog",
    ];
    let phenotype_role_count = phenotype_roles
        .iter()
        .filter(|role_name| roles.contains_key(**role_name))
        .count();
    if phenotype_role_count != 0 && phenotype_role_count != phenotype_roles.len() {
        return Err("imported phenotype evidence files are incomplete".into());
    }
    let has_phenotype_candidates = roles.contains_key("phenotype-candidate-evidence");
    if has_phenotype_candidates && phenotype_role_count != phenotype_roles.len() {
        return Err("imported phenotype ranks have no matching profile".into());
    }
    let variant_count = manifest.variant_count;
    if result_kind == "vcf-only" {
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
    validate_candidate_alleles(variants, &imported_candidates)?;

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
    let mut local_manifest = serde_json::json!({
        "schemaVersion": 1,
        "canonicalSchemaVersion": manifest.schema_version,
        "representativeSelectionContract": manifest.representative_selection_contract,
        "state": "completed",
        "runId": manifest.run_id,
        "name": name,
        "completedAt": manifest.completed_at,
        "assembly": manifest.assembly,
        "variantCount": variant_count,
        "reportKind": result_kind,
        "resultFile": variants_file.path,
        "resultSha256": variants_file.sha256,
        "consequencesFile": consequences_file.path,
        "consequencesSha256": consequences_file.sha256,
        "evidenceFile": evidence_file.path,
        "evidenceSha256": evidence_file.sha256,
        "fieldCatalogFile": catalog_file.path,
        "fieldCatalogSha256": catalog_file.sha256,
        "canonicalResultBytes": manifest.files.iter().map(|file| file.bytes).sum::<u64>(),
        "sourceIds": manifest.source_ids,
        "importedAt": crate::annotation::current_timestamp()
    });
    let object = local_manifest
        .as_object_mut()
        .ok_or("imported result manifest is not an object")?;
    if let (Some(name), Some(bytes), Some(sha256)) = (
        manifest.input_name.as_ref(),
        manifest.input_bytes,
        manifest.input_content_sha256.as_ref(),
    ) {
        object.insert("input".into(), name.clone().into());
        object.insert("inputName".into(), name.clone().into());
        object.insert("inputBytes".into(), bytes.into());
        object.insert("inputContentSha256".into(), sha256.clone().into());
    }
    if let Some(engine) = manifest.annotation_engine.as_ref() {
        if let Some(version) = engine.version.as_ref() {
            object.insert("fastvepVersion".into(), version.clone().into());
        }
        if let Some(sha256) = engine.sha256.as_ref() {
            object.insert("fastvepSha256".into(), sha256.clone().into());
        }
    }
    if let Some(provenance) = manifest.annotation_provenance.as_ref() {
        object.insert(
            "sources".into(),
            serde_json::to_value(&provenance.sources)
                .map_err(|error| format!("cannot restore source provenance: {error}"))?,
        );
        object.insert(
            "observedSourceIds".into(),
            serde_json::to_value(&provenance.observed_source_ids)
                .map_err(|error| format!("cannot restore observed sources: {error}"))?,
        );
        object.insert(
            "sourcesWithoutObservedEvidence".into(),
            serde_json::to_value(&provenance.sources_without_observed_evidence)
                .map_err(|error| format!("cannot restore missing source evidence: {error}"))?,
        );
        for (key, value) in [
            (
                "annotationSelection",
                provenance.annotation_selection.as_ref(),
            ),
            ("requestedProfile", provenance.requested_profile.as_ref()),
            (
                "referenceManifestSha256",
                provenance.reference_manifest_sha256.as_ref(),
            ),
            (
                "transcriptManifestSha256",
                provenance.transcript_manifest_sha256.as_ref(),
            ),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), value.clone().into());
            }
        }
    }
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&local_manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write imported run manifest: {error}"))?;
    if favor_role_count == favor_roles.len() {
        crate::favor::prepare_query_assets(evidence, catalog)?;
    }
    crate::library_metadata::write_candidate_snapshot(runs, &imported_candidates)?;
    if let Err(error) = fs::rename(&staging, &final_directory) {
        let _ = crate::library_metadata::remove_candidate_snapshot(runs, &manifest.run_id);
        return Err(format!("cannot publish imported result: {error}"));
    }
    std::mem::forget(cleanup);
    if phenotype_role_count == phenotype_roles.len() {
        let imported = |role_name: &str| {
            final_directory.join(
                &manifest
                    .files
                    .iter()
                    .find(|file| file.role == role_name)
                    .expect("phenotype role was validated")
                    .path,
            )
        };
        if let Err(error) = crate::phenotype::install_portable_group(
            runs,
            &manifest.run_id,
            &imported("phenotype-profile"),
            &imported("phenotype-gene-evidence"),
            &imported("phenotype-field-catalog"),
            has_phenotype_candidates
                .then(|| imported("phenotype-candidate-evidence"))
                .as_deref(),
        ) {
            let _ = fs::remove_dir_all(&final_directory);
            let _ = crate::library_metadata::remove_candidate_snapshot(runs, &manifest.run_id);
            return Err(format!(
                "cannot install imported phenotype evidence: {error}"
            ));
        }
    }
    Ok(ImportedReport {
        run_id: manifest.run_id,
        directory: final_directory,
        name,
    })
}

fn read_candidate_snapshot(
    archive: &mut zip::ZipArchive<File>,
    manifest: &PackageManifest,
) -> Result<crate::report_import::CandidateOverlay, String> {
    let Some(declared) = manifest
        .files
        .iter()
        .find(|file| file.role == "candidate-bookmarks")
    else {
        return Ok(crate::report_import::CandidateOverlay::empty(
            &manifest.run_id,
        ));
    };
    if declared.bytes == 0 || declared.bytes > crate::report_import::MAX_CANDIDATE_BYTES as u64 {
        return Err("candidate bookmark data has an invalid size".into());
    }
    let mut entry = archive
        .by_name(&declared.path)
        .map_err(|_| "validated candidate bookmarks disappeared")?;
    let mut bytes = Vec::with_capacity(declared.bytes as usize);
    let mut hasher = Sha256::new();
    let copied = copy_and_hash(&mut entry, &mut bytes, &mut hasher, declared.bytes)?;
    if copied != declared.bytes
        || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&declared.sha256)
    {
        return Err("candidate bookmarks changed during import".into());
    }
    crate::report_import::validate_candidate_bytes(&bytes, &manifest.run_id)
}

fn validate_candidate_alleles(
    variants: &Path,
    candidates: &crate::report_import::CandidateOverlay,
) -> Result<(), String> {
    if candidates.candidates.is_empty() {
        return Ok(());
    }
    let connection = Connection::open_in_memory()
        .map_err(|error| format!("cannot validate candidate bookmarks: {error}"))?;
    connection
        .execute_batch("CREATE TEMP TABLE requested_candidates(allele_id VARCHAR PRIMARY KEY);")
        .map_err(|error| format!("cannot prepare candidate bookmark validation: {error}"))?;
    {
        let mut appender = connection
            .appender("requested_candidates")
            .map_err(|error| format!("cannot prepare candidate bookmark validation: {error}"))?;
        for allele_id in candidates.candidates.keys() {
            appender
                .append_row([allele_id.as_str()])
                .map_err(|error| format!("cannot validate candidate bookmark: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot validate candidate bookmarks: {error}"))?;
    }
    let missing: u64 = connection
        .query_row(
            "SELECT count(*)
             FROM requested_candidates c
             WHERE NOT EXISTS (
               SELECT 1 FROM read_parquet(?) v WHERE v.allele_id=c.allele_id
             )",
            params![variants.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot validate candidate bookmarks: {error}"))?;
    if missing != 0 {
        return Err(format!(
            "{missing} candidate bookmark{} do not exist in this AnnoCAT result",
            if missing == 1 { "" } else { "s" }
        ));
    }
    Ok(())
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
            .map_err(|error| format!("cannot read AnnoCAT result file: {error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot write imported result file: {error}"))?;
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok(total)
}

fn role<'a>(roles: &'a BTreeMap<&str, PathBuf>, name: &str) -> Result<&'a Path, String> {
    roles
        .get(name)
        .map(PathBuf::as_path)
        .ok_or_else(|| format!("validated AnnoCAT result is missing {name}"))
}

fn existing_run(
    runs: &Path,
    package: &PackageManifest,
) -> Result<Option<(ImportedReport, PathBuf)>, String> {
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
                    "result ID {} already exists with different result content",
                    package.run_id
                ));
            }
        }
        let result_file = existing["resultFile"]
            .as_str()
            .ok_or_else(|| format!("existing result {} has no variant table", package.run_id))?;
        let variants = entry.path().join(result_file);
        if !variants.is_file() {
            return Err(format!(
                "existing result {} has no readable variant table",
                package.run_id
            ));
        }
        return Ok(Some((
            ImportedReport {
                run_id: package.run_id.clone(),
                directory: entry.path(),
                name: existing["name"]
                    .as_str()
                    .unwrap_or(&package.run_id)
                    .to_owned(),
            },
            variants,
        )));
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
    fn legacy_package_metadata_defaults_without_provenance() {
        let manifest = parse_package_manifest(
            br#"{"packageFormat":"annocat-report","packageVersion":1,"schemaVersion":1,"runId":"run-legacy","displayName":"Legacy result","completedAt":"2026-07-16T00:00:00Z","assembly":"GRCh38","variantCount":1,"files":[]}"#,
        )
        .unwrap();
        assert!(manifest.source_ids.is_empty());
        assert!(manifest.annotation_engine.is_none());
        assert!(manifest.annotation_provenance.is_none());
        assert!(manifest.input_name.is_none());
        assert!(manifest.input_content_sha256.is_none());
    }

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
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "canonicalSchemaVersion": 1,
                "representativeSelectionContract": "allele-gene-severity-v1",
                "state": "completed",
                "runId": "run-import",
                "name": "Import fixture",
                "completedAt": "2026-07-16T00:00:00Z",
                "assembly": "GRCh38",
                "variantCount": 1,
                "resultFile": "variants.parquet",
                "consequencesFile": "consequences.parquet",
                "evidenceFile": "evidence.parquet",
                "fieldCatalogFile": "field-catalog.json",
                "fastvepVersion": "0.2.0",
                "fastvepSha256": "a".repeat(64),
                "sourceIds": ["clinvar"],
                "sources": [{
                    "resourceId": "clinvar",
                    "release": "2026-07-15",
                    "assembly": "GRCh38",
                    "selectedSchema": "clinvar-20260715",
                    "cacheFormat": "osa2",
                    "osaSchemaVersion": 2,
                    "cacheBuilderContract": "fastvep-osa-v2-multivalue-v1",
                    "chromosomes": ["all"]
                }],
                "observedSourceIds": ["clinvar", "vep"],
                "sourcesWithoutObservedEvidence": [],
                "annotationSelection": "profile",
                "requestedProfile": "wgs",
                "referenceManifestSha256": "b".repeat(64),
                "transcriptManifestSha256": "c".repeat(64)
                ,"inputName": "sample.vcf.gz"
                ,"inputBytes": 1234
                ,"inputContentSha256": "d".repeat(64)
            }))
            .unwrap(),
        )
        .unwrap();
        let fingerprint = "a".repeat(64);
        let short = &fingerprint[..16];
        let phenotype_root = root.join(".annocat-library").join("run-import");
        fs::create_dir_all(&phenotype_root).unwrap();
        let phenotype_evidence = format!("phenotype-gene-evidence.{short}.parquet");
        let phenotype_catalog = format!("phenotype-field-catalog.{short}.json");
        let evidence_path = phenotype_root.join(&phenotype_evidence);
        let escaped_path = evidence_path.to_string_lossy().replace('\'', "''");
        duckdb::Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                    SELECT 'ENSG1'::VARCHAR AS gene_id, 'GENE1'::VARCHAR AS gene_symbol,
                           'gene'::VARCHAR AS scope, 'hpo'::VARCHAR AS source_id,
                           'profileLinked'::VARCHAR AS field_path, 'boolean'::VARCHAR AS value_type,
                           NULL::VARCHAR AS string_value, NULL::BIGINT AS integer_value,
                           NULL::DOUBLE AS number_value, true::BOOLEAN AS boolean_value,
                           NULL::VARCHAR AS json_value
                ) TO '{escaped_path}' (FORMAT PARQUET)"
            ))
            .unwrap();
        fs::write(
            phenotype_root.join(&phenotype_catalog),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "geneEvidenceFile": phenotype_evidence,
                "profileFingerprint": fingerprint,
                "hpoRelease": "2026-07-24",
                "mondoRelease": null,
                "algorithmVersion": "hpo-lin-query-v4",
                "sources": [{"id": "hpo"}],
                "fields": []
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            phenotype_root.join("phenotypes.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 4,
                "runId": "run-import",
                "updatedAt": "2026-07-30T00:00:00Z",
                "observed": [{"id": "HP:0001250", "label": "Seizure"}],
                "excluded": [],
                "conditions": [],
                "limitToLinkedGenes": false,
                "activeGeneration": {
                    "fingerprint": fingerprint,
                    "evidenceFile": phenotype_evidence,
                    "catalogFile": phenotype_catalog
                },
                "ranking": null,
                "monarchSuggestions": null,
                "monarchError": null
            }))
            .unwrap(),
        )
        .unwrap();
        let allele_id = format!(
            "allele-{}",
            &format!("{:x}", Sha256::digest(b"GRCh38\x001\x00100\x00A\x00G"))[..24]
        );
        let mut candidates = crate::report_import::CandidateOverlay::empty("run-import");
        candidates.revision = 2;
        candidates.updated_at = "2026-07-29T00:00:00Z".into();
        candidates.candidates.insert(
            allele_id.clone(),
            crate::report_import::CandidateEntry {
                allele_id: allele_id.clone(),
                added_at: "2026-07-29T00:00:00Z".into(),
                reason: "Imported reason".into(),
            },
        );
        let package_path = root.join("Import-fixture--20260716--import.zip");
        crate::report_package::create(&run, &package_path, &candidates).unwrap();
        let imported = import(&package_path, &library).unwrap();
        assert_eq!(imported.run_id, "run-import");
        assert!(imported.directory.join("manifest.json").is_file());
        assert!(imported.directory.join("variants.parquet").is_file());
        let imported_manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(imported.directory.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(
            imported_manifest["representativeSelectionContract"],
            "allele-gene-severity-v1"
        );
        assert_eq!(imported_manifest["fastvepVersion"], "0.2.0");
        assert_eq!(
            imported_manifest["sourceIds"],
            serde_json::json!(["clinvar", "hpo"])
        );
        assert_eq!(imported_manifest["sources"][0]["resourceId"], "clinvar");
        assert_eq!(imported_manifest["requestedProfile"], "wgs");
        assert_eq!(imported_manifest["annotationSelection"], "profile");
        assert_eq!(imported_manifest["inputName"], "sample.vcf.gz");
        assert_eq!(imported_manifest["inputBytes"], 1234);
        assert_eq!(imported_manifest["inputContentSha256"], "d".repeat(64));
        assert!(imported_manifest.get("importedFrom").is_none());
        assert!(package_path.is_file(), "source ZIP must remain untouched");
        assert!(!library.join(".import-run-import.partial").exists());
        let restored = crate::library_metadata::candidate_snapshot(&library, "run-import").unwrap();
        assert_eq!(restored.revision, 2);
        assert_eq!(restored.updated_at, "2026-07-29T00:00:00Z");
        assert_eq!(restored.candidates[&allele_id].reason, "Imported reason");
        let restored_profile = crate::phenotype::load(&library, "run-import").unwrap();
        assert_eq!(restored_profile.observed[0].id, "HP:0001250");
        assert!(
            !imported.directory.join("phenotypes.json").exists(),
            "phenotype state belongs in the result-library overlay"
        );

        let mut local = restored;
        local.candidates.get_mut(&allele_id).unwrap().reason = "Local reason".into();
        crate::library_metadata::write_candidate_snapshot(&library, &local).unwrap();
        let repeated = import(&package_path, &library).unwrap();
        assert_eq!(repeated.directory, imported.directory);
        assert_eq!(
            fs::read_dir(&library)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name() != ".annocat-library")
                .count(),
            1
        );
        assert_eq!(
            crate::library_metadata::candidate_snapshot(&library, "run-import")
                .unwrap()
                .candidates[&allele_id]
                .reason,
            "Local reason"
        );

        let invalid_library = root.join("invalid-library");
        let mut invalid = crate::report_import::CandidateOverlay::empty("run-import");
        invalid.candidates.insert(
            "allele-absent".into(),
            crate::report_import::CandidateEntry {
                allele_id: "allele-absent".into(),
                added_at: "2026-07-29T00:00:00Z".into(),
                reason: "Imported reason".into(),
            },
        );
        let invalid_package = root.join("invalid.zip");
        crate::report_package::create(&run, &invalid_package, &invalid).unwrap();
        assert!(import(&invalid_package, &invalid_library).is_err());
        assert!(
            !invalid_library
                .read_dir()
                .is_ok_and(|mut entries| entries.any(|entry| entry.is_ok()))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
