use duckdb::arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use duckdb::arrow::datatypes::{Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::types::Value as SqlValue;
use duckdb::{Connection, InterruptHandle, params, params_from_iter};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

pub const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultPage {
    schema_version: i32,
    offset: u64,
    limit: u64,
    total: i64,
    search: String,
    sort: String,
    direction: String,
    rows: Vec<Value>,
}

#[derive(Clone, Copy)]
struct PageQuery<'a> {
    variants: &'a Path,
    evidence: Option<&'a Path>,
    catalog: Option<&'a Path>,
    offset: u64,
    limit: u64,
    request: &'a PageRequest,
    candidate_ids: Option<&'a [String]>,
}

struct ActivePageQuery {
    generation: u64,
    handle: Arc<InterruptHandle>,
}

static ACTIVE_PAGE_QUERIES: OnceLock<Mutex<HashMap<String, ActivePageQuery>>> = OnceLock::new();

struct PageQueryGuard {
    key: String,
    handle: Arc<InterruptHandle>,
}

impl Drop for PageQueryGuard {
    fn drop(&mut self) {
        let Some(active) = ACTIVE_PAGE_QUERIES.get() else {
            return;
        };
        let Ok(mut active) = active.lock() else {
            return;
        };
        if active
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(&current.handle, &self.handle))
        {
            active.remove(&self.key);
        }
    }
}

