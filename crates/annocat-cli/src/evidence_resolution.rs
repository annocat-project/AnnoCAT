use duckdb::{Connection, params};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const CACHE_PREFIX: &str = ".annocat-evidence-";
const CACHE_VERSION: i32 = 1;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contract {
    groups: Vec<ContractGroup>,
    transcript_alignment: ContractAlignment,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractGroup {
    id: String,
    fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractAlignment {
    id: String,
    kind: String,
    source_id: String,
    scope: String,
    source_transcript_release: String,
    key_field: String,
    protein_field: String,
    canonical_field: String,
    separator: String,
    missing_values: Vec<String>,
    aligned_groups: Vec<String>,
    aligned_fields: Vec<String>,
    excluded_fields: Vec<String>,
}

#[derive(Clone, Debug)]
struct AlignmentSpec {
    id: String,
    kind: String,
    source_id: String,
    scope: String,
    source_transcript_release: String,
    key_field: String,
    protein_field: String,
    canonical_field: String,
    separator: String,
    missing_values: Vec<String>,
    fields: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestedResolutionKind {
    AlignedTranscriptVector,
    LegacyAllele,
    SelectedFeature,
}

#[derive(Clone, Debug)]
pub(crate) struct RequestedField {
    pub scope: String,
    pub biological_scope: String,
    pub source_id: String,
    pub field_path: String,
    pub kind: RequestedResolutionKind,
}

static BUNDLED_SPEC: OnceLock<Result<AlignmentSpec, String>> = OnceLock::new();
static BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bundled_spec() -> Result<AlignmentSpec, String> {
    BUNDLED_SPEC
        .get_or_init(|| {
            let contract: Contract = serde_json::from_str(include_str!(
                "../../../config/dbnsfp-4.9a-curated-fields.json"
            ))
            .map_err(|error| format!("invalid bundled evidence alignment contract: {error}"))?;
            let alignment = contract.transcript_alignment;
            if alignment.kind != "parallelTranscriptVector"
                || alignment.separator.is_empty()
                || alignment.separator.len() > 4
            {
                return Err("bundled evidence alignment contract is unsupported".into());
            }
            let aligned_groups = alignment
                .aligned_groups
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let excluded = alignment
                .excluded_fields
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let mut fields = contract
                .groups
                .iter()
                .filter(|group| aligned_groups.contains(group.id.as_str()))
                .flat_map(|group| group.fields.iter().cloned())
                .filter(|field| !excluded.contains(field.as_str()))
                .collect::<BTreeSet<_>>();
            fields.extend(alignment.aligned_fields.iter().cloned());
            if fields.is_empty() || !fields.contains(&alignment.key_field) {
                return Err("bundled evidence alignment fields are incomplete".into());
            }
            Ok(AlignmentSpec {
                id: alignment.id,
                kind: alignment.kind,
                source_id: alignment.source_id,
                scope: alignment.scope,
                source_transcript_release: alignment.source_transcript_release,
                key_field: alignment.key_field,
                protein_field: alignment.protein_field,
                canonical_field: alignment.canonical_field,
                separator: alignment.separator,
                missing_values: alignment.missing_values,
                fields,
            })
        })
        .clone()
}

pub(crate) fn bundled_alignment_group(
    scope: &str,
    source_id: &str,
    field_path: &str,
) -> Option<String> {
    let spec = bundled_spec().ok()?;
    (spec.scope == scope && spec.source_id == source_id && spec.fields.contains(field_path))
        .then_some(spec.id)
}

pub(crate) fn alignment_key_field(scope: &str, source_id: &str) -> Option<String> {
    let spec = bundled_spec().ok()?;
    (spec.scope == scope && spec.source_id == source_id).then_some(spec.key_field)
}

pub(crate) fn select_aligned_value(
    scope: &str,
    source_id: &str,
    field_path: &str,
    transcript_vector: &str,
    value_vector: &str,
    selected_transcript: &str,
) -> Option<String> {
    let spec = bundled_spec().ok()?;
    if spec.scope != scope || spec.source_id != source_id || !spec.fields.contains(field_path) {
        return None;
    }
    let transcripts = transcript_vector
        .split(&spec.separator)
        .map(str::trim)
        .collect::<Vec<_>>();
    let values = value_vector
        .split(&spec.separator)
        .map(str::trim)
        .collect::<Vec<_>>();
    if transcripts.len() != values.len() {
        return None;
    }
    let exact = transcripts
        .iter()
        .enumerate()
        .filter(|(_, transcript)| **transcript == selected_transcript)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let index = if exact.len() == 1 {
        exact[0]
    } else if transcripts
        .iter()
        .all(|transcript| !transcript.contains('.'))
    {
        let stable = stable_transcript_id(selected_transcript);
        let matches = transcripts
            .iter()
            .enumerate()
            .filter(|(_, transcript)| stable_transcript_id(transcript) == stable)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        *matches.first().filter(|_| matches.len() == 1)?
    } else {
        return None;
    };
    let value = values[index];
    (!spec.missing_values.iter().any(|missing| missing == value)).then(|| value.to_owned())
}

fn stable_transcript_id(value: &str) -> &str {
    value.split_once('.').map_or(value, |(stable, _)| stable)
}

pub(crate) fn catalog_alignment_groups(fields: &[Value]) -> Vec<Value> {
    let Ok(mut spec) = bundled_spec() else {
        return Vec::new();
    };
    let present = fields
        .iter()
        .filter_map(|field| {
            (field["scope"].as_str()? == spec.scope
                && field["sourceId"].as_str()? == spec.source_id)
                .then(|| field["fieldPath"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect::<HashSet<_>>();
    spec.fields.retain(|field| present.contains(field));
    if !present.contains(&spec.key_field) || spec.fields.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "id": spec.id,
        "kind": spec.kind,
        "scope": spec.scope,
        "sourceId": spec.source_id,
        "sourceTranscriptRelease": spec.source_transcript_release,
        "keyField": spec.key_field,
        "proteinField": spec.protein_field,
        "canonicalField": spec.canonical_field,
        "separator": spec.separator,
        "missingValues": spec.missing_values,
        "fields": spec.fields,
    })]
}

fn bounded_string(value: &Value, name: &str) -> Result<String, String> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("evidence alignment has no {name}"))?;
    if value.is_empty() || value.len() > 200 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(format!("evidence alignment {name} is invalid"));
    }
    Ok(value.to_owned())
}

fn catalog_specs(catalog: &Path) -> Result<Vec<AlignmentSpec>, String> {
    let metadata =
        fs::metadata(catalog).map_err(|error| format!("cannot inspect field catalog: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("field catalog has an invalid size".into());
    }
    let catalog: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid field catalog: {error}"))?;
    let present = catalog["fields"]
        .as_array()
        .ok_or("field catalog has no fields array")?
        .iter()
        .filter_map(|field| {
            Some((
                field["scope"].as_str()?.to_owned(),
                field["sourceId"].as_str()?.to_owned(),
                field["fieldPath"].as_str()?.to_owned(),
            ))
        })
        .collect::<HashSet<_>>();
    let Some(groups) = catalog["alignmentGroups"].as_array() else {
        let mut spec = bundled_spec()?;
        spec.fields.retain(|field| {
            present.contains(&(spec.scope.clone(), spec.source_id.clone(), field.clone()))
        });
        return Ok((present.contains(&(
            spec.scope.clone(),
            spec.source_id.clone(),
            spec.key_field.clone(),
        )) && !spec.fields.is_empty())
        .then_some(spec)
        .into_iter()
        .collect());
    };
    if groups.len() > 16 {
        return Err("field catalog has too many evidence alignment groups".into());
    }
    groups
        .iter()
        .map(|group| {
            let kind = bounded_string(&group["kind"], "kind")?;
            if kind != "parallelTranscriptVector" {
                return Err(format!("unsupported evidence alignment kind: {kind}"));
            }
            let scope = bounded_string(&group["scope"], "scope")?;
            let source_id = bounded_string(&group["sourceId"], "source ID")?;
            let separator = bounded_string(&group["separator"], "separator")?;
            if separator.len() > 4 {
                return Err("evidence alignment separator is too long".into());
            }
            let values = |name: &str, maximum: usize| -> Result<Vec<String>, String> {
                let values = group[name]
                    .as_array()
                    .ok_or_else(|| format!("evidence alignment has no {name}"))?;
                if values.len() > maximum {
                    return Err(format!("evidence alignment has too many {name}"));
                }
                values
                    .iter()
                    .map(|value| bounded_string(value, name))
                    .collect()
            };
            let missing_values = group["missingValues"]
                .as_array()
                .ok_or("evidence alignment has no missingValues")?;
            if missing_values.len() > 16 {
                return Err("evidence alignment has too many missingValues".into());
            }
            let missing_values = missing_values
                .iter()
                .map(|value| {
                    let value = value
                        .as_str()
                        .ok_or("evidence alignment missingValues is invalid")?;
                    if value.len() > 16 || value.bytes().any(|byte| byte.is_ascii_control()) {
                        return Err("evidence alignment missingValues is invalid".into());
                    }
                    Ok(value.to_owned())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let fields = values("fields", 2048)?.into_iter().collect::<BTreeSet<_>>();
            let key_field = bounded_string(&group["keyField"], "key field")?;
            if !fields.contains(&key_field)
                || fields.iter().any(|field| {
                    !present.contains(&(scope.clone(), source_id.clone(), field.clone()))
                })
            {
                return Err("evidence alignment references unavailable fields".into());
            }
            Ok(AlignmentSpec {
                id: bounded_string(&group["id"], "ID")?,
                kind,
                scope,
                source_id,
                source_transcript_release: bounded_string(
                    &group["sourceTranscriptRelease"],
                    "source transcript release",
                )?,
                key_field,
                protein_field: bounded_string(&group["proteinField"], "protein field")?,
                canonical_field: bounded_string(&group["canonicalField"], "canonical field")?,
                separator,
                missing_values,
                fields,
            })
        })
        .collect()
}

pub(crate) fn validate_catalog(catalog: &Path) -> Result<(), String> {
    catalog_specs(catalog).map(|_| ())
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_list(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .map(|value| sql_literal(&value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn report_table_path(evidence: &Path, name: &str) -> Result<PathBuf, String> {
    let path = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join(name);
    path.is_file()
        .then_some(path)
        .ok_or_else(|| format!("report is missing {name}"))
}

fn input_fingerprint(evidence: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    let mut paths = vec![
        report_table_path(evidence, "variants.parquet")?,
        evidence.to_path_buf(),
    ];
    let consequences = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join("consequences.parquet");
    if consequences.is_file() {
        paths.insert(1, consequences);
    }
    for path in paths {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        digest.update(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        digest.update(metadata.len().to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize())[..16].to_owned())
}

fn kind_tag(kind: RequestedResolutionKind) -> &'static str {
    match kind {
        RequestedResolutionKind::AlignedTranscriptVector => "vector",
        RequestedResolutionKind::LegacyAllele => "allele",
        RequestedResolutionKind::SelectedFeature => "selected",
    }
}

fn field_hash(field: &RequestedField) -> String {
    let mut digest = Sha256::new();
    for value in [
        kind_tag(field.kind),
        &field.scope,
        &field.biological_scope,
        &field.source_id,
        &field.field_path,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_owned()
}

fn cache_path(
    evidence: &Path,
    fingerprint: &str,
    field: &RequestedField,
) -> Result<PathBuf, String> {
    Ok(evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join(format!(
            "{CACHE_PREFIX}{fingerprint}-{}.parquet",
            field_hash(field)
        )))
}

pub(crate) fn available_path(evidence: &Path) -> Option<PathBuf> {
    let fingerprint = input_fingerprint(evidence).ok()?;
    let parent = evidence.parent()?;
    let prefix = format!("{CACHE_PREFIX}{fingerprint}-");
    let exists = fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".parquet"))
        });
    exists.then(|| parent.join(format!("{prefix}*.parquet")))
}

fn valid_requested_field(field: &RequestedField) -> bool {
    [
        &field.scope,
        &field.biological_scope,
        &field.source_id,
        &field.field_path,
    ]
    .into_iter()
    .all(|value| {
        !value.is_empty()
            && value.len() <= 200
            && !value.bytes().any(|byte| byte.is_ascii_control())
    })
}

fn cache_is_valid(path: &Path, fingerprint: &str, field: &RequestedField) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    let schema = builder.schema();
    for name in [
        "schema_version",
        "input_fingerprint",
        "allele_id",
        "source_id",
        "field_path",
        "resolution_kind",
        "resolved_string",
        "resolved_number",
    ] {
        if schema.index_of(name).is_err() {
            return false;
        }
    }
    let Ok(connection) = Connection::open_in_memory() else {
        return false;
    };
    connection
        .query_row(
            "SELECT coalesce(bool_and(
                      schema_version=? AND input_fingerprint=?
                      AND source_id=? AND field_path=?
                    ), true)
             FROM (SELECT * FROM read_parquet(?) LIMIT 1)",
            params![
                CACHE_VERSION,
                fingerprint,
                field.source_id,
                field.field_path,
                path.to_string_lossy().as_ref()
            ],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

pub(crate) fn prepare(
    variants: &Path,
    evidence: &Path,
    catalog: &Path,
    requested: &[RequestedField],
) -> Result<Option<PathBuf>, String> {
    if requested.is_empty() {
        return Ok(None);
    }
    validate_catalog(catalog)?;
    let fingerprint = input_fingerprint(evidence)?;
    let mut seen = HashSet::new();
    let fields = requested
        .iter()
        .filter(|field| {
            seen.insert((
                kind_tag(field.kind),
                field.scope.as_str(),
                field.biological_scope.as_str(),
                field.source_id.as_str(),
                field.field_path.as_str(),
            ))
        })
        .collect::<Vec<_>>();
    if fields.iter().any(|field| !valid_requested_field(field)) {
        return Err("requested evidence field is invalid".into());
    }
    let consequences = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join("consequences.parquet");
    if fields
        .iter()
        .any(|field| field.kind == RequestedResolutionKind::SelectedFeature)
        && !consequences.is_file()
    {
        return Err("report is missing consequences.parquet".into());
    }
    // ponytail: one process-wide lock is enough until concurrent report queries are measured.
    let _guard = BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "evidence cache build lock failed")?;
    for field in fields {
        let path = cache_path(evidence, &fingerprint, field)?;
        if cache_is_valid(&path, &fingerprint, field) {
            continue;
        }
        build_cache(
            &path,
            &fingerprint,
            variants,
            &consequences,
            evidence,
            field,
        )?;
    }
    available_path(evidence)
        .ok_or_else(|| "requested evidence cache was not published".into())
        .map(Some)
}

fn build_cache(
    path: &Path,
    fingerprint: &str,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<(), String> {
    let partial = path.with_extension("parquet.partial");
    let _ = fs::remove_file(&partial);
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "SET threads=4;
             SET preserve_insertion_order=false;
             SET memory_limit='1GB';",
        )
        .map_err(|error| format!("cannot configure evidence resolution: {error}"))?;
    let query = match field.kind {
        RequestedResolutionKind::AlignedTranscriptVector => {
            aligned_query(fingerprint, variants, evidence, field)?
        }
        RequestedResolutionKind::LegacyAllele => legacy_allele_query(fingerprint, evidence, field),
        RequestedResolutionKind::SelectedFeature => {
            selected_feature_query(fingerprint, variants, consequences, evidence, field)
        }
    };
    let sql = format!(
        "COPY ({query}) TO {} (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 100000)",
        sql_literal(&partial.to_string_lossy())
    );
    if let Err(error) = connection.execute_batch(&sql) {
        let _ = fs::remove_file(&partial);
        return Err(format!("cannot build requested evidence cache: {error}"));
    }
    if !cache_is_valid(&partial, fingerprint, field) {
        let _ = fs::remove_file(&partial);
        return Err("requested evidence cache failed validation".into());
    }
    if let Err(error) = crate::library_metadata::publish_atomic_file(&partial, path) {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    Ok(())
}

fn cache_projection(
    fingerprint: &str,
    source_id: &str,
    field_path: &str,
    release: &str,
    body: &str,
) -> String {
    format!(
        "SELECT {CACHE_VERSION}::INTEGER AS schema_version,
                {}::VARCHAR AS input_fingerprint,
                allele_id,
                {}::VARCHAR AS source_id,
                {}::VARCHAR AS field_path,
                {}::VARCHAR AS source_transcript_release,
                resolution_kind,
                resolved_string,
                try_cast(resolved_string AS DOUBLE) AS resolved_number,
                cast(selected_index AS SMALLINT) AS selected_index,
                cast(source_canonical_index AS SMALLINT) AS source_canonical_index,
                cast(least(reported_value_count, 32767) AS SMALLINT) AS reported_value_count,
                cast(least(distinct_value_count, 32767) AS SMALLINT) AS distinct_value_count
         FROM ({body}) resolved",
        sql_literal(fingerprint),
        sql_literal(source_id),
        sql_literal(field_path),
        sql_literal(release),
    )
}

fn aligned_query(
    fingerprint: &str,
    variants: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<String, String> {
    let spec = bundled_spec()?;
    if spec.scope != field.scope
        || spec.source_id != field.source_id
        || !spec.fields.contains(&field.field_path)
    {
        return Err("requested aligned evidence field is outside its contract".into());
    }
    let missing = sql_list(spec.missing_values.iter().cloned());
    let separator = sql_literal(&spec.separator);
    let body = format!(
        r#"
        WITH vectors AS MATERIALIZED (
          SELECT allele_id,
                 max(coalesce(string_value, cast(integer_value AS VARCHAR),
                              cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR)))
                   FILTER (WHERE field_path={key}) AS transcripts,
                 max(coalesce(string_value, cast(integer_value AS VARCHAR),
                              cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR)))
                   FILTER (WHERE field_path={field_path}) AS values
          FROM read_parquet({evidence})
          WHERE scope={scope} AND source_id={source}
            AND field_path IN ({key}, {field_path})
          GROUP BY allele_id
        ), joined AS MATERIALIZED (
          SELECT vectors.allele_id,
                 list_transform(string_split(vectors.transcripts, {separator}), x -> trim(x))
                   AS transcript_ids,
                 string_split(vectors.values, {separator}) AS values,
                 trim(coalesce(variants.transcript_id, '')) AS selected_transcript
          FROM vectors
          JOIN read_parquet({variants}) variants USING (allele_id)
          WHERE vectors.transcripts IS NOT NULL AND vectors.values IS NOT NULL
        ), indexed AS (
          SELECT *,
                 CASE
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> x=selected_transcript))=1
                     THEN list_position(transcript_ids, selected_transcript)
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> strpos(x, '.')>0))=0
                     AND list_count(list_filter(
                       transcript_ids,
                       x -> split_part(x, '.', 1)=split_part(selected_transcript, '.', 1)
                     ))=1
                     THEN list_position(
                       list_transform(transcript_ids, x -> split_part(x, '.', 1)),
                       split_part(selected_transcript, '.', 1)
                     )
                 END AS selected_index,
                 CASE
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> x=selected_transcript))=1
                     THEN 'exact_transcript'
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> strpos(x, '.')>0))=0
                     AND list_count(list_filter(
                       transcript_ids,
                       x -> split_part(x, '.', 1)=split_part(selected_transcript, '.', 1)
                     ))=1
                     THEN 'stable_id_match'
                 END AS selected_match_kind
          FROM joined
        ), classified AS (
          SELECT allele_id,
                 selected_index,
                 NULL::INTEGER AS source_canonical_index,
                 list_count(list_filter(values, x -> trim(x) NOT IN ({missing})))
                   AS reported_value_count,
                 list_unique(list_filter(values, x -> trim(x) NOT IN ({missing})))
                   AS distinct_value_count,
                 CASE
                   WHEN list_count(values)<>list_count(transcript_ids) THEN 'invalid_vector'
                   WHEN selected_index IS NULL THEN
                     CASE WHEN list_count(list_filter(values, x -> trim(x) NOT IN ({missing})))=0
                          THEN 'not_reported' ELSE 'unresolved_transcript' END
                   WHEN trim(list_extract(values, selected_index)) IN ({missing})
                     THEN 'exact_missing'
                   ELSE selected_match_kind
                 END AS resolution_kind,
                 CASE
                   WHEN selected_index IS NOT NULL
                     AND list_count(values)=list_count(transcript_ids)
                     AND trim(list_extract(values, selected_index)) NOT IN ({missing})
                     THEN trim(list_extract(values, selected_index))
                 END AS resolved_string
          FROM indexed
        )
        SELECT * FROM classified
        "#,
        key = sql_literal(&spec.key_field),
        field_path = sql_literal(&field.field_path),
        evidence = sql_literal(&evidence.to_string_lossy()),
        variants = sql_literal(&variants.to_string_lossy()),
        scope = sql_literal(&field.scope),
        source = sql_literal(&field.source_id),
    );
    Ok(cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        &spec.source_transcript_release,
        &body,
    ))
}

