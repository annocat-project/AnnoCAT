pub mod normalization;
pub mod source_catalog;
pub mod source_overrides;
pub mod vcf;

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
        .map(|resource| {
            serde_json::json!({
                "id": resource.id,
                "version": resource.release.version,
                "filename": resource.release.filename,
                "downloadBytes": resource.release.download_bytes,
                "installedBytes": null,
                "rangeResume": resource.release.range_resume,
                "installMode": if resource.delivery == "stream-cache" { "stream" } else { "download" },
                "state": "missing",
                "sizeCheckedAt": resource.release.size_checked_at,
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

#[derive(Debug, Clone)]
pub struct DemoVariant {
    pub chromosome: &'static str,
    pub position: u64,
    pub reference: &'static str,
    pub alternate: &'static str,
    pub gene: &'static str,
    pub consequence: &'static str,
    pub impact: &'static str,
    pub clinvar: &'static str,
    pub inheritance: &'static str,
    pub score: f32,
}

pub const DEMO_VARIANTS: &[DemoVariant] = &[
    DemoVariant {
        chromosome: "1",
        position: 101_001,
        reference: "G",
        alternate: "A",
        gene: "DEMO1",
        consequence: "missense_variant",
        impact: "MODERATE",
        clinvar: "Uncertain significance",
        inheritance: "Autosomal dominant",
        score: 0.82,
    },
    DemoVariant {
        chromosome: "2",
        position: 202_002,
        reference: "C",
        alternate: "T",
        gene: "DEMO2",
        consequence: "stop_gained",
        impact: "HIGH",
        clinvar: "Pathogenic",
        inheritance: "Autosomal recessive",
        score: 0.98,
    },
    DemoVariant {
        chromosome: "X",
        position: 303_003,
        reference: "A",
        alternate: "AT",
        gene: "DEMO3",
        consequence: "frameshift_variant",
        impact: "HIGH",
        clinvar: "Likely pathogenic",
        inheritance: "X-linked",
        score: 0.94,
    },
];

pub fn sources_json() -> String {
    source_catalog::sources_json()
}

pub fn profiles_json() -> String {
    source_catalog::profiles_json()
}

pub fn demo_variants_json() -> String {
    let rows = DEMO_VARIANTS.iter().map(|v| format!(
        "{{\"chromosome\":\"{}\",\"position\":{},\"reference\":\"{}\",\"alternate\":\"{}\",\"gene\":\"{}\",\"consequence\":\"{}\",\"impact\":\"{}\",\"clinvar\":\"{}\",\"inheritance\":\"{}\",\"score\":{:.2}}}",
        v.chromosome, v.position, v.reference, v.alternate, v.gene, v.consequence, v.impact, v.clinvar, v.inheritance, v.score
    )).collect::<Vec<_>>().join(",");
    format!("[{}]", rows)
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
            for (index, source_id) in profile.source_ids.iter().enumerate() {
                assert!(
                    source_catalog::source(source_id).is_some(),
                    "profile {} references unknown source {}",
                    profile.id,
                    source_id
                );
                assert!(
                    !profile.source_ids[..index].contains(source_id),
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
        for source_id in ["dbsnp", "cadd", "phylop", "gnomad", "spliceai"] {
            assert!(profile.source_ids.iter().any(|id| id == source_id));
        }
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
    fn demo_data_is_explicitly_synthetic() {
        assert!(
            DEMO_VARIANTS
                .iter()
                .all(|variant| variant.gene.starts_with("DEMO"))
        );
    }

    #[test]
    fn dbnsfp_release_has_verified_range_size() {
        let release = source_catalog::download_release("dbnsfp").unwrap();
        assert_eq!(release.download_bytes, Some(38_969_753_349));
        assert!(release.range_resume);
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
