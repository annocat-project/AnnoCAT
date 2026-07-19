use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;

const CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/source-catalog.json"
));

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCatalog {
    schema_version: u16,
    pub sources: Vec<Source>,
    pub profiles: Vec<Profile>,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub default_enabled: bool,
    pub fastvep_source: Option<String>,
    pub delivery: String,
    pub assembly: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub assembly: String,
    pub delivery: String,
    pub adapter_contract: Option<String>,
    pub manifest_ref: Option<String>,
    pub release: Release,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub policy: String,
    pub artifact_id: String,
    pub primary_url: String,
    pub filename: String,
    pub download_bytes: u64,
    pub archive_format: String,
    pub range_resume: bool,
    pub size_checked_at: String,
    pub checksum: Option<Checksum>,
    pub resolver: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checksum {
    pub algorithm: String,
    pub value: String,
}

static CATALOG: LazyLock<SourceCatalog> = LazyLock::new(|| {
    let catalog: SourceCatalog =
        serde_json::from_str(CATALOG_JSON).expect("config/source-catalog.json must be valid JSON");
    validate(&catalog).expect("config/source-catalog.json must satisfy the catalog contract");
    catalog
});

pub fn catalog() -> &'static SourceCatalog {
    &CATALOG
}

pub fn resource(id: &str) -> Option<&'static Resource> {
    catalog()
        .resources
        .iter()
        .find(|resource| resource.id == id)
}

pub fn source(id: &str) -> Option<&'static Source> {
    catalog().sources.iter().find(|source| source.id == id)
}

pub fn sources_json() -> String {
    serde_json::to_string(&catalog().sources).expect("validated sources must serialize")
}

pub fn profile(id: &str) -> Option<&'static Profile> {
    catalog().profiles.iter().find(|profile| profile.id == id)
}

pub fn profiles_json() -> String {
    serde_json::to_string(
        &catalog()
            .profiles
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "id": profile.id,
                    "name": profile.name,
                    "purpose": profile.purpose,
                    "sourceIds": profile.source_ids,
                    "requiredEngineIds": ["fastvep"],
                    "requiredResourceIds": ["grch38-reference", "transcript-cache"]
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("validated source profiles must serialize")
}

pub fn adapter_contract(id: &str) -> Option<&'static str> {
    resource(id)?.adapter_contract.as_deref()
}

pub fn download_release(id: &str) -> Option<crate::ResourceRelease> {
    let resource = resource(id)?;
    let release = &resource.release;
    Some(crate::ResourceRelease {
        resource_id: resource.id.as_str(),
        version: release.version.as_str(),
        filename: release.filename.as_str(),
        url: release.primary_url.as_str(),
        download_bytes: Some(release.download_bytes),
        installed_bytes: None,
        range_resume: release.range_resume,
        size_checked_at: release.size_checked_at.as_str(),
        archive_format: release.archive_format.as_str(),
        publisher_md5: release
            .checksum
            .as_ref()
            .filter(|checksum| checksum.algorithm == "md5")
            .map(|checksum| checksum.value.as_str()),
        publisher_sha256: release
            .checksum
            .as_ref()
            .filter(|checksum| checksum.algorithm == "sha256")
            .map(|checksum| checksum.value.as_str()),
    })
}

pub fn download_releases() -> impl Iterator<Item = crate::ResourceRelease> {
    catalog()
        .resources
        .iter()
        .filter_map(|resource| download_release(&resource.id))
}

pub fn artifact_identity(
    resource_id: &str,
    release: &str,
    assembly: &str,
    chromosome: &str,
) -> String {
    resource(resource_id)
        .filter(|resource| resource.release.version == release && resource.assembly == assembly)
        .map(|resource| format!("{}:{chromosome}", resource.release.artifact_id))
        .unwrap_or_else(|| format!("{resource_id}:{release}:{assembly}:{chromosome}"))
}

