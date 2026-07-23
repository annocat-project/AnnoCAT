use duckdb::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(crate) const FILE_NAME: &str = "evidence-resolution.parquet";
const SCHEMA_VERSION: i32 = 1;
const MAX_SIDECAR_BYTES: u64 = 512 * 1024 * 1024;

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

static BUNDLED_SPEC: OnceLock<Result<AlignmentSpec, String>> = OnceLock::new();
static BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static READY_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

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

fn sidecar_path(evidence: &Path) -> Result<PathBuf, String> {
    Ok(evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join(FILE_NAME))
}

pub(crate) fn available_path(evidence: &Path) -> Option<PathBuf> {
    let path = sidecar_path(evidence).ok()?;
    if path.is_file()
        && READY_PATHS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .ok()?
            .contains(&path)
    {
        return Some(path);
    }
    if !sidecar_is_valid(&path) {
        return None;
    }
    READY_PATHS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .ok()?
        .insert(path.clone());
    Some(path)
}

fn sidecar_is_valid(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() == 0 || metadata.len() > MAX_SIDECAR_BYTES {
        return false;
    }
    let Ok(connection) = Connection::open_in_memory() else {
        return false;
    };
    connection
        .query_row(
            "SELECT count(*), min(schema_version), max(schema_version)
             FROM read_parquet(?)",
            params![path.to_string_lossy().as_ref()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i32>>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                ))
            },
        )
        .is_ok_and(|(rows, minimum, maximum)| {
            rows == 0 || minimum == Some(SCHEMA_VERSION) && maximum == Some(SCHEMA_VERSION)
        })
}

fn resolution_query(spec: &AlignmentSpec, variants: &Path, evidence: &Path) -> String {
    let fields = sql_list(spec.fields.iter().cloned());
    let missing = sql_list(spec.missing_values.iter().cloned());
    let source = sql_literal(&spec.source_id);
    let scope = sql_literal(&spec.scope);
    let key = sql_literal(&spec.key_field);
    let canonical = sql_literal(&spec.canonical_field);
    let separator = sql_literal(&spec.separator);
    let release = sql_literal(&spec.source_transcript_release);
    let variants = sql_literal(&variants.to_string_lossy());
    let evidence = sql_literal(&evidence.to_string_lossy());
    format!(
        r#"
        WITH transcript_vectors AS MATERIALIZED (
          SELECT allele_id,
                 max(string_value) FILTER (WHERE field_path={key}) AS transcripts,
                 max(string_value) FILTER (WHERE field_path={canonical}) AS canonical
          FROM read_parquet({evidence})
          WHERE scope={scope} AND source_id={source}
            AND field_path IN ({key}, {canonical})
          GROUP BY allele_id
        ), joined AS MATERIALIZED (
          SELECT v.allele_id,
                 list_transform(string_split(t.transcripts, {separator}),
                                x -> split_part(trim(x), '.', 1)) AS transcript_ids,
                 split_part(trim(v.transcript_id), '.', 1) AS selected_transcript,
                 list_position(string_split(t.canonical, {separator}), 'YES') AS source_canonical_index
          FROM read_parquet({variants}) v
          JOIN transcript_vectors t USING (allele_id)
          WHERE t.transcripts IS NOT NULL
        ), indexed AS MATERIALIZED (
          SELECT allele_id,
                 CASE WHEN selected_transcript <> ''
                            AND list_count(list_filter(transcript_ids,
                                x -> x=selected_transcript))=1
                      THEN list_position(transcript_ids, selected_transcript)
                 END AS selected_index,
                 list_count(transcript_ids) AS transcript_count,
                 source_canonical_index
          FROM joined
        ), candidate_values AS MATERIALIZED (
          SELECT e.allele_id,
                 e.field_path,
                 i.selected_index,
                 i.transcript_count,
                 i.source_canonical_index,
                 string_split(coalesce(e.string_value,
                                       cast(e.integer_value AS VARCHAR),
                                       cast(e.number_value AS VARCHAR),
                                       cast(e.boolean_value AS VARCHAR)), {separator}) AS values,
                 list_filter(string_split(coalesce(e.string_value,
                                                   cast(e.integer_value AS VARCHAR),
                                                   cast(e.number_value AS VARCHAR),
                                                   cast(e.boolean_value AS VARCHAR)), {separator}),
                             x -> trim(x) NOT IN ({missing})) AS reported_values
          FROM read_parquet({evidence}) e
          JOIN indexed i USING (allele_id)
          WHERE e.scope={scope} AND e.source_id={source}
            AND e.field_path IN ({fields})
            AND coalesce(e.string_value,
                         cast(e.integer_value AS VARCHAR),
                         cast(e.number_value AS VARCHAR),
                         cast(e.boolean_value AS VARCHAR)) IS NOT NULL
        ), resolved AS (
          SELECT allele_id,
                 field_path,
                 selected_index,
                 source_canonical_index,
                 list_count(reported_values) AS reported_value_count,
                 list_unique(reported_values) AS distinct_value_count,
                 CASE
                   WHEN selected_index IS NOT NULL AND list_count(values)=transcript_count
                     THEN CASE WHEN trim(list_extract(values, selected_index)) IN ({missing})
                               THEN 'exact_missing' ELSE 'exact_transcript' END
                   WHEN list_count(reported_values)=0 THEN 'not_reported'
                   WHEN list_unique(reported_values)=1 THEN 'uniform'
                   WHEN list_count(values)<>transcript_count THEN 'invalid_vector'
                   ELSE 'ambiguous'
                 END AS resolution_kind,
                 CASE
                   WHEN selected_index IS NOT NULL AND list_count(values)=transcript_count
                     THEN nullif(nullif(trim(list_extract(values, selected_index)), '.'), '-')
                   WHEN list_count(reported_values)>0 AND list_unique(reported_values)=1
                     THEN list_extract(reported_values, 1)
                 END AS resolved_string
          FROM candidate_values
        )
        SELECT {SCHEMA_VERSION}::INTEGER AS schema_version,
               allele_id,
               {source}::VARCHAR AS source_id,
               field_path,
               {release}::VARCHAR AS source_transcript_release,
               resolution_kind,
               resolved_string,
               try_cast(resolved_string AS DOUBLE) AS resolved_number,
               cast(selected_index AS SMALLINT) AS selected_index,
               cast(source_canonical_index AS SMALLINT) AS source_canonical_index,
               cast(reported_value_count AS SMALLINT) AS reported_value_count,
               cast(distinct_value_count AS SMALLINT) AS distinct_value_count
        FROM resolved
        "#
    )
}