fn legacy_allele_query(fingerprint: &str, evidence: &Path, field: &RequestedField) -> String {
    let body = format!(
        r#"
        WITH candidates AS MATERIALIZED (
          SELECT allele_id,
                 scope IN ('allele', 'variant') AS direct,
                 consequence_id,
                 coalesce(string_value, cast(integer_value AS VARCHAR),
                          cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                          json_value) AS candidate_value,
                 CASE
                   WHEN string_value IS NOT NULL THEN 's:' || trim(string_value)
                   WHEN integer_value IS NOT NULL THEN 'i:' || cast(integer_value AS VARCHAR)
                   WHEN number_value IS NOT NULL THEN 'n:' || cast(number_value AS VARCHAR)
                   WHEN boolean_value IS NOT NULL THEN 'b:' || cast(boolean_value AS VARCHAR)
                   WHEN json_value IS NOT NULL THEN 'j:' || trim(json_value)
                 END AS comparison_value
          FROM read_parquet({evidence})
          WHERE source_id={source} AND field_path={field_path}
            AND scope<>'selected'
            AND coalesce(string_value, cast(integer_value AS VARCHAR),
                         cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                         json_value) IS NOT NULL
        ), aggregated AS (
          SELECT allele_id,
                 count(*) FILTER (WHERE direct) AS direct_count,
                 count(DISTINCT comparison_value) FILTER (WHERE direct) AS direct_distinct,
                 count(*) AS reported_value_count,
                 count(DISTINCT comparison_value) AS distinct_value_count,
                 first(candidate_value ORDER BY consequence_id NULLS FIRST)
                   FILTER (WHERE direct) AS direct_value,
                 first(candidate_value ORDER BY consequence_id NULLS FIRST) AS any_value
          FROM candidates
          GROUP BY allele_id
        )
        SELECT allele_id,
               NULL::INTEGER AS selected_index,
               NULL::INTEGER AS source_canonical_index,
               reported_value_count,
               distinct_value_count,
               CASE
                 WHEN direct_count>0 AND direct_distinct=1 THEN 'direct_allele'
                 WHEN direct_count>0 THEN 'conflicting_allele_values'
                 WHEN distinct_value_count=1 THEN 'legacy_allele_scope_recovered'
                 ELSE 'conflicting_legacy_values'
               END AS resolution_kind,
               CASE
                 WHEN direct_count>0 AND direct_distinct=1 THEN direct_value
                 WHEN direct_count=0 AND distinct_value_count=1 THEN any_value
               END AS resolved_string
        FROM aggregated
        "#,
        evidence = sql_literal(&evidence.to_string_lossy()),
        source = sql_literal(&field.source_id),
        field_path = sql_literal(&field.field_path),
    );
    cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        "legacy report",
        &body,
    )
}

