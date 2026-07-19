use serde::Deserialize;
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
    pub resources: Vec<Resource>,
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