fn cancellable_page_connection(
    key: &str,
    generation: u64,
) -> Result<(Connection, PageQueryGuard), String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let handle = connection.interrupt_handle();
    let mut active = ACTIVE_PAGE_QUERIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "result query cancellation lock failed")?;
    if active
        .get(key)
        .is_some_and(|current| generation < current.generation)
    {
        return Err("result query was superseded by a newer request".into());
    }
    let previous = active.insert(
        key.to_owned(),
        ActivePageQuery {
            generation,
            handle: handle.clone(),
        },
    );
    drop(active);
    if let Some(previous) = previous {
        previous.handle.interrupt();
    }
    Ok((
        connection,
        PageQueryGuard {
            key: key.to_owned(),
            handle,
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSummary {
    pub rows: u64,
    pub records: u64,
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredSummary {
    pub records: u64,
    pub consequences: u64,
    pub evidence: u64,
    pub fields: usize,
    pub sources: Vec<String>,
    pub source_value_counts: BTreeMap<String, u64>,
}

const VARIANT_CHUNK_RECORDS: usize = 32_768;

struct VariantRecord {
    line_number: usize,
    record_number: i64,
    line: String,
}

#[derive(Default)]
struct VariantBatch {
    schema_version: Vec<i32>,
    allele_id: Vec<String>,
    record_number: Vec<i64>,
    alt_index: Vec<i32>,
    chromosome: Vec<String>,
    position: Vec<i64>,
    reference: Vec<String>,
    alternate: Vec<String>,
    variant_id: Vec<Option<String>>,
    quality: Vec<Option<f64>>,
    filter: Vec<String>,
    gene_symbol: Vec<Option<String>>,
    gene_id: Vec<Option<String>>,
    transcript_id: Vec<Option<String>>,
    consequence: Vec<Option<String>>,
    impact: Vec<Option<String>>,
    canonical: Vec<bool>,
    mane_select: Vec<Option<String>>,
    sample_names_json: Vec<String>,
    format: Vec<Option<String>>,
    samples_json: Vec<String>,
    consequences_json: Vec<String>,
}

#[derive(Default)]
struct ConsequenceBatch {
    schema_version: Vec<i32>,
    consequence_id: Vec<String>,
    allele_id: Vec<String>,
    ordinal: Vec<i64>,
    transcript_id: Vec<Option<String>>,
    gene_id: Vec<Option<String>>,
    gene_symbol: Vec<Option<String>>,
    biotype: Vec<Option<String>>,
    consequence_terms_json: Vec<String>,
    primary_consequence: Vec<Option<String>>,
    impact: Vec<Option<String>>,
    canonical: Vec<bool>,
    mane_select: Vec<Option<String>>,
    protein_id: Vec<Option<String>>,
    exon: Vec<Option<String>>,
    intron: Vec<Option<String>>,
    hgvsg: Vec<Option<String>>,
    hgvsc: Vec<Option<String>>,
    hgvsp: Vec<Option<String>>,
    distance: Vec<Option<i64>>,
    strand: Vec<Option<i32>>,
    consequence_json: Vec<String>,
}

#[derive(Default)]
struct EvidenceBatch {
    schema_version: Vec<i32>,
    allele_id: Vec<String>,
    consequence_id: Vec<Option<String>>,
    scope: Vec<String>,
    source_id: Vec<String>,
    field_path: Vec<String>,
    value_type: Vec<String>,
    string_value: Vec<Option<String>>,
    integer_value: Vec<Option<i64>>,
    number_value: Vec<Option<f64>>,
    boolean_value: Vec<Option<bool>>,
    json_value: Vec<Option<String>>,
}

fn record_batch(columns: Vec<(&str, ArrayRef, bool)>) -> Result<RecordBatch, String> {
    let fields = columns
        .iter()
        .map(|(name, array, nullable)| Field::new(*name, array.data_type().clone(), *nullable))
        .collect::<Vec<_>>();
    let arrays = columns
        .into_iter()
        .map(|(_, array, _)| array)
        .collect::<Vec<_>>();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|error| error.to_string())
}

impl VariantBatch {
    fn len(&self) -> usize {
        self.allele_id.len()
    }

    fn into_record_batch(self) -> Result<RecordBatch, String> {
        record_batch(vec![
            (
                "schema_version",
                Arc::new(Int32Array::from(self.schema_version)),
                false,
            ),
            (
                "allele_id",
                Arc::new(StringArray::from(self.allele_id)),
                false,
            ),
            (
                "record_number",
                Arc::new(Int64Array::from(self.record_number)),
                false,
            ),
            (
                "alt_index",
                Arc::new(Int32Array::from(self.alt_index)),
                false,
            ),
            (
                "chromosome",
                Arc::new(StringArray::from(self.chromosome)),
                false,
            ),
            ("position", Arc::new(Int64Array::from(self.position)), false),
            (
                "reference",
                Arc::new(StringArray::from(self.reference)),
                false,
            ),
            (
                "alternate",
                Arc::new(StringArray::from(self.alternate)),
                false,
            ),
            (
                "variant_id",
                Arc::new(StringArray::from(self.variant_id)),
                true,
            ),
            ("quality", Arc::new(Float64Array::from(self.quality)), true),
            ("filter", Arc::new(StringArray::from(self.filter)), false),
            (
                "gene_symbol",
                Arc::new(StringArray::from(self.gene_symbol)),
                true,
            ),
            ("gene_id", Arc::new(StringArray::from(self.gene_id)), true),
            (
                "transcript_id",
                Arc::new(StringArray::from(self.transcript_id)),
                true,
            ),
            (
                "consequence",
                Arc::new(StringArray::from(self.consequence)),
                true,
            ),
            ("impact", Arc::new(StringArray::from(self.impact)), true),
            (
                "canonical",
                Arc::new(BooleanArray::from(self.canonical)),
                false,
            ),
            (
                "mane_select",
                Arc::new(StringArray::from(self.mane_select)),
                true,
            ),
            (
                "sample_names_json",
                Arc::new(StringArray::from(self.sample_names_json)),
                false,
            ),
            ("format", Arc::new(StringArray::from(self.format)), true),
            (
                "samples_json",
                Arc::new(StringArray::from(self.samples_json)),
                false,
            ),
            (
                "consequences_json",
                Arc::new(StringArray::from(self.consequences_json)),
                false,
            ),
        ])
    }

    fn extend(&mut self, mut other: Self) {
        self.schema_version.append(&mut other.schema_version);
        self.allele_id.append(&mut other.allele_id);
        self.record_number.append(&mut other.record_number);
        self.alt_index.append(&mut other.alt_index);
        self.chromosome.append(&mut other.chromosome);
        self.position.append(&mut other.position);
        self.reference.append(&mut other.reference);
        self.alternate.append(&mut other.alternate);
        self.variant_id.append(&mut other.variant_id);
        self.quality.append(&mut other.quality);
        self.filter.append(&mut other.filter);
        self.gene_symbol.append(&mut other.gene_symbol);
        self.gene_id.append(&mut other.gene_id);
        self.transcript_id.append(&mut other.transcript_id);
        self.consequence.append(&mut other.consequence);
        self.impact.append(&mut other.impact);
        self.canonical.append(&mut other.canonical);
        self.mane_select.append(&mut other.mane_select);
        self.sample_names_json.append(&mut other.sample_names_json);
        self.format.append(&mut other.format);
        self.samples_json.append(&mut other.samples_json);
        self.consequences_json.append(&mut other.consequences_json);
    }
}

impl ConsequenceBatch {
    fn len(&self) -> usize {
        self.allele_id.len()
    }

    fn into_record_batch(self) -> Result<RecordBatch, String> {
        record_batch(vec![
            (
                "schema_version",
                Arc::new(Int32Array::from(self.schema_version)),
                false,
            ),
            (
                "consequence_id",
                Arc::new(StringArray::from(self.consequence_id)),
                false,
            ),
            (
                "allele_id",
                Arc::new(StringArray::from(self.allele_id)),
                false,
            ),
            ("ordinal", Arc::new(Int64Array::from(self.ordinal)), false),
            (
                "transcript_id",
                Arc::new(StringArray::from(self.transcript_id)),
                true,
            ),
            ("gene_id", Arc::new(StringArray::from(self.gene_id)), true),
            (
                "gene_symbol",
                Arc::new(StringArray::from(self.gene_symbol)),
                true,
            ),
            ("biotype", Arc::new(StringArray::from(self.biotype)), true),
            (
                "consequence_terms_json",
                Arc::new(StringArray::from(self.consequence_terms_json)),
                false,
            ),
            (
                "primary_consequence",
                Arc::new(StringArray::from(self.primary_consequence)),
                true,
            ),
            ("impact", Arc::new(StringArray::from(self.impact)), true),
            (
                "canonical",
                Arc::new(BooleanArray::from(self.canonical)),
                false,
            ),
            (
                "mane_select",
                Arc::new(StringArray::from(self.mane_select)),
                true,
            ),
            (
                "protein_id",
                Arc::new(StringArray::from(self.protein_id)),
                true,
            ),
            ("exon", Arc::new(StringArray::from(self.exon)), true),
            ("intron", Arc::new(StringArray::from(self.intron)), true),
            ("hgvsg", Arc::new(StringArray::from(self.hgvsg)), true),
            ("hgvsc", Arc::new(StringArray::from(self.hgvsc)), true),
            ("hgvsp", Arc::new(StringArray::from(self.hgvsp)), true),
            ("distance", Arc::new(Int64Array::from(self.distance)), true),
            ("strand", Arc::new(Int32Array::from(self.strand)), true),
            (
                "consequence_json",
                Arc::new(StringArray::from(self.consequence_json)),
                false,
            ),
        ])
    }
}

impl EvidenceBatch {
    fn len(&self) -> usize {
        self.allele_id.len()
    }

    fn into_record_batch(self) -> Result<RecordBatch, String> {
        record_batch(vec![
            (
                "schema_version",
                Arc::new(Int32Array::from(self.schema_version)),
                false,
            ),
            (
                "allele_id",
                Arc::new(StringArray::from(self.allele_id)),
                false,
            ),
            (
                "consequence_id",
                Arc::new(StringArray::from(self.consequence_id)),
                true,
            ),
            ("scope", Arc::new(StringArray::from(self.scope)), false),
            (
                "source_id",
                Arc::new(StringArray::from(self.source_id)),
                false,
            ),
            (
                "field_path",
                Arc::new(StringArray::from(self.field_path)),
                false,
            ),
            (
                "value_type",
                Arc::new(StringArray::from(self.value_type)),
                false,
            ),
            (
                "string_value",
                Arc::new(StringArray::from(self.string_value)),
                true,
            ),
            (
                "integer_value",
                Arc::new(Int64Array::from(self.integer_value)),
                true,
            ),
            (
                "number_value",
                Arc::new(Float64Array::from(self.number_value)),
                true,
            ),
            (
                "boolean_value",
                Arc::new(BooleanArray::from(self.boolean_value)),
                true,
            ),
            (
                "json_value",
                Arc::new(StringArray::from(self.json_value)),
                true,
            ),
        ])
    }
}

fn parquet_writer(
    path: &Path,
    schema: duckdb::arrow::datatypes::SchemaRef,
) -> Result<ArrowWriter<File>, String> {
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .set_max_row_group_row_count(Some(100_000))
        .build();
    let output =
        File::create(path).map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    ArrowWriter::try_new(output, schema, Some(properties))
        .map_err(|error| format!("cannot initialize {}: {error}", path.display()))
}

fn write_batch(
    writer: &mut ArrowWriter<File>,
    batch: RecordBatch,
    description: &str,
) -> Result<(), String> {
    if batch.num_rows() == 0 {
        return Ok(());
    }
    writer
        .write(&batch)
        .map_err(|error| format!("cannot write {description}: {error}"))
}

fn append_variant_batch(
    writer: &mut ArrowWriter<File>,
    batch: &mut VariantBatch,
) -> Result<(), String> {
    if batch.len() == 0 {
        return Ok(());
    }
    let first = batch.record_number.first().copied().unwrap_or_default();
    let last = batch.record_number.last().copied().unwrap_or_default();
    let record_batch = std::mem::take(batch).into_record_batch()?;
    write_batch(
        writer,
        record_batch,
        &format!("result records {first}-{last}"),
    )
}

fn parse_variant_record(
    input: &VariantRecord,
    fields: Option<&[String]>,
    sample_names: &[String],
    sample_names_json: &str,
) -> Result<VariantBatch, String> {
    let columns = input.line.split('\t').collect::<Vec<_>>();
    if columns.len() < 8 {
        return Err(format!(
            "VCF record on line {} has fewer than 8 columns",
            input.line_number
        ));
    }
    let position = columns[1].parse::<i64>().map_err(|_| {
        format!(
            "VCF record on line {} has an invalid position",
            input.line_number
        )
    })?;
    let quality = if columns[5] == "." {
        None
    } else {
        Some(columns[5].parse::<f64>().map_err(|_| {
            format!(
                "VCF record on line {} has an invalid quality",
                input.line_number
            )
        })?)
    };
    let consequences = fields
        .map(|fields| parse_consequences(columns[7], fields))
        .transpose()?
        .unwrap_or_default();
    let samples_json = samples_json(sample_names, &columns)?;
    let mut batch = VariantBatch::default();
    for (alt_offset, alternate) in columns[4].split(',').enumerate() {
        if !annocat_core::vcf::is_variant_alternate(alternate) {
            continue;
        }
        let matching = matching_consequences(&consequences, columns[3], alternate, columns[4]);
        if fields.is_some() && matching.is_empty() {
            return Err(format!(
                "VCF record on line {} has no CSQ entry for alternate allele {alternate}",
                input.line_number
            ));
        }
        let best = best_consequence(&matching);
        let best_value = |name: &str| {
            best.and_then(|entry| entry.get(name))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        batch.schema_version.push(SCHEMA_VERSION);
        batch
            .allele_id
            .push(allele_id(columns[0], position, columns[3], alternate));
        batch.record_number.push(input.record_number);
        batch.alt_index.push((alt_offset + 1) as i32);
        batch.chromosome.push(columns[0].to_owned());
        batch.position.push(position);
        batch.reference.push(columns[3].to_owned());
        batch.alternate.push(alternate.to_owned());
        batch.variant_id.push(optional_vcf(columns[2]));
        batch.quality.push(quality);
        batch.filter.push(columns[6].to_owned());
        batch.gene_symbol.push(best_value("SYMBOL"));
        batch.gene_id.push(best_value("Gene"));
        batch.transcript_id.push(best_value("Feature"));
        batch.consequence.push(best_value("Consequence"));
        batch.impact.push(best_value("IMPACT"));
        batch
            .canonical
            .push(best_value("CANONICAL").is_some_and(|value| value == "YES"));
        batch.mane_select.push(best_value("MANE_SELECT"));
        batch.sample_names_json.push(sample_names_json.to_owned());
        batch
            .format
            .push(columns.get(8).and_then(|value| optional_vcf(value)));
        batch.samples_json.push(samples_json.clone());
        batch.consequences_json.push(
            serde_json::to_string(&best.into_iter().collect::<Vec<_>>())
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(batch)
}

fn parse_and_write_variant_chunk(
    pool: &ThreadPool,
    records: &mut Vec<VariantRecord>,
    fields: Option<&[String]>,
    sample_names: &[String],
    sample_names_json: &str,
    writer: &mut ArrowWriter<File>,
) -> Result<u64, String> {
    let parsed = pool.install(|| {
        records
            .par_iter()
            .map(|record| parse_variant_record(record, fields, sample_names, sample_names_json))
            .collect::<Result<Vec<_>, _>>()
    })?;
    let mut batch = VariantBatch::default();
    for record in parsed {
        batch.extend(record);
    }
    let rows = batch.len() as u64;
    append_variant_batch(writer, &mut batch)?;
    records.clear();
    Ok(rows)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageRequest {
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub sort_evidence: Option<usize>,
    #[serde(default)]
    pub sorts: Vec<PageSortRequest>,
    #[serde(default)]
    pub known_total: Option<u64>,
    #[serde(default)]
    pub query_session: String,
    #[serde(default)]
    pub request_generation: u64,
    #[serde(default)]
    pub chromosome: String,
    #[serde(default)]
    pub position_min: Option<i64>,
    #[serde(default)]
    pub position_max: Option<i64>,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub alternate: String,
    #[serde(default)]
    pub variant_id: String,
    #[serde(default)]
    pub gene: String,
    #[serde(default)]
    pub transcript_id: String,
    #[serde(default)]
    pub consequence: String,
    #[serde(default)]
    pub impact: String,
    #[serde(default)]
    pub quality_min: Option<f64>,
    #[serde(default)]
    pub quality_max: Option<f64>,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub canonical: Option<bool>,
    #[serde(default)]
    pub evidence_columns: Vec<usize>,
    #[serde(default)]
    pub evidence_filters: Vec<EvidenceFilterRequest>,
    #[serde(default)]
    pub filter_rules: Vec<CoreFilterRuleRequest>,
    #[serde(default)]
    pub excluded_allele_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageSortRequest {
    pub column: String,
    pub direction: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreFilterRuleRequest {
    pub column: String,
    pub operator: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceFilterRequest {
    pub index: usize,
    pub operator: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value2: String,
}

#[derive(Default)]
struct CatalogEntry {
    types: BTreeSet<&'static str>,
    occurrences: u64,
}

pub fn convert_structured(
    ndjson: &Path,
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    catalog_json: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<StructuredSummary, String> {
    let _ = fs::remove_file(consequences_parquet);
    let _ = fs::remove_file(evidence_parquet);
    let result = convert_structured_inner(
        ndjson,
        consequences_parquet,
        evidence_parquet,
        catalog_json,
        &cancelled,
        &mut progress,
    );
    if result.is_err() {
        let _ = fs::remove_file(consequences_parquet);
        let _ = fs::remove_file(evidence_parquet);
        let _ = fs::remove_file(catalog_json);
    }
    result
}

fn convert_structured_inner(
    ndjson: &Path,
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    catalog_json: &Path,
    cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<StructuredSummary, String> {
    let consequence_schema = ConsequenceBatch::default().into_record_batch()?.schema();
    let evidence_schema = EvidenceBatch::default().into_record_batch()?.schema();
    let mut consequence_writer = parquet_writer(consequences_parquet, consequence_schema)?;
    let mut evidence_writer = parquet_writer(evidence_parquet, evidence_schema)?;

    let file = fs::File::open(ndjson)
        .map_err(|error| format!("cannot open {}: {error}", ndjson.display()))?;
    let mut catalog: BTreeMap<(String, String, String), CatalogEntry> = BTreeMap::new();
    let mut consequence_count = 0_u64;
    let mut evidence_count = 0_u64;
    let mut structured_records = 0_u64;
    let mut consequence_batch = ConsequenceBatch::default();
    let mut evidence_batch = EvidenceBatch::default();
    let mut previous_bytes = 0_u64;
    let mut previous_records = 0_u64;
    let mut previous_at = Instant::now();
    {
        for (record_index, line) in BufReader::new(file).lines().enumerate() {
            if cancelled() {
                return Err("cancelled".into());
            }
            let line = line.map_err(|error| format!("cannot read structured output: {error}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(&line).map_err(|error| {
                format!(
                    "invalid structured output record {}: {error}",
                    record_index + 1
                )
            })?;
            let object = record.as_object().ok_or_else(|| {
                format!(
                    "structured output record {} is not an object",
                    record_index + 1
                )
            })?;
            let chromosome = string_field(object, "seq_region_name")?;
            let position = integer_field(object, "start")?;
            let allele_string = string_field(object, "allele_string")?;
            let alleles = allele_string.split('/').collect::<Vec<_>>();
            if alleles.len() > 1
                && alleles[1..]
                    .iter()
                    .any(|alternate| annocat_core::vcf::is_variant_alternate(alternate))
            {
                structured_records += 1;
                let reference = alleles[0];

                for (key, value) in object {
                    if !TOP_LEVEL_FIELDS.contains(&key.as_str()) {
                        for alternate in alleles[1..]
                            .iter()
                            .filter(|alternate| annocat_core::vcf::is_variant_alternate(alternate))
                        {
                            let id = allele_id(chromosome, position, reference, alternate);
                            let context = EvidenceContext {
                                allele_id: &id,
                                consequence_id: None,
                                scope: "allele",
                                source_id: key,
                            };
                            evidence_count += append_evidence_tree(
                                &mut evidence_batch,
                                &mut catalog,
                                &context,
                                "",
                                value,
                            )?;
                        }
                    }
                }

                let provided_consequences = object
                    .get("transcript_consequences")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let fallback_consequences = if provided_consequences.is_empty() {
                    let term = object
                        .get("most_severe_consequence")
                        .and_then(Value::as_str)
                        .unwrap_or("intergenic_variant");
                    alleles[1..]
                        .iter()
                        .filter(|alternate| annocat_core::vcf::is_variant_alternate(alternate))
                        .map(|alternate| {
                            json!({
                                "variant_allele": alternate,
                                "consequence_terms": [term],
                                "impact": "MODIFIER"
                            })
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let consequences = if provided_consequences.is_empty() {
                    fallback_consequences.as_slice()
                } else {
                    provided_consequences
                };
                let mut shared_evidence_written = BTreeSet::new();
                for (ordinal, consequence) in consequences.iter().enumerate() {
                    let consequence_object = consequence.as_object().ok_or_else(|| {
                        format!(
                            "structured consequence {} is not an object",
                            consequence_count + 1
                        )
                    })?;
                    let alternate = consequence_object
                        .get("variant_allele")
                        .and_then(Value::as_str)
                        .or_else(|| alleles.get(1).copied())
                        .ok_or("structured consequence has no alternate allele")?;
                    if !annocat_core::vcf::is_variant_alternate(alternate) {
                        continue;
                    }
                    let id = allele_id(chromosome, position, reference, alternate);
                    let consequence_id = format!("{id}:tx:{}", consequence_count + 1);
                    let terms = consequence_object
                        .get("consequence_terms")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let terms_json =
                        serde_json::to_string(&terms).map_err(|error| error.to_string())?;
                    let primary = terms.first().and_then(Value::as_str).map(str::to_owned);
                    let raw_json =
                        serde_json::to_string(consequence).map_err(|error| error.to_string())?;
                    consequence_batch.schema_version.push(SCHEMA_VERSION);
                    consequence_batch
                        .consequence_id
                        .push(consequence_id.clone());
                    consequence_batch.allele_id.push(id.clone());
                    consequence_batch.ordinal.push(ordinal as i64);
                    consequence_batch
                        .transcript_id
                        .push(optional_json_string(consequence_object, "transcript_id"));
                    consequence_batch
                        .gene_id
                        .push(optional_json_string(consequence_object, "gene_id"));
                    consequence_batch
                        .gene_symbol
                        .push(optional_json_string(consequence_object, "gene_symbol"));
                    consequence_batch
                        .biotype
                        .push(optional_json_string(consequence_object, "biotype"));
                    consequence_batch.consequence_terms_json.push(terms_json);
                    consequence_batch.primary_consequence.push(primary);
                    consequence_batch
                        .impact
                        .push(optional_json_string(consequence_object, "impact"));
                    consequence_batch
                        .canonical
                        .push(json_bool(consequence_object.get("canonical")));
                    consequence_batch
                        .mane_select
                        .push(optional_json_string(consequence_object, "mane_select"));
                    consequence_batch
                        .protein_id
                        .push(optional_json_string(consequence_object, "protein_id"));
                    consequence_batch
                        .exon
                        .push(optional_json_string(consequence_object, "exon"));
                    consequence_batch
                        .intron
                        .push(optional_json_string(consequence_object, "intron"));
                    consequence_batch
                        .hgvsg
                        .push(optional_json_string(consequence_object, "hgvsg"));
                    consequence_batch
                        .hgvsc
                        .push(optional_json_string(consequence_object, "hgvsc"));
                    consequence_batch
                        .hgvsp
                        .push(optional_json_string(consequence_object, "hgvsp"));
                    consequence_batch
                        .distance
                        .push(optional_json_i64(consequence_object, "distance"));
                    consequence_batch.strand.push(
                        optional_json_i64(consequence_object, "strand").map(|value| value as i32),
                    );
                    consequence_batch.consequence_json.push(raw_json);
                    consequence_count += 1;

                    for (key, value) in consequence_object {
                        if !CONSEQUENCE_FIELDS.contains(&key.as_str()) {
                            let shared = evidence_is_shared(consequences, alternate, key, value);
                            if shared && !shared_evidence_written.insert((id.clone(), key.clone()))
                            {
                                continue;
                            }
                            let context = EvidenceContext {
                                allele_id: &id,
                                consequence_id: (!shared).then_some(consequence_id.as_str()),
                                scope: if shared { "allele" } else { "transcript" },
                                source_id: key,
                            };
                            evidence_count += append_evidence_tree(
                                &mut evidence_batch,
                                &mut catalog,
                                &context,
                                "",
                                value,
                            )?;
                        }
                    }
                }
            } else {
                // fastVEP emits the reference allele alone for non-variant VCF
                // records. They contribute no allele, consequence, or evidence
                // rows, but still count toward conversion progress.
            }
            if consequence_batch.len() >= VARIANT_CHUNK_RECORDS {
                let batch = std::mem::take(&mut consequence_batch).into_record_batch()?;
                write_batch(&mut consequence_writer, batch, "transcript consequences")?;
            }
            if evidence_batch.len() >= VARIANT_CHUNK_RECORDS {
                let batch = std::mem::take(&mut evidence_batch).into_record_batch()?;
                write_batch(&mut evidence_writer, batch, "source evidence")?;
            }
            if structured_records.is_multiple_of(10_000) {
                let bytes = [consequences_parquet, evidence_parquet]
                    .iter()
                    .filter_map(|path| fs::metadata(path).ok())
                    .map(|metadata| metadata.len())
                    .sum::<u64>();
                let now = Instant::now();
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                let bytes_per_second = if elapsed > 0.0 {
                    bytes.saturating_sub(previous_bytes) as f64 / elapsed
                } else {
                    0.0
                };
                let records_per_second = if elapsed > 0.0 {
                    structured_records.saturating_sub(previous_records) as f64 / elapsed
                } else {
                    0.0
                };
                progress(
                    structured_records,
                    true,
                    bytes,
                    bytes_per_second,
                    records_per_second,
                );
                previous_bytes = bytes;
                previous_records = structured_records;
                previous_at = now;
            }
        }
    }
    if consequence_count == 0 {
        return Err("structured result contains no transcript consequences".into());
    }
    write_batch(
        &mut consequence_writer,
        consequence_batch.into_record_batch()?,
        "transcript consequences",
    )?;
    write_batch(
        &mut evidence_writer,
        evidence_batch.into_record_batch()?,
        "source evidence",
    )?;
    consequence_writer
        .close()
        .map_err(|error| format!("cannot finish transcript consequences: {error}"))?;
    evidence_writer
        .close()
        .map_err(|error| format!("cannot finish source evidence: {error}"))?;
    let output_bytes = fs::metadata(consequences_parquet)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        + fs::metadata(evidence_parquet)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    progress(structured_records, true, output_bytes, 0.0, 0.0);

    let fields = catalog
        .iter()
        .map(|((scope, source_id, field_path), entry)| {
            let mut field = json!({
                "scope": scope,
                "sourceId": source_id,
                "fieldPath": field_path,
                "valueType": if entry.types.len() == 1 { *entry.types.iter().next().unwrap() } else { "mixed" },
                "observedTypes": entry.types,
                "occurrences": entry.occurrences,
            });
            if let Some(group) = crate::evidence_resolution::bundled_alignment_group(
                scope,
                source_id,
                field_path,
            ) {
                field["alignmentGroup"] = Value::String(group);
            }
            field
        })
        .collect::<Vec<_>>();
    let alignment_groups = crate::evidence_resolution::catalog_alignment_groups(&fields);
    fs::write(
        catalog_json,
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": SCHEMA_VERSION,
            "fields": fields,
            "alignmentGroups": alignment_groups
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write field catalog: {error}"))?;
    let source_value_counts = catalog.iter().fold(
        BTreeMap::<String, u64>::new(),
        |mut counts, ((_, source, _), entry)| {
            *counts.entry(source.clone()).or_default() += entry.occurrences;
            counts
        },
    );
    Ok(StructuredSummary {
        records: structured_records,
        consequences: consequence_count,
        evidence: evidence_count,
        fields: catalog.len(),
        sources: catalog
            .keys()
            .map(|(_, source, _)| source.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        source_value_counts,
    })
}

pub fn convert_vcf(
    vcf: &Path,
    parquet: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    convert_vcf_mode(vcf, parquet, true, cancelled, &mut progress)
}

pub fn convert_input_vcf(
    vcf: &Path,
    parquet: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    convert_vcf_mode(vcf, parquet, false, cancelled, &mut progress)
}

fn convert_vcf_mode(
    vcf: &Path,
    parquet: &Path,
    require_csq: bool,
    cancelled: impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    let _ = fs::remove_file(parquet);
    let result = convert_vcf_inner(vcf, parquet, require_csq, &cancelled, progress);
    if result.is_err() {
        let _ = fs::remove_file(parquet);
    }
    result
}

fn convert_vcf_inner(
    vcf: &Path,
    parquet: &Path,
    require_csq: bool,
    cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    let schema = VariantBatch::default().into_record_batch()?.schema();
    let mut writer = parquet_writer(parquet, schema)?;
    let parser_workers = std::thread::available_parallelism()
        .map(|workers| workers.get().min(4))
        .unwrap_or(1);
    let parser_pool = ThreadPoolBuilder::new()
        .num_threads(parser_workers)
        .thread_name(|index| format!("annocat-result-parser-{index}"))
        .build()
        .map_err(|error| format!("cannot initialize result parser workers: {error}"))?;

    let reader = super::csq::open(vcf)?;
    let mut csq_fields: Option<Vec<String>> = None;
    let mut sample_names = Vec::new();
    let mut sample_names_json = "[]".to_owned();
    let mut record_number = 0_i64;
    let mut rows = 0_u64;
    {
        let mut records = Vec::with_capacity(VARIANT_CHUNK_RECORDS);
        let mut previous_bytes = 0_u64;
        let mut previous_records = 0_u64;
        let mut previous_at = Instant::now();
        for (line_index, line) in reader.lines().enumerate() {
            if cancelled() {
                return Err("cancelled".into());
            }
            let line = line.map_err(|error| format!("cannot read {}: {error}", vcf.display()))?;
            if line.starts_with("##INFO=<ID=CSQ,") {
                csq_fields = Some(super::csq::parse_header(&line)?);
                continue;
            }
            if line.starts_with("#CHROM\t") {
                let columns = line.split('\t').collect::<Vec<_>>();
                if columns.len() > 9 {
                    sample_names = columns[9..]
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect();
                }
                sample_names_json =
                    serde_json::to_string(&sample_names).map_err(|error| error.to_string())?;
                continue;
            }
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            record_number += 1;
            records.push(VariantRecord {
                line_number: line_index + 1,
                record_number,
                line,
            });
            if records.len() >= VARIANT_CHUNK_RECORDS {
                let fields = match csq_fields.as_deref() {
                    Some(fields) => Some(fields),
                    None if require_csq => {
                        return Err("VCF record appears before the CSQ schema".into());
                    }
                    None => None,
                };
                rows += parse_and_write_variant_chunk(
                    &parser_pool,
                    &mut records,
                    fields,
                    &sample_names,
                    &sample_names_json,
                    &mut writer,
                )?;
                let bytes = fs::metadata(parquet).map(|value| value.len()).unwrap_or(0);
                let now = Instant::now();
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                let bytes_per_second = if elapsed > 0.0 {
                    bytes.saturating_sub(previous_bytes) as f64 / elapsed
                } else {
                    0.0
                };
                let records_per_second = if elapsed > 0.0 {
                    (record_number as u64).saturating_sub(previous_records) as f64 / elapsed
                } else {
                    0.0
                };
                progress(
                    record_number as u64,
                    true,
                    bytes,
                    bytes_per_second,
                    records_per_second,
                );
                previous_bytes = bytes;
                previous_records = record_number as u64;
                previous_at = now;
            }
        }
        if !records.is_empty() {
            let fields = match csq_fields.as_deref() {
                Some(fields) => Some(fields),
                None if require_csq => {
                    return Err("VCF record appears before the CSQ schema".into());
                }
                None => None,
            };
            rows += parse_and_write_variant_chunk(
                &parser_pool,
                &mut records,
                fields,
                &sample_names,
                &sample_names_json,
                &mut writer,
            )?;
        }
    }
    if rows == 0 {
        return Err("canonical result contains no allele rows".into());
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    writer
        .close()
        .map_err(|error| format!("cannot finish canonical Parquet result: {error}"))?;
    progress(
        record_number as u64,
        true,
        fs::metadata(parquet).map(|value| value.len()).unwrap_or(0),
        0.0,
        0.0,
    );
    validate(parquet, rows)?;
    Ok(CanonicalSummary {
        rows,
        records: record_number as u64,
        samples: sample_names,
    })
}

pub fn write_empty_detail_tables(
    consequences: &Path,
    evidence: &Path,
    catalog: &Path,
) -> Result<(), String> {
    let _ = fs::remove_file(consequences);
    let _ = fs::remove_file(evidence);
    let _ = fs::remove_file(catalog);

    let consequence_schema = ConsequenceBatch::default().into_record_batch()?.schema();
    parquet_writer(consequences, consequence_schema)?
        .close()
        .map_err(|error| format!("cannot finish empty consequence table: {error}"))?;

    let evidence_schema = EvidenceBatch::default().into_record_batch()?.schema();
    parquet_writer(evidence, evidence_schema)?
        .close()
        .map_err(|error| format!("cannot finish empty evidence table: {error}"))?;

    fs::write(
        catalog,
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": SCHEMA_VERSION,
            "fields": []
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write empty field catalog: {error}"))?;
    Ok(())
}

pub fn validate(parquet: &Path, expected_rows: u64) -> Result<(), String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let path = parquet.to_string_lossy();
    let (rows, minimum_schema, maximum_schema): (i64, i32, i32) = connection
        .query_row(
            "SELECT count(*), min(schema_version), max(schema_version) FROM read_parquet(?)",
            params![path.as_ref()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("cannot validate canonical Parquet result: {error}"))?;
    if rows as u64 != expected_rows {
        return Err(format!(
            "canonical result has {rows} rows; expected {expected_rows}"
        ));
    }
    if minimum_schema != SCHEMA_VERSION || maximum_schema != SCHEMA_VERSION {
        return Err("canonical result contains an unsupported schema version".into());
    }
    Ok(())
}

pub fn validate_report_tables(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    catalog: &Path,
    expected_variants: u64,
) -> Result<(), String> {
    validate_report_tables_mode(
        variants,
        consequences,
        evidence,
        catalog,
        expected_variants,
        true,
    )
}

pub fn validate_report_tables_allow_empty_consequences(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    catalog: &Path,
    expected_variants: u64,
) -> Result<(), String> {
    validate_report_tables_mode(
        variants,
        consequences,
        evidence,
        catalog,
        expected_variants,
        false,
    )
}

fn validate_report_tables_mode(
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    catalog: &Path,
    expected_variants: u64,
    require_consequences: bool,
) -> Result<(), String> {
    validate(variants, expected_variants)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let consequence_path = consequences.to_string_lossy();
    let evidence_path = evidence.to_string_lossy();
    let (consequence_rows, consequence_min, consequence_max): (i64, Option<i32>, Option<i32>) =
        connection
            .query_row(
                "SELECT count(*), min(schema_version), max(schema_version) FROM read_parquet(?)",
                params![consequence_path.as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| format!("cannot validate consequence table: {error}"))?;
    if require_consequences && consequence_rows <= 0 {
        return Err("report consequence table contains no rows".into());
    }
    if consequence_rows > 0 && (consequence_min != Some(1) || consequence_max != Some(1)) {
        return Err("report consequence table has an invalid schema version or no rows".into());
    }
    connection
        .prepare(
            "SELECT consequence_id, allele_id, ordinal, transcript_id, gene_id, gene_symbol,
                    biotype, consequence_terms_json, primary_consequence, impact, canonical,
                    mane_select, protein_id, exon, intron, hgvsg, hgvsc, hgvsp, distance,
                    strand, consequence_json
             FROM read_parquet(?) LIMIT 0",
        )
        .and_then(|mut statement| statement.exists(params![consequence_path.as_ref()]))
        .map_err(|error| format!("report consequence schema is incompatible: {error}"))?;

    let (evidence_rows, evidence_min, evidence_max): (i64, Option<i32>, Option<i32>) = connection
        .query_row(
            "SELECT count(*), min(schema_version), max(schema_version) FROM read_parquet(?)",
            params![evidence_path.as_ref()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("cannot validate evidence table: {error}"))?;
    if evidence_rows > 0 && (evidence_min != Some(1) || evidence_max != Some(1)) {
        return Err("report evidence table has an invalid schema version".into());
    }
    connection
        .prepare(
            "SELECT allele_id, consequence_id, scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?) LIMIT 0",
        )
        .and_then(|mut statement| statement.exists(params![evidence_path.as_ref()]))
        .map_err(|error| format!("report evidence schema is incompatible: {error}"))?;

    let variant_path = variants.to_string_lossy();
    let orphan_consequences: i64 = connection
        .query_row(
            "SELECT count(*) FROM read_parquet(?) c
             WHERE NOT EXISTS (SELECT 1 FROM read_parquet(?) v WHERE v.allele_id=c.allele_id)",
            params![consequence_path.as_ref(), variant_path.as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot validate consequence allele references: {error}"))?;
    let orphan_evidence: i64 = connection
        .query_row(
            "SELECT count(*) FROM read_parquet(?) e
             WHERE NOT EXISTS (SELECT 1 FROM read_parquet(?) v WHERE v.allele_id=e.allele_id)",
            params![evidence_path.as_ref(), variant_path.as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot validate evidence allele references: {error}"))?;
    if orphan_consequences != 0 || orphan_evidence != 0 {
        return Err("report contains consequence or evidence rows for unknown alleles".into());
    }

    let metadata = fs::metadata(catalog)
        .map_err(|error| format!("report field catalog is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("report field catalog has an invalid size".into());
    }
    crate::evidence_resolution::validate_catalog(catalog)?;
    let catalog_value: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid report field catalog: {error}"))?;
    if catalog_value["schemaVersion"] != 1 || !catalog_value["fields"].is_array() {
        return Err("report field catalog has an unsupported schema".into());
    }
    Ok(())
}

struct CorePageFilters {
    search: String,
    chromosome: String,
    reference: String,
    alternate: String,
    variant_id: String,
    gene: String,
    transcript_id: String,
    consequence: String,
    impact: String,
    filter: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterValueKind {
    Text,
    Number,
    Boolean,
}

fn core_filter_column(column: &str) -> Option<(&'static str, FilterValueKind)> {
    match column {
        "chromosome" => Some(("v.chromosome", FilterValueKind::Text)),
        "position" => Some(("v.position", FilterValueKind::Number)),
        "reference" => Some(("v.reference", FilterValueKind::Text)),
        "alternate" => Some(("v.alternate", FilterValueKind::Text)),
        "variantId" => Some(("v.variant_id", FilterValueKind::Text)),
        "quality" => Some(("v.quality", FilterValueKind::Number)),
        "filter" => Some(("v.filter", FilterValueKind::Text)),
        "gene" => Some(("v.gene_symbol", FilterValueKind::Text)),
        "geneId" => Some(("v.gene_id", FilterValueKind::Text)),
        "transcriptId" => Some(("v.transcript_id", FilterValueKind::Text)),
        "consequence" => Some(("v.consequence", FilterValueKind::Text)),
        "impact" => Some(("v.impact", FilterValueKind::Text)),
        "canonical" => Some(("v.canonical", FilterValueKind::Boolean)),
        "maneSelect" => Some(("v.mane_select", FilterValueKind::Text)),
        _ => None,
    }
}

fn comma_filter_values(value: &str) -> Result<Vec<String>, String> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            bounded_page_text(value, "list item", 100).map(|value| value.to_ascii_lowercase())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        return Err("comma-separated filter list cannot be empty".into());
    }
    if values.len() > 2_000 {
        return Err("comma-separated filters are limited to 2,000 values".into());
    }
    Ok(values)
}

fn comparison_sql(
    expression: &str,
    kind: FilterValueKind,
    operator: &str,
    value: &str,
) -> Result<(String, Vec<SqlValue>), String> {
    let value = bounded_page_text(value, "rule value", 32 * 1024)?;
    match (kind, operator) {
        (FilterValueKind::Text, "equals") => Ok((
            format!("lower(coalesce(CAST({expression} AS VARCHAR), '')) = lower(?)"),
            vec![value.to_owned().into()],
        )),
        (FilterValueKind::Text, "not_equals") => Ok((
            format!("lower(coalesce(CAST({expression} AS VARCHAR), '')) <> lower(?)"),
            vec![value.to_owned().into()],
        )),
        (FilterValueKind::Text, "contains") => Ok((
            format!("contains(lower(coalesce(CAST({expression} AS VARCHAR), '')), lower(?))"),
            vec![value.to_owned().into()],
        )),
        (FilterValueKind::Text, "not_contains") => Ok((
            format!("NOT contains(lower(coalesce(CAST({expression} AS VARCHAR), '')), lower(?))"),
            vec![value.to_owned().into()],
        )),
        (FilterValueKind::Text, "in") => {
            let values = comma_filter_values(value)?;
            let placeholders = std::iter::repeat_n("?", values.len())
                .collect::<Vec<_>>()
                .join(",");
            Ok((
                format!("lower(coalesce(CAST({expression} AS VARCHAR), '')) IN ({placeholders})"),
                values.into_iter().map(Into::into).collect(),
            ))
        }
        (
            FilterValueKind::Number,
            operator @ ("equals" | "not_equals" | "gt" | "gte" | "lt" | "lte"),
        ) => {
            let number = value
                .parse::<f64>()
                .map_err(|_| "numeric filter value must be a number".to_string())?;
            if !number.is_finite() {
                return Err("numeric filter value must be finite".into());
            }
            let symbol = match operator {
                "equals" => "=",
                "not_equals" => "<>",
                "gt" => ">",
                "gte" => ">=",
                "lt" => "<",
                "lte" => "<=",
                _ => unreachable!(),
            };
            Ok((
                format!("CAST({expression} AS DOUBLE) {symbol} CAST(? AS DOUBLE)"),
                vec![number.into()],
            ))
        }
        (
            FilterValueKind::Text | FilterValueKind::Boolean,
            operator @ ("gt" | "gte" | "lt" | "lte"),
        ) => {
            let number = value
                .parse::<f64>()
                .map_err(|_| "numeric comparison value must be a number".to_string())?;
            if !number.is_finite() {
                return Err("numeric comparison value must be finite".into());
            }
            let symbol = match operator {
                "gt" => ">",
                "gte" => ">=",
                "lt" => "<",
                "lte" => "<=",
                _ => unreachable!(),
            };
            Ok((
                format!("try_cast({expression} AS DOUBLE) {symbol} CAST(? AS DOUBLE)"),
                vec![number.into()],
            ))
        }
        (
            FilterValueKind::Number | FilterValueKind::Boolean,
            operator @ ("contains" | "not_contains"),
        ) => Ok((
            format!(
                "{}contains(lower(coalesce(CAST({expression} AS VARCHAR), '')), lower(?))",
                if operator == "not_contains" {
                    "NOT "
                } else {
                    ""
                }
            ),
            vec![value.to_owned().into()],
        )),
        (FilterValueKind::Number | FilterValueKind::Boolean, "in") => {
            let values = comma_filter_values(value)?;
            let placeholders = std::iter::repeat_n("?", values.len())
                .collect::<Vec<_>>()
                .join(",");
            Ok((
                format!("lower(coalesce(CAST({expression} AS VARCHAR), '')) IN ({placeholders})"),
                values.into_iter().map(Into::into).collect(),
            ))
        }
        (FilterValueKind::Boolean, operator @ ("equals" | "not_equals")) => {
            let boolean = match value.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => true,
                "false" | "no" | "0" => false,
                _ => return Err("boolean filter value must be Yes or No".into()),
            };
            Ok((
                format!(
                    "CAST({expression} AS BOOLEAN) {} CAST(? AS BOOLEAN)",
                    if operator == "equals" { "=" } else { "<>" }
                ),
                vec![boolean.into()],
            ))
        }
        _ => Err(format!(
            "operator '{operator}' is not valid for this column"
        )),
    }
}

fn core_filter_rules_sql(request: &PageRequest) -> Result<(String, Vec<SqlValue>), String> {
    if request.filter_rules.len() > 24 {
        return Err("at most 24 filter rules can be applied at once".into());
    }
    let mut sql = String::new();
    let mut parameters = Vec::new();
    for rule in &request.filter_rules {
        let (expression, kind) = core_filter_column(rule.column.trim())
            .ok_or_else(|| format!("unknown filter column: {}", rule.column))?;
        let (condition, values) =
            comparison_sql(expression, kind, rule.operator.trim(), &rule.value)?;
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
        parameters.extend(values);
    }
    Ok((sql, parameters))
}

fn excluded_alleles_sql(request: &PageRequest) -> Result<(String, Vec<SqlValue>), String> {
    if request.excluded_allele_ids.len() > 10_000 {
        return Err("at most 10,000 individually deselected variants are supported".into());
    }
    if request.excluded_allele_ids.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut seen = HashSet::new();
    let mut parameters = Vec::with_capacity(request.excluded_allele_ids.len());
    for allele_id in &request.excluded_allele_ids {
        let allele_id = bounded_page_text(allele_id, "excluded allele ID", 200)?;
        if allele_id.is_empty() {
            return Err("excluded allele IDs cannot be empty".into());
        }
        if seen.insert(allele_id) {
            parameters.push(allele_id.to_owned().into());
        }
    }
    let placeholders = std::iter::repeat("?")
        .take(parameters.len())
        .collect::<Vec<_>>()
        .join(", ");
    Ok((
        format!(" AND v.allele_id NOT IN ({placeholders})"),
        parameters,
    ))
}

const CORE_PAGE_WHERE_SQL: &str = "v.alternate NOT IN ('.', '', '<NON_REF>', '<*>')
         AND (? = '' OR lower(chromosome) = lower(?))
         AND (CAST(? AS BIGINT) IS NULL OR position >= CAST(? AS BIGINT))
         AND (CAST(? AS BIGINT) IS NULL OR position <= CAST(? AS BIGINT))
         AND (? = '' OR lower(reference) = lower(?))
         AND (? = '' OR lower(alternate) = lower(?))
         AND (? = '' OR contains(lower(coalesce(variant_id, '')), lower(?)))
         AND (? = '' OR contains(lower(concat_ws(' ', coalesce(gene_symbol, ''),
             coalesce(gene_id, ''))), lower(?)))
         AND (? = '' OR contains(lower(coalesce(transcript_id, '')), lower(?)))
         AND (? = '' OR contains(lower(coalesce(consequence, '')), lower(?)))
         AND (? = '' OR upper(coalesce(impact, '')) = upper(?))
         AND (CAST(? AS DOUBLE) IS NULL OR quality >= CAST(? AS DOUBLE))
         AND (CAST(? AS DOUBLE) IS NULL OR quality <= CAST(? AS DOUBLE))
         AND (? = '' OR lower(filter) = lower(?))
         AND (CAST(? AS BOOLEAN) IS NULL OR canonical = CAST(? AS BOOLEAN))";

fn core_page_params(path: &str, request: &PageRequest, filters: &CorePageFilters) -> Vec<SqlValue> {
    vec![
        path.to_owned().into(),
        filters.chromosome.to_owned().into(),
        filters.chromosome.to_owned().into(),
        request.position_min.into(),
        request.position_min.into(),
        request.position_max.into(),
        request.position_max.into(),
        filters.reference.to_owned().into(),
        filters.reference.to_owned().into(),
        filters.alternate.to_owned().into(),
        filters.alternate.to_owned().into(),
        filters.variant_id.to_owned().into(),
        filters.variant_id.to_owned().into(),
        filters.gene.to_owned().into(),
        filters.gene.to_owned().into(),
        filters.transcript_id.to_owned().into(),
        filters.transcript_id.to_owned().into(),
        filters.consequence.to_owned().into(),
        filters.consequence.to_owned().into(),
        filters.impact.to_owned().into(),
        filters.impact.to_owned().into(),
        request.quality_min.into(),
        request.quality_min.into(),
        request.quality_max.into(),
        request.quality_max.into(),
        filters.filter.to_owned().into(),
        filters.filter.to_owned().into(),
        request.canonical.into(),
        request.canonical.into(),
    ]
}

fn validated_core_page_filters(request: &PageRequest) -> Result<CorePageFilters, String> {
    if request.evidence_columns.len() > 32 {
        return Err("at most 32 evidence columns can be displayed at once".into());
    }
    if request
        .evidence_columns
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .len()
        != request.evidence_columns.len()
    {
        return Err("evidence columns cannot be repeated".into());
    }
    let filters = CorePageFilters {
        search: bounded_page_text(&request.search, "search", 200)?
            .split(|character: char| {
                character.is_whitespace() || character == '_' || character == '-'
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        chromosome: bounded_page_text(&request.chromosome, "chromosome", 40)?.to_owned(),
        reference: bounded_page_text(&request.reference, "reference", 200)?.to_owned(),
        alternate: bounded_page_text(&request.alternate, "alternate", 200)?.to_owned(),
        variant_id: bounded_page_text(&request.variant_id, "variant ID", 100)?.to_owned(),
        gene: bounded_page_text(&request.gene, "gene", 100)?.to_owned(),
        transcript_id: bounded_page_text(&request.transcript_id, "transcript", 100)?.to_owned(),
        consequence: bounded_page_text(&request.consequence, "consequence", 100)?.to_owned(),
        impact: bounded_page_text(&request.impact, "impact", 20)?.to_ascii_uppercase(),
        filter: bounded_page_text(&request.filter, "FILTER", 100)?.to_owned(),
    };
    if !filters.impact.is_empty()
        && !matches!(
            filters.impact.as_str(),
            "HIGH" | "MODERATE" | "LOW" | "MODIFIER"
        )
    {
        return Err("impact filter must be HIGH, MODERATE, LOW, or MODIFIER".into());
    }
    if request
        .position_min
        .zip(request.position_max)
        .is_some_and(|(min, max)| min > max)
    {
        return Err("minimum position cannot exceed maximum position".into());
    }
    if request
        .quality_min
        .into_iter()
        .chain(request.quality_max)
        .any(|value| !value.is_finite())
    {
        return Err("quality filters must be finite numbers".into());
    }
    if request
        .quality_min
        .zip(request.quality_max)
        .is_some_and(|(min, max)| min > max)
    {
        return Err("minimum quality cannot exceed maximum quality".into());
    }
    Ok(filters)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn page_json(
    parquet: &Path,
    offset: u64,
    limit: u64,
    request: &PageRequest,
) -> Result<String, String> {
    page_json_internal(parquet, None, None, offset, limit, request, None)
}

pub fn existing_allele_ids(
    parquet: &Path,
    allele_ids: &[String],
) -> Result<HashSet<String>, String> {
    if allele_ids.is_empty() || allele_ids.len() > 1_000 {
        return Err("allele lookup needs between 1 and 1,000 identifiers".into());
    }
    let placeholders = std::iter::repeat_n("?", allele_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT allele_id FROM read_parquet(?) WHERE allele_id IN ({placeholders})");
    let mut parameters = Vec::<SqlValue>::with_capacity(allele_ids.len() + 1);
    parameters.push(parquet.to_string_lossy().into_owned().into());
    parameters.extend(allele_ids.iter().cloned().map(Into::into));
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("cannot prepare allele lookup: {error}"))?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("cannot read allele lookup: {error}"))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

fn page_json_internal(
    parquet: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
    candidate_ids: Option<&[String]>,
) -> Result<String, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let query = PageQuery {
        variants: parquet,
        evidence,
        catalog,
        offset,
        limit,
        request,
        candidate_ids,
    };
    serde_json::to_string(&page_result_internal(&connection, &query)?)
        .map_err(|error| error.to_string())
}

fn page_result_internal(
    connection: &Connection,
    query: &PageQuery<'_>,
) -> Result<ResultPage, String> {
    let parquet = query.variants;
    let evidence = query.evidence;
    let catalog = query.catalog;
    let offset = query.offset;
    let limit = query.limit;
    let request = query.request;
    let candidate_ids = query.candidate_ids;
    let limit = limit.clamp(1, 500);
    let core_filters = validated_core_page_filters(request)?;
    let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
    let (evidence_rule_sql, evidence_rule_params) =
        evidence_filter_rules_sql(evidence, catalog, request)?;
    let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
    let (search_sql, mut search_params) =
        displayed_field_search_sql(connection, evidence, catalog, request, &core_filters.search)?;
    let excluded_sql = format!("{search_sql}{excluded_sql}");
    search_params.extend(excluded_params);
    let excluded_params = search_params;
    let page_sorts = page_sort_specs(evidence, catalog, request)?;
    let primary_sort = page_sorts
        .first()
        .expect("every result page has an input-order sort");
    let sort_key = primary_sort.key.clone();
    let direction = primary_sort.direction.as_str();
    let candidate_sql = candidate_ids
        .map(|_| " AND v.allele_id IN (SELECT allele_id FROM candidate_alleles)")
        .unwrap_or_default();
    let where_sql = format!(
        "{CORE_PAGE_WHERE_SQL}{core_rule_sql}{evidence_rule_sql}{excluded_sql}{candidate_sql}"
    );
    if let Some(candidate_ids) = candidate_ids {
        connection
            .execute_batch("CREATE TEMP TABLE candidate_alleles(allele_id VARCHAR PRIMARY KEY)")
            .map_err(|error| format!("cannot create candidate query table: {error}"))?;
        if !candidate_ids.is_empty() {
            let placeholders = std::iter::repeat_n("(?)", candidate_ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let values = candidate_ids
                .iter()
                .cloned()
                .map(Into::into)
                .collect::<Vec<SqlValue>>();
            connection
                .execute(
                    &format!("INSERT OR IGNORE INTO candidate_alleles VALUES {placeholders}"),
                    params_from_iter(values.iter()),
                )
                .map_err(|error| format!("cannot populate candidate query: {error}"))?;
        }
    }
    let path = parquet.to_string_lossy();
    let optimized_evidence_sort = (page_sorts.len() == 1)
        .then(|| primary_sort.evidence.as_ref())
        .flatten();
    if request.known_total.is_none() && optimized_evidence_sort.is_none() {
        let order_sql = if page_sorts.len() == 1 && sort_key == "input" && direction == "ASC" {
            String::new()
        } else {
            let terms = page_sorts
                .iter()
                .map(|sort| format!("{} {} NULLS LAST", sort.expression, sort.direction))
                .chain(["record_number ASC".into(), "alt_index ASC".into()])
                .collect::<Vec<String>>()
                .join(", ");
            format!(" ORDER BY {terms}")
        };
        let sql = format!(
            "SELECT {RESULT_PAGE_COLUMNS}, count(*) OVER()
             FROM read_parquet(?) v
             WHERE {where_sql}
             {order_sql}
             LIMIT ? OFFSET ?"
        );
        let mut select_params = filtered_page_params(
            path.as_ref(),
            request,
            &core_filters,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
        );
        for sort in &page_sorts {
            select_params.extend(sort.parameters.iter().cloned());
        }
        select_params.push((limit as i64).into());
        select_params.push((offset as i64).into());
        let (rows, total) = query_result_rows_with_total(connection, &sql, &select_params)?;
        return Ok(ResultPage {
            schema_version: SCHEMA_VERSION,
            offset,
            limit,
            total,
            search: core_filters.search,
            sort: sort_key,
            direction: direction.to_ascii_lowercase(),
            rows,
        });
    }
    let total = if let Some(total) = request.known_total {
        i64::try_from(total).map_err(|_| "known result total is too large")?
    } else {
        let mut count_statement = connection
            .prepare(&format!(
                "SELECT count(*) FROM read_parquet(?) v WHERE {where_sql}"
            ))
            .map_err(|error| format!("cannot prepare result count: {error}"))?;
        let mut count_params = core_page_params(path.as_ref(), request, &core_filters);
        count_params.extend(core_rule_params.iter().cloned());
        count_params.extend(evidence_rule_params.iter().cloned());
        count_params.extend(excluded_params.iter().cloned());
        count_statement
            .query_row(params_from_iter(count_params.iter()), |row| row.get(0))
            .map_err(|error| format!("cannot count result page: {error}"))?
    };
    if total == 0 {
        return Ok(ResultPage {
            schema_version: SCHEMA_VERSION,
            offset,
            limit,
            total,
            search: core_filters.search,
            sort: sort_key,
            direction: direction.to_ascii_lowercase(),
            rows: Vec::new(),
        });
    }
    let field_first_sort = optimized_evidence_sort.is_some()
        && candidate_ids.is_none()
        && page_request_is_unfiltered(request, &core_filters);
    let rows = if field_first_sort {
        let sort = optimized_evidence_sort
            .expect("field-first evidence sort requires a sort specification");
        evidence_sorted_page_rows(
            connection,
            path.as_ref(),
            request,
            &core_filters,
            &where_sql,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
            sort,
            direction,
            offset,
            limit,
        )?
    } else if optimized_evidence_sort.is_some() && total <= FILTERED_EVIDENCE_SORT_THRESHOLD {
        filtered_evidence_sorted_page_rows(
            connection,
            path.as_ref(),
            request,
            &core_filters,
            &where_sql,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
            optimized_evidence_sort.expect("filtered evidence sort requires a sort specification"),
            direction,
            offset,
            limit,
        )?
    } else {
        let order_sql = if page_sorts.len() == 1 && sort_key == "input" && direction == "ASC" {
            String::new()
        } else {
            let terms = page_sorts
                .iter()
                .map(|sort| format!("{} {} NULLS LAST", sort.expression, sort.direction))
                .chain(["record_number ASC".into(), "alt_index ASC".into()])
                .collect::<Vec<String>>()
                .join(", ");
            format!(" ORDER BY {terms}")
        };
        let sql = format!(
            "SELECT {RESULT_PAGE_COLUMNS}
             FROM read_parquet(?) v
             WHERE {where_sql}
             {order_sql}
             LIMIT ? OFFSET ?"
        );
        let mut select_params = filtered_page_params(
            path.as_ref(),
            request,
            &core_filters,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
        );
        for sort in &page_sorts {
            select_params.extend(sort.parameters.iter().cloned());
        }
        select_params.push((limit as i64).into());
        select_params.push((offset as i64).into());
        query_result_rows(connection, &sql, &select_params)?
    };
    Ok(ResultPage {
        schema_version: SCHEMA_VERSION,
        offset,
        limit,
        total,
        search: core_filters.search,
        sort: sort_key,
        direction: direction.to_ascii_lowercase(),
        rows,
    })
}

const RESULT_PAGE_COLUMNS: &str =
    "v.allele_id, v.chromosome, v.position, v.reference, v.alternate, v.variant_id,
     v.quality, v.filter, v.gene_symbol, v.gene_id, v.transcript_id, v.consequence,
     v.impact, v.canonical, v.mane_select, v.record_number, v.alt_index";
const FILTERED_EVIDENCE_SORT_THRESHOLD: i64 = 100_000;

fn filtered_page_params(
    path: &str,
    request: &PageRequest,
    filters: &CorePageFilters,
    core_rule_params: &[SqlValue],
    evidence_rule_params: &[SqlValue],
    excluded_params: &[SqlValue],
) -> Vec<SqlValue> {
    let mut parameters = core_page_params(path, request, filters);
    parameters.extend_from_slice(core_rule_params);
    parameters.extend_from_slice(evidence_rule_params);
    parameters.extend_from_slice(excluded_params);
    parameters
}

fn page_request_is_unfiltered(request: &PageRequest, filters: &CorePageFilters) -> bool {
    filters.search.is_empty()
        && filters.chromosome.is_empty()
        && filters.reference.is_empty()
        && filters.alternate.is_empty()
        && filters.variant_id.is_empty()
        && filters.gene.is_empty()
        && filters.transcript_id.is_empty()
        && filters.consequence.is_empty()
        && filters.impact.is_empty()
        && filters.filter.is_empty()
        && request.position_min.is_none()
        && request.position_max.is_none()
        && request.quality_min.is_none()
        && request.quality_max.is_none()
        && request.canonical.is_none()
        && request.filter_rules.is_empty()
        && request.evidence_filters.is_empty()
        && request.excluded_allele_ids.is_empty()
}

fn query_result_rows(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
) -> Result<Vec<Value>, String> {
    query_result_rows_internal(connection, sql, parameters, None).map(|(rows, _)| rows)
}

fn query_result_rows_with_total(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
) -> Result<(Vec<Value>, i64), String> {
    let (rows, total) = query_result_rows_internal(connection, sql, parameters, Some(17))?;
    Ok((rows, total.unwrap_or(0)))
}

fn query_result_rows_internal(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
    total_column: Option<usize>,
) -> Result<(Vec<Value>, Option<i64>), String> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("cannot prepare result page: {error}"))?;
    let mapped = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok((
                json!({
                    "alleleId": row.get::<_, String>(0)?,
                    "chromosome": row.get::<_, String>(1)?,
                    "position": row.get::<_, i64>(2)?,
                    "reference": row.get::<_, String>(3)?,
                    "alternate": row.get::<_, String>(4)?,
                    "variantId": row.get::<_, Option<String>>(5)?,
                    "quality": row.get::<_, Option<f64>>(6)?,
                    "filter": row.get::<_, String>(7)?,
                    "geneSymbol": row.get::<_, Option<String>>(8)?,
                    "geneId": row.get::<_, Option<String>>(9)?,
                    "transcriptId": row.get::<_, Option<String>>(10)?,
                    "consequence": row.get::<_, Option<String>>(11)?,
                    "impact": row.get::<_, Option<String>>(12)?,
                    "canonical": row.get::<_, bool>(13)?,
                    "maneSelect": row.get::<_, Option<String>>(14)?,
                    "recordNumber": row.get::<_, i64>(15)?,
                    "altIndex": row.get::<_, i32>(16)?,
                }),
                total_column
                    .map(|column| row.get::<_, i64>(column))
                    .transpose()?,
            ))
        })
        .map_err(|error| format!("cannot read result page: {error}"))?;
    let rows = mapped
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let total = rows.first().and_then(|(_, total)| *total);
    Ok((rows.into_iter().map(|(row, _)| row).collect(), total))
}

#[derive(Clone)]
struct SelectedEvidenceColumn {
    index: usize,
    scope: String,
    source_id: String,
    field_path: String,
    value_type: String,
    alignment_group: Option<String>,
}

struct EvidenceSortSpec {
    evidence: String,
    field: SelectedEvidenceColumn,
    value_expression: &'static str,
    resolved: bool,
}

struct PageSortSpec {
    key: String,
    direction: String,
    expression: String,
    parameters: Vec<SqlValue>,
    evidence: Option<EvidenceSortSpec>,
}

fn evidence_sort_parameters(sort: &EvidenceSortSpec) -> Vec<SqlValue> {
    let mut parameters = vec![sort.evidence.clone().into()];
    if !sort.resolved {
        parameters.push(sort.field.scope.clone().into());
    }
    parameters.push(sort.field.source_id.clone().into());
    parameters.push(sort.field.field_path.clone().into());
    parameters
}

fn evidence_sort_expression(sort: &EvidenceSortSpec) -> String {
    if sort.resolved {
        return format!(
            "(SELECT {} FROM read_parquet(?) ev_sort
              WHERE ev_sort.allele_id = v.allele_id
                AND ev_sort.source_id = ? AND ev_sort.field_path = ?
                AND ev_sort.resolution_kind IN ('exact_transcript', 'uniform')
              LIMIT 1)",
            sort.value_expression
        );
    }
    format!(
        "(SELECT {} FROM read_parquet(?) ev_sort
          WHERE ev_sort.allele_id = v.allele_id AND ev_sort.scope = ?
            AND ev_sort.source_id = ? AND ev_sort.field_path = ?
          ORDER BY ev_sort.consequence_id NULLS FIRST LIMIT 1)",
        sort.value_expression
    )
}

fn evidence_sort_cte(sort: &EvidenceSortSpec) -> String {
    if sort.resolved {
        return format!(
            "WITH scored_evidence AS (
               SELECT allele_id, {} AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE ev_sort.source_id = ? AND ev_sort.field_path = ?
                 AND ev_sort.resolution_kind IN ('exact_transcript', 'uniform')
                 AND {} IS NOT NULL
             )",
            sort.value_expression, sort.value_expression
        );
    }
    format!(
        "WITH evidence_values AS (
           SELECT allele_id,
                  first({} ORDER BY consequence_id NULLS FIRST) AS sort_value
           FROM read_parquet(?) ev_sort
           WHERE ev_sort.scope = ? AND ev_sort.source_id = ? AND ev_sort.field_path = ?
           GROUP BY allele_id
         ), scored_evidence AS (
           SELECT allele_id, sort_value FROM evidence_values WHERE sort_value IS NOT NULL
         )",
        sort.value_expression
    )
}

#[allow(clippy::too_many_arguments)]
fn evidence_sorted_page_rows(
    connection: &Connection,
    path: &str,
    request: &PageRequest,
    filters: &CorePageFilters,
    where_sql: &str,
    core_rule_params: &[SqlValue],
    evidence_rule_params: &[SqlValue],
    excluded_params: &[SqlValue],
    sort: &EvidenceSortSpec,
    direction: &str,
    offset: u64,
    limit: u64,
) -> Result<Vec<Value>, String> {
    let cte = evidence_sort_cte(sort);
    let scored_sql = format!(
        "{cte}
         SELECT {RESULT_PAGE_COLUMNS}
         FROM scored_evidence ev_order
         JOIN read_parquet(?) v ON v.allele_id = ev_order.allele_id
         WHERE {where_sql}
         ORDER BY ev_order.sort_value {direction}, v.record_number ASC, v.alt_index ASC
         LIMIT ? OFFSET ?"
    );
    let mut scored_params = evidence_sort_parameters(sort);
    scored_params.extend(filtered_page_params(
        path,
        request,
        filters,
        core_rule_params,
        evidence_rule_params,
        excluded_params,
    ));
    scored_params.push((limit as i64).into());
    scored_params.push((offset as i64).into());
    let mut rows = query_result_rows(connection, &scored_sql, &scored_params)?;
    if rows.len() == limit as usize {
        return Ok(rows);
    }

    let scored_count = if !rows.is_empty() || offset == 0 {
        offset.saturating_add(rows.len() as u64)
    } else {
        let count_sql = format!(
            "{cte}
             SELECT count(*)
             FROM scored_evidence ev_order
             JOIN read_parquet(?) v ON v.allele_id = ev_order.allele_id
             WHERE {where_sql}"
        );
        let mut count_params = evidence_sort_parameters(sort);
        count_params.extend(filtered_page_params(
            path,
            request,
            filters,
            core_rule_params,
            evidence_rule_params,
            excluded_params,
        ));
        connection
            .query_row(&count_sql, params_from_iter(count_params.iter()), |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| format!("cannot count scored result rows: {error}"))?
            .max(0) as u64
    };
    let remaining = limit.saturating_sub(rows.len() as u64);
    let missing_offset = offset.saturating_sub(scored_count);
    let missing_sql = format!(
        "{cte}
         SELECT {RESULT_PAGE_COLUMNS}
         FROM read_parquet(?) v
         WHERE {where_sql}
           AND NOT EXISTS (
             SELECT 1 FROM scored_evidence ev_order WHERE ev_order.allele_id = v.allele_id
           )
         ORDER BY v.record_number ASC, v.alt_index ASC
         LIMIT ? OFFSET ?"
    );
    let mut missing_params = evidence_sort_parameters(sort);
    missing_params.extend(filtered_page_params(
        path,
        request,
        filters,
        core_rule_params,
        evidence_rule_params,
        excluded_params,
    ));
    missing_params.push((remaining as i64).into());
    missing_params.push((missing_offset as i64).into());
    rows.extend(query_result_rows(
        connection,
        &missing_sql,
        &missing_params,
    )?);
    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
fn filtered_evidence_sorted_page_rows(
    connection: &Connection,
    path: &str,
    request: &PageRequest,
    filters: &CorePageFilters,
    where_sql: &str,
    core_rule_params: &[SqlValue],
    evidence_rule_params: &[SqlValue],
    excluded_params: &[SqlValue],
    sort: &EvidenceSortSpec,
    direction: &str,
    offset: u64,
    limit: u64,
) -> Result<Vec<Value>, String> {
    let (selection, evidence_where) = if sort.resolved {
        (
            format!("first({})", sort.value_expression),
            "ev_sort.source_id = ? AND ev_sort.field_path = ?
             AND ev_sort.resolution_kind IN ('exact_transcript', 'uniform')"
                .to_owned(),
        )
    } else {
        (
            format!(
                "first({} ORDER BY ev_sort.consequence_id NULLS FIRST)",
                sort.value_expression
            ),
            "ev_sort.scope = ? AND ev_sort.source_id = ? AND ev_sort.field_path = ?".to_owned(),
        )
    };
    let sql = format!(
        "WITH matched_variants AS MATERIALIZED (
           SELECT v.* FROM read_parquet(?) v WHERE {where_sql}
         ), evidence_values AS (
           SELECT ev_sort.allele_id,
                  {selection} AS sort_value
           FROM read_parquet(?) ev_sort
           JOIN matched_variants matched ON matched.allele_id = ev_sort.allele_id
           WHERE {evidence_where}
           GROUP BY ev_sort.allele_id
         )
         SELECT {RESULT_PAGE_COLUMNS}
         FROM matched_variants v
         LEFT JOIN evidence_values ev_order ON ev_order.allele_id = v.allele_id
         ORDER BY ev_order.sort_value {direction} NULLS LAST,
                  v.record_number ASC, v.alt_index ASC
         LIMIT ? OFFSET ?",
    );
    let mut parameters = filtered_page_params(
        path,
        request,
        filters,
        core_rule_params,
        evidence_rule_params,
        excluded_params,
    );
    parameters.extend(evidence_sort_parameters(sort));
    parameters.push((limit as i64).into());
    parameters.push((offset as i64).into());
    query_result_rows(connection, &sql, &parameters)
}

fn evidence_field_is_numeric(field: &SelectedEvidenceColumn) -> bool {
    if matches!(field.value_type.as_str(), "integer" | "number") {
        return true;
    }
    let name = field.field_path.to_ascii_lowercase();
    name.ends_with("_score")
        || name.ends_with("_rankscore")
        || name.ends_with("_phred")
        || matches!(name.as_str(), "af" | "faf" | "ac" | "an" | "dp" | "gq")
        || name.contains("allele_frequency")
        || name.contains("phylop")
        || name.contains("gerp")
}

fn split_numeric_evidence_comparison(
    operator: &str,
    value: &str,
) -> Result<(String, Vec<SqlValue>), String> {
    let number = value
        .parse::<f64>()
        .map_err(|_| "numeric filter value must be a number".to_string())?;
    if !number.is_finite() {
        return Err("numeric filter value must be finite".into());
    }
    let symbol = match operator {
        "equals" => "=",
        "gt" => ">",
        "gte" => ">=",
        "lt" => "<",
        "lte" => "<=",
        _ => return Err(format!("operator '{operator}' is not a numeric comparison")),
    };
    Ok((
        format!(
            "EXISTS (
                SELECT 1
                FROM unnest(string_split(
                    coalesce(ev.string_value, CAST(ev.integer_value AS VARCHAR), CAST(ev.number_value AS VARCHAR), ''),
                    ';'
                )) AS numeric_parts(value)
                WHERE try_cast(nullif(trim(numeric_parts.value), '.') AS DOUBLE)
                    {symbol} CAST(? AS DOUBLE)
            )"
        ),
        vec![number.into()],
    ))
}

fn evidence_filter_rules_sql(
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
) -> Result<(String, Vec<SqlValue>), String> {
    if request.evidence_filters.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    if request.evidence_filters.len() > 24 {
        return Err("at most 24 evidence filter rules can be applied at once".into());
    }
    let evidence = evidence.ok_or("this report has no evidence table")?;
    let catalog = catalog.ok_or("this report has no field catalog")?;
    let indices = request
        .evidence_filters
        .iter()
        .map(|filter| filter.index)
        .collect::<Vec<_>>();
    let selected = selected_evidence_columns(catalog, &indices)?;
    let mut sql = String::new();
    let mut parameters = Vec::new();
    for (filter, field) in request.evidence_filters.iter().zip(selected) {
        if !filter.value2.trim().is_empty() {
            return Err("two-value evidence filters are not supported yet".into());
        }
        let kind = match field.value_type.as_str() {
            "boolean" => FilterValueKind::Boolean,
            _ if evidence_field_is_numeric(&field) => FilterValueKind::Number,
            _ => FilterValueKind::Text,
        };
        if field.alignment_group.is_some() {
            let resolved = crate::evidence_resolution::available_path(evidence)
                .ok_or("transcript evidence index is not ready")?;
            let expression = if kind == FilterValueKind::Number {
                "er.resolved_number"
            } else {
                "er.resolved_string"
            };
            let (condition, values) =
                comparison_sql(expression, kind, &filter.operator, &filter.value)?;
            sql.push_str(
                " AND EXISTS (
                   SELECT 1 FROM read_parquet(?) er
                   WHERE er.allele_id = v.allele_id
                     AND er.source_id = ? AND er.field_path = ?
                     AND er.resolution_kind IN ('exact_transcript', 'uniform') AND (",
            );
            sql.push_str(&condition);
            sql.push_str(") LIMIT 1)");
            parameters.push(resolved.to_string_lossy().into_owned().into());
            parameters.push(field.source_id.into());
            parameters.push(field.field_path.into());
            parameters.extend(values);
            continue;
        }
        let value_expression = match kind {
            FilterValueKind::Number => {
                "coalesce(ev.number_value, CAST(ev.integer_value AS DOUBLE), try_cast(ev.string_value AS DOUBLE))"
            }
            FilterValueKind::Boolean => "ev.boolean_value",
            FilterValueKind::Text => {
                "coalesce(ev.string_value, CAST(ev.integer_value AS VARCHAR), CAST(ev.number_value AS VARCHAR), CAST(ev.boolean_value AS VARCHAR), ev.json_value, '')"
            }
        };
        let negative = matches!(filter.operator.as_str(), "not_equals" | "not_contains");
        let positive_operator = match filter.operator.as_str() {
            "not_equals" => "equals",
            "not_contains" => "contains",
            operator => operator,
        };
        let (condition, values) = if kind == FilterValueKind::Number
            && matches!(positive_operator, "equals" | "gt" | "gte" | "lt" | "lte")
        {
            split_numeric_evidence_comparison(positive_operator, &filter.value)?
        } else {
            comparison_sql(value_expression, kind, positive_operator, &filter.value)?
        };
        sql.push_str(if negative {
            " AND NOT EXISTS ("
        } else {
            " AND EXISTS ("
        });
        sql.push_str(
            "SELECT 1 FROM read_parquet(?) ev
             WHERE ev.allele_id = v.allele_id AND ev.scope = ?
               AND ev.source_id = ? AND ev.field_path = ? AND (",
        );
        sql.push_str(&condition);
        sql.push_str(") LIMIT 1)");
        parameters.push(evidence.to_string_lossy().into_owned().into());
        parameters.push(field.scope.into());
        parameters.push(field.source_id.into());
        parameters.push(field.field_path.into());
        parameters.extend(values);
    }
    Ok((sql, parameters))
}

fn displayed_field_search_sql(
    connection: &Connection,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
    search: &str,
) -> Result<(String, Vec<SqlValue>), String> {
    if search.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut sql = String::from(
        " AND (contains(replace(replace(lower(concat_ws(' ', v.chromosome,
             v.position::VARCHAR, v.reference, v.alternate, coalesce(v.variant_id, ''),
             coalesce(v.gene_symbol, ''), coalesce(v.gene_id, ''),
             coalesce(v.transcript_id, ''), coalesce(v.consequence, ''),
             coalesce(v.impact, ''), v.filter)), '_', ' '), '-', ' '), lower(?))",
    );
    let parameters = vec![search.to_owned().into()];
    if let (Some(evidence), Some(catalog)) = (evidence, catalog)
        && !request.evidence_columns.is_empty()
    {
        let fields = selected_evidence_columns(catalog, &request.evidence_columns)?
            .into_iter()
            .filter(evidence_field_is_text_searchable)
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            connection
                .execute_batch(
                    "CREATE TEMP TABLE displayed_evidence_search(allele_id VARCHAR PRIMARY KEY)",
                )
                .map_err(|error| format!("cannot create evidence search table: {error}"))?;
            let (aligned, raw): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .partition(|field| field.alignment_group.is_some());
            if !raw.is_empty() {
                let conditions = std::iter::repeat_n(
                    "(ev_search.scope = ? AND ev_search.source_id = ? AND ev_search.field_path = ?)",
                    raw.len(),
                )
                .collect::<Vec<_>>()
                .join(" OR ");
                let mut search_parameters =
                    vec![SqlValue::from(evidence.to_string_lossy().into_owned())];
                for field in raw {
                    search_parameters.push(field.scope.into());
                    search_parameters.push(field.source_id.into());
                    search_parameters.push(field.field_path.into());
                }
                search_parameters.push(search.to_owned().into());
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO displayed_evidence_search
                             SELECT DISTINCT ev_search.allele_id
                             FROM read_parquet(?) ev_search
                             WHERE ({conditions})
                               AND contains(replace(replace(lower(coalesce(
                                   ev_search.string_value, ev_search.json_value, '')),
                                   '_', ' '), '-', ' '), lower(?))"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| format!("cannot search displayed evidence fields: {error}"))?;
            }
            if !aligned.is_empty() {
                let resolved = crate::evidence_resolution::available_path(evidence)
                    .ok_or("transcript evidence index is not ready")?;
                let conditions = std::iter::repeat_n(
                    "(er_search.source_id = ? AND er_search.field_path = ?)",
                    aligned.len(),
                )
                .collect::<Vec<_>>()
                .join(" OR ");
                let mut search_parameters =
                    vec![SqlValue::from(resolved.to_string_lossy().into_owned())];
                for field in aligned {
                    search_parameters.push(field.source_id.into());
                    search_parameters.push(field.field_path.into());
                }
                search_parameters.push(search.to_owned().into());
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO displayed_evidence_search
                             SELECT DISTINCT er_search.allele_id
                             FROM read_parquet(?) er_search
                             WHERE ({conditions})
                               AND er_search.resolution_kind IN ('exact_transcript', 'uniform')
                               AND contains(replace(replace(lower(coalesce(
                                   er_search.resolved_string, '')), '_', ' '), '-', ' '), lower(?))"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| {
                        format!("cannot search transcript-aligned evidence fields: {error}")
                    })?;
            }
            sql.push_str(" OR v.allele_id IN (SELECT allele_id FROM displayed_evidence_search)");
        }
    }
    sql.push(')');
    Ok((sql, parameters))
}

fn evidence_field_is_text_searchable(field: &SelectedEvidenceColumn) -> bool {
    !matches!(field.value_type.as_str(), "integer" | "number" | "boolean")
        && !evidence_field_is_numeric(field)
}

pub fn page_json_with_evidence(
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
) -> Result<String, String> {
    page_json_with_evidence_internal(variants, evidence, catalog, offset, limit, request, None)
}

fn prepare_requested_evidence_resolution(
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
) -> Result<Option<PathBuf>, String> {
    let (Some(evidence), Some(catalog)) = (evidence, catalog) else {
        return Ok(None);
    };
    let mut indices = request.evidence_columns.clone();
    indices.extend(request.evidence_filters.iter().map(|filter| filter.index));
    if let Some(index) = request.sort_evidence {
        indices.push(index);
    }
    indices.extend(request.sorts.iter().filter_map(|sort| {
        sort.column
            .strip_prefix("evidence:")
            .and_then(|index| index.parse::<usize>().ok())
    }));
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty()
        || !selected_evidence_columns(catalog, &indices)?
            .iter()
            .any(|field| field.alignment_group.is_some())
    {
        return Ok(None);
    }
    crate::evidence_resolution::prepare(variants, evidence, catalog)
}

fn page_json_with_evidence_internal(
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
    candidate_ids: Option<&[String]>,
) -> Result<String, String> {
    prepare_requested_evidence_resolution(variants, evidence, catalog, request)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let query = PageQuery {
        variants,
        evidence,
        catalog,
        offset,
        limit,
        request,
        candidate_ids,
    };
    serde_json::to_string(&page_with_evidence_result(&connection, &query)?)
        .map_err(|error| error.to_string())
}

fn page_with_evidence_result(
    connection: &Connection,
    query: &PageQuery<'_>,
) -> Result<ResultPage, String> {
    let evidence = query.evidence;
    let catalog = query.catalog;
    let request = query.request;
    if request.evidence_columns.is_empty() {
        return page_result_internal(connection, query);
    }
    let evidence = evidence.ok_or("this report has no evidence table")?;
    let catalog = catalog.ok_or("this report has no field catalog")?;
    let selected = selected_evidence_columns(catalog, &request.evidence_columns)?;
    let mut page = page_result_internal(connection, query)?;
    let rows = &mut page.rows;
    if rows.is_empty() {
        return Ok(page);
    }
    let allele_ids = rows
        .iter()
        .filter_map(|row| row["alleleId"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let allele_placeholders = std::iter::repeat_n("?", allele_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let field_conditions = std::iter::repeat_n(
        "(scope = ? AND source_id = ? AND field_path = ?)",
        selected.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        "SELECT allele_id, scope, source_id, field_path,
                coalesce(string_value, cast(integer_value AS VARCHAR),
                         cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR), json_value)
         FROM read_parquet(?)
         WHERE allele_id IN ({allele_placeholders}) AND ({field_conditions})
         ORDER BY allele_id, scope, source_id, field_path, consequence_id NULLS FIRST"
    );
    let mut parameters = Vec::<SqlValue>::new();
    parameters.push(evidence.to_string_lossy().into_owned().into());
    parameters.extend(allele_ids.iter().cloned().map(Into::into));
    for field in &selected {
        parameters.push(field.scope.clone().into());
        parameters.push(field.source_id.clone().into());
        parameters.push(field.field_path.clone().into());
    }
    let lookup = selected
        .iter()
        .map(|field| {
            (
                (
                    field.scope.clone(),
                    field.source_id.clone(),
                    field.field_path.clone(),
                ),
                field.index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("cannot prepare evidence columns: {error}"))?;
    let mapped = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|error| format!("cannot read evidence columns: {error}"))?;
    let mut values: HashMap<(String, usize), Vec<String>> = HashMap::new();
    for row in mapped {
        let (allele_id, scope, source_id, field_path, value) =
            row.map_err(|error| error.to_string())?;
        let Some(value) = value else { continue };
        let Some(index) = lookup.get(&(scope, source_id, field_path)) else {
            continue;
        };
        let entry = values.entry((allele_id, *index)).or_default();
        if !entry.contains(&value) {
            entry.push(value);
        }
    }
    drop(statement);

    let aligned = selected
        .iter()
        .filter(|field| field.alignment_group.is_some())
        .collect::<Vec<_>>();
    let mut resolutions: HashMap<(String, usize), (String, Option<String>, String, i16, i16)> =
        HashMap::new();
    if !aligned.is_empty() {
        let resolved = crate::evidence_resolution::available_path(evidence)
            .ok_or("transcript evidence index is not ready")?;
        let conditions = std::iter::repeat_n("(source_id = ? AND field_path = ?)", aligned.len())
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT allele_id, source_id, field_path, resolution_kind, resolved_string,
                    source_transcript_release, reported_value_count, distinct_value_count
             FROM read_parquet(?)
             WHERE allele_id IN ({allele_placeholders}) AND ({conditions})"
        );
        let mut parameters = vec![SqlValue::from(resolved.to_string_lossy().into_owned())];
        parameters.extend(allele_ids.iter().cloned().map(Into::into));
        for field in &aligned {
            parameters.push(field.source_id.clone().into());
            parameters.push(field.field_path.clone().into());
        }
        let aligned_lookup = aligned
            .iter()
            .map(|field| {
                (
                    (field.source_id.clone(), field.field_path.clone()),
                    field.index,
                )
            })
            .collect::<HashMap<_, _>>();
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("cannot prepare transcript evidence columns: {error}"))?;
        let mapped = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i16>(6)?,
                    row.get::<_, i16>(7)?,
                ))
            })
            .map_err(|error| format!("cannot read transcript evidence columns: {error}"))?;
        for row in mapped {
            let (allele_id, source_id, field_path, kind, value, release, reported, distinct) =
                row.map_err(|error| error.to_string())?;
            if let Some(index) = aligned_lookup.get(&(source_id, field_path)) {
                resolutions.insert(
                    (allele_id, *index),
                    (kind, value, release, reported, distinct),
                );
            }
        }
    }
    for row in rows {
        let Some(allele_id) = row["alleleId"].as_str() else {
            continue;
        };
        let mut object = Map::new();
        let mut resolution_metadata = Map::new();
        for field in &selected {
            if let Some((kind, resolved, release, reported, distinct)) =
                resolutions.get(&(allele_id.to_owned(), field.index))
            {
                resolution_metadata.insert(
                    field.index.to_string(),
                    json!({
                        "kind": kind,
                        "sourceTranscriptRelease": release,
                        "reportedValueCount": reported,
                        "distinctValueCount": distinct,
                    }),
                );
                if matches!(kind.as_str(), "exact_transcript" | "uniform") {
                    if let Some(resolved) = resolved {
                        object.insert(field.index.to_string(), Value::String(resolved.clone()));
                    }
                    continue;
                }
                if matches!(kind.as_str(), "exact_missing" | "not_reported") {
                    continue;
                }
            }
            let Some(field_values) = values.get(&(allele_id.to_owned(), field.index)) else {
                continue;
            };
            object.insert(
                field.index.to_string(),
                if field_values.len() == 1 {
                    Value::String(field_values[0].clone())
                } else {
                    Value::Array(field_values.iter().cloned().map(Value::String).collect())
                },
            );
        }
        row["evidence"] = Value::Object(object);
        if !resolution_metadata.is_empty() {
            row["evidenceResolution"] = Value::Object(resolution_metadata);
        }
    }
    Ok(page)
}

pub fn page_json_with_details(
    query_key: &str,
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
) -> Result<String, String> {
    page_json_with_details_query(
        query_key,
        PageQuery {
            variants,
            evidence,
            catalog,
            offset,
            limit,
            request,
            candidate_ids: None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn page_json_with_details_for_candidates(
    query_key: &str,
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
    candidate_ids: &[String],
) -> Result<String, String> {
    page_json_with_details_query(
        query_key,
        PageQuery {
            variants,
            evidence,
            catalog,
            offset,
            limit,
            request,
            candidate_ids: Some(candidate_ids),
        },
    )
}

fn page_json_with_details_query(query_key: &str, query: PageQuery<'_>) -> Result<String, String> {
    prepare_requested_evidence_resolution(
        query.variants,
        query.evidence,
        query.catalog,
        query.request,
    )?;
    let session_key = if query.request.query_session.is_empty() {
        query_key.to_owned()
    } else {
        format!("{query_key}:{}", query.request.query_session)
    };
    let (connection, _guard) =
        cancellable_page_connection(&session_key, query.request.request_generation)?;
    let page = page_with_evidence_result(&connection, &query)?;
    serde_json::to_string(&page).map_err(|error| error.to_string())
}

fn selected_evidence_columns(
    catalog: &Path,
    indices: &[usize],
) -> Result<Vec<SelectedEvidenceColumn>, String> {
    let metadata =
        fs::metadata(catalog).map_err(|error| format!("field catalog is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("field catalog has an invalid size".into());
    }
    let catalog: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid field catalog: {error}"))?;
    let fields = catalog["fields"]
        .as_array()
        .ok_or("field catalog has no fields array")?;
    indices
        .iter()
        .map(|index| {
            let field = fields
                .get(*index)
                .ok_or_else(|| format!("evidence column {index} is outside the field catalog"))?;
            Ok(SelectedEvidenceColumn {
                index: *index,
                scope: field["scope"]
                    .as_str()
                    .ok_or("evidence field has no scope")?
                    .to_owned(),
                source_id: field["sourceId"]
                    .as_str()
                    .ok_or("evidence field has no source ID")?
                    .to_owned(),
                field_path: field["fieldPath"]
                    .as_str()
                    .ok_or("evidence field has no field path")?
                    .to_owned(),
                value_type: field["valueType"]
                    .as_str()
                    .ok_or("evidence field has no value type")?
                    .to_owned(),
                alignment_group: field["alignmentGroup"].as_str().map(str::to_owned).or_else(
                    || {
                        crate::evidence_resolution::bundled_alignment_group(
                            field["scope"].as_str()?,
                            field["sourceId"].as_str()?,
                            field["fieldPath"].as_str()?,
                        )
                    },
                ),
            })
        })
        .collect()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn export_filtered_rows(
    parquet: &Path,
    destination: &Path,
    request: &PageRequest,
    columns: &[String],
) -> Result<u64, String> {
    export_filtered_rows_with_details(parquet, None, None, destination, request, columns)
}

pub fn export_filtered_rows_with_details(
    parquet: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    destination: &Path,
    request: &PageRequest,
    columns: &[String],
) -> Result<u64, String> {
    prepare_requested_evidence_resolution(parquet, evidence, catalog, request)?;
    let filters = validated_core_page_filters(request)?;
    let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
    let (evidence_rule_sql, evidence_rule_params) =
        evidence_filter_rules_sql(evidence, catalog, request)?;
    let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
    let where_sql =
        format!("{CORE_PAGE_WHERE_SQL}{core_rule_sql}{evidence_rule_sql}{excluded_sql}");
    let requested = export_columns(columns)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let path = parquet.to_string_lossy();
    let mut statement = connection
        .prepare(&format!(
            "SELECT chromosome, position, reference, alternate, variant_id, quality, filter,
                    gene_symbol, gene_id, transcript_id, consequence, impact, canonical, mane_select
             FROM read_parquet(?) v WHERE {where_sql}
             ORDER BY record_number ASC, alt_index ASC"
        ))
        .map_err(|error| format!("cannot prepare filtered row export: {error}"))?;
    let mut params = core_page_params(path.as_ref(), request, &filters);
    params.extend(core_rule_params);
    params.extend(evidence_rule_params);
    params.extend(excluded_params);
    let mut rows = statement
        .query(params_from_iter(params.iter()))
        .map_err(|error| format!("cannot read filtered rows: {error}"))?;
    write_export_file(destination, |writer| {
        writer
            .write_all(b"\xEF\xBB\xBF")
            .map_err(|error| error.to_string())?;
        write_csv_record(
            writer,
            requested
                .iter()
                .map(|column| column.label())
                .collect::<Vec<_>>(),
        )?;
        let mut count = 0_u64;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("cannot read filtered export row: {error}"))?
        {
            let values = ExportRow {
                chromosome: row.get(0).map_err(|error| error.to_string())?,
                position: row.get(1).map_err(|error| error.to_string())?,
                reference: row.get(2).map_err(|error| error.to_string())?,
                alternate: row.get(3).map_err(|error| error.to_string())?,
                variant_id: row.get(4).map_err(|error| error.to_string())?,
                quality: row.get(5).map_err(|error| error.to_string())?,
                filter: row.get(6).map_err(|error| error.to_string())?,
                gene: row.get(7).map_err(|error| error.to_string())?,
                gene_id: row.get(8).map_err(|error| error.to_string())?,
                transcript_id: row.get(9).map_err(|error| error.to_string())?,
                consequence: row.get(10).map_err(|error| error.to_string())?,
                impact: row.get(11).map_err(|error| error.to_string())?,
                canonical: row.get(12).map_err(|error| error.to_string())?,
                mane_select: row.get(13).map_err(|error| error.to_string())?,
            };
            write_csv_record(
                writer,
                requested
                    .iter()
                    .map(|column| column.value(&values))
                    .collect::<Vec<_>>(),
            )?;
            count += 1;
        }
        Ok(count)
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn export_filtered_genes(
    parquet: &Path,
    destination: &Path,
    request: &PageRequest,
) -> Result<u64, String> {
    export_filtered_genes_with_details(parquet, None, None, destination, request)
}

pub fn export_filtered_genes_with_details(
    parquet: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    destination: &Path,
    request: &PageRequest,
) -> Result<u64, String> {
    prepare_requested_evidence_resolution(parquet, evidence, catalog, request)?;
    let filters = validated_core_page_filters(request)?;
    let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
    let (evidence_rule_sql, evidence_rule_params) =
        evidence_filter_rules_sql(evidence, catalog, request)?;
    let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
    let where_sql =
        format!("{CORE_PAGE_WHERE_SQL}{core_rule_sql}{evidence_rule_sql}{excluded_sql}");
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let path = parquet.to_string_lossy();
    let mut statement = connection
        .prepare(&format!(
            "SELECT DISTINCT trim(gene_symbol) AS gene
             FROM read_parquet(?) v WHERE {where_sql}
               AND gene_symbol IS NOT NULL AND trim(gene_symbol) <> ''
             ORDER BY upper(gene), gene"
        ))
        .map_err(|error| format!("cannot prepare filtered gene export: {error}"))?;
    let mut params = core_page_params(path.as_ref(), request, &filters);
    params.extend(core_rule_params);
    params.extend(evidence_rule_params);
    params.extend(excluded_params);
    let mut rows = statement
        .query(params_from_iter(params.iter()))
        .map_err(|error| format!("cannot read filtered genes: {error}"))?;
    write_export_file(destination, |writer| {
        let mut count = 0_u64;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("cannot read filtered gene export: {error}"))?
        {
            if count > 0 {
                writer.write_all(b",").map_err(|error| error.to_string())?;
            }
            let gene: String = row.get(0).map_err(|error| error.to_string())?;
            writer
                .write_all(gene.as_bytes())
                .map_err(|error| error.to_string())?;
            count += 1;
        }
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        Ok(count)
    })
}

#[derive(Clone, Copy)]
enum ExportColumn {
    Chromosome,
    Position,
    Reference,
    Alternate,
    VariantId,
    Quality,
    Filter,
    Gene,
    GeneId,
    TranscriptId,
    Consequence,
    Impact,
    Canonical,
    ManeSelect,
}

impl ExportColumn {
    fn label(self) -> &'static str {
        match self {
            Self::Chromosome => "Chr",
            Self::Position => "Position",
            Self::Reference => "Ref",
            Self::Alternate => "Alt",
            Self::VariantId => "Variant ID",
            Self::Quality => "QUAL",
            Self::Filter => "VCF filter",
            Self::Gene => "Gene",
            Self::GeneId => "Gene ID",
            Self::TranscriptId => "Transcript",
            Self::Consequence => "Consequence",
            Self::Impact => "Impact",
            Self::Canonical => "Canonical",
            Self::ManeSelect => "MANE Select",
        }
    }

    fn value(self, row: &ExportRow) -> String {
        match self {
            Self::Chromosome => row.chromosome.clone(),
            Self::Position => row.position.to_string(),
            Self::Reference => row.reference.clone(),
            Self::Alternate => row.alternate.clone(),
            Self::VariantId => row.variant_id.clone().unwrap_or_default(),
            Self::Quality => row
                .quality
                .map(|value| value.to_string())
                .unwrap_or_default(),
            Self::Filter => row.filter.clone(),
            Self::Gene => row.gene.clone().unwrap_or_default(),
            Self::GeneId => row.gene_id.clone().unwrap_or_default(),
            Self::TranscriptId => row.transcript_id.clone().unwrap_or_default(),
            Self::Consequence => row.consequence.clone().unwrap_or_default(),
            Self::Impact => row.impact.clone().unwrap_or_default(),
            Self::Canonical => {
                if row.canonical {
                    "Yes".into()
                } else {
                    "No".into()
                }
            }
            Self::ManeSelect => row.mane_select.clone().unwrap_or_default(),
        }
    }
}

struct ExportRow {
    chromosome: String,
    position: i64,
    reference: String,
    alternate: String,
    variant_id: Option<String>,
    quality: Option<f64>,
    filter: String,
    gene: Option<String>,
    gene_id: Option<String>,
    transcript_id: Option<String>,
    consequence: Option<String>,
    impact: Option<String>,
    canonical: bool,
    mane_select: Option<String>,
}

fn export_columns(columns: &[String]) -> Result<Vec<ExportColumn>, String> {
    if columns.is_empty() || columns.len() > 32 {
        return Err("row export needs between 1 and 32 visible columns".into());
    }
    columns
        .iter()
        .map(|column| match column.as_str() {
            "chromosome" => Ok(ExportColumn::Chromosome),
            "position" => Ok(ExportColumn::Position),
            "reference" => Ok(ExportColumn::Reference),
            "alternate" => Ok(ExportColumn::Alternate),
            "variantId" => Ok(ExportColumn::VariantId),
            "quality" => Ok(ExportColumn::Quality),
            "filter" => Ok(ExportColumn::Filter),
            "gene" => Ok(ExportColumn::Gene),
            "geneId" => Ok(ExportColumn::GeneId),
            "transcriptId" => Ok(ExportColumn::TranscriptId),
            "consequence" => Ok(ExportColumn::Consequence),
            "impact" => Ok(ExportColumn::Impact),
            "canonical" => Ok(ExportColumn::Canonical),
            "maneSelect" => Ok(ExportColumn::ManeSelect),
            _ => Err(format!("unknown export column: {column}")),
        })
        .collect()
}

fn write_export_file<T>(
    destination: &Path,
    write: impl FnOnce(&mut BufWriter<fs::File>) -> Result<T, String>,
) -> Result<T, String> {
    let partial = destination.with_extension(format!(
        "{}.partial",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("export")
    ));
    let file = fs::File::create(&partial)
        .map_err(|error| format!("cannot create export {}: {error}", partial.display()))?;
    let mut writer = BufWriter::new(file);
    let result = write(&mut writer);
    if result.is_ok() {
        writer
            .flush()
            .map_err(|error| format!("cannot finish export: {error}"))?;
    }
    drop(writer);
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("cannot replace {}: {error}", destination.display()))?;
    }
    fs::rename(&partial, destination)
        .map_err(|error| format!("cannot publish export {}: {error}", destination.display()))?;
    Ok(result)
}

fn write_csv_record<'a>(
    writer: &mut BufWriter<fs::File>,
    values: impl IntoIterator<Item = impl AsRef<str> + 'a>,
) -> Result<(), String> {
    let mut first = true;
    for value in values {
        if !first {
            writer.write_all(b",").map_err(|error| error.to_string())?;
        }
        first = false;
        let mut value = value.as_ref().to_owned();
        if value.starts_with(['=', '+', '-', '@']) {
            value.insert(0, '\'');
        }
        writer.write_all(b"\"").map_err(|error| error.to_string())?;
        writer
            .write_all(value.replace('"', "\"\"").as_bytes())
            .map_err(|error| error.to_string())?;
        writer.write_all(b"\"").map_err(|error| error.to_string())?;
    }
    writer.write_all(b"\r\n").map_err(|error| error.to_string())
}

fn bounded_page_text<'a>(value: &'a str, name: &str, maximum: usize) -> Result<&'a str, String> {
    let value = value.trim();
    if value.len() > maximum {
        return Err(format!("{name} filter is limited to {maximum} characters"));
    }
    if value
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(format!(
            "{name} filter contains unsupported control characters"
        ));
    }
    Ok(value)
}

fn evidence_sort_spec(
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    index: usize,
) -> Result<EvidenceSortSpec, String> {
    let evidence = evidence.ok_or("this report has no evidence table")?;
    let catalog = catalog.ok_or("this report has no field catalog")?;
    let mut selected = selected_evidence_columns(catalog, &[index])?;
    let field = selected.pop().ok_or("unknown evidence sort column")?;
    let resolved = field.alignment_group.is_some();
    let evidence_path = if resolved {
        crate::evidence_resolution::available_path(evidence)
            .ok_or("transcript evidence index is not ready")?
    } else {
        evidence.to_path_buf()
    };
    let value_expression = match field.value_type.as_str() {
        "integer" | "number" if resolved => "ev_sort.resolved_number",
        "integer" | "number" => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE))"
        }
        "boolean" if resolved => "ev_sort.resolved_string",
        "boolean" => "ev_sort.boolean_value",
        _ if resolved && evidence_field_is_numeric(&field) => "ev_sort.resolved_number",
        _ if resolved => "ev_sort.resolved_string",
        _ if evidence_field_is_numeric(&field) => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE),
                      try_cast(nullif(trim(split_part(ev_sort.string_value, ';', 1)), '.') AS DOUBLE))"
        }
        _ => {
            "coalesce(ev_sort.string_value, CAST(ev_sort.integer_value AS VARCHAR), CAST(ev_sort.number_value AS VARCHAR), CAST(ev_sort.boolean_value AS VARCHAR), ev_sort.json_value)"
        }
    };
    Ok(EvidenceSortSpec {
        evidence: evidence_path.to_string_lossy().into_owned(),
        field,
        value_expression,
        resolved,
    })
}

fn page_sort_direction(direction: &str) -> Result<String, String> {
    match direction.trim().to_ascii_lowercase().as_str() {
        "" | "asc" => Ok("ASC".into()),
        "desc" => Ok("DESC".into()),
        _ => Err("sort direction must be asc or desc".into()),
    }
}

fn page_sort_specs(
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
) -> Result<Vec<PageSortSpec>, String> {
    const MAX_SORT_COLUMNS: usize = 8;
    let requested = if request.sorts.is_empty() {
        vec![PageSortRequest {
            column: request
                .sort_evidence
                .map(|index| format!("evidence:{index}"))
                .unwrap_or_else(|| request.sort.clone()),
            direction: request.direction.clone(),
        }]
    } else {
        if request.sorts.len() > MAX_SORT_COLUMNS {
            return Err(format!(
                "up to {MAX_SORT_COLUMNS} result sort columns are supported"
            ));
        }
        request.sorts.clone()
    };
    let mut seen = HashSet::new();
    requested
        .into_iter()
        .map(|requested| {
            let direction = page_sort_direction(&requested.direction)?;
            let column = requested.column.trim();
            let sort = if let Some(index) = column.strip_prefix("evidence:") {
                let index = index
                    .parse::<usize>()
                    .map_err(|_| "unknown evidence sort column")?;
                let evidence_sort = evidence_sort_spec(evidence, catalog, index)?;
                let key = format!("evidence:{index}");
                PageSortSpec {
                    key,
                    direction,
                    expression: evidence_sort_expression(&evidence_sort),
                    parameters: evidence_sort_parameters(&evidence_sort),
                    evidence: Some(evidence_sort),
                }
            } else {
                let (key, expression) = page_sort_expression(column)?;
                PageSortSpec {
                    key: key.into(),
                    direction,
                    expression: expression.into(),
                    parameters: Vec::new(),
                    evidence: None,
                }
            };
            if !seen.insert(sort.key.clone()) {
                return Err("result sort columns must be unique".into());
            }
            Ok(sort)
        })
        .collect()
}

fn page_sort_expression(sort: &str) -> Result<(&'static str, &'static str), String> {
    match sort {
        "" | "input" => Ok(("input", "record_number")),
        "chromosome" => Ok(("chromosome", "chromosome")),
        "position" => Ok(("position", "position")),
        "reference" => Ok(("reference", "reference")),
        "alternate" => Ok(("alternate", "alternate")),
        "variantId" => Ok(("variantId", "variant_id")),
        "quality" => Ok(("quality", "quality")),
        "filter" => Ok(("filter", "filter")),
        "gene" => Ok(("gene", "gene_symbol")),
        "geneId" => Ok(("geneId", "gene_id")),
        "transcriptId" => Ok(("transcriptId", "transcript_id")),
        "consequence" => Ok(("consequence", "consequence")),
        "impact" => Ok((
            "impact",
            "CASE impact WHEN 'HIGH' THEN 0 WHEN 'MODERATE' THEN 1 WHEN 'LOW' THEN 2 ELSE 3 END",
        )),
        "canonical" => Ok(("canonical", "canonical")),
        "maneSelect" => Ok(("maneSelect", "mane_select")),
        _ => Err("unknown result sort column".into()),
    }
}

fn detail_value(
    allele_id: &str,
    consequence_rows: Vec<(String, String)>,
    evidence_rows: Vec<Value>,
) -> Value {
    let consequences_truncated = consequence_rows.len() > 1000;
    let consequences = consequence_rows
        .into_iter()
        .take(1000)
        .map(|(consequence_id, value)| {
            let mut parsed = serde_json::from_str(&value).unwrap_or(Value::String(value));
            if let Value::Object(object) = &mut parsed {
                object.insert(
                    "_annocatConsequenceId".into(),
                    Value::String(consequence_id),
                );
            }
            parsed
        })
        .collect::<Vec<_>>();
    let evidence_truncated = evidence_rows.len() > 5000;
    let evidence = evidence_rows.into_iter().take(5000).collect::<Vec<_>>();
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "alleleId": allele_id,
        "consequences": consequences,
        "consequencesTruncated": consequences_truncated,
        "evidence": evidence,
        "evidenceTruncated": evidence_truncated,
    })
}

