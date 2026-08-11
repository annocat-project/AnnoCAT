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
    evidence_calibration_ref: String,
    pub sources: Vec<Source>,
    pub services: Vec<Service>,
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
    pub evidence_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_identity: Option<String>,
    pub delivery: String,
    pub assembly: String,
    pub license_policy: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Service {
    pub id: String,
    pub provider: String,
    pub purpose: String,
    pub api_url: String,
    #[serde(default)]
    pub coding_api_url: Option<String>,
    pub provider_url: String,
    pub timeout_seconds: u64,
    pub max_results: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub knowledge_source_ids: Vec<String>,
    #[serde(default)]
    pub service_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    #[serde(default = "default_true")]
    pub user_visible: bool,
    pub assembly: String,
    pub delivery: String,
    #[serde(default)]
    pub preferred_cache_format: Option<String>,
    pub adapter_contract: Option<String>,
    pub manifest_ref: Option<String>,
    pub manifest_role: Option<String>,
    pub field_contract: Option<FieldContract>,
    pub release: Release,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldContract {
    pub manifest_ref: String,
    pub contract_id: String,
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
    pub resolver_api_url: Option<String>,
    pub resolver_directory_url: Option<String>,
    pub resolver_notes_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
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

pub fn feature_identity(id: &str) -> Option<&'static str> {
    source(id)?.feature_identity.as_deref()
}

pub fn service(id: &str) -> Option<&'static Service> {
    catalog().services.iter().find(|service| service.id == id)
}

pub fn sources_json() -> String {
    serde_json::to_string(&catalog().sources).expect("validated sources must serialize")
}

pub fn evidence_calibrations_json() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/evidence-calibrations.json"
    ))
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
                    "knowledgeSourceIds": profile.knowledge_source_ids,
                    "serviceIds": profile.service_ids,
                    "requiredEngineIds": ["fastvep"],
                    "requiredResourceIds": ["grch38-reference", "ensembl-gff3"]
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("validated source profiles must serialize")
}

pub fn adapter_contract(id: &str) -> Option<&'static str> {
    resource(id)?.adapter_contract.as_deref()
}

pub fn preferred_cache_format(id: &str) -> Option<&'static str> {
    resource(id)?.preferred_cache_format.as_deref()
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
        .filter(|resource| resource.user_visible)
        .filter_map(|resource| download_release(&resource.id))
}

pub fn resolver_directory_url(id: &str) -> Option<&'static str> {
    resource(id)?.release.resolver_directory_url.as_deref()
}

pub fn resolver_api_url(id: &str) -> Option<&'static str> {
    resource(id)?.release.resolver_api_url.as_deref()
}

pub fn resolver_notes_url(id: &str) -> Option<&'static str> {
    resource(id)?.release.resolver_notes_url.as_deref()
}