fn validate(catalog: &SourceCatalog) -> Result<(), String> {
    if catalog.schema_version != 1 {
        return Err(format!(
            "unsupported source catalog schema {}",
            catalog.schema_version
        ));
    }
    let mut catalog_source_ids = HashSet::new();
    for source in &catalog.sources {
        if !safe_id(&source.id) || !catalog_source_ids.insert(source.id.as_str()) {
            return Err(format!("invalid or duplicate source id {}", source.id));
        }
        if source.name.trim().is_empty()
            || source.purpose.trim().is_empty()
            || source.assembly.trim().is_empty()
            || !matches!(
                source.delivery.as_str(),
                "bundled-engine"
                    | "managed-public"
                    | "managed-public-noncommercial"
                    | "adapter-required"
                    | "catalog-pending"
                    | "user-supplied-licensed"
            )
            || source
                .fastvep_source
                .as_deref()
                .is_some_and(|id| !safe_id(id))
        {
            return Err(format!("source {} has invalid metadata", source.id));
        }
    }
    let mut ids = HashSet::new();
    let mut artifacts = HashSet::new();
    for resource in &catalog.resources {
        if !safe_id(&resource.id) || !ids.insert(resource.id.as_str()) {
            return Err(format!("invalid or duplicate resource id {}", resource.id));
        }
        if resource.assembly.trim().is_empty()
            || resource.release.version.trim().is_empty()
            || resource.release.download_bytes == 0
        {
            return Err(format!(
                "resource {} has incomplete release metadata",
                resource.id
            ));
        }
        if resource.delivery == "stream-cache" && resource.adapter_contract.is_none() {
            return Err(format!("resource {} has no adapter contract", resource.id));
        }
        if !safe_id(&resource.release.artifact_id.replace(':', "-"))
            || !artifacts.insert(resource.release.artifact_id.as_str())
        {
            return Err(format!(
                "invalid or duplicate artifact id {}",
                resource.release.artifact_id
            ));
        }
        if !resource.release.primary_url.starts_with("https://")
            || resource.release.filename.contains(['/', '\\'])
            || resource.release.archive_format.trim().is_empty()
            || resource.release.size_checked_at.trim().is_empty()
        {
            return Err(format!(
                "resource {} has unsafe release metadata",
                resource.id
            ));
        }
        if resource.release.range_resume && resource.release.archive_format.ends_with("-shards") {
            return Err(format!(
                "resource {} cannot range-resume an independently sharded release",
                resource.id
            ));
        }
        match resource.release.policy.as_str() {
            "pinned" if resource.release.resolver.is_none() => {}
            "rolling" if resource.release.resolver.as_deref().is_some_and(safe_id) => {}
            _ => {
                return Err(format!(
                    "resource {} has an invalid release policy",
                    resource.id
                ));
            }
        }
        if let Some(path) = resource.manifest_ref.as_deref()
            && (!path.starts_with("config/")
                || path.contains("..")
                || path.contains('\\')
                || !path.ends_with(".json")
                || !embedded_manifest_exists(path))
        {
            return Err(format!(
                "resource {} has an unsafe manifest path",
                resource.id
            ));
        }
        if let Some(checksum) = &resource.release.checksum {
            let expected_length = match checksum.algorithm.as_str() {
                "md5" => 32,
                "sha256" => 64,
                _ => return Err(format!("resource {} has an unknown checksum", resource.id)),
            };
            if checksum.value.len() != expected_length
                || !checksum.value.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("resource {} has an invalid checksum", resource.id));
            }
        }
        if !matches!(resource.id.as_str(), "grch38-reference" | "ensembl-gff3")
            && !catalog_source_ids.contains(resource.id.as_str())
        {
            return Err(format!(
                "resource {} has no matching annotation source",
                resource.id
            ));
        }
    }
    let mut profile_ids = HashSet::new();
    for profile in &catalog.profiles {
        if !safe_id(&profile.id)
            || !profile_ids.insert(profile.id.as_str())
            || profile.name.trim().is_empty()
            || profile.purpose.trim().is_empty()
            || profile.source_ids.is_empty()
        {
            return Err(format!("invalid or duplicate profile {}", profile.id));
        }
        let mut profile_source_ids = HashSet::new();
        for source_id in &profile.source_ids {
            if !catalog_source_ids.contains(source_id.as_str())
                || !ids.contains(source_id.as_str())
                || !profile_source_ids.insert(source_id.as_str())
            {
                return Err(format!(
                    "profile {} references an unknown or duplicate source {}",
                    profile.id, source_id
                ));
            }
        }
    }
    Ok(())
}