pub fn detail_json(
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    allele_id: &str,
) -> Result<String, String> {
    validate_allele_identity(allele_id)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let consequence_path = consequences_parquet.to_string_lossy();
    let mut consequence_statement = connection
        .prepare(
            "SELECT consequence_id, consequence_json FROM read_parquet(?) WHERE allele_id = ?
             ORDER BY ordinal LIMIT 1001",
        )
        .map_err(|error| format!("cannot prepare consequence detail query: {error}"))?;
    let consequence_rows = consequence_statement
        .query_map(params![consequence_path.as_ref(), allele_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("cannot read consequence details: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let evidence_path = evidence_parquet.to_string_lossy();
    let mut evidence_statement = connection
        .prepare(
            "SELECT consequence_id, scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?) WHERE allele_id = ?
             ORDER BY scope, source_id, field_path, consequence_id LIMIT 5001",
        )
        .map_err(|error| format!("cannot prepare evidence detail query: {error}"))?;
    let evidence_rows = evidence_statement
        .query_map(params![evidence_path.as_ref(), allele_id], |row| {
            let value_type = row.get::<_, String>(4)?;
            let value = match value_type.as_str() {
                "string" => row.get::<_, Option<String>>(5)?.map(Value::String),
                "integer" => row
                    .get::<_, Option<i64>>(6)?
                    .map(|value| Value::Number(value.into())),
                "number" => row
                    .get::<_, Option<f64>>(7)?
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number),
                "boolean" => row.get::<_, Option<bool>>(8)?.map(Value::Bool),
                "json" => row
                    .get::<_, Option<String>>(9)?
                    .map(|value| serde_json::from_str(&value).unwrap_or(Value::String(value))),
                _ => None,
            }
            .unwrap_or(Value::Null);
            Ok(json!({
                "consequenceId": row.get::<_, Option<String>>(0)?,
                "scope": row.get::<_, String>(1)?,
                "sourceId": row.get::<_, String>(2)?,
                "fieldPath": row.get::<_, String>(3)?,
                "valueType": value_type,
                "value": value,
            }))
        })
        .map_err(|error| format!("cannot read evidence details: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&detail_value(allele_id, consequence_rows, evidence_rows))
        .map_err(|error| error.to_string())
}

fn validate_allele_identity(allele_id: &str) -> Result<(), String> {
    if allele_id.len() > 64
        || !allele_id.starts_with("allele-")
        || !allele_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid allele identity".into());
    }
    Ok(())
}

#[cfg(test)]
pub fn complete_detail_json(
    variants_parquet: &Path,
    consequences_parquet: Option<&Path>,
    evidence_parquet: Option<&Path>,
    allele_id: &str,
) -> Result<String, String> {
    complete_detail_json_at(
        variants_parquet,
        consequences_parquet,
        evidence_parquet,
        allele_id,
        None,
        None,
    )
}

pub fn complete_detail_json_at(
    variants_parquet: &Path,
    consequences_parquet: Option<&Path>,
    evidence_parquet: Option<&Path>,
    allele_id: &str,
    record_number: Option<i64>,
    alt_index: Option<i32>,
) -> Result<String, String> {
    validate_allele_identity(allele_id)?;
    if let (Some(consequences), Some(evidence), Some(record_number), Some(alt_index)) = (
        consequences_parquet,
        evidence_parquet,
        record_number,
        alt_index,
    ) {
        if let Ok(Some(indexed)) = crate::detail_lookup::lookup(
            variants_parquet,
            consequences,
            evidence,
            allele_id,
            record_number,
            alt_index,
        ) {
            let embedded_consequences = indexed
                .variant
                .get("fallbackConsequences")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty());
            if !indexed.consequences.is_empty() || !embedded_consequences {
                let mut detail = detail_value(allele_id, indexed.consequences, indexed.evidence);
                detail
                    .as_object_mut()
                    .ok_or("variant detail response is not an object")?
                    .insert("variant".into(), indexed.variant);
                return serde_json::to_string(&detail).map_err(|error| error.to_string());
            }
        }
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let path = variants_parquet.to_string_lossy();
    let context = connection
        .query_row(
            "SELECT chromosome, position, reference, alternate, alt_index, variant_id, quality, filter,
                    gene_symbol, gene_id, transcript_id, consequence, impact, canonical,
                    mane_select, format, samples_json, consequences_json
             FROM read_parquet(?) WHERE allele_id = ? LIMIT 1",
            params![path.as_ref(), allele_id],
            |row| {
                let samples_json = row.get::<_, String>(16)?;
                let consequences_json = row.get::<_, String>(17)?;
                Ok(json!({
                    "chromosome": row.get::<_, String>(0)?,
                    "position": row.get::<_, i64>(1)?,
                    "reference": row.get::<_, String>(2)?,
                    "alternate": row.get::<_, String>(3)?,
                    "altIndex": row.get::<_, i32>(4)?,
                    "variantId": row.get::<_, Option<String>>(5)?,
                    "quality": row.get::<_, Option<f64>>(6)?,
                    "filter": row.get::<_, String>(7)?,
                    "geneSymbol": row.get::<_, Option<String>>(8)?,
                    "geneId": row.get::<_, Option<String>>(9)?,
                    "transcriptId": row.get::<_, Option<String>>(10)?,
                    "consequence": row.get::<_, Option<String>>(11)?,
                    "impact": row.get::<_, Option<String>>(12)?,
                    "canonical": row.get::<_, bool>(13)?,
                    "maneSelect": row.get::<_, Option<String>>(14)?,
                    "format": row.get::<_, Option<String>>(15)?,
                    "samples": serde_json::from_str::<Value>(&samples_json).unwrap_or(Value::Array(Vec::new())),
                    "fallbackConsequences": serde_json::from_str::<Value>(&consequences_json).unwrap_or(Value::Array(Vec::new())),
                }))
            },
        )
        .map_err(|error| format!("cannot read variant details: {error}"))?;
    let mut detail = match (consequences_parquet, evidence_parquet) {
        (Some(consequences), Some(evidence)) => {
            serde_json::from_str::<Value>(&detail_json(consequences, evidence, allele_id)?)
                .map_err(|error| error.to_string())?
        }
        _ => json!({
            "schemaVersion": SCHEMA_VERSION,
            "alleleId": allele_id,
            "consequences": context["fallbackConsequences"],
            "consequencesTruncated": false,
            "evidence": [],
            "evidenceTruncated": false,
        }),
    };
    detail
        .as_object_mut()
        .ok_or("variant detail response is not an object")?
        .insert("variant".into(), context);
    serde_json::to_string(&detail).map_err(|error| error.to_string())
}

fn parse_consequences(info: &str, fields: &[String]) -> Result<Vec<Map<String, Value>>, String> {
    let value = info
        .split(';')
        .find_map(|item| item.strip_prefix("CSQ="))
        .ok_or("VCF record has no CSQ value")?;
    value
        .split(',')
        .map(|entry| {
            let values = entry.split('|').collect::<Vec<_>>();
            if values.len() > fields.len() {
                return Err("CSQ entry has more values than its declared schema".into());
            }
            Ok(fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    (
                        field.clone(),
                        Value::String(values.get(index).copied().unwrap_or("").to_owned()),
                    )
                })
                .collect())
        })
        .collect()
}

const TOP_LEVEL_FIELDS: &[&str] = &[
    "allele_string",
    "end",
    "id",
    "input",
    "most_severe_consequence",
    "seq_region_name",
    "start",
    "strand",
    "transcript_consequences",
    "variant_type",
];

const CONSEQUENCE_FIELDS: &[&str] = &[
    "amino_acids",
    "appris",
    "biotype",
    "canonical",
    "ccds",
    "cdna_end",
    "cdna_start",
    "cds_end",
    "cds_start",
    "codons",
    "consequence_terms",
    "distance",
    "exon",
    "flags",
    "gencode_primary",
    "gene_id",
    "gene_symbol",
    "hgnc_id",
    "hgvsc",
    "hgvsg",
    "hgvsp",
    "impact",
    "intron",
    "mane_plus_clinical",
    "mane_select",
    "polyphen_prediction",
    "polyphen_score",
    "protein_end",
    "protein_id",
    "protein_start",
    "sift_prediction",
    "sift_score",
    "source",
    "strand",
    "symbol_source",
    "transcript_id",
    "tsl",
    "variant_allele",
];

struct EvidenceContext<'a> {
    allele_id: &'a str,
    consequence_id: Option<&'a str>,
    scope: &'a str,
    source_id: &'a str,
}

