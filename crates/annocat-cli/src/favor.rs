use duckdb::types::Value as SqlValue;
use duckdb::{Connection, appender_params_from_iter, params, params_from_iter};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const SOURCE_ID: &str = "favor-online";
pub const SERVICE_ID: &str = "favor-variant-annotation";
pub const EVIDENCE_FILE: &str = "favor-evidence.parquet";
pub const STATUS_FILE: &str = "favor-status.parquet";
pub const FIELD_CATALOG_FILE: &str = "favor-field-catalog.json";
pub const PROVENANCE_FILE: &str = "favor-provenance.json";

const QUERY_DIRECTORY: &str = "query-evidence";
const QUERY_CATALOG_FILE: &str = "query-field-catalog.json";
pub const QUERY_GENE_EVIDENCE_FILE: &str = "query-gene-evidence.parquet";
const REQUEST_CHUNK: usize = 1_000;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrichRequest {
    pub allele_ids: Vec<String>,
    pub consent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichSummary {
    requested: usize,
    found: u64,
    not_found: u64,
    ambiguous: u64,
    errors: u64,
    coding_found: u64,
    total_cached: u64,
    latest_fetch: String,
}

#[derive(Debug, Clone)]
struct Coordinate {
    allele_id: String,
    reference: String,
}

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    references: &'a [String],
    depth: &'static str,
}

#[derive(Debug, Serialize)]
struct CodingApiRequest<'a> {
    references: &'a [String],
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    items: Vec<ApiItem>,
}

#[derive(Debug, Deserialize)]
struct CodingApiResponse {
    items: Vec<CodingApiItem>,
}

