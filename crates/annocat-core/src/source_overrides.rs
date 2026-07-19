use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::source_catalog;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceOverrides {
    pub schema_version: u16,
    pub overrides: Vec<SourceOverride>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceOverride {
    pub source_id: String,
    pub artifact_id: String,
    pub mirrors: Vec<MirrorOverride>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MirrorOverride {
    pub url: String,
    pub expected_bytes: u64,
    pub checksum: Option<source_catalog::Checksum>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMirror<'a> {
    pub url: &'a str,
    pub resume_compatible: bool,
}

impl SourceOverrides {
    pub fn from_json(contents: &str) -> Result<Self, String> {
        let overrides: Self = serde_json::from_str(contents)
            .map_err(|error| format!("invalid source override file: {error}"))?;
        overrides.validate()?;
        Ok(overrides)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported source override schema {}",
                self.schema_version
            ));
        }
        let mut overridden_sources = HashSet::new();
        for source_override in &self.overrides {
            if !overridden_sources.insert(source_override.source_id.as_str()) {
                return Err(format!(
                    "source {} has more than one override entry",
                    source_override.source_id
                ));
            }
            let resource =
                source_catalog::resource(&source_override.source_id).ok_or_else(|| {
                    format!(
                        "source override references unknown managed source {}",
                        source_override.source_id
                    )
                })?;
            if source_override.artifact_id != resource.release.artifact_id {
                return Err(format!(
                    "source {} override targets artifact {}, but the catalog requires {}",
                    source_override.source_id,
                    source_override.artifact_id,
                    resource.release.artifact_id
                ));
            }
            if source_override.mirrors.is_empty() {
                return Err(format!(
                    "source {} override has no mirror URLs",
                    source_override.source_id
                ));
            }
            let mut urls = HashSet::new();
            for mirror in &source_override.mirrors {
                if !safe_https_url(&mirror.url) || !urls.insert(mirror.url.as_str()) {
                    return Err(format!(
                        "source {} has an unsafe or duplicate mirror URL",
                        source_override.source_id
                    ));
                }
                if mirror.expected_bytes != resource.release.download_bytes {
                    return Err(format!(
                        "source {} mirror byte identity differs from catalog artifact {}",
                        source_override.source_id, resource.release.artifact_id
                    ));
                }
                match (&resource.release.checksum, &mirror.checksum) {
                    (Some(catalog), Some(candidate))
                        if catalog.algorithm == candidate.algorithm
                            && catalog.value.eq_ignore_ascii_case(&candidate.value) => {}
                    (Some(_), _) => {
                        return Err(format!(
                            "source {} mirror checksum does not match the catalog artifact",
                            source_override.source_id
                        ));
                    }
                    (None, None) => {}
                    (None, Some(_)) => {
                        return Err(format!(
                            "source {} mirror cannot introduce an untrusted checksum identity",
                            source_override.source_id
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn mirrors_for(&self, source_id: &str) -> Vec<ValidatedMirror<'_>> {
        let Some(resource) = source_catalog::resource(source_id) else {
            return Vec::new();
        };
        self.overrides
            .iter()
            .find(|source_override| source_override.source_id == source_id)
            .map(|source_override| {
                source_override
                    .mirrors
                    .iter()
                    .map(|mirror| ValidatedMirror {
                        url: mirror.url.as_str(),
                        // Matching length alone cannot prove that a retained byte prefix
                        // belongs to the same object. Cross-mirror range resume is enabled
                        // only when the shipped catalog already supplies a matching checksum.
                        resume_compatible: resource
                            .release
                            .checksum
                            .as_ref()
                            .is_some_and(|checksum| checksum.algorithm == "sha256"),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn safe_https_url(value: &str) -> bool {
    let Some(authority_and_path) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = authority_and_path.split('/').next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !authority.chars().any(char::is_whitespace)
        && !value.contains(['\r', '\n', '#'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dbnsfp_override(url: &str, checksum: &str) -> String {
        format!(
            r#"{{
                "schemaVersion": 1,
                "overrides": [{{
                    "sourceId": "dbnsfp",
                    "artifactId": "dbnsfp-official-variant-archive:4.9a",
                    "mirrors": [{{
                        "url": "{url}",
                        "expectedBytes": 38969753349,
                        "checksum": {{"algorithm": "md5", "value": "{checksum}"}}
                    }}]
                }}]
            }}"#
        )
    }

    #[test]
    fn matching_sha256_artifact_mirror_can_resume_across_hosts() {
        let overrides = SourceOverrides::from_json(
            r#"{
                "schemaVersion": 1,
                "overrides": [{
                    "sourceId": "ensembl-gff3",
                    "artifactId": "ensembl-homo-sapiens-grch38-gff3:115",
                    "mirrors": [{
                        "url": "https://mirror.example.org/Homo_sapiens.GRCh38.115.gff3.gz",
                        "expectedBytes": 83835106,
                        "checksum": {
                            "algorithm": "sha256",
                            "value": "1e553efa8496d662e7264061a5cecf3001eb9a1157aaa66d80cd7ac35841509c"
                        }
                    }]
                }]
            }"#,
        )
        .unwrap();
        assert!(overrides.mirrors_for("ensembl-gff3")[0].resume_compatible);
    }

    #[test]
    fn matching_md5_artifact_mirror_requires_a_fresh_stream() {
        let overrides = SourceOverrides::from_json(&dbnsfp_override(
            "https://mirror.example.org/dbNSFP4.9a.zip",
            "be89346ab3dc5c14a8a7b602f50c66fb",
        ))
        .unwrap();
        assert_eq!(
            overrides.mirrors_for("dbnsfp"),
            [ValidatedMirror {
                url: "https://mirror.example.org/dbNSFP4.9a.zip",
                resume_compatible: false,
            }]
        );
    }

    #[test]
    fn wrong_artifact_size_or_checksum_is_rejected() {
        let wrong_checksum = SourceOverrides::from_json(&dbnsfp_override(
            "https://mirror.example.org/dbNSFP4.9a.zip",
            "00000000000000000000000000000000",
        ));
        assert!(wrong_checksum.is_err());

        let wrong_size = dbnsfp_override(
            "https://mirror.example.org/dbNSFP4.9a.zip",
            "be89346ab3dc5c14a8a7b602f50c66fb",
        )
        .replace("38969753349", "38969753348");
        assert!(SourceOverrides::from_json(&wrong_size).is_err());
    }

    #[test]
    fn unchecked_or_credential_bearing_urls_are_rejected() {
        for url in [
            "http://mirror.example.org/dbNSFP4.9a.zip",
            "https://user:secret@mirror.example.org/dbNSFP4.9a.zip",
            "https://mirror.example.org/dbNSFP4.9a.zip#fragment",
        ] {
            assert!(
                SourceOverrides::from_json(&dbnsfp_override(
                    url,
                    "be89346ab3dc5c14a8a7b602f50c66fb"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn unchecksummed_sources_never_claim_cross_mirror_resume_safety() {
        let overrides = SourceOverrides::from_json(
            r#"{
                "schemaVersion": 1,
                "overrides": [{
                    "sourceId": "spliceai",
                    "artifactId": "ensembl-spliceai-mane-v1.4-masked-snv:GRCh38",
                    "mirrors": [{
                        "url": "https://mirror.example.org/spliceai.vcf.gz",
                        "expectedBytes": 28643031420,
                        "checksum": null
                    }]
                }]
            }"#,
        )
        .unwrap();
        assert!(!overrides.mirrors_for("spliceai")[0].resume_compatible);
    }
}
