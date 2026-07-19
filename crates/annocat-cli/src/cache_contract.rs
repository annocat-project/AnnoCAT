use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

pub const CACHE_CONTRACT_MANIFEST_SCHEMA_VERSION: u16 = 2;
pub const OSA_READER_COMPATIBILITY_V1: &str = "fastvep-osa-v1";
pub const OSA_BUILDER_CONTRACT_V1: &str = "annocat-sa-build-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuilderProvenance {
    pub repository: String,
    pub commit: String,
    pub binary_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheContract {
    pub osa_schema_version: u16,
    pub reader_compatibility: String,
    pub builder_contract: String,
    pub adapter_contract: String,
    pub selected_field_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceArtifactIdentity {
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub chromosome: String,
    pub artifact_id: String,
    pub content_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheContractManifest {
    pub schema_version: u16,
    pub builder_provenance: BuilderProvenance,
    pub cache_contract: CacheContract,
    pub source_artifact: SourceArtifactIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_fastvep_identity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCompatibilityDecision {
    Ready,
    VerifyAndUpgradeManifest,
    RebuildAffectedSource,
    RebuildAllOsaV1,
    Unsupported,
}

pub fn classify_legacy_manifest(
    resource_id: &str,
    osa_schema_version: u16,
) -> CacheCompatibilityDecision {
    if osa_schema_version != 1 {
        return CacheCompatibilityDecision::RebuildAllOsaV1;
    }
    match resource_id {
        "dbnsfp" | "clinvar" | "dbsnp" | "gnomad" | "gnomad-genomes" | "cadd" | "phylop"
        | "spliceai" | "revel" => CacheCompatibilityDecision::VerifyAndUpgradeManifest,
        _ => CacheCompatibilityDecision::Unsupported,
    }
}

pub fn prove_legacy_source_contract(
    resource_id: &str,
    release: &str,
    assembly: &str,
    selected_schema: &str,
    osa_schema_version: u16,
) -> Result<(), String> {
    if classify_legacy_manifest(resource_id, osa_schema_version)
        != CacheCompatibilityDecision::VerifyAndUpgradeManifest
    {
        return Err("legacy cache uses an unsupported OSA or source contract".into());
    }
    let catalog = annocat_core::source_catalog::resource(resource_id)
        .ok_or_else(|| format!("legacy cache source {resource_id} is not cataloged"))?;
    if catalog.assembly != assembly {
        return Err("legacy cache assembly differs from the source catalog".into());
    }
    if catalog.release.policy == "pinned" && catalog.release.version != release {
        return Err("legacy cache release differs from the pinned source catalog".into());
    }
    if release.trim().is_empty() {
        return Err("legacy cache release is empty".into());
    }
    let schema_prefix = match resource_id {
        "dbnsfp" => "dbnsfp-4.9a-annocat-core-v1",
        "clinvar" => "clinvar-",
        "dbsnp" => "dbsnp-",
        "gnomad" => "gnomad-v4.1.1-exomes-",
        "gnomad-genomes" => "gnomad-v4.1.1-genomes-",
        "cadd" => "cadd-v1.7-grch38",
        "phylop" => "ucsc-hg38-phylop100way-per-base",
        "spliceai" => "spliceai-ensembl-mane-v1.4-masked-snv",
        "revel" => "revel-v1.3-transcript-matched",
        _ => return Err("legacy cache source has no migration proof rule".into()),
    };
    if !selected_schema.starts_with(schema_prefix) {
        return Err(format!(
            "legacy {resource_id} cache has an unrecognized selected-field schema"
        ));
    }
    Ok(())
}

impl CacheContractManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn current(
        builder_provenance: BuilderProvenance,
        resource_id: &str,
        release: &str,
        assembly: &str,
        chromosome: &str,
        expected_compressed_bytes: u64,
        source_etag: Option<&str>,
        source_last_modified: Option<&str>,
        selected_field_schema: &str,
        osa_schema_version: u16,
        legacy_fastvep_identity: Option<&str>,
    ) -> Self {
        Self {
            schema_version: CACHE_CONTRACT_MANIFEST_SCHEMA_VERSION,
            builder_provenance,
            cache_contract: CacheContract {
                osa_schema_version,
                reader_compatibility: OSA_READER_COMPATIBILITY_V1.into(),
                builder_contract: OSA_BUILDER_CONTRACT_V1.into(),
                adapter_contract: adapter_contract(resource_id).into(),
                selected_field_schema: selected_field_schema.into(),
            },
            source_artifact: SourceArtifactIdentity {
                resource_id: resource_id.into(),
                release: release.into(),
                assembly: assembly.into(),
                chromosome: chromosome.into(),
                artifact_id: annocat_core::source_catalog::artifact_identity(
                    resource_id,
                    release,
                    assembly,
                    chromosome,
                ),
                content_identity: source_content_identity(
                    release,
                    expected_compressed_bytes,
                    source_etag,
                    source_last_modified,
                ),
            },
            legacy_fastvep_identity: legacy_fastvep_identity.map(str::to_string),
        }
    }

    pub fn compatibility_with(&self, expected: &Self) -> CacheCompatibilityDecision {
        if self.schema_version != CACHE_CONTRACT_MANIFEST_SCHEMA_VERSION
            || expected.schema_version != CACHE_CONTRACT_MANIFEST_SCHEMA_VERSION
        {
            return CacheCompatibilityDecision::Unsupported;
        }
        if self.cache_contract.osa_schema_version != expected.cache_contract.osa_schema_version
            || self.cache_contract.reader_compatibility
                != expected.cache_contract.reader_compatibility
            || self.cache_contract.builder_contract != expected.cache_contract.builder_contract
        {
            return CacheCompatibilityDecision::RebuildAllOsaV1;
        }
        if self.cache_contract.adapter_contract != expected.cache_contract.adapter_contract
            || self.cache_contract.selected_field_schema
                != expected.cache_contract.selected_field_schema
            || self.source_artifact != expected.source_artifact
        {
            return CacheCompatibilityDecision::RebuildAffectedSource;
        }
        // The exact builder is provenance. A different commit or binary hash is
        // compatible when every cache contract and artifact identity still match.
        CacheCompatibilityDecision::Ready
    }
}

pub fn read(path: &Path) -> Result<CacheContractManifest, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read cache contract {}: {error}", path.display()))?;
    let manifest: CacheContractManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot decode cache contract {}: {error}", path.display()))?;
    if manifest.schema_version != CACHE_CONTRACT_MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported cache contract schema {}",
            manifest.schema_version
        ));
    }
    Ok(manifest)
}