pub fn resource_manifest_json(id: &str) -> Result<&'static str, String> {
    let resource = resource(id).ok_or_else(|| format!("unknown resource '{id}'"))?;
    let path = resource
        .manifest_ref
        .as_deref()
        .ok_or_else(|| format!("resource '{id}' has no asset manifest"))?;
    embedded_manifest(path)
        .ok_or_else(|| format!("resource '{id}' references unavailable manifest '{path}'"))
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
    if catalog.evidence_calibration_ref != "config/evidence-calibrations.json" {
        return Err("source catalog must reference config/evidence-calibrations.json".into());
    }
    validate_evidence_calibrations()?;
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
            || !matches!(
                source.license_policy.as_str(),
                "bundled-open-source"
                    | "publisher-terms"
                    | "noncommercial-restricted"
                    | "user-supplied-license"
                    | "pending-review"
            )
            || source
                .fastvep_source
                .as_deref()
                .is_some_and(|id| !safe_id(id))
            || !matches!(
                source.evidence_scope.as_str(),
                "allele" | "transcript" | "feature" | "gene"
            )
            || source.feature_identity.as_deref().is_some_and(|identity| {
                source.evidence_scope != "feature"
                    || !matches!(identity, "gene" | "selectedFeature")
            })
        {
            return Err(format!("source {} has invalid metadata", source.id));
        }
    }
    let mut service_ids = HashSet::new();
    for service in &catalog.services {
        if !safe_id(&service.id)
            || !service_ids.insert(service.id.as_str())
            || service.provider.trim().is_empty()
            || service.purpose.trim().is_empty()
            || !service.api_url.starts_with("https://")
            || service
                .coding_api_url
                .as_deref()
                .is_some_and(|url| !url.starts_with("https://"))
            || !service.provider_url.starts_with("https://")
            || !(1..=120).contains(&service.timeout_seconds)
            || !(1..=10_000).contains(&service.max_results)
        {
            return Err(format!("service {} has invalid metadata", service.id));
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
        match (
            resource.delivery.as_str(),
            resource.preferred_cache_format.as_deref(),
        ) {
            ("stream-cache", Some("osa" | "osa2")) | (_, None) => {}
            ("stream-cache", _) => {
                return Err(format!(
                    "resource {} has no supported preferred cache format",
                    resource.id
                ));
            }
            _ => {
                return Err(format!(
                    "resource {} declares a cache format without using stream-cache delivery",
                    resource.id
                ));
            }
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
        for url in [
            resource.release.resolver_api_url.as_deref(),
            resource.release.resolver_directory_url.as_deref(),
            resource.release.resolver_notes_url.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !url.starts_with("https://") {
                return Err(format!(
                    "resource {} has an unsafe resolver URL",
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
        if let Some(field_contract) = &resource.field_contract
            && (!safe_id(&field_contract.contract_id)
                || !embedded_field_contract_exists(
                    &field_contract.manifest_ref,
                    &resource.id,
                    &field_contract.contract_id,
                ))
        {
            return Err(format!(
                "resource {} has an invalid retained-field contract",
                resource.id
            ));
        }
        if resource.delivery == "stream-cache" && resource.field_contract.is_none() {
            return Err(format!(
                "resource {} has no retained-field contract",
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
        if resource.user_visible
            && !matches!(resource.id.as_str(), "grch38-reference" | "ensembl-gff3")
            && !catalog_source_ids.contains(resource.id.as_str())
        {
            return Err(format!(
                "resource {} has no matching annotation source",
                resource.id
            ));
        }
    }
    if let Some(hpo) = catalog
        .resources
        .iter()
        .find(|resource| resource.id == "hpo")
    {
        validate_hpo_manifest_contract(hpo)?;
    }
    if let Some(mondo) = catalog
        .resources
        .iter()
        .find(|resource| resource.id == "mondo")
    {
        validate_mondo_manifest_contract(mondo)?;
    }
    for resource_id in ["cadd", "spliceai"] {
        let resource = catalog
            .resources
            .iter()
            .find(|resource| resource.id == resource_id)
            .ok_or_else(|| format!("catalog is missing resource {resource_id}"))?;
        if resource.manifest_ref.as_deref() != Some("config/indexed-sources.json") {
            return Err(format!(
                "resource {resource_id} must reference config/indexed-sources.json"
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
                && profile.knowledge_source_ids.is_empty()
                && profile.service_ids.is_empty()
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
        for source_id in &profile.knowledge_source_ids {
            if !catalog_source_ids.contains(source_id.as_str())
                || !ids.contains(source_id.as_str())
                || !profile_source_ids.insert(source_id.as_str())
            {
                return Err(format!(
                    "profile {} references an unknown or duplicate knowledge source {}",
                    profile.id, source_id
                ));
            }
        }
        let mut profile_service_ids = HashSet::new();
        for service_id in &profile.service_ids {
            if !service_ids.contains(service_id.as_str())
                || !profile_service_ids.insert(service_id.as_str())
            {
                return Err(format!(
                    "profile {} references an unknown or duplicate service {}",
                    profile.id, service_id
                ));
            }
        }
    }
    Ok(())
}

fn validate_evidence_calibrations() -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(evidence_calibrations_json())
        .map_err(|error| format!("invalid evidence calibration manifest: {error}"))?;
    if value["schemaVersion"] != 2 {
        return Err("unsupported evidence calibration schema".into());
    }
    let policy = value["interpretationPolicy"]
        .as_object()
        .ok_or("evidence calibration manifest has no interpretation policy")?;
    if policy.get("mode").and_then(serde_json::Value::as_str) != Some("display-only")
        || policy
            .get("automaticClassification")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        || policy
            .get("unregisteredPredictors")
            .and_then(serde_json::Value::as_str)
            != Some("contextual-only")
        || policy
            .get("calibrationScope")
            .and_then(serde_json::Value::as_str)
            != Some("global-only")
        || policy
            .get("geneSpecificOverrides")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        || policy
            .get("thresholdComparison")
            .and_then(serde_json::Value::as_str)
            != Some("raw-numeric-no-rounding")
    {
        return Err("evidence calibration policy must remain display-only".into());
    }
    let strength_display = policy
        .get("strengthDisplay")
        .and_then(serde_json::Value::as_object)
        .ok_or("evidence calibration policy has no strength display contract")?;
    if strength_display
        .get("hueEncodesDirection")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || strength_display
            .get("saturationEncodesStrength")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        || strength_display
            .get("strengthPresentation")
            .and_then(serde_json::Value::as_str)
            != Some("text-and-tooltip")
    {
        return Err("evidence strength must not be encoded by color saturation".into());
    }
    let calibrations = value["calibrations"]
        .as_array()
        .ok_or("evidence calibration manifest has no calibrations")?;
    let mut ids = HashSet::new();
    for calibration in calibrations {
        let id = calibration["id"]
            .as_str()
            .filter(|id| safe_id(id))
            .ok_or("evidence calibration has an invalid ID")?;
        let reference_url = calibration["referenceUrl"]
            .as_str()
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| format!("evidence calibration {id} has an invalid reference URL"))?;
        if calibration["reference"]
            .as_str()
            .is_none_or(|text| text.is_empty())
            || calibration["scope"]
                .as_str()
                .is_none_or(|text| text.is_empty())
            || calibration["geneSpecific"].as_bool() != Some(false)
            || !calibration["singlePredictorOnly"].is_boolean()
        {
            return Err(format!(
                "evidence calibration {id} has incomplete provenance"
            ));
        }
        let bands = calibration["bands"]
            .as_array()
            .filter(|bands| !bands.is_empty())
            .ok_or_else(|| format!("evidence calibration {id} has no bands"))?;
        if !ids.insert(id) || reference_url.len() > 2048 {
            return Err(format!("evidence calibration {id} has invalid metadata"));
        }
        let range = calibration
            .get("scoreRange")
            .and_then(serde_json::Value::as_object);
        let range_minimum = range
            .and_then(|range| range.get("minimumInclusive"))
            .and_then(serde_json::Value::as_f64);
        let range_maximum = range
            .and_then(|range| range.get("maximumInclusive"))
            .and_then(serde_json::Value::as_f64);
        if range.is_some()
            && (!range_minimum.is_some_and(f64::is_finite)
                || !range_maximum.is_some_and(f64::is_finite)
                || range_minimum >= range_maximum)
        {
            return Err(format!(
                "evidence calibration {id} has an invalid score range"
            ));
        }
        let interval_policy = calibration
            .get("intervalPolicy")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("continuous");
        if !matches!(interval_policy, "continuous" | "published-discrete")
            || interval_policy == "published-discrete"
                && !calibration
                    .get("scorePrecision")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|precision| (1..=12).contains(&precision))
        {
            return Err(format!(
                "evidence calibration {id} has an invalid interval policy"
            ));
        }
        let allow_published_gaps = interval_policy == "published-discrete";
        let mut previous_maximum: Option<(f64, bool)> = None;
        for (index, band) in bands.iter().enumerate() {
            if !band["label"].is_string()
                || !band["direction"]
                    .as_str()
                    .is_some_and(|direction| matches!(direction, "benign" | "pathogenic" | "none"))
                || !band["strength"].as_str().is_some_and(|strength| {
                    matches!(
                        strength,
                        "none" | "supporting" | "moderate" | "3-point" | "strong" | "very strong"
                    )
                })
                || !band["tone"].as_str().is_some_and(|tone| {
                    matches!(
                        tone,
                        "neutral" | "informative" | "reassuring" | "caution" | "adverse"
                    )
                })
            {
                return Err(format!("evidence calibration {id} has an invalid band"));
            }
            let direction = band["direction"].as_str().unwrap();
            let tone = band["tone"].as_str().unwrap();
            if direction == "pathogenic" && tone != "adverse"
                || direction == "benign" && tone != "reassuring"
                || direction == "none" && tone != "neutral"
            {
                return Err(format!(
                    "evidence calibration {id} has a tone that conflicts with its direction"
                ));
            }
            let minimum_inclusive = band
                .get("minimumInclusive")
                .and_then(serde_json::Value::as_f64);
            let minimum_exclusive = band
                .get("minimumExclusive")
                .and_then(serde_json::Value::as_f64);
            let maximum_inclusive = band
                .get("maximumInclusive")
                .and_then(serde_json::Value::as_f64);
            let maximum_exclusive = band
                .get("maximumExclusive")
                .and_then(serde_json::Value::as_f64);
            if minimum_inclusive.is_some() && minimum_exclusive.is_some()
                || maximum_inclusive.is_some() && maximum_exclusive.is_some()
            {
                return Err(format!(
                    "evidence calibration {id} has ambiguous band boundaries"
                ));
            }
            let minimum = minimum_inclusive
                .map(|value| (value, true))
                .or_else(|| minimum_exclusive.map(|value| (value, false)))
                .or_else(|| {
                    (index == 0)
                        .then_some(range_minimum)
                        .flatten()
                        .map(|value| (value, true))
                });
            let maximum = maximum_inclusive
                .map(|value| (value, true))
                .or_else(|| maximum_exclusive.map(|value| (value, false)))
                .or_else(|| {
                    (index + 1 == bands.len())
                        .then_some(range_maximum)
                        .flatten()
                        .map(|value| (value, true))
                });
            if index == 0 && range_minimum.is_none() && minimum.is_some()
                || index + 1 == bands.len() && range_maximum.is_none() && maximum.is_some()
                || index > 0 && minimum.is_none()
                || index + 1 < bands.len() && maximum.is_none()
            {
                return Err(format!(
                    "evidence calibration {id} does not cover its declared score domain"
                ));
            }
            if let Some((minimum, minimum_inclusive)) = minimum {
                if !minimum.is_finite()
                    || range_minimum.is_some_and(|range_minimum| minimum < range_minimum)
                    || index > 0
                        && previous_maximum.is_none_or(|(previous, previous_inclusive)| {
                            if allow_published_gaps {
                                previous > minimum
                                    || previous == minimum
                                        && previous_inclusive
                                        && minimum_inclusive
                            } else {
                                previous != minimum || previous_inclusive == minimum_inclusive
                            }
                        })
                {
                    return Err(format!(
                        "evidence calibration {id} has a gap, overlap, or out-of-range band"
                    ));
                }
            } else if index > 0 {
                return Err(format!(
                    "evidence calibration {id} has a gap, overlap, or out-of-range band"
                ));
            }
            if let Some((maximum, maximum_inclusive)) = maximum {
                if !maximum.is_finite()
                    || range_maximum.is_some_and(|range_maximum| maximum > range_maximum)
                    || minimum.is_some_and(|(minimum, minimum_inclusive)| {
                        minimum > maximum
                            || minimum == maximum && (!minimum_inclusive || !maximum_inclusive)
                    })
                {
                    return Err(format!(
                        "evidence calibration {id} has a gap, overlap, or out-of-range band"
                    ));
                }
                previous_maximum = Some((maximum, maximum_inclusive));
            } else {
                previous_maximum = None;
            }
        }
    }
    let predictors = value["predictors"]
        .as_array()
        .filter(|predictors| !predictors.is_empty())
        .ok_or("evidence calibration manifest has no predictor registry")?;
    let mut predictor_ids = HashSet::new();
    let mut field_matches: HashSet<(String, String)> = HashSet::new();
    for predictor in predictors {
        let id = predictor["id"]
            .as_str()
            .filter(|id| safe_id(id))
            .ok_or("evidence predictor has an invalid ID")?;
        let status = predictor["calibrationStatus"]
            .as_str()
            .filter(|status| matches!(*status, "published" | "unverified" | "none"))
            .ok_or_else(|| format!("evidence predictor {id} has an invalid calibration status"))?;
        if !predictor_ids.insert(id)
            || predictor["label"]
                .as_str()
                .is_none_or(|value| value.is_empty())
            || predictor["scoreIdentity"]
                .as_str()
                .is_none_or(|value| value.is_empty())
            || !predictor["evidenceGroup"]
                .as_str()
                .is_some_and(|value| matches!(value, "missense-protein-effect" | "splicing"))
            || !predictor["role"]
                .as_str()
                .is_some_and(|value| matches!(value, "primary" | "alternate" | "contextual"))
            || !predictor["variantClasses"]
                .as_array()
                .is_some_and(|values| {
                    !values.is_empty() && values.iter().all(serde_json::Value::is_string)
                })
            || predictor
                .get("excludedVariantClasses")
                .is_some_and(|values| {
                    !values.as_array().is_some_and(|values| {
                        !values.is_empty() && values.iter().all(serde_json::Value::is_string)
                    })
                })
        {
            return Err(format!("evidence predictor {id} has invalid metadata"));
        }
        match (
            status,
            predictor
                .get("calibrationId")
                .and_then(serde_json::Value::as_str),
        ) {
            ("published", Some(calibration_id)) if ids.contains(calibration_id) => {}
            ("published", _) => {
                return Err(format!(
                    "published evidence predictor {id} has no known calibration"
                ));
            }
            (_, Some(_)) => {
                return Err(format!(
                    "uncalibrated evidence predictor {id} references a calibration"
                ));
            }
            _ => {}
        }
        let matches = predictor["matches"]
            .as_array()
            .filter(|matches| !matches.is_empty())
            .ok_or_else(|| format!("evidence predictor {id} has no field matches"))?;
        for field_match in matches {
            let verification_status = field_match["verificationStatus"]
                .as_str()
                .filter(|status| matches!(*status, "approved" | "unverified"))
                .ok_or_else(|| {
                    format!("evidence predictor {id} has an invalid source verification status")
                })?;
            match verification_status {
                "approved"
                    if field_match["sourceVersion"]
                        .as_str()
                        .is_none_or(|value| value.trim().is_empty()) =>
                {
                    return Err(format!(
                        "approved evidence predictor {id} has no source version"
                    ));
                }
                "unverified"
                    if field_match["reason"]
                        .as_str()
                        .is_none_or(|value| value.trim().is_empty()) =>
                {
                    return Err(format!("unverified evidence predictor {id} has no reason"));
                }
                _ => {}
            }
            let source_ids = field_match["sourceIds"]
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| format!("evidence predictor {id} has invalid source matches"))?;
            let field_names = field_match["fieldNames"]
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| format!("evidence predictor {id} has invalid field matches"))?;
            for source_id in source_ids {
                let source_id = source_id
                    .as_str()
                    .filter(|value| safe_id(value))
                    .ok_or_else(|| format!("evidence predictor {id} has an invalid source ID"))?;
                for field_name in field_names {
                    let field_name = field_name
                        .as_str()
                        .filter(|value| !value.is_empty() && value.len() <= 128)
                        .ok_or_else(|| {
                            format!("evidence predictor {id} has an invalid field name")
                        })?;
                    if !field_matches
                        .insert((source_id.to_ascii_lowercase(), field_name.to_owned()))
                    {
                        return Err(format!(
                            "evidence predictor field match {source_id}.{field_name} is duplicated"
                        ));
                    }
                }
            }
        }
    }
    let categorical_predictions = value["categoricalPredictions"]
        .as_array()
        .filter(|mappings| !mappings.is_empty())
        .ok_or("evidence calibration manifest has no categorical prediction registry")?;
    let mut categorical_fields = HashSet::new();
    for mapping in categorical_predictions {
        let source_id = mapping["sourceId"]
            .as_str()
            .filter(|value| safe_id(value))
            .ok_or("categorical prediction mapping has an invalid source ID")?;
        let field_name = mapping["fieldName"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or("categorical prediction mapping has an invalid field name")?;
        if mapping["displayOnly"].as_bool() != Some(true)
            || !mapping["referenceUrl"]
                .as_str()
                .is_some_and(|url| url.starts_with("https://") && url.len() <= 2048)
            || !categorical_fields.insert((source_id.to_ascii_lowercase(), field_name.to_owned()))
        {
            return Err(format!(
                "categorical prediction mapping {source_id}.{field_name} has invalid metadata"
            ));
        }
        let codes = mapping["codes"]
            .as_array()
            .filter(|codes| !codes.is_empty())
            .ok_or_else(|| {
                format!("categorical prediction mapping {source_id}.{field_name} has no codes")
            })?;
        let mut code_values = HashSet::new();
        for code in codes {
            let value = code["value"]
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 64)
                .ok_or_else(|| {
                    format!(
                        "categorical prediction mapping {source_id}.{field_name} has an invalid code"
                    )
                })?;
            if !code_values.insert(value.to_ascii_lowercase())
                || !code["label"]
                    .as_str()
                    .is_some_and(|label| !label.trim().is_empty() && label.len() <= 128)
                || !code["tone"].as_str().is_some_and(|tone| {
                    matches!(
                        tone,
                        "neutral" | "informative" | "reassuring" | "caution" | "adverse"
                    )
                })
            {
                return Err(format!(
                    "categorical prediction mapping {source_id}.{field_name} has invalid codes"
                ));
            }
        }
    }
    let dbnsfp_contract: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/dbnsfp-4.9a-curated-fields.json"
    )))
    .map_err(|error| format!("invalid dbNSFP field contract: {error}"))?;
    let expected_dbnsfp_predictions = dbnsfp_contract["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["fields"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_str)
        .filter(|field| field.ends_with("_pred"))
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mapped_dbnsfp_predictions = categorical_fields
        .iter()
        .filter(|(source_id, _)| source_id == "dbnsfp")
        .map(|(_, field_name)| field_name.to_owned())
        .collect::<HashSet<_>>();
    if mapped_dbnsfp_predictions != expected_dbnsfp_predictions {
        return Err(
            "categorical prediction registry must cover every curated dbNSFP _pred field".into(),
        );
    }
    let missense_policy = &value["interpretationPolicy"]["missenseProteinEffect"];
    let splicing_policy = &value["interpretationPolicy"]["splicing"];
    if missense_policy["primaryPredictorId"] != "revel"
        || missense_policy["fallback"] != "none"
        || missense_policy["aggregation"] != "single-predeclared-predictor"
        || splicing_policy["primaryPredictorId"] != "spliceai-max-delta"
        || splicing_policy["aggregation"] != "maximum-delta-score"
    {
        return Err("evidence calibration predictor selection policy is unsafe".into());
    }
    Ok(())
}

fn validate_hpo_manifest_contract(resource: &Resource) -> Result<(), String> {
    if resource.manifest_ref.as_deref() != Some("config/hpo-assets.json")
        || resource.manifest_role.as_deref() != Some("bootstrap-fallback")
        || resource.release.policy != "rolling"
        || resource.release.version != "latest"
        || resource.release.resolver.as_deref() != Some("github-hpo-release-assets")
        || resource.release.resolver_api_url.as_deref()
            != Some(
                "https://api.github.com/repos/obophenotype/human-phenotype-ontology/releases/latest",
            )
    {
        return Err("HPO must use the cataloged rolling resolver and bootstrap manifest".into());
    }
    let contents = resource
        .manifest_ref
        .as_deref()
        .and_then(embedded_manifest)
        .ok_or("HPO resource has no embedded asset manifest")?;
    let manifest: serde_json::Value = serde_json::from_str(contents)
        .map_err(|error| format!("invalid HPO asset manifest: {error}"))?;
    manifest
        .get("release")
        .and_then(serde_json::Value::as_str)
        .ok_or("HPO asset manifest has no release")?;
    manifest
        .get("releaseUrl")
        .and_then(serde_json::Value::as_str)
        .ok_or("HPO asset manifest has no release URL")?;
    let assets = manifest
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or("HPO asset manifest has no assets")?;
    let required_kinds = ["ontology", "disease-annotations", "disease-genes"];
    for kind in required_kinds {
        let asset = assets
            .iter()
            .find(|asset| asset.get("kind").and_then(serde_json::Value::as_str) == Some(kind))
            .ok_or_else(|| format!("HPO bootstrap manifest is missing {kind}"))?;
        let valid = asset
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|bytes| bytes > 0)
            && asset
                .get("url")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|url| url.starts_with("https://"))
            && asset
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .is_some_and(valid_sha256);
        if !valid {
            return Err(format!(
                "HPO bootstrap manifest has invalid {kind} metadata"
            ));
        }
    }
    Ok(())
}

fn validate_mondo_manifest_contract(resource: &Resource) -> Result<(), String> {
    if resource.user_visible
        || resource.manifest_ref.as_deref() != Some("config/mondo-assets.json")
        || resource.manifest_role.as_deref() != Some("bootstrap-fallback")
        || resource.release.policy != "rolling"
        || resource.release.version != "latest"
        || resource.release.resolver.as_deref() != Some("github-mondo-release-assets")
        || resource.release.resolver_api_url.as_deref()
            != Some("https://api.github.com/repos/monarch-initiative/mondo/releases/latest")
    {
        return Err("MONDO must use the hidden rolling resolver and bootstrap manifest".into());
    }
    let contents = resource
        .manifest_ref
        .as_deref()
        .and_then(embedded_manifest)
        .ok_or("MONDO resource has no embedded asset manifest")?;
    let manifest: serde_json::Value = serde_json::from_str(contents)
        .map_err(|error| format!("invalid MONDO asset manifest: {error}"))?;
    let assets = manifest
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or("MONDO asset manifest has no assets")?;
    let valid = assets.len() == 1
        && assets[0].get("kind").and_then(serde_json::Value::as_str) == Some("condition-ontology")
        && assets[0]
            .get("filename")
            .and_then(serde_json::Value::as_str)
            == Some("mondo.json")
        && assets[0]
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|bytes| bytes > 0)
        && assets[0]
            .get("url")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|url| url.starts_with("https://github.com/monarch-initiative/mondo/"))
        && assets[0]
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .is_some_and(valid_sha256);
    if !valid {
        return Err("MONDO bootstrap manifest has invalid asset metadata".into());
    }
    Ok(())
}

