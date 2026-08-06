use super::checkpoint::{
    CHECKPOINT_SCHEMA_VERSION, CacheFormat, CheckpointState, PreparationCheckpoint,
    PreparationIdentity, RestartDecision, ShardPaths, read_checkpoint, write_checkpoint,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifiedCacheCompatibility {
    Missing,
    Ready,
    RebuildRequired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceVerification {
    pub source_id: String,
    pub shard_count: usize,
    pub hashed_file_count: usize,
    pub structurally_verified_shard_count: usize,
    pub verified_bytes: u64,
}

pub(crate) fn verified_cache_compatibility(
    paths: &ShardPaths,
    expected: &PreparationIdentity,
) -> VerifiedCacheCompatibility {
    if !paths.final_directory.is_dir() {
        return VerifiedCacheCompatibility::Missing;
    }
    let Ok(checkpoint) = read_checkpoint(&paths.verification()) else {
        return VerifiedCacheCompatibility::RebuildRequired;
    };
    if verified_cache_files(&paths.final_directory, &checkpoint).is_err() {
        return VerifiedCacheCompatibility::RebuildRequired;
    }
    if paths.cache_contract().is_file() {
        let Ok(installed) = crate::cache_contract::read(&paths.cache_contract()) else {
            return VerifiedCacheCompatibility::RebuildRequired;
        };
        let Ok(checkpoint_contract) = cache_contract_manifest(&checkpoint.identity) else {
            return VerifiedCacheCompatibility::RebuildRequired;
        };
        let Ok(expected) = cache_contract_manifest(expected) else {
            return VerifiedCacheCompatibility::RebuildRequired;
        };
        return if installed.compatibility_with(&checkpoint_contract)
            == crate::cache_contract::CacheCompatibilityDecision::Ready
            && installed.compatibility_with(&expected)
                == crate::cache_contract::CacheCompatibilityDecision::Ready
        {
            VerifiedCacheCompatibility::Ready
        } else {
            VerifiedCacheCompatibility::RebuildRequired
        };
    }
    VerifiedCacheCompatibility::RebuildRequired
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaVerificationReport {
    name: String,
    version: String,
    assembly: String,
    json_key: String,
    chromosomes: Vec<String>,
    block_count: u64,
    pub(super) record_count: u64,
    lookup_count: u64,
}

pub(super) fn restart_decision(
    paths: &ShardPaths,
    identity: &PreparationIdentity,
) -> RestartDecision {
    match verified_cache_compatibility(paths, identity) {
        VerifiedCacheCompatibility::Ready => return RestartDecision::AlreadyVerified,
        VerifiedCacheCompatibility::RebuildRequired => {
            return RestartDecision::StaleIdentity;
        }
        VerifiedCacheCompatibility::Missing => {}
    }
    match read_checkpoint(&paths.checkpoint()) {
        Ok(checkpoint) if checkpoint.identity == *identity => {
            RestartDecision::RestartCurrentChromosome
        }
        Ok(_) => RestartDecision::StaleIdentity,
        Err(_) => RestartDecision::Start,
    }
}

pub(crate) fn effective_cache_format(
    paths: &ShardPaths,
    resource_id: &str,
) -> Result<CacheFormat, String> {
    let mut selected = None;
    if let Some(shards) = paths.final_directory.parent()
        && shards.is_dir()
    {
        for entry in fs::read_dir(shards)
            .map_err(|error| format!("cannot inspect {resource_id} cache shards: {error}"))?
        {
            let directory = entry
                .map_err(|error| format!("cannot inspect {resource_id} cache shard: {error}"))?
                .path();
            if let Some(format) = verified_shard_format(&directory, resource_id) {
                select_cache_format(&mut selected, format, resource_id)?;
            }
        }
    }
    if let Ok(checkpoint) = read_checkpoint(&paths.checkpoint())
        && checkpoint.identity.resource_id == resource_id
        && let Ok(format) = checkpoint.identity.cache_format()
    {
        select_cache_format(&mut selected, format, resource_id)?;
    }
    if required_nonempty_file(paths.source_part()).is_ok()
        && let Ok(bytes) = fs::read(paths.source_part_identity())
        && let Ok(identity) = serde_json::from_slice::<PreparationIdentity>(&bytes)
        && identity.resource_id == resource_id
        && let Ok(format) = identity.cache_format()
    {
        select_cache_format(&mut selected, format, resource_id)?;
    }
    selected.map_or_else(|| CacheFormat::preferred_for_resource(resource_id), Ok)
}

fn verified_shard_format(directory: &Path, resource_id: &str) -> Option<CacheFormat> {
    let checkpoint = read_checkpoint(&directory.join("verified.json")).ok()?;
    if checkpoint.state != CheckpointState::Verified
        || checkpoint.identity.resource_id != resource_id
    {
        return None;
    }
    let format = verified_cache_files(directory, &checkpoint).ok()?;
    let contract = crate::cache_contract::read(&directory.join("cache-contract-v2.json")).ok()?;
    (contract.cache_contract.osa_schema_version == format.schema_version()
        && contract.cache_contract.reader_compatibility == format.reader_compatibility()
        && contract.cache_contract.builder_contract == format.builder_contract())
    .then_some(format)
}

pub(crate) fn verified_cache_files(
    directory: &Path,
    checkpoint: &PreparationCheckpoint,
) -> Result<CacheFormat, String> {
    if checkpoint.state != CheckpointState::Verified {
        return Err("cache checkpoint is not verified".into());
    }
    let format = checkpoint.identity.cache_format()?;
    let data = directory.join(format.data_file_name());
    let data_bytes = required_nonempty_file(&data)?;
    if data_bytes != checkpoint.prepared_bytes {
        return Err(format!(
            "prepared data size differs from verified.json ({data_bytes} != {})",
            checkpoint.prepared_bytes
        ));
    }
    match format.index_file_name() {
        Some(name) => {
            let index_bytes = required_nonempty_file(&directory.join(name))?;
            if index_bytes != checkpoint.prepared_index_bytes {
                return Err(format!(
                    "prepared index size differs from verified.json ({index_bytes} != {})",
                    checkpoint.prepared_index_bytes
                ));
            }
        }
        None if checkpoint.prepared_index_bytes != 0 => {
            return Err("OSA2 verified.json declares an external index".into());
        }
        None if directory.join("source.osa.idx").exists() => {
            return Err("OSA2 cache has an unexpected external index".into());
        }
        None => {}
    }
    Ok(format)
}

pub(crate) fn verify_source_cache(
    fastvep_executable: &Path,
    resource_root: &Path,
    resource_id: &str,
    chromosomes: &[String],
    mut progress: impl FnMut(&str),
) -> Result<SourceVerification, String> {
    let mut result = SourceVerification {
        source_id: resource_id.into(),
        shard_count: 0,
        hashed_file_count: 0,
        structurally_verified_shard_count: 0,
        verified_bytes: 0,
    };
    for chromosome in chromosomes {
        progress(chromosome);
        let paths = ShardPaths::new(resource_root, chromosome)?;
        let checkpoint = read_checkpoint(&paths.verification())
            .map_err(|error| format!("{resource_id} chromosome {chromosome}: {error}"))?;
        if checkpoint.identity.resource_id != resource_id
            || checkpoint.identity.chromosome != *chromosome
            || verified_cache_compatibility(&paths, &checkpoint.identity)
                != VerifiedCacheCompatibility::Ready
        {
            return Err(format!(
                "{resource_id} chromosome {chromosome}: cache identity or contract is not ready"
            ));
        }
        let format = verified_cache_files(&paths.final_directory, &checkpoint)
            .map_err(|error| format!("{resource_id} chromosome {chromosome}: {error}"))?;
        let data = paths.final_data(format);
        let mut needs_structural_verification = checkpoint.prepared_sha256.is_none();
        if let Some(expected) = checkpoint.prepared_sha256.as_deref() {
            verify_file_hash(&data, expected, resource_id, chromosome)?;
            result.hashed_file_count += 1;
        }
        result.verified_bytes = result
            .verified_bytes
            .saturating_add(checkpoint.prepared_bytes);
        if let Some(index) = paths.final_index(format) {
            if let Some(expected) = checkpoint.prepared_index_sha256.as_deref() {
                verify_file_hash(&index, expected, resource_id, chromosome)?;
                result.hashed_file_count += 1;
            } else {
                needs_structural_verification = true;
            }
            result.verified_bytes = result
                .verified_bytes
                .saturating_add(checkpoint.prepared_index_bytes);
        } else if checkpoint.prepared_index_sha256.is_some() {
            return Err(format!(
                "{resource_id} chromosome {chromosome}: OSA2 checkpoint contains an index hash"
            ));
        }
        if needs_structural_verification {
            verify_osa(fastvep_executable, &data, &checkpoint.identity)
                .map_err(|error| format!("{resource_id} chromosome {chromosome}: {error}"))?;
            result.structurally_verified_shard_count += 1;
        }
        result.shard_count += 1;
    }
    if result.shard_count == 0 {
        return Err(format!("{resource_id} has no installed cache shards"));
    }
    Ok(result)
}

fn verify_file_hash(
    path: &Path,
    expected: &str,
    resource_id: &str,
    chromosome: &str,
) -> Result<(), String> {
    let actual = crate::fastvep::sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "{resource_id} chromosome {chromosome}: SHA-256 mismatch for {}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("prepared cache")
        ))
    }
}

