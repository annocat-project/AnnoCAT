use duckdb::types::Value as SqlValue;
use duckdb::{Connection, appender_params_from_iter, params, params_from_iter};
use reqwest::StatusCode;
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

#[derive(Debug, Deserialize)]
struct ApiResponse {
    items: Vec<ApiItem>,
}

#[derive(Debug, Deserialize)]
struct ApiItem {
    reference: String,
    status: String,
    variant: Option<ApiVariant>,
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
];

pub fn enrich(
    run_directory: &Path,
    variants: &Path,
    canonical_evidence: &Path,
    canonical_catalog: &Path,
    request: EnrichRequest,
) -> Result<EnrichSummary, String> {
    validate_request(&request)?;
    validate_assembly(run_directory)?;
    let service = annocat_core::source_catalog::service(SERVICE_ID)
        .ok_or("FAVOR service configuration is missing")?;
    if request.allele_ids.len() > service.max_results {
        return Err(format!(
            "FAVOR enrichment is limited to {} variants per operation",
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
        )?;
    }
    for chunk in supported.chunks(REQUEST_CHUNK) {
        let references = chunk
            .iter()
            .map(|coordinate| coordinate.reference.clone())
            .collect::<Vec<_>>();
        let response = call_api(
            service.api_url.as_str(),
            service.timeout_seconds,
            &references,
        )?;
        let items = validate_response(chunk, response)?;
        publish_items(
            run_directory,
            canonical_evidence,
            canonical_catalog,
            &items,
            &fetched_at,
            service.api_url.as_str(),
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
            "latestFetch": null
        }));
    }
    let counts = status_counts(&status_path)?;
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
        "latestFetch": counts.latest_fetch
    }))
}

pub fn effective_evidence(canonical: &Path) -> PathBuf {
    let Some(run_directory) = canonical.parent() else {
        return canonical.to_path_buf();
    };
    let query = run_directory.join(QUERY_DIRECTORY);
    if query.join("canonical.parquet").is_file() && query.join("favor.parquet").is_file() {
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
    let run_directory = canonical_evidence
        .parent()
        .ok_or("completed evidence path has no parent")?;
    let favor_evidence = run_directory.join(EVIDENCE_FILE);
    let favor_catalog = run_directory.join(FIELD_CATALOG_FILE);
    if !favor_evidence.is_file() || !favor_catalog.is_file() {
        return Ok(());
    }
    let query_directory = run_directory.join(QUERY_DIRECTORY);
    fs::create_dir_all(&query_directory)
        .map_err(|error| format!("cannot create FAVOR query directory: {error}"))?;
    publish_hard_link(
        canonical_evidence,
        &query_directory.join("canonical.parquet"),
    )?;
    publish_hard_link(&favor_evidence, &query_directory.join("favor.parquet"))?;
    merge_catalogs(
        canonical_catalog,
        &favor_catalog,
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
        return Err("FAVOR enrichment files are incomplete".into());
    }
    Ok(files.to_vec())
}

fn validate_request(request: &EnrichRequest) -> Result<(), String> {
    if !request.consent {
        return Err(
            "FAVOR enrichment requires confirmation before variant coordinates are sent".into(),
        );
    }
    if request.allele_ids.is_empty() {
        return Err("select at least one variant for FAVOR enrichment".into());
    }
    let mut seen = HashSet::new();
    for allele_id in &request.allele_ids {
        if allele_id.len() > 64
            || !allele_id.starts_with("allele-")
            || !allele_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !seen.insert(allele_id)
        {
            return Err("FAVOR enrichment requires unique valid allele identifiers".into());
        }
    }
    Ok(())
}

fn validate_assembly(run_directory: &Path) -> Result<(), String> {
    let bytes = fs::read(run_directory.join("manifest.json"))
        .map_err(|error| format!("cannot read completed run manifest: {error}"))?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid completed run manifest: {error}"))?;
    if manifest["state"] != "completed" || manifest["assembly"] != "GRCh38" {
        return Err("FAVOR enrichment currently supports completed GRCh38 reports only".into());
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
            let chromosome = favor_chromosome(&chromosome)?;
            if position < 1 {
                return Err("report contains an invalid variant position".into());
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
            found
                .remove(allele_id)
                .ok_or_else(|| "one or more variants do not belong to this report".to_string())
        })
        .collect()
}

fn favor_chromosome(value: &str) -> Result<String, String> {
    let chromosome = value.strip_prefix("chr").unwrap_or(value);
    let chromosome = match chromosome {
        "M" => "MT",
        other => other,
    };
    let valid = matches!(chromosome, "X" | "Y" | "MT")
        || chromosome
            .parse::<u8>()
            .is_ok_and(|number| (1..=22).contains(&number));
    valid
        .then(|| chromosome.to_owned())
        .ok_or_else(|| "FAVOR enrichment supports primary GRCh38 chromosomes only".into())
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

fn call_api(
    endpoint: &str,
    timeout_seconds: u64,
    references: &[String],
) -> Result<ApiResponse, String> {
    let client = super::http_client::source()?;
    let body = serde_json::to_vec(&ApiRequest {
        references,
        depth: "standard",
    })
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
            })
        })
        .collect()
}