fn embedded_manifest_exists(path: &str) -> bool {
    embedded_manifest(path).is_some()
}

fn embedded_manifest(path: &str) -> Option<&'static str> {
    match path {
        "config/dbnsfp-4.9a-members.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/dbnsfp-4.9a-members.json"
        ))),
        "config/hpo-assets.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/hpo-assets.json"
        ))),
        "config/mondo-assets.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/mondo-assets.json"
        ))),
        "config/indexed-sources.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/indexed-sources.json"
        ))),
        "config/revel-1.3-archives.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/revel-1.3-archives.json"
        ))),
        "config/wgs-streams.json" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/wgs-streams.json"
        ))),
        _ => None,
    }
}

fn embedded_field_contract_exists(path: &str, resource_id: &str, contract_id: &str) -> bool {
    let contents = match path {
        "config/dbnsfp-4.9a-curated-fields.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/dbnsfp-4.9a-curated-fields.json"
        )),
        "config/supplementary-source-fields.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/supplementary-source-fields.json"
        )),
        _ => return false,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(contents) else {
        return false;
    };
    if path.ends_with("dbnsfp-4.9a-curated-fields.json") {
        return value["resourceId"] == resource_id && value["id"] == contract_id;
    }
    value["sources"].as_array().is_some_and(|sources| {
        sources.iter().any(|source| {
            source["resourceId"] == resource_id && source["contractId"] == contract_id
        })
    })
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_the_complete_source_api() {
        let json: serde_json::Value = serde_json::from_str(&sources_json()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), catalog().sources.len());
        assert_eq!(json[0]["id"], "fastvep");
        assert_eq!(json[0]["defaultEnabled"], true);
        let spliceai = json
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == "spliceai")
            .unwrap();
        assert_eq!(spliceai["evidenceScope"], "feature");
        assert_eq!(spliceai["featureIdentity"], "gene");
        assert!(json[0].get("featureIdentity").is_none());
    }

    #[test]
    fn every_actionable_release_projects_to_the_downloader_contract() {
        assert_eq!(
            download_releases().count(),
            catalog()
                .resources
                .iter()
                .filter(|resource| resource.user_visible)
                .count()
        );
        for current in &catalog().resources {
            let projected = download_release(&current.id).expect("download release");
            assert_eq!(projected.resource_id, current.id);
            assert_eq!(projected.version, current.release.version);
            assert_eq!(projected.url, current.release.primary_url);
            assert_eq!(projected.filename, current.release.filename);
            assert_eq!(
                projected.download_bytes,
                Some(current.release.download_bytes)
            );
            assert_eq!(projected.archive_format, current.release.archive_format);
            assert_eq!(projected.range_resume, current.release.range_resume);
            assert_eq!(projected.size_checked_at, current.release.size_checked_at);
        }
    }

    #[test]
    fn hpo_projects_as_one_source_card_and_one_managed_resource() {
        let hpo_sources = catalog()
            .sources
            .iter()
            .filter(|source| source.id == "hpo")
            .collect::<Vec<_>>();
        let hpo_resources = catalog()
            .resources
            .iter()
            .filter(|resource| resource.id == "hpo")
            .collect::<Vec<_>>();

        assert_eq!(hpo_sources.len(), 1);
        assert_eq!(hpo_sources[0].name, "Phenotype and condition knowledge");
        assert_eq!(hpo_sources[0].fastvep_source, None);
        assert_eq!(hpo_resources.len(), 1);
        assert_eq!(hpo_resources[0].delivery, "knowledge-cache");
        assert_eq!(
            hpo_resources[0].manifest_ref.as_deref(),
            Some("config/hpo-assets.json")
        );
        assert_eq!(
            hpo_resources[0].manifest_role.as_deref(),
            Some("bootstrap-fallback")
        );
        assert_eq!(hpo_resources[0].release.version, "latest");
        assert_eq!(hpo_resources[0].release.policy, "rolling");
        let mondo = resource("mondo").unwrap();
        assert!(!mondo.user_visible);
        assert_eq!(
            mondo.manifest_ref.as_deref(),
            Some("config/mondo-assets.json")
        );
    }

    #[test]
    fn every_asset_manifest_is_reached_through_its_resource() {
        for resource_id in [
            "dbnsfp",
            "gnomad",
            "gnomad-genomes",
            "phylop",
            "cadd",
            "spliceai",
            "revel",
            "hpo",
            "mondo",
        ] {
            assert!(
                resource_manifest_json(resource_id).is_ok(),
                "{resource_id} asset manifest is not catalog-reachable"
            );
        }
        assert_eq!(
            resource("cadd").unwrap().manifest_ref.as_deref(),
            Some("config/indexed-sources.json")
        );
        assert_eq!(
            resource("spliceai").unwrap().manifest_ref.as_deref(),
            Some("config/indexed-sources.json")
        );
    }

    #[test]
    fn evidence_calibrations_have_one_catalog_entry_point() {
        assert_eq!(
            catalog().evidence_calibration_ref,
            "config/evidence-calibrations.json"
        );
        let manifest: serde_json::Value =
            serde_json::from_str(evidence_calibrations_json()).unwrap();
        assert_eq!(manifest["schemaVersion"], 2);
        assert_eq!(manifest["interpretationPolicy"]["mode"], "display-only");
        assert_eq!(
            manifest["interpretationPolicy"]["automaticClassification"],
            false
        );
        assert_eq!(
            manifest["interpretationPolicy"]["calibrationScope"],
            "global-only"
        );
        assert_eq!(
            manifest["interpretationPolicy"]["geneSpecificOverrides"],
            false
        );
        assert_eq!(
            manifest["interpretationPolicy"]["thresholdComparison"],
            "raw-numeric-no-rounding"
        );
        assert_eq!(
            manifest["interpretationPolicy"]["strengthDisplay"]["saturationEncodesStrength"],
            false
        );
        assert_eq!(
            manifest["interpretationPolicy"]["missenseProteinEffect"]["primaryPredictorId"],
            "revel"
        );
        assert_eq!(
            manifest["interpretationPolicy"]["missenseProteinEffect"]["fallback"],
            "none"
        );
        let revel = manifest["calibrations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|calibration| calibration["id"] == "revel-clingen-svi-2022-global")
            .unwrap();
        assert_eq!(revel["id"], "revel-clingen-svi-2022-global");
        assert_eq!(revel["scoreRange"]["minimumInclusive"], 0.0);
        assert_eq!(revel["scoreRange"]["maximumInclusive"], 1.0);
        assert_eq!(revel["bands"].as_array().unwrap().len(), 8);
        let predictors = manifest["predictors"].as_array().unwrap();
        let primate_ai = predictors
            .iter()
            .find(|predictor| predictor["id"] == "primateai")
            .unwrap();
        assert_eq!(
            primate_ai["calibrationId"],
            "primateai-clingen-svi-2022-global"
        );
        let splice_ai = predictors
            .iter()
            .find(|predictor| predictor["id"] == "spliceai-max-delta")
            .unwrap();
        assert_eq!(splice_ai["role"], "primary");
        assert_eq!(
            splice_ai["excludedVariantClasses"],
            serde_json::json!(["splice_acceptor_variant", "splice_donor_variant"])
        );
        assert_eq!(
            manifest["interpretationPolicy"]["splicing"]["aggregation"],
            "maximum-delta-score"
        );
        assert!(
            manifest["calibrations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|calibration| calibration["geneSpecific"] == false)
        );
        let favor_revel_match = predictors
            .iter()
            .find(|predictor| predictor["id"] == "revel")
            .unwrap()["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field_match| field_match["sourceIds"] == serde_json::json!(["favor-online"]))
            .unwrap();
        assert_eq!(favor_revel_match["verificationStatus"], "unverified");
        let dbnsfp_revel_match = predictors
            .iter()
            .find(|predictor| predictor["id"] == "revel")
            .unwrap()["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field_match| field_match["sourceIds"] == serde_json::json!(["dbnsfp"]))
            .unwrap();
        assert_eq!(dbnsfp_revel_match["verificationStatus"], "approved");
        assert_eq!(
            manifest["categoricalPredictions"].as_array().unwrap().len(),
            30
        );
    }

    #[test]
    fn evidence_calibration_boundaries_preserve_published_intervals() {
        let manifest: serde_json::Value =
            serde_json::from_str(evidence_calibrations_json()).unwrap();
        let calibration = |id: &str| {
            manifest["calibrations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|calibration| calibration["id"] == id)
                .unwrap()
        };
        let label_at = |calibration: &serde_json::Value, score: f64| {
            calibration["bands"]
                .as_array()
                .unwrap()
                .iter()
                .find(|band| {
                    band.get("minimumInclusive")
                        .and_then(serde_json::Value::as_f64)
                        .is_none_or(|minimum| score >= minimum)
                        && band
                            .get("minimumExclusive")
                            .and_then(serde_json::Value::as_f64)
                            .is_none_or(|minimum| score > minimum)
                        && band
                            .get("maximumInclusive")
                            .and_then(serde_json::Value::as_f64)
                            .is_none_or(|maximum| score <= maximum)
                        && band
                            .get("maximumExclusive")
                            .and_then(serde_json::Value::as_f64)
                            .is_none_or(|maximum| score < maximum)
                })
                .and_then(|band| band["label"].as_str())
                .unwrap_or("Uncalibrated precision gap")
                .to_owned()
        };

        let revel = calibration("revel-clingen-svi-2022-global");
        assert_eq!(
            label_at(revel, 0.290),
            "Supporting benign computational evidence"
        );
        assert_eq!(label_at(revel, 0.291), "Indeterminate calibrated range");
        assert_eq!(
            label_at(revel, 0.644),
            "Supporting pathogenic computational evidence"
        );
        assert_eq!(
            label_at(revel, 0.932),
            "Strong pathogenic computational evidence"
        );

        let alpha_missense = calibration("alphamissense-clingen-svi-2025-global");
        assert_eq!(
            label_at(alpha_missense, 0.070),
            "3-point benign computational interval"
        );
        assert_eq!(
            label_at(alpha_missense, 0.0705),
            "Uncalibrated precision gap"
        );
        assert_eq!(
            label_at(alpha_missense, 0.071),
            "Moderate benign computational evidence"
        );
        assert_eq!(
            label_at(alpha_missense, 0.791),
            "Indeterminate calibrated range"
        );
        assert_eq!(
            label_at(alpha_missense, 0.792),
            "Supporting pathogenic computational evidence"
        );
        assert_eq!(
            label_at(alpha_missense, 0.990),
            "Strong pathogenic computational evidence"
        );
        assert_eq!(
            label_at(alpha_missense, 0.972),
            "3-point pathogenic computational interval"
        );

        let varity_r = calibration("varity-r-clingen-svi-2025-global");
        assert_eq!(
            label_at(varity_r, 0.036),
            "Strong benign computational evidence"
        );
        assert_eq!(label_at(varity_r, 0.0365), "Uncalibrated precision gap");
        assert_eq!(
            label_at(varity_r, 0.037),
            "3-point benign computational interval"
        );
        assert_eq!(
            label_at(varity_r, 0.675),
            "Supporting pathogenic computational evidence"
        );
        assert_eq!(
            label_at(varity_r, 0.915),
            "3-point pathogenic computational interval"
        );
        assert_eq!(
            label_at(varity_r, 0.965),
            "Strong pathogenic computational evidence"
        );

        let esm1b = calibration("esm1b-clingen-svi-2025-global");
        assert_eq!(
            label_at(esm1b, -24.0),
            "Strong pathogenic computational evidence"
        );
        assert_eq!(
            label_at(esm1b, -23.9),
            "3-point pathogenic computational interval"
        );
        assert_eq!(
            label_at(esm1b, -10.7),
            "Supporting pathogenic computational evidence"
        );
        assert_eq!(label_at(esm1b, -6.35), "Uncalibrated precision gap");
        assert_eq!(label_at(esm1b, -6.4), "Indeterminate calibrated range");
        assert_eq!(
            label_at(esm1b, -6.3),
            "Supporting benign computational evidence"
        );
        assert_eq!(
            label_at(esm1b, 8.8),
            "3-point benign computational interval"
        );

        let splice_ai = calibration("spliceai-clingen-svi-2023-global");
        assert_eq!(
            label_at(splice_ai, 0.100),
            "Supporting benign splice-effect evidence"
        );
        assert_eq!(label_at(splice_ai, 0.101), "Indeterminate calibrated range");
        assert_eq!(
            label_at(splice_ai, 0.200),
            "Supporting pathogenic splice-effect evidence"
        );

        for calibration in manifest["calibrations"].as_array().unwrap() {
            for band in calibration["bands"].as_array().unwrap() {
                if band["direction"] == "pathogenic" {
                    assert_eq!(
                        band["tone"], "adverse",
                        "{} uses a non-adverse pathogenic tone",
                        calibration["id"]
                    );
                }
            }
        }

        let polyphen = manifest["predictors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|predictor| predictor["id"] == "polyphen2-hvar")
            .unwrap();
        assert_eq!(
            polyphen["matches"][0]["fieldNames"],
            serde_json::json!(["Polyphen2_HVAR_score"])
        );
    }

    #[test]
    fn online_services_are_centralized_in_the_catalog() {
        let monarch = service("monarch-phenotype-gene-ranking").unwrap();
        assert_eq!(monarch.provider, "Monarch Initiative");
        assert_eq!(
            monarch.api_url,
            "https://api.monarchinitiative.org/v3/api/semsim/search"
        );
        assert_eq!(monarch.timeout_seconds, 45);
        assert_eq!(monarch.max_results, 50);
        let favor = service("favor-variant-annotation").unwrap();
        assert_eq!(
            favor.coding_api_url.as_deref(),
            Some("https://api-v2.genohub.org/api/v1/variants/batch/coding")
        );
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
    fn osa2_rollout_is_limited_to_verified_source_encodings() {
        for resource_id in ["dbnsfp", "phylop", "cadd", "revel"] {
            assert_eq!(preferred_cache_format(resource_id), Some("osa2"));
        }
        for resource_id in ["clinvar", "dbsnp", "gnomad", "gnomad-genomes", "spliceai"] {
            assert_eq!(preferred_cache_format(resource_id), Some("osa"));
        }
    }

    #[test]
    fn rolling_sources_require_named_resolvers() {
        assert_eq!(resource("clinvar").unwrap().release.policy, "rolling");
        assert_eq!(resource("dbsnp").unwrap().release.policy, "rolling");
        assert_eq!(resource("hpo").unwrap().release.policy, "rolling");
        assert_eq!(
            resolver_api_url("hpo"),
            Some(
                "https://api.github.com/repos/obophenotype/human-phenotype-ontology/releases/latest"
            )
        );
        assert!(catalog().resources.iter().all(|resource| {
            resource.release.policy != "rolling" || resource.release.resolver.is_some()
        }));
    }

    #[test]
    fn profiles_serialize_from_the_catalog() {
        let json: serde_json::Value = serde_json::from_str(&profiles_json()).unwrap();
        assert_eq!(json[0]["id"], "wgs");
        assert_eq!(json[1]["id"], "standard");
        assert_eq!(json[2]["id"], "online");
        assert_eq!(
            json[2]["serviceIds"],
            serde_json::json!(["favor-variant-annotation"])
        );
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