#[derive(Debug, Deserialize)]
struct ApiItem {
    reference: String,
    status: String,
    variant: Option<ApiVariant>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodingApiItem {
    reference: String,
    status: String,
    coding: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiVariant {
    rsid: Option<String>,
    vcf: Option<String>,
    gene: Option<String>,
    consequence: Option<String>,
    clinical_significance: Option<String>,
    cadd_phred: Option<f64>,
    revel: Option<f64>,
    alpha_missense: Option<f64>,
    spliceai_ds_max: Option<f64>,
    sift_cat: Option<String>,
    polyphen_cat: Option<String>,
    metasvm_pred: Option<String>,
    gnomad_af: Option<f64>,
    bravo_af: Option<f64>,
    tg_all: Option<f64>,
    apc_conservation: Option<f64>,
    apc_epigenetics: Option<f64>,
    apc_protein_function: Option<f64>,
}

#[derive(Debug, Clone)]
struct StoredItem {
    allele_id: String,
    reference: String,
    status: String,
    error: Option<String>,
    variant: Option<ApiVariant>,
    coding: Option<Value>,
}

#[derive(Clone, Copy)]
struct FieldDefinition {
    path: &'static str,
    value_type: &'static str,
}

const FIELDS: &[FieldDefinition] = &[
    FieldDefinition {
        path: "rsid",
        value_type: "string",
    },
    FieldDefinition {
        path: "vcf",
        value_type: "string",
    },
    FieldDefinition {
        path: "gene",
        value_type: "string",
    },
    FieldDefinition {
        path: "consequence",
        value_type: "string",
    },
    FieldDefinition {
        path: "clinicalSignificance",
        value_type: "string",
    },
    FieldDefinition {
        path: "caddPhred",
        value_type: "number",
    },
    FieldDefinition {
        path: "revel",
        value_type: "number",
    },
    FieldDefinition {
        path: "alphaMissense",
        value_type: "number",
    },
    FieldDefinition {
        path: "spliceaiDsMax",
        value_type: "number",
    },
    FieldDefinition {
        path: "siftCat",
        value_type: "string",
    },
    FieldDefinition {
        path: "polyphenCat",
        value_type: "string",
    },
    FieldDefinition {
        path: "metasvmPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "gnomadAf",
        value_type: "number",
    },
    FieldDefinition {
        path: "bravoAf",
        value_type: "number",
    },
    FieldDefinition {
        path: "tgAll",
        value_type: "number",
    },
    FieldDefinition {
        path: "apcConservation",
        value_type: "number",
    },
    FieldDefinition {
        path: "apcEpigenetics",
        value_type: "number",
    },
    FieldDefinition {
        path: "apcProteinFunction",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingTranscriptBasis",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingGene",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingHgvsp",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingCaddPhred",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingRevelScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingAlphaMissenseScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingAlphaMissensePred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingSiftScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingSiftPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingPolyphen2HvarScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingPolyphen2HvarPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingMetaSvmScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingMetaSvmPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingBayesDelNoAfScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingVest4Score",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingMutPred2Score",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingMutPred2Pred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingPrimateAiScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingPrimateAiPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingMpcScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingVarityRScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingEsm1bScore",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingEsm1bPred",
        value_type: "string",
    },
    FieldDefinition {
        path: "codingGerpRs",
        value_type: "number",
    },
    FieldDefinition {
        path: "codingPhyloP100way",
        value_type: "number",
    },
];

pub fn enrich(
    run_directory: &Path,
    variants: &Path,
    canonical_evidence: &Path,
    canonical_catalog: &Path,
    mut request: EnrichRequest,
) -> Result<EnrichSummary, String> {
    normalize_request(&mut request)?;
    validate_assembly(run_directory)?;
    let service = annocat_core::source_catalog::service(SERVICE_ID)
        .ok_or("FAVOR service configuration is missing")?;
    let coding_endpoint = service
        .coding_api_url
        .as_deref()
        .ok_or("FAVOR coding service configuration is missing")?;
    if request.allele_ids.len() > service.max_results {
        return Err(format!(
            "Online annotations are limited to {} variants per request",
            service.max_results
        ));
    }
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let coordinates = report_coordinates(variants, &request.allele_ids)?;
    let fetched_at = super::annotation::current_timestamp();
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    for coordinate in coordinates {
        if valid_favor_reference(&coordinate.reference) {
            supported.push(coordinate);
        } else {
            unsupported.push(StoredItem {
                allele_id: coordinate.allele_id,
                reference: coordinate.reference,
                status: "error".into(),
                error: Some("This allele representation is not supported by FAVOR".into()),
                variant: None,
                coding: None,
            });
        }
    }
    if !unsupported.is_empty() {
        publish_items(
            run_directory,
            canonical_evidence,
            canonical_catalog,
            &unsupported,
            &fetched_at,
            service.api_url.as_str(),
            coding_endpoint,
        )?;
    }
    for chunk in supported.chunks(REQUEST_CHUNK) {
        let references = chunk
            .iter()
            .map(|coordinate| coordinate.reference.clone())
            .collect::<Vec<_>>();
        let (standard_response, coding_response) = std::thread::scope(|scope| {
            let standard = scope.spawn(|| {
                call_standard_api(
                    service.api_url.as_str(),
                    service.timeout_seconds,
                    &references,
                )
            });
            let coding = scope
                .spawn(|| call_coding_api(coding_endpoint, service.timeout_seconds, &references));
            let standard = standard
                .join()
                .map_err(|_| "FAVOR standard request stopped unexpectedly".to_string())??;
            let coding = coding
                .join()
                .map_err(|_| "FAVOR coding request stopped unexpectedly".to_string())??;
            Ok::<_, String>((standard, coding))
        })?;
        let mut items = validate_response(chunk, standard_response)?;
        merge_coding_response(chunk, &mut items, coding_response)?;
        publish_items(
            run_directory,
            canonical_evidence,
            canonical_catalog,
            &items,
            &fetched_at,
            service.api_url.as_str(),
            coding_endpoint,
        )?;
    }
    summary(run_directory, request.allele_ids.len(), &fetched_at)
}

pub fn status(run_directory: &Path, enabled: bool) -> Result<Value, String> {
    let service = annocat_core::source_catalog::service(SERVICE_ID)
        .ok_or("FAVOR service configuration is missing")?;
    let status_path = run_directory.join(STATUS_FILE);
    if !status_path.is_file() {
        return Ok(json!({
            "enabled": enabled,
            "hasData": false,
            "provider": service.provider,
            "maxVariants": service.max_results,
            "cached": 0,
            "found": 0,
            "notFound": 0,
            "ambiguous": 0,
            "errors": 0,
            "codingFound": 0,
            "latestFetch": null
        }));
    }
    let counts = status_counts(&status_path)?;
    let coding_found = coding_evidence_count(&run_directory.join(EVIDENCE_FILE))?;
    Ok(json!({
        "enabled": enabled,
        "hasData": true,
        "provider": service.provider,
        "maxVariants": service.max_results,
        "cached": counts.total,
        "found": counts.found,
        "notFound": counts.not_found,
        "ambiguous": counts.ambiguous,
        "errors": counts.errors,
        "codingFound": coding_found,
        "latestFetch": counts.latest_fetch
    }))
}

pub fn effective_evidence(canonical: &Path) -> PathBuf {
    let Some(run_directory) = canonical.parent() else {
        return canonical.to_path_buf();
    };
    let query = run_directory.join(QUERY_DIRECTORY);
    if query.join("canonical.parquet").is_file() {
        query.join("*.parquet")
    } else {
        canonical.to_path_buf()
    }
}

pub fn effective_catalog(canonical: &Path) -> PathBuf {
    let Some(run_directory) = canonical.parent() else {
        return canonical.to_path_buf();
    };
    let merged = run_directory.join(QUERY_CATALOG_FILE);
    if merged.is_file() {
        merged
    } else {
        canonical.to_path_buf()
    }
}

pub fn prepare_query_assets(
    canonical_evidence: &Path,
    canonical_catalog: &Path,
) -> Result<(), String> {
    prepare_query_assets_with_gene(canonical_evidence, canonical_catalog, None)
}

pub fn prepare_query_assets_with_gene(
    canonical_evidence: &Path,
    canonical_catalog: &Path,
    gene_assets: Option<(&Path, &Path)>,
) -> Result<(), String> {
    let run_directory = canonical_evidence
        .parent()
        .ok_or("completed evidence path has no parent")?;
    let favor_evidence = run_directory.join(EVIDENCE_FILE);
    let favor_catalog = run_directory.join(FIELD_CATALOG_FILE);
    let has_favor = favor_evidence.is_file() && favor_catalog.is_file();
    if !has_favor && gene_assets.is_none() {
        for stale in [
            run_directory.join(QUERY_CATALOG_FILE),
            run_directory.join(QUERY_GENE_EVIDENCE_FILE),
            run_directory
                .join(QUERY_DIRECTORY)
                .join("canonical.parquet"),
            run_directory.join(QUERY_DIRECTORY).join("favor.parquet"),
            run_directory
                .join(QUERY_DIRECTORY)
                .join("phenotype-candidates.parquet"),
        ] {
            if stale.is_file() {
                fs::remove_file(&stale)
                    .map_err(|error| format!("cannot remove stale query evidence: {error}"))?;
            }
        }
        let query_directory = run_directory.join(QUERY_DIRECTORY);
        if query_directory.is_dir() {
            fs::remove_dir(&query_directory)
                .map_err(|error| format!("cannot remove empty query directory: {error}"))?;
        }
        return Ok(());
    }
    let query_directory = run_directory.join(QUERY_DIRECTORY);
    fs::create_dir_all(&query_directory)
        .map_err(|error| format!("cannot create FAVOR query directory: {error}"))?;
    publish_hard_link(
        canonical_evidence,
        &query_directory.join("canonical.parquet"),
    )?;
    if has_favor {
        publish_hard_link(&favor_evidence, &query_directory.join("favor.parquet"))?;
    } else {
        let stale = query_directory.join("favor.parquet");
        if stale.is_file() {
            fs::remove_file(stale)
                .map_err(|error| format!("cannot remove stale FAVOR evidence: {error}"))?;
        }
    }
    let legacy_candidates = query_directory.join("phenotype-candidates.parquet");
    if legacy_candidates.is_file() {
        fs::remove_file(legacy_candidates).map_err(|error| {
            format!("cannot remove legacy phenotype candidate evidence: {error}")
        })?;
    }
    let gene_catalog = if let Some((gene_evidence, gene_catalog)) = gene_assets {
        publish_hard_link(gene_evidence, &run_directory.join(QUERY_GENE_EVIDENCE_FILE))?;
        Some(gene_catalog)
    } else {
        let stale = run_directory.join(QUERY_GENE_EVIDENCE_FILE);
        if stale.is_file() {
            fs::remove_file(stale)
                .map_err(|error| format!("cannot remove stale phenotype evidence: {error}"))?;
        }
        None
    };
    merge_query_catalogs(
        canonical_catalog,
        has_favor.then_some(favor_catalog.as_path()),
        gene_catalog,
        &run_directory.join(QUERY_CATALOG_FILE),
    )
}

pub fn packaged_assets(run_directory: &Path) -> Result<Vec<(&'static str, &'static str)>, String> {
    let files = [
        (EVIDENCE_FILE, "favor-evidence"),
        (STATUS_FILE, "favor-status"),
        (FIELD_CATALOG_FILE, "favor-field-catalog"),
        (PROVENANCE_FILE, "favor-provenance"),
    ];
    let existing = files
        .iter()
        .filter(|(name, _)| run_directory.join(name).is_file())
        .count();
    if existing == 0 {
        return Ok(Vec::new());
    }
    if existing != files.len() {
        return Err("online annotation files are incomplete".into());
    }
    Ok(files.to_vec())
}

fn normalize_request(request: &mut EnrichRequest) -> Result<(), String> {
    if !request.consent {
        return Err("Confirm the request before AnnoCAT sends variant coordinates to FAVOR".into());
    }
    if request.allele_ids.is_empty() {
        return Err("select at least one variant for online annotations".into());
    }
    for allele_id in &request.allele_ids {
        if allele_id.len() > 64
            || !allele_id.starts_with("allele-")
            || !allele_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("online annotations require valid allele identifiers".into());
        }
    }
    let mut seen = HashSet::new();
    request
        .allele_ids
        .retain(|allele_id| seen.insert(allele_id.clone()));
    Ok(())
}

fn validate_assembly(run_directory: &Path) -> Result<(), String> {
    let bytes = fs::read(run_directory.join("manifest.json"))
        .map_err(|error| format!("cannot read completed run manifest: {error}"))?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid completed run manifest: {error}"))?;
    if manifest["state"] != "completed" || manifest["assembly"] != "GRCh38" {
        return Err("online annotations require a completed GRCh38 AnnoCAT result".into());
    }
    Ok(())
}

fn report_coordinates(variants: &Path, allele_ids: &[String]) -> Result<Vec<Coordinate>, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut found = HashMap::new();
    for chunk in allele_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT allele_id, chromosome, position, reference, alternate
             FROM read_parquet(?) WHERE allele_id IN ({placeholders})"
        );
        let mut values = vec![SqlValue::from(variants.to_string_lossy().into_owned())];
        values.extend(chunk.iter().cloned().map(Into::into));
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("cannot prepare FAVOR variant lookup: {error}"))?;
        let rows = statement
            .query_map(params_from_iter(values.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|error| format!("cannot read FAVOR variant coordinates: {error}"))?;
        for row in rows {
            let (allele_id, chromosome, position, reference, alternate) =
                row.map_err(|error| error.to_string())?;
            let chromosome = normalized_chromosome(&chromosome);
            if position < 1 {
                return Err("the AnnoCAT result contains an invalid variant position".into());
            }
            found.insert(
                allele_id.clone(),
                Coordinate {
                    allele_id,
                    reference: format!(
                        "{chromosome}-{position}-{}-{}",
                        reference.to_ascii_uppercase(),
                        alternate.to_ascii_uppercase()
                    ),
                },
            );
        }
    }
    allele_ids
        .iter()
        .map(|allele_id| {
            found.remove(allele_id).ok_or_else(|| {
                "one or more variants do not belong to this AnnoCAT result".to_string()
            })
        })
        .collect()
}

