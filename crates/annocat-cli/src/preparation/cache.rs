use super::checkpoint::{
    CHECKPOINT_SCHEMA_VERSION, CheckpointState, PreparationCheckpoint, PreparationIdentity,
    RestartDecision, ShardPaths, read_checkpoint, write_checkpoint,
};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

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
    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
        if checkpoint.state != CheckpointState::Verified {
            return RestartDecision::StaleIdentity;
        }
        if paths.cache_contract().is_file() {
            let Ok(installed) = crate::cache_contract::read(&paths.cache_contract()) else {
                return RestartDecision::StaleIdentity;
            };
            let expected = cache_contract_manifest(identity);
            return if installed.compatibility_with(&expected)
                == crate::cache_contract::CacheCompatibilityDecision::Ready
            {
                RestartDecision::AlreadyVerified
            } else {
                RestartDecision::StaleIdentity
            };
        }
        // A verified schema-v1 cache predates the compatibility sidecar. Preserve
        // known OSA-v1 resources until their source-specific verifier can prove and
        // atomically publish cache-contract-v2.json. Do not rebuild merely because
        // the fork moved, and do not silently bless an unknown legacy adapter.
        if checkpoint.identity != *identity {
            return RestartDecision::StaleIdentity;
        }
        return match crate::cache_contract::classify_legacy_manifest(
            &identity.resource_id,
            identity.osa_schema_version,
        ) {
            crate::cache_contract::CacheCompatibilityDecision::VerifyAndUpgradeManifest => {
                RestartDecision::AlreadyVerified
            }
            _ => RestartDecision::StaleIdentity,
        };
    }
    match read_checkpoint(&paths.checkpoint()) {
        Ok(checkpoint) if checkpoint.identity == *identity => {
            RestartDecision::RestartCurrentChromosome
        }
        Ok(_) => RestartDecision::StaleIdentity,
        Err(_) => RestartDecision::Start,
    }
}

pub(super) fn restart_decision_with_legacy_upgrade(
    fastvep_executable: &Path,
    paths: &ShardPaths,
    identity: &PreparationIdentity,
) -> RestartDecision {
    let decision = restart_decision(paths, identity);
    if decision == RestartDecision::AlreadyVerified
        && !paths.cache_contract().is_file()
        && upgrade_legacy_cache_contract(fastvep_executable, paths, identity).is_err()
    {
        return RestartDecision::StaleIdentity;
    }
    decision
}

fn upgrade_legacy_cache_contract(
    fastvep_executable: &Path,
    paths: &ShardPaths,
    expected: &PreparationIdentity,
) -> Result<(), String> {
    let checkpoint = read_checkpoint(&paths.verification())?;
    if checkpoint.state != CheckpointState::Verified || checkpoint.identity != *expected {
        return Err("legacy cache checkpoint does not match the expected source identity".into());
    }
    crate::cache_contract::prove_legacy_source_contract(
        &expected.resource_id,
        &expected.release,
        &expected.assembly,
        &expected.selected_schema,
        expected.osa_schema_version,
    )?;
    required_nonempty_file(&paths.final_osa())?;
    required_nonempty_file(&paths.final_index())?;
    let verification = verify_osa(fastvep_executable, &paths.final_osa(), expected)?;
    if verification.record_count != checkpoint.parsed_records {
        return Err("legacy cache record count differs from its verified checkpoint".into());
    }
    let mut manifest = cache_contract_manifest(expected);
    manifest.builder_provenance = crate::cache_contract::BuilderProvenance {
        repository: "unknown-legacy".into(),
        commit: "unknown-legacy".into(),
        binary_sha256: "unknown-legacy".into(),
    };
    crate::cache_contract::write_atomic(&paths.cache_contract(), &manifest)
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
    write_checkpoint(
        &paths.checkpoint(),
        &PreparationCheckpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            identity,
            state: CheckpointState::Preparing,
            compressed_bytes_read: 0,
            parsed_records: 0,
            prepared_bytes: 0,
            prepared_index_bytes: 0,
        },
    )
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
    let osa_bytes = required_nonempty_file(&paths.partial_osa())?;
    let index_bytes = required_nonempty_file(&paths.partial_index())?;
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
    };
    write_checkpoint(&paths.partial_directory.join("verified.json"), &verified)?;
    crate::cache_contract::write_atomic(
        &paths.partial_cache_contract(),
        &cache_contract_manifest(&verified.identity),
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
    let result = verify_osa(fastvep_executable, &paths.partial_osa(), identity);
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
) -> crate::cache_contract::CacheContractManifest {
    crate::cache_contract::CacheContractManifest::current(
        crate::fastvep::pinned_builder_provenance(),
        &identity.resource_id,
        &identity.release,
        &identity.assembly,
        &identity.chromosome,
        identity.expected_compressed_bytes,
        identity.source_etag.as_deref(),
        identity.source_last_modified.as_deref(),
        &identity.selected_schema,
        identity.osa_schema_version,
        Some(&identity.fastvep_commit),
    )
}