fn publish_items(
    run_directory: &Path,
    canonical_evidence: &Path,
    canonical_catalog: &Path,
    items: &[StoredItem],
    fetched_at: &str,
    endpoint: &str,
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
        "schemaVersion": 1,
        "sourceId": SOURCE_ID,
        "serviceId": SERVICE_ID,
        "provider": "FAVOR",
        "assembly": "GRCh38",
        "requestDepth": "standard",
        "releasePolicy": "rolling",
        "endpoint": endpoint,
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
        let Some(variant) = &item.variant else {
            continue;
        };
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
            ("apcProteinFunction", variant.apc_protein_function),
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
    appender
        .flush()
        .map_err(|error| format!("cannot flush FAVOR evidence: {error}"))
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
        "allele".to_owned().into(),
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

fn catalog_json(occurrences: &BTreeMap<String, u64>) -> Value {
    let fields = FIELDS
        .iter()
        .filter_map(|field| {
            let count = occurrences.get(field.path).copied().unwrap_or(0);
            (count > 0).then(|| {
                json!({
                    "scope": "allele",
                    "sourceId": SOURCE_ID,
                    "fieldPath": field.path,
                    "valueType": field.value_type,
                    "observedTypes": [field.value_type],
                    "occurrences": count
                })
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
    super::library_metadata::publish_atomic_file(&temporary, destination)
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

fn merge_catalogs(canonical: &Path, favor: &Path, destination: &Path) -> Result<(), String> {
    let mut canonical_value: Value = serde_json::from_slice(
        &fs::read(canonical)
            .map_err(|error| format!("cannot read canonical field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid canonical field catalog: {error}"))?;
    let favor_value: Value = serde_json::from_slice(
        &fs::read(favor).map_err(|error| format!("cannot read FAVOR field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid FAVOR field catalog: {error}"))?;
    let canonical_fields = canonical_value["fields"]
        .as_array_mut()
        .ok_or("canonical field catalog has no fields")?;
    canonical_fields.retain(|field| field["sourceId"] != SOURCE_ID);
    canonical_fields.extend(
        favor_value["fields"]
            .as_array()
            .ok_or("FAVOR field catalog has no fields")?
            .iter()
            .cloned(),
    );
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

fn summary(
    run_directory: &Path,
    requested: usize,
    fetched_at: &str,
) -> Result<EnrichSummary, String> {
    let counts = status_counts(&run_directory.join(STATUS_FILE))?;
    Ok(EnrichSummary {
        requested,
        found: counts.found,
        not_found: counts.not_found,
        ambiguous: counts.ambiguous,
        errors: counts.errors,
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
    fn duplicate_allele_ids_are_rejected() {
        let request = EnrichRequest {
            allele_ids: vec!["allele-one".into(), "allele-one".into()],
            consent: true,
        };
        assert!(validate_request(&request).is_err());
    }
}