fn append_evidence_tree(
    batch: &mut EvidenceBatch,
    catalog: &mut BTreeMap<(String, String, String), CatalogEntry>,
    context: &EvidenceContext<'_>,
    path: &str,
    value: &Value,
) -> Result<u64, String> {
    if let Value::Object(object) = value {
        let mut rows = 0;
        for (key, child) in object {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            rows += append_evidence_tree(batch, catalog, context, &child_path, child)?;
        }
        return Ok(rows);
    }
    if value.is_null() {
        return Ok(0);
    }
    let field_path = if path.is_empty() { "value" } else { path };
    let (value_type, string_value, integer_value, number_value, boolean_value, json_value) =
        match value {
            Value::Bool(value) => ("boolean", None, None, None, Some(*value), None),
            Value::Number(value) if value.is_i64() || value.is_u64() => (
                "integer",
                None,
                value
                    .as_i64()
                    .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok())),
                None,
                None,
                None,
            ),
            Value::Number(value) => ("number", None, None, value.as_f64(), None, None),
            Value::String(value) => ("string", Some(value.clone()), None, None, None, None),
            Value::Array(_) | Value::Object(_) => (
                "json",
                None,
                None,
                None,
                None,
                Some(serde_json::to_string(value).map_err(|error| error.to_string())?),
            ),
            Value::Null => unreachable!(),
        };
    batch.schema_version.push(SCHEMA_VERSION);
    batch.allele_id.push(context.allele_id.to_owned());
    batch
        .consequence_id
        .push(context.consequence_id.map(str::to_owned));
    batch.scope.push(context.scope.to_owned());
    batch.source_id.push(context.source_id.to_owned());
    batch.field_path.push(field_path.to_owned());
    batch.value_type.push(value_type.to_owned());
    batch.string_value.push(string_value);
    batch.integer_value.push(integer_value);
    batch.number_value.push(number_value);
    batch.boolean_value.push(boolean_value);
    batch.json_value.push(json_value);
    let entry = catalog
        .entry((
            context.scope.to_owned(),
            context.source_id.to_owned(),
            field_path.to_owned(),
        ))
        .or_default();
    entry.types.insert(value_type);
    entry.occurrences += 1;
    Ok(1)
}