fn build_sidecar(
    path: &Path,
    variants: &Path,
    evidence: &Path,
    specs: &[AlignmentSpec],
) -> Result<(), String> {
    let queries = specs
        .iter()
        .filter(|spec| !spec.fields.is_empty())
        .map(|spec| format!("({})", resolution_query(spec, variants, evidence)))
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return Ok(());
    }
    let partial = path.with_extension("parquet.partial");
    let _ = fs::remove_file(&partial);
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch("SET threads=4; SET preserve_insertion_order=false;")
        .map_err(|error| format!("cannot configure evidence resolution: {error}"))?;
    let sql = format!(
        "COPY (SELECT * FROM ({}) resolved_union
               ORDER BY source_id, field_path, allele_id)
         TO {} (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 100000)",
        queries.join(" UNION ALL "),
        sql_literal(&partial.to_string_lossy())
    );
    if let Err(error) = connection.execute_batch(&sql) {
        let _ = fs::remove_file(&partial);
        return Err(format!("cannot build transcript evidence index: {error}"));
    }
    if !sidecar_is_valid(&partial) {
        let _ = fs::remove_file(&partial);
        return Err("transcript evidence index failed validation".into());
    }
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("cannot replace transcript evidence index: {error}"))?;
    }
    fs::rename(&partial, path)
        .map_err(|error| format!("cannot publish transcript evidence index: {error}"))
}

pub(crate) fn prepare(
    variants: &Path,
    evidence: &Path,
    catalog: &Path,
) -> Result<Option<PathBuf>, String> {
    let path = sidecar_path(evidence)?;
    if let Some(path) = available_path(evidence) {
        return Ok(Some(path));
    }
    let specs = catalog_specs(catalog)?;
    if specs.is_empty() {
        return Ok(None);
    }
    let _guard = BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "transcript evidence index build lock failed")?;
    if let Some(path) = available_path(evidence) {
        return Ok(Some(path));
    }
    build_sidecar(&path, variants, evidence, &specs)?;
    READY_PATHS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map_err(|_| "transcript evidence index cache failed")?
        .insert(path.clone());
    Ok(Some(path))
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
}
