use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const CHECKPOINT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFormat {
    OsaV1,
    OsaV2,
}

impl CacheFormat {
    pub fn from_schema_version(version: u16) -> Result<Self, String> {
        match version {
            1 => Ok(Self::OsaV1),
            2 => Ok(Self::OsaV2),
            _ => Err(format!("unsupported OSA schema version {version}")),
        }
    }

    pub fn preferred_for_resource(resource_id: &str) -> Result<Self, String> {
        match annocat_core::source_catalog::preferred_cache_format(resource_id) {
            Some("osa") => Ok(Self::OsaV1),
            Some("osa2") => Ok(Self::OsaV2),
            Some(format) => Err(format!(
                "resource {resource_id} has unsupported preferred cache format {format}"
            )),
            None => Err(format!(
                "resource {resource_id} has no preferred cache format"
            )),
        }
    }

    pub const fn schema_version(self) -> u16 {
        match self {
            Self::OsaV1 => 1,
            Self::OsaV2 => 2,
        }
    }

    pub const fn builder_argument(self) -> &'static str {
        match self {
            Self::OsaV1 => "osa",
            Self::OsaV2 => "osa2",
        }
    }

    pub const fn reader_compatibility(self) -> &'static str {
        match self {
            Self::OsaV1 => "fastvep-osa-v1",
            Self::OsaV2 => "fastvep-osa-v2",
        }
    }

    pub const fn builder_contract(self) -> &'static str {
        match self {
            Self::OsaV1 => "annocat-sa-build-v1",
            Self::OsaV2 => "annocat-sa-build-v2",
        }
    }

    pub const fn data_file_name(self) -> &'static str {
        match self {
            Self::OsaV1 => "source.osa",
            Self::OsaV2 => "source.osa2",
        }
    }

    pub const fn index_file_name(self) -> Option<&'static str> {
        match self {
            Self::OsaV1 => Some("source.osa.idx"),
            Self::OsaV2 => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationIdentity {
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub chromosome: String,
    pub source_url: String,
    pub expected_compressed_bytes: u64,
    pub source_etag: Option<String>,
    pub source_last_modified: Option<String>,
    pub selected_schema: String,
    /// Legacy schema-v1 identity field. It is retained at its original value so
    /// existing checkpoints and source parts remain resumable. Exact builder
    /// provenance and compatibility live in cache-contract-v2.json.
    pub fastvep_commit: String,
    pub osa_schema_version: u16,
}

impl PreparationIdentity {
    pub fn cache_format(&self) -> Result<CacheFormat, String> {
        CacheFormat::from_schema_version(self.osa_schema_version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationCheckpoint {
    pub schema_version: u16,
    pub identity: PreparationIdentity,
    pub state: CheckpointState,
    pub compressed_bytes_read: u64,
    pub parsed_records: u64,
    pub prepared_bytes: u64,
    pub prepared_index_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_index_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointState {
    Preparing,
    Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    Start,
    RestartCurrentChromosome,
    AlreadyVerified,
    StaleIdentity,
}

#[derive(Debug, Clone)]
pub struct ShardPaths {
    pub partial_directory: PathBuf,
    pub final_directory: PathBuf,
    source_part: PathBuf,
    source_part_identity: PathBuf,
}

impl ShardPaths {
    pub fn new(resource_root: &Path, chromosome: &str) -> Result<Self, String> {
        let chromosome = super::safe_chromosome_component(chromosome)?;
        Ok(Self {
            partial_directory: resource_root
                .join("staging")
                .join(format!("{chromosome}.partial")),
            final_directory: resource_root.join("shards").join(&chromosome),
            source_part: resource_root
                .join("source-parts")
                .join(format!("{chromosome}.part")),
            source_part_identity: resource_root
                .join("source-parts")
                .join(format!("{chromosome}.identity.json")),
        })
    }

    pub fn partial_data(&self, format: CacheFormat) -> PathBuf {
        self.partial_directory.join(format.data_file_name())
    }

    pub fn partial_index(&self, format: CacheFormat) -> Option<PathBuf> {
        format
            .index_file_name()
            .map(|name| self.partial_directory.join(name))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn final_data(&self, format: CacheFormat) -> PathBuf {
        self.final_directory.join(format.data_file_name())
    }

    pub fn final_index(&self, format: CacheFormat) -> Option<PathBuf> {
        format
            .index_file_name()
            .map(|name| self.final_directory.join(name))
    }

    pub fn checkpoint(&self) -> PathBuf {
        self.partial_directory.join("checkpoint.json")
    }

    pub fn verification(&self) -> PathBuf {
        self.final_directory.join("verified.json")
    }

    pub fn cache_contract(&self) -> PathBuf {
        self.final_directory.join("cache-contract-v2.json")
    }

    pub(super) fn partial_cache_contract(&self) -> PathBuf {
        self.partial_directory.join("cache-contract-v2.json")
    }

    pub(super) fn source_part(&self) -> &Path {
        &self.source_part
    }

    pub(super) fn source_part_identity(&self) -> &Path {
        &self.source_part_identity
    }

    pub(super) fn source_part_variant(&self, tag: &str) -> Self {
        let mut paths = self.clone();
        let base = self
            .source_part
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source");
        let parent = self.source_part.parent().unwrap_or(Path::new("."));
        paths.source_part = parent.join(format!("{base}.{tag}.part"));
        paths.source_part_identity = parent.join(format!("{base}.{tag}.identity.json"));
        paths
    }
}

pub(super) fn write_checkpoint(
    path: &Path,
    checkpoint: &PreparationCheckpoint,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(checkpoint).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub(super) fn read_checkpoint(path: &Path) -> Result<PreparationCheckpoint, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let checkpoint: PreparationCheckpoint =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if checkpoint.schema_version != CHECKPOINT_SCHEMA_VERSION {
        return Err("unsupported preparation checkpoint schema".into());
    }
    Ok(checkpoint)
}