fn string_field<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("structured output is missing string field {name}"))
}

fn integer_field(object: &Map<String, Value>, name: &str) -> Result<i64, String> {
    object
        .get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("structured output is missing integer field {name}"))
}

fn optional_json_string(object: &Map<String, Value>, name: &str) -> Option<String> {
    object.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn optional_json_i64(object: &Map<String, Value>, name: &str) -> Option<i64> {
    object.get(name).and_then(Value::as_i64)
}

fn json_bool(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value
            .as_bool()
            .unwrap_or_else(|| value.as_i64().is_some_and(|value| value != 0))
    })
}

fn evidence_is_shared(
    consequences: &[Value],
    alternate: &str,
    source_id: &str,
    expected: &Value,
) -> bool {
    let matching = consequences
        .iter()
        .filter_map(Value::as_object)
        .filter(|object| object.get("variant_allele").and_then(Value::as_str) == Some(alternate))
        .filter_map(|object| object.get(source_id))
        .collect::<Vec<_>>();
    matching.len() > 1 && matching.iter().all(|value| *value == expected)
}

fn matching_consequences(
    consequences: &[Map<String, Value>],
    reference: &str,
    alternate: &str,
    all_alternates: &str,
) -> Vec<Map<String, Value>> {
    if !all_alternates.contains(',') {
        return consequences.to_vec();
    }
    let normalized_alternate = vep_allele(reference, alternate);
    consequences
        .iter()
        .filter(|entry| {
            entry
                .get("UPLOADED_ALLELE")
                .and_then(Value::as_str)
                .and_then(|value| value.rsplit('/').next())
                == Some(alternate)
                || entry.get("Allele").and_then(Value::as_str)
                    == Some(normalized_alternate.as_str())
        })
        .cloned()
        .collect()
}