fn embedded_manifest_exists(path: &str) -> bool {
    matches!(
        path,
        "config/dbnsfp-4.9a-members.json"
            | "config/revel-1.3-archives.json"
            | "config/wgs-streams.json"
    )
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_every_legacy_source_and_implementation_policy() {
        assert_eq!(catalog().sources.len(), crate::SOURCES.len());
        assert_eq!(catalog().sources.len(), crate::SOURCE_IMPLEMENTATIONS.len());
        for legacy in crate::SOURCES {
            let current = source(legacy.id).expect("catalog source");
            let implementation = crate::source_implementation(legacy.id)
                .expect("legacy source implementation policy");
            assert_eq!(current.name, legacy.name);
            assert_eq!(current.purpose, legacy.purpose);
            assert_eq!(current.default_enabled, legacy.default_enabled);
            assert_eq!(
                current.fastvep_source.as_deref(),
                implementation.fastvep_source
            );
            assert_eq!(current.delivery, implementation.delivery);
            assert_eq!(current.assembly, implementation.assembly);
        }
        let json: serde_json::Value = serde_json::from_str(&sources_json()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), crate::SOURCES.len());
        assert_eq!(json[0]["id"], "fastvep");
        assert_eq!(json[0]["defaultEnabled"], true);
    }

    #[test]
    fn catalog_matches_every_actionable_legacy_release() {
        assert_eq!(catalog().resources.len(), crate::RESOURCE_RELEASES.len());
        for legacy in crate::RESOURCE_RELEASES {
            let current = resource(legacy.resource_id).expect("catalog resource");
            let projected = download_release(legacy.resource_id).expect("download release");
            assert_eq!(projected.resource_id, legacy.resource_id);
            assert_eq!(projected.version, legacy.version);
            assert_eq!(projected.url, legacy.url);
            assert_eq!(current.release.version, legacy.version);
            assert_eq!(current.release.primary_url, legacy.url);
            assert_eq!(current.release.filename, legacy.filename);
            assert_eq!(
                current.release.download_bytes,
                legacy.download_bytes.unwrap()
            );
            assert_eq!(current.release.archive_format, legacy.archive_format);
            assert_eq!(current.release.range_resume, legacy.range_resume);
            assert_eq!(current.release.size_checked_at, legacy.size_checked_at);
            assert_eq!(
                current
                    .release
                    .checksum
                    .as_ref()
                    .filter(|checksum| checksum.algorithm == "md5")
                    .map(|checksum| checksum.value.as_str()),
                legacy.publisher_md5
            );
            assert_eq!(
                current
                    .release
                    .checksum
                    .as_ref()
                    .filter(|checksum| checksum.algorithm == "sha256")
                    .map(|checksum| checksum.value.as_str()),
                legacy.publisher_sha256
            );
        }
    }

    #[test]
    fn every_streamed_resource_has_a_stable_adapter_and_artifact_identity() {
        for resource in &catalog().resources {
            if resource.delivery == "stream-cache" {
                assert!(resource.adapter_contract.is_some());
                assert!(!resource.release.artifact_id.is_empty());
            }
        }
    }

    #[test]
    fn rolling_sources_require_named_resolvers() {
        assert_eq!(resource("clinvar").unwrap().release.policy, "rolling");
        assert_eq!(resource("dbsnp").unwrap().release.policy, "rolling");
        assert!(catalog().resources.iter().all(|resource| {
            resource.release.policy != "rolling" || resource.release.resolver.is_some()
        }));
    }

    #[test]
    fn profiles_match_the_legacy_api_contract() {
        assert_eq!(catalog().profiles.len(), crate::ANNOTATION_PROFILES.len());
        for legacy in crate::ANNOTATION_PROFILES {
            let current = profile(legacy.id).expect("catalog profile");
            assert_eq!(current.name, legacy.name);
            assert_eq!(current.purpose, legacy.purpose);
            assert_eq!(
                current
                    .source_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                legacy.source_ids
            );
        }
        let json: serde_json::Value = serde_json::from_str(&profiles_json()).unwrap();
        assert_eq!(json[0]["id"], "wgs");
        assert_eq!(json[1]["id"], "standard");
    }

    #[test]
    fn catalog_artifact_identity_is_stable_across_mirror_urls() {
        assert_eq!(
            artifact_identity("revel", "1.3", "GRCh38", "22"),
            "zenodo:7072866:revel-v1.3-chromosome-archives:22"
        );
        assert_eq!(
            artifact_identity("clinvar", "20260715", "GRCh38", "22"),
            "clinvar:20260715:GRCh38:22"
        );
    }
}