fn favor_chromosome(value: &str) -> Result<String, String> {
    let chromosome = normalized_chromosome(value);
    let valid = matches!(chromosome.as_str(), "X" | "Y")
        || chromosome
            .parse::<u8>()
            .is_ok_and(|number| (1..=22).contains(&number));
    valid
        .then_some(chromosome)
        .ok_or_else(|| "FAVOR supports primary GRCh38 chromosomes only".into())
}

fn normalized_chromosome(value: &str) -> String {
    match value.strip_prefix("chr").unwrap_or(value) {
        "M" => "MT".into(),
        chromosome => chromosome.into(),
    }
}

fn valid_favor_reference(reference: &str) -> bool {
    let mut parts = reference.split('-');
    let chromosome = parts.next().unwrap_or_default();
    let position = parts.next().unwrap_or_default();
    let reference_allele = parts.next().unwrap_or_default();
    let alternate = parts.next().unwrap_or_default();
    parts.next().is_none()
        && favor_chromosome(chromosome).is_ok_and(|normalized| normalized == chromosome)
        && position.parse::<u64>().is_ok_and(|value| value > 0)
        && valid_sequence(reference_allele)
        && valid_sequence(alternate)
}

fn valid_sequence(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 10_000
        && value
            .bytes()
            .all(|byte| matches!(byte, b'A' | b'C' | b'G' | b'T' | b'N'))
}

fn call_standard_api(
    endpoint: &str,
    timeout_seconds: u64,
    references: &[String],
) -> Result<ApiResponse, String> {
    post_api(
        endpoint,
        timeout_seconds,
        &ApiRequest {
            references,
            depth: "standard",
        },
    )
}

fn call_coding_api(
    endpoint: &str,
    timeout_seconds: u64,
    references: &[String],
) -> Result<CodingApiResponse, String> {
    post_api(endpoint, timeout_seconds, &CodingApiRequest { references })
}