fn vep_allele(reference: &str, alternate: &str) -> String {
    let allele = if reference.len() != alternate.len()
        && reference.as_bytes().first() == alternate.as_bytes().first()
    {
        &alternate[1..]
    } else {
        alternate
    };
    if allele.is_empty() {
        "-".to_owned()
    } else {
        allele.to_owned()
    }
}

fn best_consequence(entries: &[Map<String, Value>]) -> Option<&Map<String, Value>> {
    entries.iter().min_by_key(|entry| {
        let text = |name: &str| entry.get(name).and_then(Value::as_str).unwrap_or("");
        let preferred = if text("MANE_SELECT").is_empty() {
            if text("CANONICAL") == "YES" { 1 } else { 2 }
        } else {
            0
        };
        let impact = match text("IMPACT") {
            "HIGH" => 0,
            "MODERATE" => 1,
            "LOW" => 2,
            _ => 3,
        };
        (preferred, impact)
    })
}

fn samples_json(sample_names: &[String], columns: &[&str]) -> Result<String, String> {
    let values = columns.get(9..).unwrap_or(&[]);
    let samples = sample_names.iter().enumerate().map(|(index, name)| {
        json!({"name": name, "value": values.get(index).copied().unwrap_or(".")})
    }).collect::<Vec<_>>();
    serde_json::to_string(&samples).map_err(|error| error.to_string())
}

