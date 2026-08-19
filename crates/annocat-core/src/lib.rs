pub mod normalization;
pub mod sample_call;
pub mod source_catalog;
pub mod source_overrides;
pub mod vcf;

pub const RESULT_SCHEMA_VERSION: i32 = 2;

#[derive(Debug, Clone, Copy)]
pub struct ResourceRelease {
    pub resource_id: &'static str,
    pub version: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub download_bytes: Option<u64>,
    pub range_resume: bool,
    pub size_checked_at: &'static str,
    pub archive_format: &'static str,
    pub publisher_md5: Option<&'static str>,
    pub publisher_sha256: Option<&'static str>,
}

pub fn practical_resource_plan_json() -> String {
    let catalog = source_catalog::catalog();
    let mut rows = catalog
        .resources
        .iter()
        .filter(|resource| resource.user_visible)
        .map(|resource| {
            let rolling = resource.release.policy == "rolling";
            serde_json::json!({
                "id": resource.id,
                "version": if rolling { None } else { Some(resource.release.version.as_str()) },
                "filename": resource.release.filename,
                "downloadBytes": if rolling { None } else { Some(resource.release.download_bytes) },
                "installedBytes": null,
                "rangeResume": resource.release.range_resume,
                "installMode": if matches!(resource.delivery.as_str(), "stream-cache" | "knowledge-cache") { "stream" } else { "download" },
                "state": "missing",
                "sizeCheckedAt": if rolling { None } else { Some(resource.release.size_checked_at.as_str()) },
            })
        })
        .collect::<Vec<_>>();
    rows.extend(
        catalog
            .sources
            .iter()
            .filter(|source| source.delivery != "bundled-engine")
            .filter(|source| {
                !catalog
                    .resources
                    .iter()
                    .any(|resource| resource.id == source.id)
            })
            .map(|source| {
                serde_json::json!({
                    "id": source.id,
                    "version": null,
                    "filename": null,
                    "downloadBytes": null,
                    "installedBytes": null,
                    "rangeResume": null,
                    "state": "catalog-pending",
                })
            }),
    );
    serde_json::json!({
        "profile": "practical-wgs",
        "assembly": "GRCh38",
        "resources": rows,
    })
    .to_string()
}

pub fn sources_json() -> String {
    source_catalog::sources_json()
}

pub fn profiles_json() -> String {
    source_catalog::profiles_json()
}

pub fn evidence_calibrations_json() -> &'static str {
    source_catalog::evidence_calibrations_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ids_are_unique() {
        let sources = &source_catalog::catalog().sources;
        for (index, source) in sources.iter().enumerate() {
            assert!(!sources[..index].iter().any(|other| other.id == source.id));
        }
    }

    #[test]
    fn every_source_has_one_explicit_implementation_policy() {
        for restricted in ["omim", "cosmic"] {
            assert_eq!(
                source_catalog::source(restricted).unwrap().delivery,
                "user-supplied-licensed"
            );
        }
    }

    #[test]
    fn profiles_reference_known_unique_sources() {
        for profile in &source_catalog::catalog().profiles {
            let source_ids = profile
                .source_ids
                .iter()
                .chain(profile.knowledge_source_ids.iter())
                .collect::<Vec<_>>();
            for (index, source_id) in source_ids.iter().enumerate() {
                assert!(
                    source_catalog::source(source_id).is_some(),
                    "profile {} references unknown source {}",
                    profile.id,
                    source_id
                );
                assert!(
                    !source_ids[..index].contains(source_id),
                    "profile {} repeats source {}",
                    profile.id,
                    source_id
                );
            }
        }
    }

    #[test]
    fn comprehensive_profile_has_requested_genome_wide_sources() {
        let profile = source_catalog::profile("wgs").unwrap();
        for source_id in ["dbsnp", "cadd", "phylop", "gnomad-genomes", "spliceai"] {
            assert!(profile.source_ids.iter().any(|id| id == source_id));
        }
        assert!(!profile.source_ids.iter().any(|id| id == "gnomad"));
        assert!(!profile.source_ids.iter().any(|id| id == "revel"));
        assert!(!profile.source_ids.iter().any(|id| id == "fastvep"));
    }

    #[test]
    fn pending_predictors_remain_outside_recommended_profiles() {
        for source_id in ["gerp", "primateai", "dann"] {
            let source = source_catalog::source(source_id).unwrap();
            assert!(!source.default_enabled);
            assert!(
                source_catalog::catalog()
                    .profiles
                    .iter()
                    .all(|profile| !profile.source_ids.iter().any(|id| id == source_id))
            );
        }
        let minimal = source_catalog::profile("standard").unwrap();
        assert_eq!(
            minimal.source_ids,
            ["clinvar", "dbsnp", "gnomad", "phylop", "revel"]
        );
        assert!(!minimal.source_ids.iter().any(|id| id == "dbnsfp"));
    }

    #[test]
    fn dbnsfp_release_has_verified_range_size() {
        let release = source_catalog::download_release("dbnsfp").unwrap();
        assert_eq!(release.download_bytes, Some(38_969_753_349));
        assert!(release.range_resume);
    }

    #[test]
    fn rolling_resources_do_not_publish_stale_plan_versions_or_sizes() {
        let plan: serde_json::Value =
            serde_json::from_str(&practical_resource_plan_json()).unwrap();
        let resources = plan["resources"].as_array().unwrap();
        for resource_id in ["clinvar", "dbsnp", "hpo"] {
            let resource = resources
                .iter()
                .find(|resource| resource["id"] == resource_id)
                .unwrap();
            assert!(resource["version"].is_null());
            assert!(resource["downloadBytes"].is_null());
            assert!(resource["sizeCheckedAt"].is_null());
        }
    }

    #[test]
    fn dbsnp_release_is_actionable_through_the_native_parser() {
        let release = source_catalog::download_release("dbsnp").unwrap();
        assert_eq!(release.version, "b157-GRCh38.p14");
        assert_eq!(release.download_bytes, Some(29_552_227_779));
        assert_eq!(
            release.publisher_md5,
            Some("6a6f313e92a39c337571174dad12cfe1")
        );
        assert_eq!(
            source_catalog::source("dbsnp")
                .unwrap()
                .fastvep_source
                .as_deref(),
            Some("dbsnp")
        );
    }

    #[test]
    fn unfinished_sources_remain_adapter_gated() {
        for source_id in ["alphamissense", "clingen", "gencc"] {
            assert_eq!(
                source_catalog::source(source_id).unwrap().delivery,
                "adapter-required"
            );
        }
    }
}