fn post_api<T: DeserializeOwned>(
    endpoint: &str,
    timeout_seconds: u64,
    request: &impl Serialize,
) -> Result<T, String> {
    let client = super::http_client::source()?;
    let body = serde_json::to_vec(request)
        .map_err(|error| format!("cannot serialize FAVOR request: {error}"))?;
    let mut response = client
        .post(endpoint)
        .timeout(Duration::from_secs(timeout_seconds))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .map_err(|error| format!("cannot reach FAVOR: {error}"))?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("later");
        return Err(format!("FAVOR rate limit reached; retry after {retry}"));
    }
    if !response.status().is_success() {
        return Err(format!(
            "FAVOR returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_RESPONSE_BYTES)
    {
        return Err("FAVOR response exceeded the safety limit".into());
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read FAVOR response: {error}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("FAVOR response exceeded the safety limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid FAVOR response: {error}"))
}

fn validate_response(
    coordinates: &[Coordinate],
    response: ApiResponse,
) -> Result<Vec<StoredItem>, String> {
    if response.items.len() != coordinates.len() {
        return Err("FAVOR response did not preserve the requested variant count".into());
    }
    coordinates
        .iter()
        .zip(response.items)
        .map(|(coordinate, item)| {
            if item.reference != coordinate.reference {
                return Err("FAVOR response order or variant identity changed".into());
            }
            if !matches!(
                item.status.as_str(),
                "found" | "not_found" | "ambiguous" | "error"
            ) {
                return Err("FAVOR returned an unknown item status".into());
            }
            if item.status == "found" && item.variant.is_none() {
                return Err("FAVOR returned a found item without annotations".into());
            }
            Ok(StoredItem {
                allele_id: coordinate.allele_id.clone(),
                reference: coordinate.reference.clone(),
                status: item.status,
                error: item.error,
                variant: item.variant,
                coding: None,
            })
        })
        .collect()
}

fn merge_coding_response(
    coordinates: &[Coordinate],
    items: &mut [StoredItem],
    response: CodingApiResponse,
) -> Result<(), String> {
    if response.items.len() != coordinates.len() || items.len() != coordinates.len() {
        return Err("FAVOR coding response did not preserve the requested variant count".into());
    }
    for ((coordinate, stored), item) in coordinates.iter().zip(items).zip(response.items) {
        if item.reference != coordinate.reference || stored.reference != coordinate.reference {
            return Err("FAVOR coding response order or variant identity changed".into());
        }
        if !matches!(
            item.status.as_str(),
            "found" | "not_coding" | "not_found" | "ambiguous" | "error"
        ) {
            return Err("FAVOR returned an unknown coding item status".into());
        }
        if item.status == "found" && item.coding.is_none() {
            return Err("FAVOR returned a found coding item without annotations".into());
        }
        if item.status == "error" && stored.error.is_none() {
            stored.error = item.error;
        }
        stored.coding = (item.status == "found").then_some(item.coding).flatten();
    }
    Ok(())
}

fn publish_items(
    run_directory: &Path,
    canonical_evidence: &Path,
    canonical_catalog: &Path,
    items: &[StoredItem],
    fetched_at: &str,
    standard_endpoint: &str,
    coding_endpoint: &str,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE updated_alleles(allele_id VARCHAR PRIMARY KEY);
             CREATE TABLE evidence(
                schema_version INTEGER NOT NULL,
                allele_id VARCHAR NOT NULL,
                consequence_id VARCHAR,
                scope VARCHAR NOT NULL,
                source_id VARCHAR NOT NULL,
                field_path VARCHAR NOT NULL,
                value_type VARCHAR NOT NULL,
                string_value VARCHAR,
                integer_value BIGINT,
                number_value DOUBLE,
                boolean_value BOOLEAN,
                json_value VARCHAR
             );
             CREATE TABLE statuses(
                schema_version INTEGER NOT NULL,
                allele_id VARCHAR NOT NULL,
                reference VARCHAR NOT NULL,
                status VARCHAR NOT NULL,
                fetched_at VARCHAR NOT NULL,
                error VARCHAR
             );",
        )
        .map_err(|error| format!("cannot initialize FAVOR evidence tables: {error}"))?;
    {
        let mut appender = connection
            .appender("updated_alleles")
            .map_err(|error| format!("cannot update FAVOR alleles: {error}"))?;
        for item in items {
            appender
                .append_row([item.allele_id.as_str()])
                .map_err(|error| format!("cannot append FAVOR allele: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot flush FAVOR alleles: {error}"))?;
    }
    let evidence_path = run_directory.join(EVIDENCE_FILE);
    if evidence_path.is_file() {
        connection
            .execute(
                "INSERT INTO evidence
                 SELECT * FROM read_parquet(?)
                 WHERE allele_id NOT IN (SELECT allele_id FROM updated_alleles)",
                params![evidence_path.to_string_lossy().as_ref()],
            )
            .map_err(|error| format!("cannot merge existing FAVOR evidence: {error}"))?;
    }
    let status_path = run_directory.join(STATUS_FILE);
    if status_path.is_file() {
        connection
            .execute(
                "INSERT INTO statuses
                 SELECT * FROM read_parquet(?)
                 WHERE allele_id NOT IN (SELECT allele_id FROM updated_alleles)",
                params![status_path.to_string_lossy().as_ref()],
            )
            .map_err(|error| format!("cannot merge existing FAVOR statuses: {error}"))?;
    }
    append_evidence(&connection, items)?;
    remove_unsubstantiated_protein_apc(&connection)?;
    append_statuses(&connection, items, fetched_at)?;
    let occurrences = field_occurrences(&connection)?;
    let favor_catalog = catalog_json(&occurrences);
    let favor_catalog_path = run_directory.join(FIELD_CATALOG_FILE);
    super::library_metadata::atomic_write(
        &favor_catalog_path,
        &serde_json::to_vec_pretty(&favor_catalog).map_err(|error| error.to_string())?,
    )?;
    publish_table(&connection, "evidence", &evidence_path)?;
    publish_table(&connection, "statuses", &status_path)?;
    let provenance = json!({
        "schemaVersion": 2,
        "sourceId": SOURCE_ID,
        "serviceId": SERVICE_ID,
        "provider": "FAVOR",
        "assembly": "GRCh38",
        "requestDepth": "standard",
        "codingContract": "dbNSFP 5.3.1a (FAVOR live API schema)",
        "codingPredictorVersions": {
            "bayesDel": "v1",
            "cadd": "v1.7",
            "mpc": "release 1",
            "mutPred2": "MutPred2",
            "polyphen2": "v2.2.2",
            "revel": "May 3, 2021 release",
            "sift": "Ensembl 66, January 2015",
            "vest": "v4.0"
        },
        "releasePolicy": "rolling",
        "endpoints": {
            "standard": standard_endpoint,
            "coding": coding_endpoint
        },
        "catalogReference": "https://favor-beta.genohub.org/docs/data",
        "versionNote": "The FAVOR coding API and data catalog identify different dbNSFP releases. AnnoCAT uses the interpretation from each source. It does not assume that the calibrated releases are equivalent.",
        "latestFetch": fetched_at
    });
    super::library_metadata::atomic_write(
        &run_directory.join(PROVENANCE_FILE),
        &serde_json::to_vec_pretty(&provenance).map_err(|error| error.to_string())?,
    )?;
    prepare_query_assets(canonical_evidence, canonical_catalog)
}

fn append_evidence(connection: &Connection, items: &[StoredItem]) -> Result<(), String> {
    let mut appender = connection
        .appender("evidence")
        .map_err(|error| format!("cannot append FAVOR evidence: {error}"))?;
    for item in items {
        if let Some(variant) = &item.variant {
            let strings = [
                ("rsid", variant.rsid.as_deref()),
                ("vcf", variant.vcf.as_deref()),
                ("gene", variant.gene.as_deref()),
                ("consequence", variant.consequence.as_deref()),
                (
                    "clinicalSignificance",
                    variant.clinical_significance.as_deref(),
                ),
                ("siftCat", variant.sift_cat.as_deref()),
                ("polyphenCat", variant.polyphen_cat.as_deref()),
                ("metasvmPred", variant.metasvm_pred.as_deref()),
            ];
            for (field, value) in strings {
                let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                    continue;
                };
                append_evidence_row(
                    &mut appender,
                    &item.allele_id,
                    field,
                    "string",
                    Some(value),
                    None,
                )?;
            }
            let numbers = [
                ("caddPhred", variant.cadd_phred),
                ("revel", variant.revel),
                ("alphaMissense", variant.alpha_missense),
                ("spliceaiDsMax", variant.spliceai_ds_max),
                ("gnomadAf", variant.gnomad_af),
                ("bravoAf", variant.bravo_af),
                ("tgAll", variant.tg_all),
                ("apcConservation", variant.apc_conservation),
                ("apcEpigenetics", variant.apc_epigenetics),
                ("apcProteinFunction", protein_function_apc(variant)),
            ];
            for (field, value) in numbers {
                let Some(value) = value.filter(|value| value.is_finite()) else {
                    continue;
                };
                append_evidence_row(
                    &mut appender,
                    &item.allele_id,
                    field,
                    "number",
                    None,
                    Some(value),
                )?;
            }
        }
        if let Some(coding) = &item.coding {
            append_coding_evidence(&mut appender, &item.allele_id, coding)?;
        }
    }
    appender
        .flush()
        .map_err(|error| format!("cannot flush FAVOR evidence: {error}"))
}

fn protein_function_apc(variant: &ApiVariant) -> Option<f64> {
    [
        variant.sift_cat.as_deref(),
        variant.polyphen_cat.as_deref(),
        variant.metasvm_pred.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !coding_value_is_missing(value))
    .then_some(variant.apc_protein_function)
    .flatten()
}

fn remove_unsubstantiated_protein_apc(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM evidence
             WHERE source_id=? AND field_path='apcProteinFunction'
               AND allele_id NOT IN (
                 SELECT allele_id FROM evidence
                 WHERE source_id=? AND field_path IN ('siftCat', 'polyphenCat', 'metasvmPred')
               )",
            params![SOURCE_ID, SOURCE_ID],
        )
        .map(|_| ())
        .map_err(|error| format!("cannot apply FAVOR protein-function evidence scope: {error}"))
}