fn optional_vcf(value: &str) -> Option<String> {
    (value != "." && !value.is_empty()).then(|| value.to_owned())
}

fn allele_id(chromosome: &str, position: i64, reference: &str, alternate: &str) -> String {
    let identity = format!("GRCh38\0{chromosome}\0{position}\0{reference}\0{alternate}");
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    format!("allele-{}", &digest[..24])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn pinned_fastvep_fixture_round_trips_through_parquet() {
        let root = std::env::temp_dir().join(format!(
            "annocat-parquet-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fastvep/expected.vcf");
        let parquet = root.join("variants.parquet");
        let summary = convert_vcf(&input, &parquet, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(summary.rows, 8);
        let page: Value =
            serde_json::from_str(&page_json(&parquet, 0, 3, &PageRequest::default()).unwrap())
                .unwrap();
        assert_eq!(page["total"], 8);
        assert_eq!(page["rows"].as_array().unwrap().len(), 3);
        assert!(
            page["rows"][0]["alleleId"]
                .as_str()
                .unwrap()
                .starts_with("allele-")
        );
        let next_page: Value = serde_json::from_str(
            &page_json(
                &parquet,
                3,
                3,
                &PageRequest {
                    known_total: Some(8),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(next_page["total"], 8);
        assert_eq!(next_page["rows"].as_array().unwrap().len(), 3);
        let candidate_id = page["rows"][1]["alleleId"].as_str().unwrap().to_owned();
        let candidates: Value = serde_json::from_str(
            &page_json_with_details_for_candidates(
                "synthetic-candidates",
                &parquet,
                None,
                None,
                0,
                100,
                &PageRequest::default(),
                std::slice::from_ref(&candidate_id),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(candidates["total"], 1);
        assert_eq!(candidates["rows"][0]["alleleId"], candidate_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiallelic_records_keep_consequences_and_samples_with_the_right_allele() {
        let root = std::env::temp_dir().join(format!(
            "annocat-parquet-multiallelic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("input.vcf");
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tCASE\n1\t100\trs1\tA\tC,G\t50\tPASS\tCSQ=C|missense_variant|MODERATE|GENE_C|ENSGC|ENSTC|A/C|YES|NM_C,G|stop_gained|HIGH|GENE_G|ENSGG|ENSTG|A/G|YES|NM_G\tGT\t1/2\n1\t101\t.\tC\t.\t50\tPASS\tCSQ=.|intergenic_variant|MODIFIER||||C/.||\tGT\t0/0\n",
        )
        .unwrap();
        let parquet = root.join("variants.parquet");
        let summary = convert_vcf(&input, &parquet, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(summary.rows, 2);
        let page: Value =
            serde_json::from_str(&page_json(&parquet, 0, 10, &PageRequest::default()).unwrap())
                .unwrap();
        assert_eq!(page["rows"][0]["geneSymbol"], "GENE_C");
        assert_eq!(page["rows"][1]["geneSymbol"], "GENE_G");
        let filtered: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                10,
                &PageRequest {
                    search: "GENE_G".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["rows"][0]["alternate"], "G");
        let normalized_search: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                10,
                &PageRequest {
                    search: "GENE G".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(normalized_search["total"], 1);
        assert_eq!(normalized_search["rows"][0]["geneSymbol"], "GENE_G");
        let gene_list: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                10,
                &PageRequest {
                    filter_rules: vec![CoreFilterRuleRequest {
                        column: "gene".into(),
                        operator: "in".into(),
                        value: "gene_g, missing_gene".into(),
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(gene_list["total"], 1);
        assert_eq!(gene_list["rows"][0]["geneSymbol"], "GENE_G");
        let sorted: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                10,
                &PageRequest {
                    sort: "alternate".into(),
                    direction: "desc".into(),
                    impact: "HIGH".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sorted["total"], 1);
        assert_eq!(sorted["rows"][0]["alternate"], "G");
        assert_eq!(sorted["sort"], "alternate");
        assert_eq!(sorted["direction"], "desc");
        assert!(
            page_json(
                &parquet,
                0,
                10,
                &PageRequest {
                    sort: "alternate; DROP TABLE variants".into(),
                    ..PageRequest::default()
                }
            )
            .is_err()
        );
        let connection = Connection::open_in_memory().unwrap();
        let samples: String = connection
            .query_row(
                "SELECT samples_json FROM read_parquet(?) LIMIT 1",
                params![parquet.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(samples.contains("CASE"));
        assert!(samples.contains("1/2"));
        let filtered_request = PageRequest {
            impact: "HIGH".into(),
            ..PageRequest::default()
        };
        let csv = root.join("filtered.csv");
        let exported = export_filtered_rows(
            &parquet,
            &csv,
            &filtered_request,
            &["chromosome".into(), "alternate".into(), "gene".into()],
        )
        .unwrap();
        assert_eq!(exported, 1);
        assert_eq!(
            fs::read_to_string(csv).unwrap(),
            "\u{feff}\"Chr\",\"Alt\",\"Gene\"\r\n\"1\",\"G\",\"GENE_G\"\r\n"
        );
        let genes = root.join("genes.txt");
        assert_eq!(
            export_filtered_genes(&parquet, &genes, &filtered_request).unwrap(),
            1
        );
        assert_eq!(fs::read_to_string(genes).unwrap(), "GENE_G\n");
        let allele = page["rows"][1]["alleleId"].as_str().unwrap();
        let excluded_request = PageRequest {
            impact: "HIGH".into(),
            excluded_allele_ids: vec![allele.into()],
            ..PageRequest::default()
        };
        let excluded_csv = root.join("excluded.csv");
        assert_eq!(
            export_filtered_rows(
                &parquet,
                &excluded_csv,
                &excluded_request,
                &["chromosome".into(), "alternate".into(), "gene".into()],
            )
            .unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(excluded_csv).unwrap(),
            "\u{feff}\"Chr\",\"Alt\",\"Gene\"\r\n"
        );
        let excluded_genes = root.join("excluded-genes.txt");
        assert_eq!(
            export_filtered_genes(&parquet, &excluded_genes, &excluded_request).unwrap(),
            0
        );
        assert_eq!(fs::read_to_string(excluded_genes).unwrap(), "\n");
        let detail: Value =
            serde_json::from_str(&complete_detail_json(&parquet, None, None, allele).unwrap())
                .unwrap();
        assert_eq!(detail["variant"]["alternate"], "G");
        assert_eq!(detail["variant"]["samples"][0]["name"], "CASE");
        assert_eq!(detail["variant"]["samples"][0]["value"], "1/2");
        assert_eq!(detail["consequences"][0]["SYMBOL"], "GENE_G");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_vcf_builds_a_review_report_without_fabricating_consequences() {
        let root = std::env::temp_dir().join(format!(
            "annocat-vcf-review-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("input.vcf");
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n##reference=GRCh38\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tCASE\n22\t100\trs1\tA\tC,G\t50\tPASS\tAF=0.1,0.2\tGT:DP\t1/2:32\n22\t101\t.\tC\t.\t.\tPASS\t.\tGT\t0/0\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        let summary = convert_input_vcf(&input, &variants, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(summary.rows, 2);
        assert_eq!(summary.records, 2);
        assert_eq!(summary.samples, vec!["CASE"]);

        let page: Value =
            serde_json::from_str(&page_json(&variants, 0, 10, &PageRequest::default()).unwrap())
                .unwrap();
        assert_eq!(page["total"], 2);
        assert!(page["rows"][0]["geneSymbol"].is_null());
        let allele = page["rows"][0]["alleleId"].as_str().unwrap();
        let detail: Value =
            serde_json::from_str(&complete_detail_json(&variants, None, None, allele).unwrap())
                .unwrap();
        assert_eq!(detail["variant"]["format"], "GT:DP");
        assert_eq!(detail["variant"]["samples"][0]["name"], "CASE");
        assert_eq!(detail["variant"]["samples"][0]["value"], "1/2:32");

        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        write_empty_detail_tables(&consequences, &evidence, &catalog).unwrap();
        validate_report_tables_allow_empty_consequences(
            &variants,
            &consequences,
            &evidence,
            &catalog,
            2,
        )
        .unwrap();
        assert!(validate_report_tables(&variants, &consequences, &evidence, &catalog, 2).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiallelic_indels_match_fastvep_normalized_alleles() {
        let consequences = vec![
            Map::from_iter([
                ("Allele".into(), Value::String("-".into())),
                ("UPLOADED_ALLELE".into(), Value::String("GCC/G&GC".into())),
            ]),
            Map::from_iter([
                ("Allele".into(), Value::String("C".into())),
                ("UPLOADED_ALLELE".into(), Value::String("GCC/G&GC".into())),
            ]),
        ];

        let deletion = matching_consequences(&consequences, "GCC", "G", "G,GC");
        let shorter_deletion = matching_consequences(&consequences, "GCC", "GC", "G,GC");

        assert_eq!(deletion.len(), 1);
        assert_eq!(deletion[0]["Allele"], "-");
        assert_eq!(shorter_deletion.len(), 1);
        assert_eq!(shorter_deletion[0]["Allele"], "C");
    }

    #[test]
    fn structured_output_keeps_transcripts_and_catalogs_unknown_source_fields() {
        let root = std::env::temp_dir().join(format!(
            "annocat-structured-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("fastvep.ndjson");
        fs::write(
            &input,
            concat!(
                r#"{"allele_string":"A","start":65000,"end":65000,"seq_region_name":"1","variant_type":"Snv","transcript_consequences":[]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"start_lost","variant_type":"Snv","transcript_consequences":[{"variant_allele":"G","consequence_terms":["start_lost"],"impact":"HIGH","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000641515","biotype":"protein_coding","canonical":1,"mane_select":"ENST00000641515.2","hgvsg":"1:g.65565A>G","hgvsc":"ENST00000641515.2:c.1A>G","hgvsp":"ENSP00000493376.1:p.Met1Val","cadd":{"raw":1.25,"phred":12.5},"clinvar":"Likely_benign"},{"variant_allele":"G","consequence_terms":["downstream_gene_variant"],"impact":"MODIFIER","gene_id":"ENSG00000290826","transcript_id":"ENST00000832531","biotype":"lncRNA","distance":2039,"cadd":{"raw":1.25,"phred":12.5},"custom_source":{"labels":["one","two"],"score":"0.25"}}]}"#,
                "\n",
                r#"{"allele_string":"C/T","start":70000,"end":70000,"seq_region_name":"1","most_severe_consequence":"intergenic_variant","variant_type":"Snv","transcript_consequences":[]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        let summary = convert_structured(
            &input,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(summary.records, 2);
        assert_eq!(summary.consequences, 3);
        assert_eq!(summary.evidence, 5);
        assert!(summary.sources.contains(&"clinvar".to_owned()));
        let connection = Connection::open_in_memory().unwrap();
        let consequence_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?)",
                params![consequences.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(consequence_rows, 3);
        let cadd_phred: f64 = connection
            .query_row(
                "SELECT number_value FROM read_parquet(?) WHERE source_id='cadd' AND field_path='phred'",
                params![evidence.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cadd_phred, 12.5);
        let cadd_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?) WHERE source_id='cadd'",
                params![evidence.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cadd_rows, 2,
            "shared CADD fields are stored once per allele"
        );
        let id = allele_id("1", 65565, "A", "G");
        let detail: Value =
            serde_json::from_str(&detail_json(&consequences, &evidence, &id).unwrap()).unwrap();
        assert_eq!(detail["consequences"].as_array().unwrap().len(), 2);
        let linked_consequence = detail["consequences"][1]["_annocatConsequenceId"]
            .as_str()
            .unwrap();
        assert!(detail["evidence"].as_array().unwrap().iter().any(|entry| {
            entry["sourceId"] == "custom_source"
                && entry["scope"] == "transcript"
                && entry["consequenceId"] == linked_consequence
        }));
        assert!(detail["evidence"].as_array().unwrap().iter().any(|entry| {
            entry["sourceId"] == "cadd" && entry["fieldPath"] == "phred" && entry["value"] == 12.5
        }));
        let catalog: Value = serde_json::from_slice(&fs::read(catalog).unwrap()).unwrap();
        assert!(
            catalog["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| { field["sourceId"] == "clinvar" && field["valueType"] == "string" })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_output_keeps_all_selected_sources_namespaced() {
        let root = std::env::temp_dir().join(format!(
            "annocat-multi-source-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("fastvep.ndjson");
        fs::write(
            &input,
            concat!(
                r#"{"allele_string":"A/G","start":65564,"end":65564,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000641515","clinvar":{"significance":["Likely_benign"]},"gnomad":{"af":0.002},"dbnsfp":{"sift":"deleterious"},"cadd":{"phred":13.1},"spliceai":{"ds_ag":0.12},"phylop":{"score":1.9},"revel":{"score":0.28}}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000641515","clinvar":{"significance":["Likely_benign","Pathogenic"]},"gnomad":{"af":0.001},"dbnsfp":{"sift":"deleterious"},"cadd":{"phred":14.2},"spliceai":{"ds_ag":0.18},"phylop":{"score":2.4},"revel":{"score":0.31}}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        convert_structured(
            &input,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let mut statement = connection
            .prepare("SELECT DISTINCT source_id FROM read_parquet(?) ORDER BY source_id")
            .unwrap();
        let source_ids = statement
            .query_map(params![evidence.to_string_lossy().as_ref()], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            source_ids,
            [
                "cadd", "clinvar", "dbnsfp", "gnomad", "phylop", "revel", "spliceai"
            ]
        );
        let catalog_value: Value = serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        assert_eq!(catalog_value["fields"].as_array().unwrap().len(), 7);
        let id = allele_id("1", 65565, "A", "G");
        let detail: Value =
            serde_json::from_str(&detail_json(&consequences, &evidence, &id).unwrap()).unwrap();
        assert_eq!(detail["evidence"].as_array().unwrap().len(), 7);
        let revel_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| field["sourceId"] == "revel" && field["fieldPath"] == "score")
            .unwrap();
        let clinvar_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| {
                field["sourceId"] == "clinvar" && field["fieldPath"] == "significance"
            })
            .unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t65565\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|OR4F5|ENSG00000186092|ENST00000641515|A/G|YES|ENST00000641515.2\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let page: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![revel_index],
                    sort_evidence: Some(revel_index),
                    direction: "desc".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["rows"][0]["evidence"][revel_index.to_string()], "0.31");
        assert_eq!(page["sort"], format!("evidence:{revel_index}"));
        assert_eq!(page["direction"], "desc");
        let record_number = page["rows"][0]["recordNumber"].as_i64().unwrap();
        let alt_index = page["rows"][0]["altIndex"].as_i64().unwrap() as i32;
        let indexed_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&evidence),
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(indexed_detail["variant"]["geneSymbol"], "OR4F5");
        assert_eq!(indexed_detail["consequences"].as_array().unwrap().len(), 1);
        assert_eq!(indexed_detail["evidence"].as_array().unwrap().len(), 7);
        let detail_index = root.join("detail-row-groups.json");
        assert!(fs::metadata(&detail_index).unwrap().len() < 1024 * 1024);
        let detail_index_value: Value =
            serde_json::from_slice(&fs::read(&detail_index).unwrap()).unwrap();
        assert_eq!(detail_index_value["schemaVersion"], 2);
        assert_eq!(detail_index_value["variants"]["groups"][0]["rowOffset"], 0);
        assert_eq!(detail_index_value["variants"]["groups"][0]["rowCount"], 1);
        let mut incomplete_index = detail_index_value.clone();
        incomplete_index["consequences"]["groups"] = json!([]);
        fs::write(
            &detail_index,
            serde_json::to_vec(&incomplete_index).unwrap(),
        )
        .unwrap();
        let fallback_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&evidence),
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(fallback_detail["consequences"].as_array().unwrap().len(), 1);
        assert_eq!(fallback_detail["evidence"].as_array().unwrap().len(), 7);
        fs::write(&detail_index, b"not a valid index").unwrap();
        assert!(
            complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&evidence),
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .is_ok()
        );
        let evidence_filtered: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_filters: vec![EvidenceFilterRequest {
                        index: revel_index,
                        operator: "gte".into(),
                        value: "0.3".into(),
                        value2: String::new(),
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(evidence_filtered["total"], 1);
        let clinvar_filtered: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_filters: vec![EvidenceFilterRequest {
                        index: clinvar_index,
                        operator: "contains".into(),
                        value: "pathogenic".into(),
                        value2: String::new(),
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(clinvar_filtered["total"], 1);
        let clinvar_search: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    search: "path".into(),
                    evidence_columns: vec![clinvar_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(clinvar_search["total"], 1);
        let numeric_search: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    search: "0.31".into(),
                    evidence_columns: vec![revel_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(numeric_search["total"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn numeric_comparisons_accept_score_fields_cataloged_as_text() {
        let (sql, parameters) =
            comparison_sql("ev.string_value", FilterValueKind::Text, "gte", "0.803").unwrap();
        assert!(sql.contains("try_cast(ev.string_value AS DOUBLE) >="));
        assert_eq!(parameters.len(), 1);
        assert!(
            comparison_sql(
                "ev.string_value",
                FilterValueKind::Text,
                "lt",
                "not-a-number"
            )
            .unwrap_err()
            .contains("must be a number")
        );
        assert!(
            comparison_sql(
                "ev.number_value",
                FilterValueKind::Number,
                "contains",
                "0.8"
            )
            .unwrap()
            .0
            .contains("CAST(ev.number_value AS VARCHAR)")
        );
    }

    #[test]
    fn dbnsfp_table_columns_follow_the_representative_transcript() {
        let root = std::env::temp_dir().join(format!(
            "annocat-dbnsfp-alignment-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("fastvep.ndjson");
        fs::write(
            &input,
            concat!(
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":" ENST000001.8 ; ENST000002.4 ","AlphaMissense_score":"0.9;0.1"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1"},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002"}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65566,"end":65566,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":"ENST000001.8;ENST000002.4","AlphaMissense_score":"46;5.1"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1"},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        convert_structured(
            &input,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        let catalog_value: Value = serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        let score_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| {
                field["sourceId"] == "dbnsfp" && field["fieldPath"] == "AlphaMissense_score"
            })
            .unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t65565\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002|A/G|YES|ENST000002.1\n1\t65566\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002|A/G|YES|ENST000002.1\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let page: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![score_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["rows"][0]["evidence"][score_index.to_string()], "0.1");
        assert_eq!(
            page["rows"][0]["evidenceResolution"][score_index.to_string()]["kind"],
            "exact_transcript"
        );
        assert!(root.join(crate::evidence_resolution::FILE_NAME).is_file());
        let sorted: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![score_index],
                    sort_evidence: Some(score_index),
                    direction: "desc".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sorted["rows"][0]["position"], 65566);
        assert_eq!(
            sorted["rows"][0]["evidence"][score_index.to_string()],
            "5.1"
        );
        assert_eq!(
            sorted["rows"][0]["evidenceResolution"][score_index.to_string()]["kind"],
            "exact_transcript"
        );
        let multi_sorted: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![score_index],
                    sorts: vec![
                        PageSortRequest {
                            column: "gene".into(),
                            direction: "asc".into(),
                        },
                        PageSortRequest {
                            column: format!("evidence:{score_index}"),
                            direction: "desc".into(),
                        },
                    ],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(multi_sorted["sort"], "gene");
        assert_eq!(multi_sorted["direction"], "asc");
        assert_eq!(multi_sorted["rows"][0]["position"], 65566);
        assert!(
            page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    sorts: vec![
                        PageSortRequest {
                            column: "position".into(),
                            direction: "asc".into(),
                        },
                        PageSortRequest {
                            column: "position".into(),
                            direction: "desc".into(),
                        },
                    ],
                    ..PageRequest::default()
                },
            )
            .unwrap_err()
            .contains("must be unique")
        );
        for (threshold, expected) in [("0.8", 1), ("0.95", 1)] {
            let filtered: Value = serde_json::from_str(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &PageRequest {
                        evidence_filters: vec![EvidenceFilterRequest {
                            index: score_index,
                            operator: "gt".into(),
                            value: threshold.into(),
                            value2: String::new(),
                        }],
                        ..PageRequest::default()
                    },
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(filtered["total"], expected);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