fn select_cache_format(
    selected: &mut Option<CacheFormat>,
    format: CacheFormat,
    resource_id: &str,
) -> Result<(), String> {
    if selected.is_some_and(|current| current != format) {
        return Err(format!(
            "{resource_id} has mixed OSA1 and OSA2 cache state; cancel the partial installation and rebuild it"
        ));
    }
    *selected = Some(format);
    Ok(())
}

pub fn initialize_partial(paths: &ShardPaths, identity: PreparationIdentity) -> Result<(), String> {
    if paths.final_directory.exists() {
        return Err(format!(
            "verified shard already exists: {}",
            paths.final_directory.display()
        ));
    }
    if paths.partial_directory.exists() {
        if let Some(detail) = super::builder::discard_parts_from_previous_corruption(
            &paths.partial_directory.join("fastvep.log"),
            paths.source_part(),
        ) {
            crate::terminal_log(
                "resources",
                format!(
                    "{} chromosome {}: discarded corrupt retained source data ({detail}); downloading this chromosome again",
                    identity.resource_id, identity.chromosome
                ),
            );
        }
        fs::remove_dir_all(&paths.partial_directory)
            .map_err(|error| format!("cannot clear incomplete shard: {error}"))?;
    }
    fs::create_dir_all(&paths.partial_directory)
        .map_err(|error| format!("cannot create shard staging directory: {error}"))?;
    let manifest = cache_contract_manifest(&identity)?;
    write_checkpoint(
        &paths.checkpoint(),
        &PreparationCheckpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            identity: identity.clone(),
            state: CheckpointState::Preparing,
            compressed_bytes_read: 0,
            parsed_records: 0,
            prepared_bytes: 0,
            prepared_index_bytes: 0,
            prepared_sha256: None,
            prepared_index_sha256: None,
        },
    )?;
    crate::cache_contract::write_atomic(&paths.partial_cache_contract(), &manifest)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn promote_verified(
    paths: &ShardPaths,
    identity: PreparationIdentity,
    compressed_bytes_read: u64,
    parsed_records: u64,
) -> Result<(), String> {
    if compressed_bytes_read != identity.expected_compressed_bytes {
        return Err(format!(
            "compressed stream length mismatch: received {compressed_bytes_read}, expected {}",
            identity.expected_compressed_bytes
        ));
    }
    let format = identity.cache_format()?;
    let osa_bytes = required_nonempty_file(&paths.partial_data(format))?;
    let index_bytes = match paths.partial_index(format) {
        Some(index) => required_nonempty_file(&index)?,
        None => 0,
    };
    if parsed_records == 0 {
        return Err("refusing to promote an empty prepared chromosome".into());
    }

    let verified = PreparationCheckpoint {
        schema_version: CHECKPOINT_SCHEMA_VERSION,
        identity,
        state: CheckpointState::Verified,
        compressed_bytes_read,
        parsed_records,
        prepared_bytes: osa_bytes,
        prepared_index_bytes: index_bytes,
        prepared_sha256: Some(crate::fastvep::sha256_file(&paths.partial_data(format))?),
        prepared_index_sha256: paths
            .partial_index(format)
            .map(|path| crate::fastvep::sha256_file(&path))
            .transpose()?,
    };
    write_checkpoint(&paths.partial_directory.join("verified.json"), &verified)?;
    crate::cache_contract::write_atomic(
        &paths.partial_cache_contract(),
        &cache_contract_manifest(&verified.identity)?,
    )?;
    fs::create_dir_all(
        paths
            .final_directory
            .parent()
            .ok_or("final shard has no parent")?,
    )
    .map_err(|error| format!("cannot create final shard directory: {error}"))?;
    if paths.final_directory.exists() {
        return Err(format!(
            "refusing to replace existing shard: {}",
            paths.final_directory.display()
        ));
    }
    fs::rename(&paths.partial_directory, &paths.final_directory)
        .map_err(|error| format!("cannot atomically promote verified shard: {error}"))?;
    // The compressed part is only a restart aid. Once the cache is verified,
    // retaining it would duplicate source storage without adding safety.
    if let Some(parent) = paths.source_part().parent() {
        let base = paths
            .source_part()
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source")
            .to_string();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == format!("{base}.part")
                    || name == format!("{base}.identity.json")
                    || name.starts_with(&format!("{base}."))
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    if let Some(parent) = paths.source_part().parent()
        && parent.is_dir()
        && fs::read_dir(parent).is_ok_and(|mut entries| entries.next().is_none())
    {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

pub(super) fn verify_partial_osa(
    fastvep_executable: &Path,
    paths: &ShardPaths,
    identity: &PreparationIdentity,
) -> Result<SaVerificationReport, String> {
    let result = identity
        .cache_format()
        .and_then(|format| verify_osa(fastvep_executable, &paths.partial_data(format), identity));
    if result.is_err() {
        super::remove_incomplete_outputs(paths);
    }
    result
}

fn verify_osa(
    fastvep_executable: &Path,
    osa_path: &Path,
    identity: &PreparationIdentity,
) -> Result<SaVerificationReport, String> {
    let mut command = Command::new(fastvep_executable);
    command
        .arg("sa-verify")
        .arg("--input")
        .arg(osa_path)
        .arg("--assembly")
        .arg(&identity.assembly)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
    if identity.chromosome != "all" {
        command.arg("--chromosome").arg(&identity.chromosome);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot start fastVEP OSA verifier: {error}"))?;
    if !output.status.success() {
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "fastVEP OSA verification failed: {}",
            diagnostic.trim()
        ));
    }
    let report: SaVerificationReport = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("fastVEP returned an invalid OSA verification report: {error}"))?;
    if report.assembly != identity.assembly {
        return Err("verified OSA assembly differs from the preparation identity".into());
    }
    if report.name.trim().is_empty()
        || report.version.trim().is_empty()
        || report.json_key.trim().is_empty()
    {
        return Err("verified OSA source metadata is incomplete".into());
    }
    if report.chromosomes.is_empty()
        || report.block_count == 0
        || report.record_count == 0
        || report.lookup_count == 0
    {
        return Err("fastVEP OSA verification report is unexpectedly empty".into());
    }
    Ok(report)
}

pub(super) fn required_nonempty_file(path: &Path) -> Result<u64, String> {
    let bytes = path
        .metadata()
        .map_err(|error| {
            format!(
                "required prepared file {} is unavailable: {error}",
                path.display()
            )
        })?
        .len();
    if bytes == 0 {
        Err(format!(
            "required prepared file is empty: {}",
            path.display()
        ))
    } else {
        Ok(bytes)
    }
}

fn cache_contract_manifest(
    identity: &PreparationIdentity,
) -> Result<crate::cache_contract::CacheContractManifest, String> {
    Ok(crate::cache_contract::CacheContractManifest::current(
        crate::fastvep::pinned_builder_provenance(),
        &identity.resource_id,
        &identity.release,
        &identity.assembly,
        &identity.chromosome,
        identity.expected_compressed_bytes,
        identity.source_etag.as_deref(),
        identity.source_last_modified.as_deref(),
        &identity.selected_schema,
        identity.cache_format()?,
        Some(&identity.fastvep_commit),
    ))
}