fn append_coding_evidence(
    appender: &mut duckdb::Appender<'_>,
    allele_id: &str,
    coding: &Value,
) -> Result<(), String> {
    let mane = coding_string(coding, &["/dbnsfp/annotation/mane"]);
    let canonical = coding_string(coding, &["/dbnsfp/annotation/vep_canonical"]);
    let basis = if mane.is_some_and(|value| value.eq_ignore_ascii_case("select")) {
        "MANE Select"
    } else if mane.is_some_and(|value| value.to_ascii_lowercase().contains("clinical")) {
        "MANE Plus Clinical"
    } else if canonical.is_some_and(|value| value.eq_ignore_ascii_case("yes")) {
        "Canonical transcript"
    } else {
        "FAVOR coding record"
    };
    let strings = [
        ("codingTranscriptBasis", Some(basis)),
        (
            "codingGene",
            coding_string(coding, &["/dbnsfp/annotation/genename"]),
        ),
        (
            "codingHgvsp",
            coding_string(coding, &["/dbnsfp/annotation/hgvsp_vep"]),
        ),
        (
            "codingAlphaMissensePred",
            coding_string(coding, &["/dbnsfp/alphamissense/pred"]),
        ),
        (
            "codingSiftPred",
            coding_string(coding, &["/dbnsfp/sift/pred"]),
        ),
        (
            "codingPolyphen2HvarPred",
            coding_string(coding, &["/dbnsfp/polyphen2_hvar/pred"]),
        ),
        (
            "codingMetaSvmPred",
            coding_string(coding, &["/dbnsfp/metasvm/pred"]),
        ),
        (
            "codingPrimateAiPred",
            coding_string(coding, &["/dbnsfp/primate_ai/pred"]),
        ),
        (
            "codingEsm1bPred",
            coding_string(coding, &["/dbnsfp/esm1b/pred"]),
        ),
        (
            "codingMutPred2Pred",
            coding_string(coding, &["/dbnsfp/mutpred2/pred"]),
        ),
    ];
    for (field, value) in strings {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        append_evidence_row(appender, allele_id, field, "string", Some(value), None)?;
    }
    let numbers = [
        (
            "codingCaddPhred",
            coding_number(coding, &["/dbnsfp/cadd/phred"]),
        ),
        (
            "codingRevelScore",
            coding_number(coding, &["/dbnsfp/revel/score"]),
        ),
        (
            "codingAlphaMissenseScore",
            coding_number(coding, &["/dbnsfp/alphamissense/score"]),
        ),
        (
            "codingSiftScore",
            coding_number(coding, &["/dbnsfp/sift/score"]),
        ),
        (
            "codingPolyphen2HvarScore",
            coding_number(coding, &["/dbnsfp/polyphen2_hvar/score"]),
        ),
        (
            "codingMetaSvmScore",
            coding_number(coding, &["/dbnsfp/metasvm/score"]),
        ),
        (
            "codingBayesDelNoAfScore",
            coding_number(coding, &["/dbnsfp/bayesdel_noaf/score"]),
        ),
        (
            "codingVest4Score",
            coding_number(coding, &["/dbnsfp/vest4/score"]),
        ),
        (
            "codingMutPred2Score",
            coding_number(coding, &["/dbnsfp/mutpred2/score"]),
        ),
        (
            "codingPrimateAiScore",
            coding_number(coding, &["/dbnsfp/primate_ai/score"]),
        ),
        (
            "codingMpcScore",
            coding_number(coding, &["/dbnsfp/mpc/score"]),
        ),
        (
            "codingVarityRScore",
            coding_number(coding, &["/dbnsfp/varity_r/score"]),
        ),
        (
            "codingEsm1bScore",
            coding_number(coding, &["/dbnsfp/esm1b/score"]),
        ),
        (
            "codingGerpRs",
            coding_number(coding, &["/dbnsfp/conservation/gerp_rs"]),
        ),
        (
            "codingPhyloP100way",
            coding_number(
                coding,
                &[
                    "/dbnsfp/conservation/phylop_100v",
                    "/dbnsfp/conservation/phylop100way_vertebrate",
                ],
            ),
        ),
    ];
    for (field, value) in numbers {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            continue;
        };
        append_evidence_row(appender, allele_id, field, "number", None, Some(value))?;
    }
    Ok(())
}

fn coding_string<'a>(coding: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers.iter().find_map(|pointer| {
        let value = coding.pointer(pointer)?;
        if let Some(value) = value.as_str() {
            return (!coding_value_is_missing(value)).then_some(value);
        }
        let mut selected = None;
        for value in value.as_array()? {
            if value.is_null() || value.as_str().is_some_and(coding_value_is_missing) {
                continue;
            }
            let value = value.as_str()?;
            if selected.is_some_and(|selected| selected != value) {
                return None;
            }
            selected = Some(value);
        }
        selected
    })
}

fn coding_number(coding: &Value, pointers: &[&str]) -> Option<f64> {
    pointers.iter().find_map(|pointer| {
        let value = coding.pointer(pointer)?;
        let parse = |value: &Value| -> Option<f64> {
            value.as_f64().or_else(|| {
                value.as_str().and_then(|value| {
                    (!coding_value_is_missing(value))
                        .then(|| value.parse().ok())
                        .flatten()
                })
            })
        };
        if let Some(value) = parse(value) {
            return Some(value);
        }
        let mut selected = None;
        for value in value.as_array()? {
            if value.is_null() || value.as_str().is_some_and(coding_value_is_missing) {
                continue;
            }
            let value = parse(value)?;
            if selected.is_some_and(|selected| selected != value) {
                return None;
            }
            selected = Some(value);
        }
        selected
    })
}

