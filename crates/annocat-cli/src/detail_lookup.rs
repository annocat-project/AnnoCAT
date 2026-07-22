use duckdb::arrow::array::{
    Array, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use duckdb::arrow::record_batch::RecordBatch;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const INDEX_SCHEMA_VERSION: u32 = 1;
const INDEX_FILE: &str = "detail-row-groups.json";
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROW_GROUPS: usize = 100_000;
const MAX_GROUPS_PER_LOOKUP: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct FileStamp {
    bytes: u64,
    rows: i64,
    row_groups: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowGroupRange {
    row_group: usize,
    first_record: i64,
    last_record: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexedFile {
    stamp: FileStamp,
    groups: Vec<RowGroupRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailIndex {
    schema_version: u32,
    variants: IndexedFile,
    consequences: IndexedFile,
    evidence: IndexedFile,
}

struct AlleleBoundaries {
    stamp: FileStamp,
    groups: Vec<(usize, String, String)>,
}

pub(crate) struct IndexedDetail {
    pub variant: Value,
    pub consequences: Vec<(String, String)>,
    pub evidence: Vec<Value>,
}

static INDEX_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn stamp(path: &Path) -> Result<FileStamp, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open report table: {error}"))?,
    )
    .map_err(|error| format!("cannot read report table metadata: {error}"))?;
    let row_groups = builder.metadata().row_groups().len();
    if row_groups > MAX_ROW_GROUPS {
        return Err("report table has too many row groups".into());
    }
    Ok(FileStamp {
        bytes: fs::metadata(path)
            .map_err(|error| format!("cannot inspect report table: {error}"))?
            .len(),
        rows: builder.metadata().file_metadata().num_rows(),
        row_groups,
    })
}

fn projection(
    builder: &ParquetRecordBatchReaderBuilder<File>,
    names: &[&str],
) -> Result<ProjectionMask, String> {
    let indices = names
        .iter()
        .map(|name| {
            builder
                .schema()
                .index_of(name)
                .map_err(|_| format!("report table is missing {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectionMask::roots(builder.parquet_schema(), indices))
}

fn string_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("report table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| format!("report field {name} has the wrong type"))
}

fn i64_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("report table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| format!("report field {name} has the wrong type"))
}

fn i32_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("report table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| format!("report field {name} has the wrong type"))
}

fn f64_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("report table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| format!("report field {name} has the wrong type"))
}

fn bool_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("report table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| format!("report field {name} has the wrong type"))
}

fn string_value(array: &StringArray, row: usize) -> Option<String> {
    (!array.is_null(row)).then(|| array.value(row).to_owned())
}

fn row_group_ends(builder: &ParquetRecordBatchReaderBuilder<File>) -> Vec<usize> {
    let mut total = 0usize;
    builder
        .metadata()
        .row_groups()
        .iter()
        .map(|group| {
            total = total.saturating_add(group.num_rows() as usize);
            total
        })
        .collect()
}

fn scan_allele_boundaries(path: &Path) -> Result<AlleleBoundaries, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open report detail table: {error}"))?,
    )
    .map_err(|error| format!("cannot read report detail metadata: {error}"))?;
    let stamp = FileStamp {
        bytes: fs::metadata(path).map_err(|error| error.to_string())?.len(),
        rows: builder.metadata().file_metadata().num_rows(),
        row_groups: builder.metadata().row_groups().len(),
    };
    if stamp.row_groups > MAX_ROW_GROUPS {
        return Err("report detail table has too many row groups".into());
    }
    let ends = row_group_ends(&builder);
    let mask = projection(&builder, &["allele_id"])?;
    let reader = builder
        .with_projection(mask)
        .with_batch_size(16_384)
        .build()
        .map_err(|error| format!("cannot scan report detail table: {error}"))?;
    let mut groups = vec![(None::<String>, None::<String>); stamp.row_groups];
    let mut global_row = 0usize;
    let mut group = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| format!("cannot decode report detail table: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        for row in 0..batch.num_rows() {
            while group < ends.len() && global_row >= ends[group] {
                group += 1;
            }
            if group >= groups.len() || alleles.is_null(row) {
                return Err("report detail row groups are inconsistent".into());
            }
            let allele = alleles.value(row);
            if groups[group].0.is_none() {
                groups[group].0 = Some(allele.to_owned());
            }
            groups[group].1 = Some(allele.to_owned());
            global_row += 1;
        }
    }
    if global_row as i64 != stamp.rows {
        return Err("report detail row count changed while indexing".into());
    }
    let groups = groups
        .into_iter()
        .enumerate()
        .map(|(row_group, (first, last))| {
            Ok((
                row_group,
                first.ok_or("report detail table contains an empty row group")?,
                last.ok_or("report detail table contains an empty row group")?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AlleleBoundaries { stamp, groups })
}

fn scan_variants(
    path: &Path,
    boundary_ids: &HashSet<String>,
) -> Result<(IndexedFile, HashMap<String, i64>), String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open variants table: {error}"))?,
    )
    .map_err(|error| format!("cannot read variants metadata: {error}"))?;
    let stamp = FileStamp {
        bytes: fs::metadata(path).map_err(|error| error.to_string())?.len(),
        rows: builder.metadata().file_metadata().num_rows(),
        row_groups: builder.metadata().row_groups().len(),
    };
    if stamp.row_groups > MAX_ROW_GROUPS {
        return Err("variants table has too many row groups".into());
    }
    let ends = row_group_ends(&builder);
    let mask = projection(&builder, &["allele_id", "record_number"])?;
    let reader = builder
        .with_projection(mask)
        .with_batch_size(16_384)
        .build()
        .map_err(|error| format!("cannot scan variants table: {error}"))?;
    let mut ranges = vec![(None::<i64>, None::<i64>); stamp.row_groups];
    let mut records = HashMap::with_capacity(boundary_ids.len());
    let mut global_row = 0usize;
    let mut group = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| format!("cannot decode variants table: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        let record_numbers = i64_array(&batch, "record_number")?;
        for row in 0..batch.num_rows() {
            while group < ends.len() && global_row >= ends[group] {
                group += 1;
            }
            if group >= ranges.len() || alleles.is_null(row) || record_numbers.is_null(row) {
                return Err("variants row groups are inconsistent".into());
            }
            let record = record_numbers.value(row);
            if ranges[group].0.is_none() {
                ranges[group].0 = Some(record);
            }
            ranges[group].1 = Some(record);
            let allele = alleles.value(row);
            if boundary_ids.contains(allele) {
                records.insert(allele.to_owned(), record);
            }
            global_row += 1;
        }
    }
    let groups = ranges
        .into_iter()
        .enumerate()
        .map(|(row_group, (first, last))| {
            let first_record = first.ok_or("variants table contains an empty row group")?;
            let last_record = last.ok_or("variants table contains an empty row group")?;
            if first_record > last_record {
                return Err("variants are not stored in input order".into());
            }
            Ok(RowGroupRange {
                row_group,
                first_record,
                last_record,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((IndexedFile { stamp, groups }, records))
}

fn resolve_detail_file(
    boundaries: AlleleBoundaries,
    records: &HashMap<String, i64>,
) -> Result<IndexedFile, String> {
    let groups = boundaries
        .groups
        .into_iter()
        .map(|(row_group, first, last)| {
            let first_record = *records
                .get(&first)
                .ok_or("detail index boundary refers to an unknown allele")?;
            let last_record = *records
                .get(&last)
                .ok_or("detail index boundary refers to an unknown allele")?;
            if first_record > last_record {
                return Err("detail rows are not stored in input order".into());
            }
            Ok(RowGroupRange {
                row_group,
                first_record,
                last_record,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(IndexedFile {
        stamp: boundaries.stamp,
        groups,
    })
}

fn build_index(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
) -> Result<DetailIndex, String> {
    let consequence_bounds = scan_allele_boundaries(consequences)?;
    let evidence_bounds = scan_allele_boundaries(evidence)?;
    let boundary_ids = consequence_bounds
        .groups
        .iter()
        .chain(&evidence_bounds.groups)
        .flat_map(|(_, first, last)| [first.clone(), last.clone()])
        .collect::<HashSet<_>>();
    let (variants, records) = scan_variants(variants, &boundary_ids)?;
    Ok(DetailIndex {
        schema_version: INDEX_SCHEMA_VERSION,
        variants,
        consequences: resolve_detail_file(consequence_bounds, &records)?,
        evidence: resolve_detail_file(evidence_bounds, &records)?,
    })
}

fn index_is_valid(
    index: &DetailIndex,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
) -> bool {
    if index.schema_version != INDEX_SCHEMA_VERSION
        || stamp(variants).ok().as_ref() != Some(&index.variants.stamp)
        || stamp(consequences).ok().as_ref() != Some(&index.consequences.stamp)
        || stamp(evidence).ok().as_ref() != Some(&index.evidence.stamp)
    {
        return false;
    }
    [&index.variants, &index.consequences, &index.evidence]
        .into_iter()
        .all(|file| {
            file.groups.len() == file.stamp.row_groups
                && file.groups.iter().enumerate().all(|(expected, group)| {
                    group.row_group == expected
                        && group.row_group < file.stamp.row_groups
                        && group.first_record <= group.last_record
                })
                && file.groups.windows(2).all(|groups| {
                    groups[0].first_record <= groups[1].first_record
                        && groups[0].last_record <= groups[1].last_record
                })
        })
}

fn read_index(path: &Path) -> Result<DetailIndex, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() == 0 || metadata.len() > MAX_INDEX_BYTES {
        return Err("detail index has an invalid size".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("cannot decode detail index: {error}"))
}

fn ensure_index(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
) -> Result<DetailIndex, String> {
    let directory = variants
        .parent()
        .ok_or("variants table has no parent directory")?;
    let path = directory.join(INDEX_FILE);
    if let Ok(index) = read_index(&path)
        && index_is_valid(&index, variants, consequences, evidence)
    {
        return Ok(index);
    }
    let _guard = INDEX_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "detail index build lock failed")?;
    if let Ok(index) = read_index(&path)
        && index_is_valid(&index, variants, consequences, evidence)
    {
        return Ok(index);
    }
    let index = build_index(variants, consequences, evidence)?;
    let encoded = serde_json::to_vec(&index).map_err(|error| error.to_string())?;
    if encoded.len() as u64 > MAX_INDEX_BYTES {
        return Err("detail index is unexpectedly large".into());
    }
    let partial = path.with_extension("json.partial");
    fs::write(&partial, encoded).map_err(|error| format!("cannot write detail index: {error}"))?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| format!("cannot replace detail index: {error}"))?;
    }
    fs::rename(&partial, &path).map_err(|error| format!("cannot finish detail index: {error}"))?;
    Ok(index)
}

fn groups_for_record(file: &IndexedFile, record_number: i64) -> Result<Vec<usize>, String> {
    let groups = file
        .groups
        .iter()
        .filter(|group| group.first_record <= record_number && record_number <= group.last_record)
        .map(|group| group.row_group)
        .take(MAX_GROUPS_PER_LOOKUP + 1)
        .collect::<Vec<_>>();
    if groups.len() > MAX_GROUPS_PER_LOOKUP {
        return Err("detail locator matched too many row groups".into());
    }
    Ok(groups)
}

fn projected_reader(
    path: &Path,
    groups: Vec<usize>,
    names: &[&str],
) -> Result<parquet::arrow::arrow_reader::ParquetRecordBatchReader, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open report table: {error}"))?,
    )
    .map_err(|error| format!("cannot read report table metadata: {error}"))?;
    let mask = projection(&builder, names)?;
    builder
        .with_row_groups(groups)
        .with_projection(mask)
        .with_batch_size(16_384)
        .build()
        .map_err(|error| format!("cannot read indexed report rows: {error}"))
}

fn read_variant(
    path: &Path,
    groups: Vec<usize>,
    allele_id: &str,
    record_number: i64,
    alt_index: i32,
) -> Result<Option<Value>, String> {
    const FIELDS: &[&str] = &[
        "allele_id",
        "record_number",
        "chromosome",
        "position",
        "reference",
        "alternate",
        "alt_index",
        "variant_id",
        "quality",
        "filter",
        "gene_symbol",
        "gene_id",
        "transcript_id",
        "consequence",
        "impact",
        "canonical",
        "mane_select",
        "format",
        "samples_json",
        "consequences_json",
    ];
    for batch in projected_reader(path, groups, FIELDS)? {
        let batch =
            batch.map_err(|error| format!("cannot decode indexed variant rows: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        let records = i64_array(&batch, "record_number")?;
        let alternate_indices = i32_array(&batch, "alt_index")?;
        for row in 0..batch.num_rows() {
            if !alleles.is_null(row)
                && !records.is_null(row)
                && alleles.value(row) == allele_id
                && records.value(row) == record_number
                && !alternate_indices.is_null(row)
                && alternate_indices.value(row) == alt_index
            {
                let string = |name| string_array(&batch, name);
                let samples = string("samples_json")?;
                let consequences = string("consequences_json")?;
                let quality = f64_array(&batch, "quality")?;
                let canonical = bool_array(&batch, "canonical")?;
                return Ok(Some(json!({
                    "chromosome": string_value(string("chromosome")?, row).unwrap_or_default(),
                    "position": i64_array(&batch, "position")?.value(row),
                    "reference": string_value(string("reference")?, row).unwrap_or_default(),
                    "alternate": string_value(string("alternate")?, row).unwrap_or_default(),
                    "altIndex": i32_array(&batch, "alt_index")?.value(row),
                    "variantId": string_value(string("variant_id")?, row),
                    "quality": (!quality.is_null(row)).then(|| quality.value(row)),
                    "filter": string_value(string("filter")?, row).unwrap_or_default(),
                    "geneSymbol": string_value(string("gene_symbol")?, row),
                    "geneId": string_value(string("gene_id")?, row),
                    "transcriptId": string_value(string("transcript_id")?, row),
                    "consequence": string_value(string("consequence")?, row),
                    "impact": string_value(string("impact")?, row),
                    "canonical": !canonical.is_null(row) && canonical.value(row),
                    "maneSelect": string_value(string("mane_select")?, row),
                    "format": string_value(string("format")?, row),
                    "samples": string_value(samples, row)
                        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
                        .unwrap_or_else(|| Value::Array(Vec::new())),
                    "fallbackConsequences": string_value(consequences, row)
                        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
                        .unwrap_or_else(|| Value::Array(Vec::new())),
                })));
            }
        }
    }
    Ok(None)
}

fn read_consequences(
    path: &Path,
    groups: Vec<usize>,
    allele_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut rows = Vec::new();
    for batch in projected_reader(
        path,
        groups,
        &["allele_id", "consequence_id", "ordinal", "consequence_json"],
    )? {
        let batch =
            batch.map_err(|error| format!("cannot decode indexed consequence rows: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        let ids = string_array(&batch, "consequence_id")?;
        let ordinals = i64_array(&batch, "ordinal")?;
        let values = string_array(&batch, "consequence_json")?;
        for row in 0..batch.num_rows() {
            if !alleles.is_null(row) && alleles.value(row) == allele_id {
                rows.push((
                    ordinals.value(row),
                    ids.value(row).to_owned(),
                    values.value(row).to_owned(),
                ));
            }
        }
    }
    rows.sort_by_key(|row| row.0);
    Ok(rows
        .into_iter()
        .map(|(_, id, value)| (id, value))
        .take(1001)
        .collect())
}

fn read_evidence(path: &Path, groups: Vec<usize>, allele_id: &str) -> Result<Vec<Value>, String> {
    const FIELDS: &[&str] = &[
        "allele_id",
        "consequence_id",
        "scope",
        "source_id",
        "field_path",
        "value_type",
        "string_value",
        "integer_value",
        "number_value",
        "boolean_value",
        "json_value",
    ];
    let mut rows = Vec::new();
    for batch in projected_reader(path, groups, FIELDS)? {
        let batch =
            batch.map_err(|error| format!("cannot decode indexed evidence rows: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        for row in 0..batch.num_rows() {
            if alleles.is_null(row) || alleles.value(row) != allele_id {
                continue;
            }
            let strings = |name| string_array(&batch, name);
            let value_type = strings("value_type")?.value(row).to_owned();
            let value = match value_type.as_str() {
                "string" => string_value(strings("string_value")?, row).map(Value::String),
                "integer" => {
                    let values = i64_array(&batch, "integer_value")?;
                    (!values.is_null(row)).then(|| Value::Number(values.value(row).into()))
                }
                "number" => {
                    let values = f64_array(&batch, "number_value")?;
                    (!values.is_null(row))
                        .then(|| values.value(row))
                        .and_then(serde_json::Number::from_f64)
                        .map(Value::Number)
                }
                "boolean" => {
                    let values = bool_array(&batch, "boolean_value")?;
                    (!values.is_null(row)).then(|| Value::Bool(values.value(row)))
                }
                "json" => string_value(strings("json_value")?, row)
                    .map(|value| serde_json::from_str(&value).unwrap_or(Value::String(value))),
                _ => None,
            }
            .unwrap_or(Value::Null);
            rows.push(json!({
                "consequenceId": string_value(strings("consequence_id")?, row),
                "scope": strings("scope")?.value(row),
                "sourceId": strings("source_id")?.value(row),
                "fieldPath": strings("field_path")?.value(row),
                "valueType": value_type,
                "value": value,
            }));
        }
    }
    rows.sort_by(|left, right| {
        let key = |value: &Value| {
            ["scope", "sourceId", "fieldPath", "consequenceId"]
                .map(|field| value[field].as_str().unwrap_or_default().to_owned())
        };
        key(left).cmp(&key(right))
    });
    rows.truncate(5001);
    Ok(rows)
}

pub(crate) fn lookup(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    allele_id: &str,
    record_number: i64,
    alt_index: i32,
) -> Result<Option<IndexedDetail>, String> {
    if record_number < 1 || alt_index < 1 {
        return Ok(None);
    }
    let index = ensure_index(variants, consequences, evidence)?;
    let variant_groups = groups_for_record(&index.variants, record_number)?;
    let consequence_groups = groups_for_record(&index.consequences, record_number)?;
    let evidence_groups = groups_for_record(&index.evidence, record_number)?;
    if variant_groups.is_empty() {
        return Ok(None);
    }
    let Some(variant) = read_variant(
        variants,
        variant_groups,
        allele_id,
        record_number,
        alt_index,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(IndexedDetail {
        variant,
        consequences: if consequence_groups.is_empty() {
            Vec::new()
        } else {
            read_consequences(consequences, consequence_groups, allele_id)?
        },
        evidence: if evidence_groups.is_empty() {
            Vec::new()
        } else {
            read_evidence(evidence, evidence_groups, allele_id)?
        },
    }))
}