fn selected_feature_query(
    fingerprint: &str,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> String {
    let mode = match field.biological_scope.as_str() {
        "gene" => "gene",
        "feature"
            if annocat_core::source_catalog::feature_identity(&field.source_id) == Some("gene") =>
        {
            "gene"
        }
        "feature" => "feature",
        _ => "transcript",
    };
    let match_rank = match mode {
        "gene" => "CASE WHEN selected_gene<>'' AND candidate_gene=selected_gene THEN 0 ELSE 99 END",
        "feature" => {
            "CASE
               WHEN selected_transcript<>'' AND candidate_transcript=selected_transcript THEN 0
               ELSE 99
             END"
        }
        _ => {
            "CASE
               WHEN selected_transcript<>'' AND candidate_transcript=selected_transcript THEN 0
               WHEN selected_transcript<>'' AND candidate_transcript<>''
                 AND strpos(candidate_transcript, '.')=0
                 AND split_part(candidate_transcript, '.', 1)
                     = split_part(selected_transcript, '.', 1)
                 AND versioned_candidates=0 AND stable_matches=1 THEN 1
               ELSE 99
             END"
        }
    };
    let success_kind = match mode {
        "gene" => "exact_gene",
        "feature" => "policy_selected",
        _ => "exact_transcript",
    };
    let body = format!(
        r#"
        WITH raw AS MATERIALIZED (
          SELECT allele_id, consequence_id,
                 coalesce(string_value, cast(integer_value AS VARCHAR),
                          cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                          json_value) AS candidate_value,
                 CASE
                   WHEN string_value IS NOT NULL THEN 's:' || trim(string_value)
                   WHEN integer_value IS NOT NULL THEN 'i:' || cast(integer_value AS VARCHAR)
                   WHEN number_value IS NOT NULL THEN 'n:' || cast(number_value AS VARCHAR)
                   WHEN boolean_value IS NOT NULL THEN 'b:' || cast(boolean_value AS VARCHAR)
                   WHEN json_value IS NOT NULL THEN 'j:' || trim(json_value)
                 END AS comparison_value
          FROM read_parquet({evidence})
          WHERE scope={scope} AND source_id={source} AND field_path={field_path}
            AND consequence_id IS NOT NULL
            AND coalesce(string_value, cast(integer_value AS VARCHAR),
                         cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                         json_value) IS NOT NULL
        ), joined AS MATERIALIZED (
          SELECT raw.*,
                 c.ordinal,
                 trim(coalesce(c.transcript_id, '')) AS candidate_transcript,
                 trim(coalesce(c.gene_id, '')) AS candidate_gene,
                 trim(coalesce(v.transcript_id, '')) AS selected_transcript,
                 trim(coalesce(v.gene_id, '')) AS selected_gene
          FROM raw
          JOIN read_parquet({consequences}) c USING (allele_id, consequence_id)
          JOIN read_parquet({variants}) v USING (allele_id)
        ), measured AS (
          SELECT *,
                 count(*) FILTER (
                   WHERE candidate_transcript<>''
                     AND strpos(candidate_transcript, '.')=0
                     AND split_part(candidate_transcript, '.', 1)
                         = split_part(selected_transcript, '.', 1)
                 ) OVER (PARTITION BY allele_id) AS stable_matches,
                 count(*) FILTER (
                   WHERE candidate_transcript<>'' AND strpos(candidate_transcript, '.')>0
                 ) OVER (PARTITION BY allele_id) AS versioned_candidates
          FROM joined
        ), ranked AS (
          SELECT *, {match_rank} AS match_rank
          FROM measured
        ), chosen AS (
          SELECT *, min(match_rank) OVER (PARTITION BY allele_id) AS best_rank
          FROM ranked
        ), aggregated AS (
          SELECT allele_id,
                 min(best_rank) AS best_rank,
                 count(*) FILTER (WHERE match_rank=best_rank) AS reported_value_count,
                 count(DISTINCT comparison_value) FILTER (WHERE match_rank=best_rank)
                   AS distinct_value_count,
                 first(candidate_value ORDER BY ordinal)
                   FILTER (WHERE match_rank=best_rank) AS candidate_value
          FROM chosen
          GROUP BY allele_id
        )
        SELECT allele_id,
               NULL::INTEGER AS selected_index,
               NULL::INTEGER AS source_canonical_index,
               reported_value_count,
               distinct_value_count,
               CASE
                 WHEN best_rank>=99 THEN 'unresolved_feature'
                 WHEN distinct_value_count>1 THEN 'conflicting_selected_values'
                 WHEN best_rank=1 THEN 'stable_id_match'
                 ELSE {success_kind}
               END AS resolution_kind,
               CASE WHEN best_rank<99 AND distinct_value_count=1 THEN candidate_value END
                 AS resolved_string
        FROM aggregated
        "#,
        evidence = sql_literal(&evidence.to_string_lossy()),
        consequences = sql_literal(&consequences.to_string_lossy()),
        variants = sql_literal(&variants.to_string_lossy()),
        scope = sql_literal(&field.scope),
        source = sql_literal(&field.source_id),
        field_path = sql_literal(&field.field_path),
        success_kind = sql_literal(success_kind),
    );
    cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        "legacy report",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_contract_marks_transcript_vectors_but_not_scalar_scores() {
        assert!(bundled_alignment_group("allele", "dbnsfp", "AlphaMissense_score").is_some());
        assert!(bundled_alignment_group("allele", "dbnsfp", "REVEL_score").is_some());
        assert!(bundled_alignment_group("allele", "dbnsfp", "CADD_phred").is_none());
        assert!(bundled_alignment_group("allele", "dbnsfp", "GERP++_RS").is_none());
    }

    #[test]
    fn aligned_values_require_an_exact_or_safe_unique_transcript() {
        assert_eq!(
            select_aligned_value(
                "allele",
                "dbnsfp",
                "REVEL_score",
                "ENST1;ENST2",
                "0.1;0.8",
                "ENST2.4"
            )
            .as_deref(),
            Some("0.8")
        );
        assert!(
            select_aligned_value(
                "allele",
                "dbnsfp",
                "REVEL_score",
                "ENST1.2;ENST2.4",
                "0.1;0.8",
                "ENST2"
            )
            .is_none()
        );
    }
}