fn coding_value_is_missing(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "" | "." | "-" | "NA" | "N/A" | "NONE" | "NULL"
    )
}

fn append_evidence_row(
    appender: &mut duckdb::Appender<'_>,
    allele_id: &str,
    field: &str,
    value_type: &str,
    string_value: Option<&str>,
    number_value: Option<f64>,
) -> Result<(), String> {
    let values = vec![
        SqlValue::Int(annocat_core::RESULT_SCHEMA_VERSION),
        allele_id.to_owned().into(),
        SqlValue::Null,
        field_physical_scope(field).to_owned().into(),
        SOURCE_ID.to_owned().into(),
        field.to_owned().into(),
        value_type.to_owned().into(),
        string_value.map_or(SqlValue::Null, |value| value.to_owned().into()),
        SqlValue::Null,
        number_value.map_or(SqlValue::Null, SqlValue::Double),
        SqlValue::Null,
        SqlValue::Null,
    ];
    appender
        .append_row(appender_params_from_iter(values))
        .map_err(|error| format!("cannot append FAVOR field {field}: {error}"))
}

fn append_statuses(
    connection: &Connection,
    items: &[StoredItem],
    fetched_at: &str,
) -> Result<(), String> {
    let mut appender = connection
        .appender("statuses")
        .map_err(|error| format!("cannot append FAVOR statuses: {error}"))?;
    for item in items {
        let values = vec![
            SqlValue::Int(1),
            item.allele_id.clone().into(),
            item.reference.clone().into(),
            item.status.clone().into(),
            fetched_at.to_owned().into(),
            item.error.clone().map_or(SqlValue::Null, Into::into),
        ];
        appender
            .append_row(appender_params_from_iter(values))
            .map_err(|error| format!("cannot append FAVOR status: {error}"))?;
    }
    appender
        .flush()
        .map_err(|error| format!("cannot flush FAVOR statuses: {error}"))
}

fn field_occurrences(connection: &Connection) -> Result<BTreeMap<String, u64>, String> {
    let mut statement = connection
        .prepare("SELECT field_path, count(*) FROM evidence GROUP BY field_path")
        .map_err(|error| format!("cannot count FAVOR fields: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| format!("cannot read FAVOR field counts: {error}"))?;
    rows.map(|row| {
        let (field, count) = row.map_err(|error| error.to_string())?;
        Ok((field, count.max(0) as u64))
    })
    .collect()
}

fn field_physical_scope(field_path: &str) -> &'static str {
    match field_path {
        "codingCaddPhred" | "codingGerpRs" | "codingPhyloP100way" => "allele",
        value if value.starts_with("coding") => "selected",
        _ => "allele",
    }
}

fn provider_selected_standard_field(field_path: &str) -> bool {
    matches!(
        field_path,
        "gene"
            | "consequence"
            | "revel"
            | "alphaMissense"
            | "siftCat"
            | "polyphenCat"
            | "metasvmPred"
    )
}

fn field_biological_scope(field_path: &str) -> &'static str {
    (field_physical_scope(field_path) == "selected" || provider_selected_standard_field(field_path))
        .then_some("feature")
        .unwrap_or("allele")
}

fn field_resolution_policy(field_path: &str) -> &'static str {
    if provider_selected_standard_field(field_path) {
        "providerSelected"
    } else if field_physical_scope(field_path) == "selected" {
        "materializedSelected"
    } else {
        "direct"
    }
}