pub fn write_atomic(path: &Path, manifest: &CacheContractManifest) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache contract has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create cache contract directory {}: {error}",
            parent.display()
        )
    })?;
    let temporary = parent.join(format!(".cache-contract-v2.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("cannot encode cache contract: {error}"))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "cannot create cache contract staging file {}: {error}",
                temporary.display()
            )
        })?;
    file.write_all(&bytes).map_err(|error| {
        format!(
            "cannot write cache contract staging file {}: {error}",
            temporary.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "cannot flush cache contract staging file {}: {error}",
            temporary.display()
        )
    })?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("cannot publish cache contract {}: {error}", path.display())
    })
}

fn adapter_contract(resource_id: &str) -> &'static str {
    annocat_core::source_catalog::adapter_contract(resource_id)
        .unwrap_or("annocat-supplementary-v1")
}

fn source_content_identity(
    release: &str,
    expected_compressed_bytes: u64,
    source_etag: Option<&str>,
    source_last_modified: Option<&str>,
) -> String {
    if let Some(etag) = source_etag.filter(|value| !value.trim().is_empty()) {
        return format!("etag:{etag}:bytes:{expected_compressed_bytes}");
    }
    if let Some(last_modified) = source_last_modified.filter(|value| !value.trim().is_empty()) {
        return format!("last-modified:{last_modified}:bytes:{expected_compressed_bytes}");
    }
    format!("release:{release}:bytes:{expected_compressed_bytes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(commit: &str) -> BuilderProvenance {
        BuilderProvenance {
            repository: "https://github.com/annocat-project/fastVEP".into(),
            commit: commit.into(),
            binary_sha256: format!("sha-{commit}"),
        }
    }

    fn manifest(commit: &str) -> CacheContractManifest {
        CacheContractManifest::current(
            provenance(commit),
            "gnomad",
            "4.1",
            "GRCh38",
            "1",
            100,
            Some("same-object"),
            None,
            "gnomad-v4.1-fields-a",
            1,
            Some("7038e7c"),
        )
    }

    #[test]
    fn builder_update_does_not_invalidate_a_compatible_cache() {
        assert_eq!(
            manifest("old").compatibility_with(&manifest("new")),
            CacheCompatibilityDecision::Ready
        );
    }

    #[test]
    fn selected_field_change_rebuilds_only_the_affected_source() {
        let installed = manifest("same");
        let mut expected = installed.clone();
        expected.cache_contract.selected_field_schema = "gnomad-v4.1-fields-b".into();
        assert_eq!(
            installed.compatibility_with(&expected),
            CacheCompatibilityDecision::RebuildAffectedSource
        );
    }

    #[test]
    fn osa_contract_change_rebuilds_osa_caches() {
        let installed = manifest("same");
        let mut expected = installed.clone();
        expected.cache_contract.reader_compatibility = "fastvep-osa-v2".into();
        assert_eq!(
            installed.compatibility_with(&expected),
            CacheCompatibilityDecision::RebuildAllOsaV1
        );
    }

    #[test]
    fn equivalent_url_is_not_part_of_artifact_identity() {
        let value = manifest("same");
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(!encoded.contains("download.example"));
    }

    #[test]
    fn known_legacy_osa_v1_requires_verification_before_manifest_upgrade() {
        assert_eq!(
            classify_legacy_manifest("dbnsfp", 1),
            CacheCompatibilityDecision::VerifyAndUpgradeManifest
        );
        assert_eq!(
            classify_legacy_manifest("unknown-source", 1),
            CacheCompatibilityDecision::Unsupported
        );
    }

    #[test]
    fn legacy_migration_requires_a_source_specific_release_and_schema() {
        assert!(
            prove_legacy_source_contract(
                "revel",
                "1.3",
                "GRCh38",
                "revel-v1.3-transcript-matched",
                1
            )
            .is_ok()
        );
        assert!(
            prove_legacy_source_contract(
                "revel",
                "1.4",
                "GRCh38",
                "revel-v1.3-transcript-matched",
                1
            )
            .unwrap_err()
            .contains("release")
        );
        assert!(
            prove_legacy_source_contract("revel", "1.3", "GRCh38", "generic-vcf", 1)
                .unwrap_err()
                .contains("schema")
        );
    }
}