fn catalog_json(occurrences: &BTreeMap<String, u64>) -> Value {
    let fields = FIELDS
        .iter()
        .filter_map(|field| {
            let count = occurrences.get(field.path).copied().unwrap_or(0);
            (count > 0).then(|| {
                let mut value = json!({
                    "scope": field_biological_scope(field.path),
                    "biologicalScope": field_biological_scope(field.path),
                    "physicalScope": field_physical_scope(field.path),
                    "sourceId": SOURCE_ID,
                    "fieldPath": field.path,
                    "valueType": field.value_type,
                    "observedTypes": [field.value_type],
                    "occurrences": count,
                    "storageEncoding": "scalar",
                    "resolutionPolicy": field_resolution_policy(field.path)
                });
                if field_physical_scope(field.path) == "selected"
                    || provider_selected_standard_field(field.path)
                {
                    value["selectionOrigin"] = Value::String("provider".into());
                }
                value
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schemaVersion": annocat_core::RESULT_SCHEMA_VERSION,
        "fields": fields,
        "alignmentGroups": []
    })
}

fn publish_table(connection: &Connection, table: &str, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension("parquet.partial");
    let _ = fs::remove_file(&temporary);
    connection
        .execute_batch(&format!(
            "COPY {table} TO '{}' (FORMAT PARQUET, COMPRESSION ZSTD)",
            sql_path(&temporary)
        ))
        .map_err(|error| format!("cannot write {}: {error}", destination.display()))?;
    let schema_query = match table {
        "evidence" => {
            "SELECT schema_version, allele_id, consequence_id, scope, source_id, field_path,
                    value_type, string_value, integer_value, number_value, boolean_value,
                    json_value FROM read_parquet(?) LIMIT 0"
        }
        "statuses" => {
            "SELECT schema_version, allele_id, reference, status, fetched_at, error
             FROM read_parquet(?) LIMIT 0"
        }
        _ => return Err("cannot publish an unknown FAVOR table".into()),
    };
    if let Err(error) = connection
        .prepare(schema_query)
        .and_then(|mut statement| statement.exists(params![temporary.to_string_lossy().as_ref()]))
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot validate {}: {error}",
            destination.display()
        ));
    }
    if let Err(error) = super::library_metadata::publish_atomic_file(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn publish_hard_link(source: &Path, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension("parquet.partial");
    let _ = fs::remove_file(&temporary);
    fs::hard_link(source, &temporary).map_err(|error| {
        format!(
            "cannot link FAVOR query evidence {}: {error}",
            source.display()
        )
    })?;
    if let Err(error) = super::library_metadata::publish_atomic_file(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn merge_query_catalogs(
    canonical: &Path,
    favor: Option<&Path>,
    gene: Option<&Path>,
    destination: &Path,
) -> Result<(), String> {
    let mut canonical_value: Value = serde_json::from_slice(
        &fs::read(canonical)
            .map_err(|error| format!("cannot read canonical field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid canonical field catalog: {error}"))?;
    let canonical_fields = canonical_value["fields"]
        .as_array_mut()
        .ok_or("canonical field catalog has no fields")?;
    canonical_fields.retain(|field| field["sourceId"] != SOURCE_ID);
    canonical_fields
        .retain(|field| field["sourceId"] != "hpo" && field["storageRelation"] != "geneEvidence");
    if let Some(favor) = favor {
        let favor_value: Value = serde_json::from_slice(
            &fs::read(favor)
                .map_err(|error| format!("cannot read FAVOR field catalog: {error}"))?,
        )
        .map_err(|error| format!("invalid FAVOR field catalog: {error}"))?;
        canonical_fields.extend(
            favor_value["fields"]
                .as_array()
                .ok_or("FAVOR field catalog has no fields")?
                .iter()
                .cloned(),
        );
    }
    if let Some(gene) = gene {
        let gene_value: Value = serde_json::from_slice(
            &fs::read(gene)
                .map_err(|error| format!("cannot read phenotype field catalog: {error}"))?,
        )
        .map_err(|error| format!("invalid phenotype field catalog: {error}"))?;
        canonical_fields.extend(
            gene_value["fields"]
                .as_array()
                .ok_or("phenotype field catalog has no fields")?
                .iter()
                .cloned(),
        );
        canonical_value["geneEvidenceFile"] = Value::String(QUERY_GENE_EVIDENCE_FILE.into());
    } else if let Some(object) = canonical_value.as_object_mut() {
        object.remove("geneEvidenceFile");
    }
    super::library_metadata::atomic_write(
        destination,
        &serde_json::to_vec_pretty(&canonical_value).map_err(|error| error.to_string())?,
    )
}

fn sql_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[derive(Default)]
struct StatusCounts {
    total: u64,
    found: u64,
    not_found: u64,
    ambiguous: u64,
    errors: u64,
    latest_fetch: Option<String>,
}

fn status_counts(path: &Path) -> Result<StatusCounts, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT count(*),
                    count(*) FILTER (WHERE status='found'),
                    count(*) FILTER (WHERE status='not_found'),
                    count(*) FILTER (WHERE status='ambiguous'),
                    count(*) FILTER (WHERE status='error'),
                    max(fetched_at)
             FROM read_parquet(?)",
            params![path.to_string_lossy().as_ref()],
            |row| {
                Ok(StatusCounts {
                    total: row.get::<_, i64>(0)?.max(0) as u64,
                    found: row.get::<_, i64>(1)?.max(0) as u64,
                    not_found: row.get::<_, i64>(2)?.max(0) as u64,
                    ambiguous: row.get::<_, i64>(3)?.max(0) as u64,
                    errors: row.get::<_, i64>(4)?.max(0) as u64,
                    latest_fetch: row.get(5)?,
                })
            },
        )
        .map_err(|error| format!("cannot read FAVOR status: {error}"))
}

fn coding_evidence_count(path: &Path) -> Result<u64, String> {
    if !path.is_file() {
        return Ok(0);
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT count(DISTINCT allele_id)
             FROM read_parquet(?)
             WHERE source_id=? AND field_path='codingTranscriptBasis'",
            params![path.to_string_lossy().as_ref(), SOURCE_ID],
            |row| Ok(row.get::<_, i64>(0)?.max(0) as u64),
        )
        .map_err(|error| format!("cannot count FAVOR coding evidence: {error}"))
}

fn summary(
    run_directory: &Path,
    requested: usize,
    fetched_at: &str,
) -> Result<EnrichSummary, String> {
    let counts = status_counts(&run_directory.join(STATUS_FILE))?;
    let coding_found = coding_evidence_count(&run_directory.join(EVIDENCE_FILE))?;
    Ok(EnrichSummary {
        requested,
        found: counts.found,
        not_found: counts.not_found,
        ambiguous: counts.ambiguous,
        errors: counts.errors,
        coding_found,
        total_cached: counts.total,
        latest_fetch: fetched_at.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_favor_references_are_bounded_sequences() {
        assert!(valid_favor_reference("19-44908822-C-T"));
        assert!(valid_favor_reference("22-17008105-G-A"));
        assert!(!valid_favor_reference("22-17008105-G-<DEL>"));
        assert!(!valid_favor_reference("MT-100-A-G"));
        assert!(!valid_favor_reference("GL000220.1-1-A-G"));
    }

    #[test]
    fn response_identity_and_order_are_required() {
        let coordinates = vec![Coordinate {
            allele_id: "allele-one".into(),
            reference: "19-44908822-C-T".into(),
        }];
        let response: ApiResponse = serde_json::from_value(json!({
            "items": [{
                "reference": "19-44908822-C-T",
                "status": "found",
                "variant": {"rsid": "rs7412", "caddPhred": 25.3}
            }],
            "summary": {"found": 1}
        }))
        .unwrap();
        let items = validate_response(&coordinates, response).unwrap();
        assert_eq!(
            items[0].variant.as_ref().unwrap().rsid.as_deref(),
            Some("rs7412")
        );
    }

    #[test]
    fn protein_function_apc_requires_a_reported_protein_predictor() {
        let imputed: ApiVariant = serde_json::from_value(json!({
            "apcProteinFunction": 20.24944305419922
        }))
        .unwrap();
        assert_eq!(protein_function_apc(&imputed), None);

        let supported: ApiVariant = serde_json::from_value(json!({
            "apcProteinFunction": 23.43232536315918,
            "polyphenCat": "possibly_damaging"
        }))
        .unwrap();
        assert_eq!(protein_function_apc(&supported), Some(23.43232536315918));
    }

    #[test]
    fn cached_imputed_protein_apc_is_removed_without_predictor_support() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE evidence(allele_id VARCHAR, source_id VARCHAR, field_path VARCHAR);
                 INSERT INTO evidence VALUES
                   ('allele-imputed', 'favor-online', 'apcProteinFunction'),
                   ('allele-supported', 'favor-online', 'apcProteinFunction'),
                   ('allele-supported', 'favor-online', 'polyphenCat');",
            )
            .unwrap();
        remove_unsubstantiated_protein_apc(&connection).unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT count(*) FROM evidence WHERE field_path='apcProteinFunction'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn coding_response_is_merged_without_synthesizing_missing_scores() {
        let coordinates = vec![
            Coordinate {
                allele_id: "allele-one".into(),
                reference: "19-44908822-C-T".into(),
            },
            Coordinate {
                allele_id: "allele-two".into(),
                reference: "22-17008105-G-A".into(),
            },
        ];
        let standard: ApiResponse = serde_json::from_value(json!({
            "items": [
                {"reference": "19-44908822-C-T", "status": "found", "variant": {"rsid": "rs7412"}},
                {"reference": "22-17008105-G-A", "status": "found", "variant": {"rsid": "rs-test"}}
            ]
        }))
        .unwrap();
        let coding: CodingApiResponse = serde_json::from_value(json!({
            "items": [
                {
                    "reference": "19-44908822-C-T",
                    "status": "found",
                    "coding": {
                        "dbnsfp": {
                            "annotation": {"mane": "Select", "hgvsp_vep": "p.Arg176Cys"},
                            "revel": {"score": 0.42},
                            "cadd": {"phred": 25.1},
                            "mutpred2": {"score": 0.465, "pred": "UC"},
                            "conservation": {"phylop_100v": 0.906}
                        }
                    }
                },
                {"reference": "22-17008105-G-A", "status": "not_coding", "coding": null}
            ]
        }))
        .unwrap();
        let mut items = validate_response(&coordinates, standard).unwrap();
        merge_coding_response(&coordinates, &mut items, coding).unwrap();
        assert_eq!(
            coding_number(items[0].coding.as_ref().unwrap(), &["/dbnsfp/revel/score"]),
            Some(0.42)
        );
        assert_eq!(
            coding_number(items[0].coding.as_ref().unwrap(), &["/dbnsfp/cadd/phred"]),
            Some(25.1)
        );
        assert_eq!(
            coding_number(
                items[0].coding.as_ref().unwrap(),
                &["/dbnsfp/mutpred2/score"]
            ),
            Some(0.465)
        );
        assert_eq!(
            coding_string(
                items[0].coding.as_ref().unwrap(),
                &["/dbnsfp/mutpred2/pred"]
            ),
            Some("UC")
        );
        assert_eq!(
            coding_number(
                items[0].coding.as_ref().unwrap(),
                &["/dbnsfp/conservation/phylop_100v"]
            ),
            Some(0.906)
        );
        assert!(items[1].coding.is_none());
    }

    #[test]
    fn catalog_distinguishes_position_and_source_selected_coding_fields() {
        let occurrences = BTreeMap::from([
            ("gnomadAf".to_owned(), 1),
            ("revel".to_owned(), 1),
            ("codingRevelScore".to_owned(), 1),
            ("codingCaddPhred".to_owned(), 1),
            ("codingGerpRs".to_owned(), 1),
            ("codingPhyloP100way".to_owned(), 1),
        ]);
        let catalog = catalog_json(&occurrences);
        let fields = catalog["fields"].as_array().unwrap();
        let field = |path: &str| {
            fields
                .iter()
                .find(|field| field["fieldPath"] == path)
                .unwrap()["resolutionPolicy"]
                .clone()
        };
        assert_eq!(field("gnomadAf"), "direct");
        assert_eq!(field("revel"), "providerSelected");
        assert_eq!(field("codingRevelScore"), "materializedSelected");
        assert_eq!(field("codingCaddPhred"), "direct");
        assert_eq!(field("codingGerpRs"), "direct");
        assert_eq!(field("codingPhyloP100way"), "direct");
        let revel = fields
            .iter()
            .find(|field| field["fieldPath"] == "codingRevelScore")
            .unwrap();
        assert_eq!(revel["scope"], "feature");
        assert_eq!(revel["physicalScope"], "selected");
        assert_eq!(revel["selectionOrigin"], "provider");
        assert_eq!(field_physical_scope("codingRevelScore"), "selected");
        assert_eq!(field_physical_scope("codingCaddPhred"), "allele");
        let standard_revel = fields
            .iter()
            .find(|field| field["fieldPath"] == "revel")
            .unwrap();
        assert_eq!(standard_revel["biologicalScope"], "feature");
        assert_eq!(standard_revel["physicalScope"], "allele");
        assert_eq!(standard_revel["selectionOrigin"], "provider");
    }

    #[test]
    fn coding_arrays_must_have_one_unambiguous_value() {
        let coding = json!({
            "sameString": ["D", "D"],
            "mixedString": ["D", "T"],
            "missingString": [null, ".", "D"],
            "malformedString": ["D", 4],
            "sameNumber": [0.42, "0.42"],
            "mixedNumber": [0.42, 0.17],
            "missingNumber": [null, "NA", 0.42],
            "malformedNumber": [0.42, "not-a-number"]
        });
        assert_eq!(coding_string(&coding, &["/sameString"]), Some("D"));
        assert_eq!(coding_string(&coding, &["/mixedString"]), None);
        assert_eq!(coding_string(&coding, &["/missingString"]), Some("D"));
        assert_eq!(coding_string(&coding, &["/malformedString"]), None);
        assert_eq!(coding_number(&coding, &["/sameNumber"]), Some(0.42));
        assert_eq!(coding_number(&coding, &["/mixedNumber"]), None);
        assert_eq!(coding_number(&coding, &["/missingNumber"]), Some(0.42));
        assert_eq!(coding_number(&coding, &["/malformedNumber"]), None);
    }

    #[test]
    fn duplicate_allele_ids_are_collapsed() {
        let mut request = EnrichRequest {
            allele_ids: vec!["allele-one".into(), "allele-one".into()],
            consent: true,
        };
        normalize_request(&mut request).unwrap();
        assert_eq!(request.allele_ids, ["allele-one"]);
    }

    #[test]
    fn invalid_allele_ids_are_rejected() {
        let mut request = EnrichRequest {
            allele_ids: vec!["not an allele".into()],
            consent: true,
        };
        assert!(normalize_request(&mut request).is_err());
    }

    #[test]
    fn active_phenotype_catalog_replaces_legacy_hpo_fields() {
        let root =
            std::env::temp_dir().join(format!("annocat-phenotype-catalog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let canonical = root.join("canonical.json");
        let phenotype = root.join("phenotype.json");
        let merged = root.join("merged.json");
        fs::write(
            &canonical,
            serde_json::to_vec(&json!({
                "fields": [
                    {"sourceId": "clinvar", "fieldPath": "significance"},
                    {"sourceId": "hpo", "fieldPath": "phenotypeRelevance"},
                    {"sourceId": "hpo", "fieldPath": "legacyConditionMatches"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &phenotype,
            serde_json::to_vec(&json!({
                "fields": [
                    {"sourceId": "hpo", "fieldPath": "phenotypeRelevance", "storageRelation": "geneEvidence"},
                    {"sourceId": "hpo", "fieldPath": "selectedConditionMatches", "storageRelation": "geneEvidence"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        merge_query_catalogs(&canonical, None, Some(&phenotype), &merged).unwrap();

        let catalog: Value = serde_json::from_slice(&fs::read(&merged).unwrap()).unwrap();
        let fields = catalog["fields"].as_array().unwrap();
        assert_eq!(
            fields
                .iter()
                .filter(|field| field["sourceId"] == "hpo")
                .count(),
            2
        );
        assert!(
            fields
                .iter()
                .all(|field| field["fieldPath"] != "legacyConditionMatches")
        );
        assert_eq!(catalog["geneEvidenceFile"], QUERY_GENE_EVIDENCE_FILE);
        fs::remove_dir_all(root).unwrap();
    }
}
