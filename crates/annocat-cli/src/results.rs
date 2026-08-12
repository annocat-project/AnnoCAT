use annocat_core::normalization::{
    CanonicalAllele, IndexedReference, NormalizeError, ReferenceSequence, canonical_chromosome,
    canonicalize,
};
use duckdb::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use duckdb::arrow::datatypes::{Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::types::Value as SqlValue;
use duckdb::{Connection, InterruptHandle, appender_params_from_iter, params, params_from_iter};
use parquet::arrow::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

pub const SCHEMA_VERSION: i32 = annocat_core::RESULT_SCHEMA_VERSION;
pub const REPRESENTATIVE_SELECTION_CONTRACT: &str = "allele-gene-severity-v1";
const QUERY_PROJECTION_CONTRACT: &str = "field-indexed-evidence-v4";
const QUERY_PROJECTION_PREFIX: &str = ".annocat-query-v3-";
const LEGACY_QUERY_PROJECTION_PREFIXES: [&str; 2] = [".annocat-query-v1-", ".annocat-query-v2-"];
const SAMPLE_CALL_PROJECTION_PREFIX: &str = ".annocat-sample-calls-v1-";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultPage {
    schema_version: i32,
    offset: u64,
    limit: u64,
    total: Option<i64>,
    has_more: bool,
    search: String,
    sort: String,
    direction: String,
    rows: Vec<Value>,
}

#[derive(Clone, Copy)]
struct PageQuery<'a> {
    variants: &'a Path,
    evidence: Option<&'a Path>,
    evidence_files: Option<&'a [PathBuf]>,
    catalog: Option<&'a Path>,
    offset: u64,
    limit: u64,
    request: &'a PageRequest,
    candidate_ids: Option<&'a [String]>,
}

fn evidence_read(evidence: &Path, files: Option<&[PathBuf]>) -> (String, Vec<SqlValue>) {
    let Some(files) = files else {
        return (
            "read_parquet(?)".into(),
            vec![evidence.to_string_lossy().into_owned().into()],
        );
    };
    let paths = files
        .iter()
        .map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    (format!("read_parquet([{paths}])"), Vec::new())
}

fn evidence_read_for_fields(
    evidence: &Path,
    files: Option<&[PathBuf]>,
    field_indices: impl IntoIterator<Item = usize>,
) -> (String, Vec<SqlValue>) {
    let Some(files) = files else {
        return evidence_read(evidence, None);
    };
    let prefixes = field_indices
        .into_iter()
        .map(|index| format!("{QUERY_PROJECTION_PREFIX}{index}-"))
        .collect::<Vec<_>>();
    let selected = files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| prefixes.iter().any(|prefix| name.starts_with(prefix)))
        })
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        evidence_read(evidence, Some(files))
    } else {
        evidence_read(evidence, Some(&selected))
    }
}

struct ActivePageQuery {
    generation: u64,
    handle: Arc<InterruptHandle>,
}

static ACTIVE_PAGE_QUERIES: OnceLock<Mutex<HashMap<String, ActivePageQuery>>> = OnceLock::new();
static REPRESENTATIVE_OVERRIDE_BUILD: OnceLock<Mutex<()>> = OnceLock::new();
static QUERY_PROJECTION_BUILD: OnceLock<Mutex<()>> = OnceLock::new();
static SAMPLE_CALL_PROJECTION_BUILD: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone, Debug, PartialEq)]
struct CachedResultRow {
    record_number: i64,
    alt_index: i32,
    allele_id: String,
    chromosome: String,
    position: i64,
    reference: String,
    alternate: String,
    variant_id: Option<String>,
    quality: Option<f64>,
    filter: String,
    gene_symbol: Option<String>,
    gene_id: Option<String>,
    transcript_id: Option<String>,
    consequence: Option<String>,
    impact: Option<String>,
    canonical: bool,
    mane_select: Option<String>,
    alternate_count: i32,
    format: Option<String>,
    samples_json: String,
    zygosity: Option<String>,
    zygosity_sort: Option<i32>,
}

static MATCHED_ROW_CACHE: OnceLock<Mutex<VecDeque<(String, Arc<Vec<CachedResultRow>>)>>> =
    OnceLock::new();
const MATCHED_ROW_CACHE_ENTRIES: usize = 8;
const MATCHED_ROW_CACHE_LIMIT: i64 = 10_000;

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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
    pub excluded_auxiliary_records: u64,
    pub samples: Vec<String>,
    pub input_content_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredSummary {
    pub records: u64,
    pub excluded_auxiliary_records: u64,
    pub consequences: u64,
    pub evidence: u64,
    pub fields: usize,
    pub sources: Vec<String>,
    pub source_value_counts: BTreeMap<String, u64>,
}

const VARIANT_CHUNK_RECORDS: usize = 32_768;
const STRUCTURED_CHUNK_RECORDS: usize = 4_096;

struct VariantRecord {
    line_number: usize,
    record_number: i64,
    line: String,
    canonical_alleles: Vec<CanonicalAllele>,
}

struct ContentHashReader<R> {
    inner: R,
    digest: Rc<RefCell<Sha256>>,
}

impl<R: Read> Read for ContentHashReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.digest.borrow_mut().update(&buffer[..read]);
        Ok(read)
    }
}

#[derive(Default)]
struct VariantBatch {
    schema_version: Vec<i32>,
    allele_id: Vec<String>,
    record_number: Vec<i64>,
    alt_index: Vec<i32>,
    alternate_count: Vec<i32>,
    chromosome: Vec<String>,
    position: Vec<i64>,
    reference: Vec<String>,
    alternate: Vec<String>,
    original_chromosome: Vec<String>,
    original_position: Vec<i64>,
    original_reference: Vec<String>,
    original_alternate: Vec<String>,
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
    zygosity: Vec<Option<String>>,
    zygosity_sort: Vec<Option<i32>>,
    consequences_json: Vec<String>,
}

#[derive(Default)]
struct SampleCallProjectionBatch {
    record_number: Vec<i64>,
    alt_index: Vec<i32>,
    zygosity: Vec<Option<String>>,
    zygosity_sort: Vec<Option<i32>>,
}

struct LegacySampleCallRow {
    record_number: i64,
    alt_index: i32,
    format: Option<String>,
    samples_json: String,
}

#[derive(Default)]
struct ConsequenceBatch {
    schema_version: Vec<i32>,
    consequence_id: Vec<String>,
    allele_id: Vec<String>,
    ordinal: Vec<i64>,
    feature_type: Vec<String>,
    feature_id: Vec<Option<String>>,
    transcript_id: Vec<Option<String>>,
    gene_id: Vec<Option<String>>,
    gene_symbol: Vec<Option<String>>,
    biotype: Vec<Option<String>>,
    consequence_terms_json: Vec<String>,
    primary_consequence: Vec<Option<String>>,
    impact: Vec<Option<String>>,
    canonical: Vec<bool>,
    mane_select: Vec<Option<String>>,
    mane_plus_clinical: Vec<Option<String>>,
    protein_id: Vec<Option<String>>,
    exon: Vec<Option<String>>,
    intron: Vec<Option<String>>,
    hgvsg: Vec<Option<String>>,
    hgvsc: Vec<Option<String>>,
    hgvsp: Vec<Option<String>>,
    distance: Vec<Option<i64>>,
    strand: Vec<Option<i32>>,
    consequence_json: Vec<String>,
    selected: Vec<bool>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RepresentativeFields {
    gene_symbol: Option<String>,
    gene_id: Option<String>,
    transcript_id: Option<String>,
    consequence: Option<String>,
    impact: Option<String>,
    canonical: bool,
    mane_select: Option<String>,
}

#[derive(Default)]
struct RepresentativeOverrideBatch {
    schema_version: Vec<i32>,
    input_fingerprint: Vec<String>,
    selection_contract: Vec<String>,
    allele_id: Vec<String>,
    gene_symbol: Vec<Option<String>>,
    gene_id: Vec<Option<String>>,
    transcript_id: Vec<Option<String>>,
    consequence: Vec<Option<String>>,
    impact: Vec<Option<String>>,
    canonical: Vec<bool>,
    mane_select: Vec<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EvidenceValue {
    String(String),
    Integer(i64),
    Number(u64),
    Boolean(bool),
    Json(String),
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
                "alternate_count",
                Arc::new(Int32Array::from(self.alternate_count)),
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
                "original_chromosome",
                Arc::new(StringArray::from(self.original_chromosome)),
                false,
            ),
            (
                "original_position",
                Arc::new(Int64Array::from(self.original_position)),
                false,
            ),
            (
                "original_reference",
                Arc::new(StringArray::from(self.original_reference)),
                false,
            ),
            (
                "original_alternate",
                Arc::new(StringArray::from(self.original_alternate)),
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
            ("zygosity", Arc::new(StringArray::from(self.zygosity)), true),
            (
                "zygosity_sort",
                Arc::new(Int32Array::from(self.zygosity_sort)),
                true,
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
        self.alternate_count.append(&mut other.alternate_count);
        self.chromosome.append(&mut other.chromosome);
        self.position.append(&mut other.position);
        self.reference.append(&mut other.reference);
        self.alternate.append(&mut other.alternate);
        self.original_chromosome
            .append(&mut other.original_chromosome);
        self.original_position.append(&mut other.original_position);
        self.original_reference
            .append(&mut other.original_reference);
        self.original_alternate
            .append(&mut other.original_alternate);
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
        self.zygosity.append(&mut other.zygosity);
        self.zygosity_sort.append(&mut other.zygosity_sort);
        self.consequences_json.append(&mut other.consequences_json);
    }
}

impl SampleCallProjectionBatch {
    fn len(&self) -> usize {
        self.record_number.len()
    }

    fn into_record_batch(self) -> Result<RecordBatch, String> {
        record_batch(vec![
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
            ("zygosity", Arc::new(StringArray::from(self.zygosity)), true),
            (
                "zygosity_sort",
                Arc::new(Int32Array::from(self.zygosity_sort)),
                true,
            ),
        ])
    }
}

fn append_legacy_sample_call_group(
    output: &mut SampleCallProjectionBatch,
    rows: &mut Vec<LegacySampleCallRow>,
) -> Result<(), String> {
    let alternate_count = rows
        .iter()
        .map(|row| row.alt_index)
        .max()
        .ok_or("legacy result sample-call group is empty")?;
    if alternate_count < 1 {
        return Err("legacy result has an invalid alternate allele index".into());
    }
    for row in rows.drain(..) {
        let (zygosity, zygosity_sort) = zygosity_from_samples_json(
            row.format.as_deref(),
            &row.samples_json,
            row.alt_index,
            alternate_count,
        );
        output.record_number.push(row.record_number);
        output.alt_index.push(row.alt_index);
        output.zygosity.push(zygosity);
        output.zygosity_sort.push(zygosity_sort);
    }
    Ok(())
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
                "feature_type",
                Arc::new(StringArray::from(self.feature_type)),
                false,
            ),
            (
                "feature_id",
                Arc::new(StringArray::from(self.feature_id)),
                true,
            ),
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
                "mane_plus_clinical",
                Arc::new(StringArray::from(self.mane_plus_clinical)),
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
            (
                "selected",
                Arc::new(BooleanArray::from(self.selected)),
                false,
            ),
        ])
    }

    fn extend(&mut self, mut other: Self) {
        self.schema_version.append(&mut other.schema_version);
        self.consequence_id.append(&mut other.consequence_id);
        self.allele_id.append(&mut other.allele_id);
        self.ordinal.append(&mut other.ordinal);
        self.feature_type.append(&mut other.feature_type);
        self.feature_id.append(&mut other.feature_id);
        self.transcript_id.append(&mut other.transcript_id);
        self.gene_id.append(&mut other.gene_id);
        self.gene_symbol.append(&mut other.gene_symbol);
        self.biotype.append(&mut other.biotype);
        self.consequence_terms_json
            .append(&mut other.consequence_terms_json);
        self.primary_consequence
            .append(&mut other.primary_consequence);
        self.impact.append(&mut other.impact);
        self.canonical.append(&mut other.canonical);
        self.mane_select.append(&mut other.mane_select);
        self.mane_plus_clinical
            .append(&mut other.mane_plus_clinical);
        self.protein_id.append(&mut other.protein_id);
        self.exon.append(&mut other.exon);
        self.intron.append(&mut other.intron);
        self.hgvsg.append(&mut other.hgvsg);
        self.hgvsc.append(&mut other.hgvsc);
        self.hgvsp.append(&mut other.hgvsp);
        self.distance.append(&mut other.distance);
        self.strand.append(&mut other.strand);
        self.consequence_json.append(&mut other.consequence_json);
        self.selected.append(&mut other.selected);
    }
}

impl RepresentativeOverrideBatch {
    fn len(&self) -> usize {
        self.allele_id.len()
    }

    fn push(&mut self, fingerprint: &str, allele_id: &str, fields: RepresentativeFields) {
        self.schema_version.push(SCHEMA_VERSION);
        self.input_fingerprint.push(fingerprint.to_owned());
        self.selection_contract
            .push(REPRESENTATIVE_SELECTION_CONTRACT.to_owned());
        self.allele_id.push(allele_id.to_owned());
        self.gene_symbol.push(fields.gene_symbol);
        self.gene_id.push(fields.gene_id);
        self.transcript_id.push(fields.transcript_id);
        self.consequence.push(fields.consequence);
        self.impact.push(fields.impact);
        self.canonical.push(fields.canonical);
        self.mane_select.push(fields.mane_select);
    }

    fn into_record_batch(self) -> Result<RecordBatch, String> {
        record_batch(vec![
            (
                "schema_version",
                Arc::new(Int32Array::from(self.schema_version)),
                false,
            ),
            (
                "input_fingerprint",
                Arc::new(StringArray::from(self.input_fingerprint)),
                false,
            ),
            (
                "selection_contract",
                Arc::new(StringArray::from(self.selection_contract)),
                false,
            ),
            (
                "allele_id",
                Arc::new(StringArray::from(self.allele_id)),
                false,
            ),
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

    fn extend(&mut self, mut other: Self) {
        self.schema_version.append(&mut other.schema_version);
        self.allele_id.append(&mut other.allele_id);
        self.consequence_id.append(&mut other.consequence_id);
        self.scope.append(&mut other.scope);
        self.source_id.append(&mut other.source_id);
        self.field_path.append(&mut other.field_path);
        self.value_type.append(&mut other.value_type);
        self.string_value.append(&mut other.string_value);
        self.integer_value.append(&mut other.integer_value);
        self.number_value.append(&mut other.number_value);
        self.boolean_value.append(&mut other.boolean_value);
        self.json_value.append(&mut other.json_value);
    }

    fn value(&self, index: usize) -> Option<EvidenceValue> {
        match self.value_type[index].as_str() {
            "string" => self.string_value[index]
                .as_ref()
                .map(|value| EvidenceValue::String(value.clone())),
            "integer" => self.integer_value[index].map(EvidenceValue::Integer),
            "number" => {
                self.number_value[index].map(|value| EvidenceValue::Number(value.to_bits()))
            }
            "boolean" => self.boolean_value[index].map(EvidenceValue::Boolean),
            "json" => self.json_value[index]
                .as_ref()
                .map(|value| EvidenceValue::Json(value.clone())),
            _ => None,
        }
    }

    fn text_value(&self, index: usize) -> Option<String> {
        match self.value(index)? {
            EvidenceValue::String(value) | EvidenceValue::Json(value) => Some(value),
            EvidenceValue::Integer(value) => Some(value.to_string()),
            EvidenceValue::Number(value) => Some(f64::from_bits(value).to_string()),
            EvidenceValue::Boolean(value) => Some(value.to_string()),
        }
    }

    fn push_selected_copy(&mut self, index: usize, consequence_id: &str) {
        self.schema_version.push(SCHEMA_VERSION);
        self.allele_id.push(self.allele_id[index].clone());
        self.consequence_id.push(Some(consequence_id.to_owned()));
        self.scope.push("selected".into());
        self.source_id.push(self.source_id[index].clone());
        self.field_path.push(self.field_path[index].clone());
        self.value_type.push(self.value_type[index].clone());
        self.string_value.push(self.string_value[index].clone());
        self.integer_value.push(self.integer_value[index]);
        self.number_value.push(self.number_value[index]);
        self.boolean_value.push(self.boolean_value[index]);
        self.json_value.push(self.json_value[index].clone());
    }

    fn push_selected_string(
        &mut self,
        allele_id: &str,
        consequence_id: &str,
        source_id: &str,
        field_path: &str,
        value: String,
    ) {
        self.schema_version.push(SCHEMA_VERSION);
        self.allele_id.push(allele_id.to_owned());
        self.consequence_id.push(Some(consequence_id.to_owned()));
        self.scope.push("selected".into());
        self.source_id.push(source_id.to_owned());
        self.field_path.push(field_path.to_owned());
        self.value_type.push("string".into());
        self.string_value.push(Some(value));
        self.integer_value.push(None);
        self.number_value.push(None);
        self.boolean_value.push(None);
        self.json_value.push(None);
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

fn zygosity_label_and_sort(
    sample_name: Option<&str>,
    format: Option<&str>,
    sample_value: Option<&str>,
    sample_count: usize,
    alt_index: usize,
    alternate_count: usize,
) -> (Option<String>, Option<i32>) {
    if sample_count == 0 {
        return (None, None);
    }
    if sample_count > 1 {
        return (Some("Multiple sample calls".into()), None);
    }
    let call = annocat_core::sample_call::parse_sample_call(
        sample_name.unwrap_or_default(),
        format,
        sample_value.unwrap_or("."),
        alt_index,
        alternate_count,
    );
    use annocat_core::sample_call::GenotypeRelation;
    match call.genotype_relation {
        GenotypeRelation::Reference => (Some("Reference".into()), Some(0)),
        GenotypeRelation::OtherAlternate => (Some("Other alternate".into()), Some(1)),
        GenotypeRelation::Heterozygous => (Some("Heterozygous".into()), Some(2)),
        GenotypeRelation::HaploidAlternate => (Some("Haploid alternate".into()), Some(3)),
        GenotypeRelation::MixedAlternate => (Some("Mixed alternate".into()), Some(4)),
        GenotypeRelation::HomozygousAlternate => (Some("Homozygous alternate".into()), Some(5)),
        GenotypeRelation::PartiallyCalled => (Some("Partially called".into()), None),
        GenotypeRelation::NotCalled => (Some("Not called".into()), None),
        GenotypeRelation::Invalid => (Some("Invalid genotype".into()), None),
        GenotypeRelation::Unavailable => (None, None),
    }
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
    let alternate_count = columns[4].split(',').count();
    if input.canonical_alleles.len() != alternate_count {
        return Err(format!(
            "VCF record on line {} has inconsistent normalized allele metadata",
            input.line_number
        ));
    }
    let mut batch = VariantBatch::default();
    for (alt_offset, alternate) in columns[4].split(',').enumerate() {
        if !annocat_core::vcf::is_variant_alternate(alternate) {
            continue;
        }
        let canonical = &input.canonical_alleles[alt_offset];
        let matching = matching_consequences(&consequences, columns[3], alternate, columns[4]);
        let best = best_consequence(&matching);
        let best_value = |names: &[&str]| {
            best.and_then(|entry| consequence_text(entry, names))
                .map(str::to_owned)
        };
        batch.schema_version.push(SCHEMA_VERSION);
        batch.allele_id.push(allele_id(
            &canonical.chromosome,
            i64::try_from(canonical.position)
                .map_err(|_| "normalized allele position exceeds supported range")?,
            &canonical.reference,
            &canonical.alternate,
        ));
        batch.record_number.push(input.record_number);
        batch.alt_index.push((alt_offset + 1) as i32);
        batch.alternate_count.push(
            i32::try_from(alternate_count)
                .map_err(|_| "VCF record contains too many alternate alleles")?,
        );
        batch.chromosome.push(canonical.chromosome.clone());
        batch.position.push(
            i64::try_from(canonical.position)
                .map_err(|_| "normalized allele position exceeds supported range")?,
        );
        batch.reference.push(canonical.reference.clone());
        batch.alternate.push(canonical.alternate.clone());
        batch.original_chromosome.push(columns[0].to_owned());
        batch.original_position.push(position);
        batch.original_reference.push(columns[3].to_owned());
        batch.original_alternate.push(alternate.to_owned());
        batch.variant_id.push(optional_vcf(columns[2]));
        batch.quality.push(quality);
        batch.filter.push(columns[6].to_owned());
        batch
            .gene_symbol
            .push(best_value(&["SYMBOL", "gene_symbol"]));
        batch.gene_id.push(best_value(&["Gene", "gene_id"]));
        batch
            .transcript_id
            .push(best_value(&["Feature", "transcript_id"]));
        batch
            .consequence
            .push(best_value(&["Consequence", "primary_consequence"]));
        batch.impact.push(best_value(&["IMPACT", "impact"]));
        batch
            .canonical
            .push(best.is_some_and(|entry| consequence_truthy(entry, &["CANONICAL", "canonical"])));
        batch
            .mane_select
            .push(best_value(&["MANE_SELECT", "mane_select", "MANE", "mane"]));
        batch.sample_names_json.push(sample_names_json.to_owned());
        let format = columns.get(8).and_then(|value| optional_vcf(value));
        let (zygosity, zygosity_sort) = zygosity_label_and_sort(
            sample_names.first().map(String::as_str),
            format.as_deref(),
            columns.get(9).copied(),
            sample_names.len(),
            alt_offset + 1,
            alternate_count,
        );
        batch.format.push(format);
        batch.samples_json.push(samples_json.clone());
        batch.zygosity.push(zygosity);
        batch.zygosity_sort.push(zygosity_sort);
        batch.consequences_json.push(
            serde_json::to_string(&best.into_iter().collect::<Vec<_>>())
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(batch)
}

fn fallback_canonical_allele(
    chromosome: &str,
    position: i64,
    reference: &str,
    alternate: &str,
) -> Result<CanonicalAllele, String> {
    Ok(CanonicalAllele {
        chromosome: canonical_chromosome(chromosome),
        position: u64::try_from(position).map_err(|_| "VCF position must be positive")?,
        reference: reference.to_ascii_uppercase(),
        alternate: alternate.to_ascii_uppercase(),
    })
}

fn canonicalize_or_preserve_auxiliary<R: ReferenceSequence>(
    reference_source: &mut R,
    chromosome: &str,
    position: u64,
    reference: &str,
    alternate: &str,
) -> Result<CanonicalAllele, NormalizeError> {
    match canonicalize(reference_source, chromosome, position, reference, alternate) {
        Err(NormalizeError::MissingChromosome(_)) if !is_main_grch38_chromosome(chromosome) => {
            Ok(CanonicalAllele {
                chromosome: canonical_chromosome(chromosome),
                position,
                reference: reference.to_ascii_uppercase(),
                alternate: alternate.to_ascii_uppercase(),
            })
        }
        result => result,
    }
}

fn is_excluded_auxiliary_contig(chromosome: &str) -> bool {
    let chromosome = chromosome.to_ascii_lowercase();
    chromosome == "ebv"
        || chromosome == "chrebv"
        || chromosome == "hs37d5"
        || chromosome.contains("decoy")
}

fn is_main_grch38_chromosome(chromosome: &str) -> bool {
    let chromosome = chromosome
        .strip_prefix("chr")
        .unwrap_or(chromosome)
        .to_ascii_uppercase();
    if matches!(chromosome.as_str(), "X" | "Y" | "M" | "MT")
        || chromosome
            .parse::<u8>()
            .is_ok_and(|value| (1..=22).contains(&value))
    {
        return true;
    }
    let accession = chromosome.split('.').next().unwrap_or(&chromosome);
    accession
        .strip_prefix("NC_")
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|value| (1..=24).contains(&value) || value == 12_920)
        || accession
            .strip_prefix("CM")
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|value| (663..=686).contains(&value))
}

fn canonical_alleles_for_vcf_line(
    line_number: usize,
    line: &str,
    reference_source: Option<&mut IndexedReference>,
) -> Result<Option<Vec<CanonicalAllele>>, String> {
    let columns = line.split('\t').take(5).collect::<Vec<_>>();
    if columns.len() < 5 {
        return Err(format!(
            "VCF record on line {line_number} has fewer than 5 columns"
        ));
    }
    if is_excluded_auxiliary_contig(columns[0]) {
        return Ok(None);
    }
    let position = columns[1]
        .parse::<i64>()
        .map_err(|_| format!("VCF record on line {line_number} has an invalid position"))?;
    let mut reference_source = reference_source;
    columns[4]
        .split(',')
        .map(|alternate| {
            if !annocat_core::vcf::is_variant_alternate(alternate) {
                return fallback_canonical_allele(
                    columns[0], position, columns[3], alternate,
                );
            }
            let Some(source) = reference_source.as_deref_mut() else {
                return fallback_canonical_allele(
                    columns[0], position, columns[3], alternate,
                );
            };
            canonicalize_or_preserve_auxiliary(
                source,
                columns[0],
                u64::try_from(position).map_err(|_| "VCF position must be positive")?,
                columns[3],
                alternate,
            )
            .map_err(|error| {
                format!(
                    "GRCh38 allele validation failed on VCF line {line_number} at {}:{} {}>{}: {error}",
                    columns[0], columns[1], columns[3], alternate
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
    pub exact_total: bool,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageSortRequest {
    pub column: String,
    pub direction: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreFilterRuleRequest {
    pub column: String,
    pub operator: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub include_missing: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceFilterRequest {
    pub index: usize,
    pub operator: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value2: String,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub include_missing: Option<bool>,
}

struct CatalogEntry {
    types: BTreeSet<&'static str>,
    occurrences: u64,
    observed_categories: BTreeMap<String, String>,
    observed_categories_complete: bool,
}

impl Default for CatalogEntry {
    fn default() -> Self {
        Self {
            types: BTreeSet::new(),
            occurrences: 0,
            observed_categories: BTreeMap::new(),
            observed_categories_complete: true,
        }
    }
}

const MAX_CATEGORICAL_VALUES: usize = 100;
const MAX_CATEGORICAL_VALUE_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CategoricalParser {
    Scalar,
    Json,
}

struct CategoricalContract {
    source_id: String,
    field_name: String,
    match_mode: String,
    parser: CategoricalParser,
    values: Vec<Value>,
    discover_observed: bool,
}

struct StructuredRecord {
    line_number: usize,
    line: String,
    canonical_alleles: BTreeMap<String, CanonicalAllele>,
}

#[derive(Deserialize)]
struct StructuredIdentity {
    allele_string: String,
    start: i64,
    seq_region_name: String,
}

#[derive(Deserialize)]
struct StructuredDocument {
    allele_string: String,
    start: i64,
    seq_region_name: String,
    #[serde(default)]
    alleles: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    most_severe_consequence: Option<String>,
    #[serde(default)]
    transcript_consequences: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    regulatory_feature_consequences: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    motif_feature_consequences: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    intergenic_consequences: Option<Vec<Map<String, Value>>>,
    #[serde(flatten)]
    extra_fields: Map<String, Value>,
}

struct ParsedStructuredRecord {
    is_variant: bool,
    consequences: ConsequenceBatch,
    evidence: EvidenceBatch,
    catalog: BTreeMap<(String, String, String), CatalogEntry>,
}

#[derive(Default)]
struct StructuredCounts {
    records: u64,
    consequences: u64,
    evidence: u64,
}

impl ParsedStructuredRecord {
    fn rebase_consequence_ids(&mut self, consequence_count: &mut u64) -> Result<(), String> {
        let mut replacements = HashMap::with_capacity(self.consequences.len());
        for index in 0..self.consequences.len() {
            *consequence_count = consequence_count.saturating_add(1);
            let previous = self.consequences.consequence_id[index].clone();
            let replacement = format!(
                "{}:consequence:{}",
                self.consequences.allele_id[index], *consequence_count
            );
            self.consequences.consequence_id[index] = replacement.clone();
            replacements.insert(previous, replacement);
        }
        for consequence_id in self.evidence.consequence_id.iter_mut().flatten() {
            *consequence_id = replacements.get(consequence_id).cloned().ok_or_else(|| {
                format!("structured evidence references an unknown consequence: {consequence_id}")
            })?;
        }
        Ok(())
    }
}

fn canonical_structured_alleles(
    line_number: usize,
    identity: &StructuredIdentity,
    canonical_record: Option<&[CanonicalAllele]>,
    reference_source: Option<&mut IndexedReference>,
) -> Result<BTreeMap<String, CanonicalAllele>, String> {
    let alleles = identity.allele_string.split('/').collect::<Vec<_>>();
    if alleles.len() < 2 {
        return Ok(BTreeMap::new());
    }
    if let Some(record) = canonical_record
        && record.len() != alleles.len() - 1
    {
        return Err(format!(
            "structured record {line_number} has {} alternate alleles but the canonical VCF record has {}",
            alleles.len() - 1,
            record.len()
        ));
    }
    let mut reference_source = reference_source;
    let mut canonical = BTreeMap::new();
    for (alternate_index, alternate) in alleles[1..].iter().enumerate() {
        if !annocat_core::vcf::is_variant_alternate(alternate) {
            continue;
        }
        let allele = if let Some(record) = canonical_record {
            record[alternate_index].clone()
        } else if let Some(source) = reference_source.as_deref_mut() {
            let start = u64::try_from(identity.start)
                .map_err(|_| format!("structured record {line_number} has an invalid start"))?;
            let (position, reference, alternate_anchored) = match (alleles[0], *alternate) {
                ("-", inserted) => {
                    let position = start.checked_sub(1).ok_or_else(|| {
                        format!(
                            "structured insertion record {line_number} cannot be reference-anchored"
                        )
                    })?;
                    let anchor = char::from(
                        source
                            .base(&identity.seq_region_name, position)
                            .map_err(|error| {
                                format!(
                                    "cannot anchor structured insertion record {line_number}: {error}"
                                )
                            })?,
                    );
                    (position, anchor.to_string(), format!("{anchor}{inserted}"))
                }
                (deleted, "-") => {
                    let position = start.checked_sub(1).ok_or_else(|| {
                        format!(
                            "structured deletion record {line_number} cannot be reference-anchored"
                        )
                    })?;
                    let anchor = char::from(
                        source
                            .base(&identity.seq_region_name, position)
                            .map_err(|error| {
                                format!(
                                    "cannot anchor structured deletion record {line_number}: {error}"
                                )
                            })?,
                    );
                    (position, format!("{anchor}{deleted}"), anchor.to_string())
                }
                (reference, alternate) => (start, reference.to_owned(), alternate.to_owned()),
            };
            canonicalize_or_preserve_auxiliary(
                source,
                &identity.seq_region_name,
                position,
                &reference,
                &alternate_anchored,
            )
            .map_err(|error| {
                format!(
                    "cannot normalize structured record {line_number} allele {}>{}: {error}",
                    alleles[0], alternate
                )
            })?
        } else {
            fallback_canonical_allele(
                &identity.seq_region_name,
                identity.start,
                alleles[0],
                alternate,
            )?
        };
        if canonical.insert((*alternate).to_owned(), allele).is_some() {
            return Err(format!(
                "structured record {line_number} has ambiguous duplicate alternate allele {alternate}"
            ));
        }
    }
    Ok(canonical)
}

struct CanonicalVcfRecords {
    lines: std::io::Lines<Box<dyn BufRead>>,
    line_number: usize,
    reference: IndexedReference,
}

impl CanonicalVcfRecords {
    fn open(vcf: &Path, fasta: &Path) -> Result<Self, String> {
        Ok(Self {
            lines: super::csq::open(vcf)?.lines(),
            line_number: 0,
            reference: IndexedReference::open(fasta).map_err(|error| {
                format!("cannot initialize canonical VCF allele normalization: {error}")
            })?,
        })
    }

    fn next(&mut self) -> Result<Option<Vec<CanonicalAllele>>, String> {
        for line in self.lines.by_ref() {
            self.line_number += 1;
            let line =
                line.map_err(|error| format!("cannot read canonical annotated VCF: {error}"))?;
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let canonical_alleles =
                canonical_alleles_for_vcf_line(self.line_number, &line, Some(&mut self.reference))?;
            if canonical_alleles.is_some() {
                return Ok(canonical_alleles);
            }
        }
        Ok(None)
    }
}

fn parse_structured_record(
    record: &StructuredRecord,
    source_aliases: &BTreeMap<String, String>,
) -> Result<ParsedStructuredRecord, String> {
    let document: StructuredDocument = serde_json::from_str(&record.line).map_err(|error| {
        format!(
            "invalid structured output record {}: {error}",
            record.line_number
        )
    })?;
    let StructuredDocument {
        allele_string,
        start,
        seq_region_name,
        alleles: supplementary_alleles,
        most_severe_consequence,
        transcript_consequences,
        regulatory_feature_consequences,
        motif_feature_consequences,
        intergenic_consequences,
        extra_fields,
    } = document;
    let alleles = allele_string.split('/').collect::<Vec<_>>();
    let real_alternates = alleles[1..]
        .iter()
        .copied()
        .filter(|alternate| annocat_core::vcf::is_variant_alternate(alternate))
        .collect::<Vec<_>>();
    let is_variant = alleles.len() > 1 && !real_alternates.is_empty();
    if !is_variant {
        return Ok(ParsedStructuredRecord {
            is_variant: false,
            consequences: ConsequenceBatch::default(),
            evidence: EvidenceBatch::default(),
            catalog: BTreeMap::new(),
        });
    }

    let reference = alleles[0];
    let mut evidence = EvidenceBatch::default();
    let mut catalog = BTreeMap::new();
    let mut allele_evidence = BTreeMap::<(String, String, String), Value>::new();
    let mut conflicting_allele_evidence = BTreeSet::<(String, String, String)>::new();
    let mut record_lists = BTreeMap::<(String, String), Value>::new();
    let mut conflicting_record_lists = BTreeSet::<(String, String)>::new();
    let mut deferred_scoped_evidence = Vec::<(String, String, String, Value)>::new();
    for (key, value) in &extra_fields {
        if !TOP_LEVEL_FIELDS.contains(&key.as_str()) {
            let source_id = structured_source_alias(source_aliases, key).unwrap_or(key);
            for alternate in &real_alternates {
                let id = record
                    .canonical_alleles
                    .get(*alternate)
                    .map(canonical_allele_id)
                    .unwrap_or_else(|| allele_id(&seq_region_name, start, reference, alternate));
                if crate::evidence_resolution::is_record_list(source_id, value) {
                    merge_record_list(
                        &mut record_lists,
                        &mut conflicting_record_lists,
                        &id,
                        source_id,
                        value,
                    );
                    continue;
                }
                if let Some(scope) = source_evidence_scope(source_id)
                    && scope != SourceEvidenceScope::Allele
                {
                    deferred_scoped_evidence.push((
                        id,
                        (*alternate).to_owned(),
                        source_id.to_owned(),
                        value.clone(),
                    ));
                    continue;
                }
                merge_allele_evidence(
                    &mut allele_evidence,
                    &mut conflicting_allele_evidence,
                    &id,
                    source_id,
                    value,
                );
            }
        }
    }

    let mut structured_consequences = Vec::<(&'static str, Map<String, Value>)>::new();
    structured_consequences.extend(
        transcript_consequences
            .unwrap_or_default()
            .into_iter()
            .map(|value| ("transcript", value)),
    );
    structured_consequences.extend(
        regulatory_feature_consequences
            .unwrap_or_default()
            .into_iter()
            .map(|value| ("regulatory", value)),
    );
    structured_consequences.extend(
        motif_feature_consequences
            .unwrap_or_default()
            .into_iter()
            .map(|value| ("motif", value)),
    );
    structured_consequences.extend(
        intergenic_consequences
            .unwrap_or_default()
            .into_iter()
            .map(|value| ("intergenic", value)),
    );
    if structured_consequences.is_empty()
        && real_alternates.len() == 1
        && let Some(term) = most_severe_consequence
    {
        let feature_type = if term == "intergenic_variant" {
            "intergenic"
        } else {
            "unresolved"
        };
        structured_consequences.push((
            feature_type,
            Map::from_iter([
                (
                    "variant_allele".into(),
                    Value::String(real_alternates[0].into()),
                ),
                (
                    "consequence_terms".into(),
                    Value::Array(vec![Value::String(term.clone())]),
                ),
                (
                    "impact".into(),
                    Value::String(consequence_impact(&term).into()),
                ),
            ]),
        ));
    }
    let mut consequences = ConsequenceBatch::default();
    let mut linked_evidence_written = BTreeSet::new();
    let mut consequence_indices = HashMap::<String, Vec<usize>>::new();
    for (ordinal, (feature_type, consequence_object)) in structured_consequences.iter().enumerate()
    {
        let alternate = consequence_object
            .get("variant_allele")
            .and_then(Value::as_str)
            .or_else(|| (real_alternates.len() == 1).then_some(real_alternates[0]))
            .ok_or_else(|| {
                format!(
                    "structured consequence {} on multiallelic record {} has no alternate allele",
                    ordinal + 1,
                    record.line_number
                )
            })?;
        if !annocat_core::vcf::is_variant_alternate(alternate) {
            continue;
        }
        let id = record
            .canonical_alleles
            .get(alternate)
            .map(canonical_allele_id)
            .unwrap_or_else(|| allele_id(&seq_region_name, start, reference, alternate));
        consequence_indices
            .entry(id.clone())
            .or_default()
            .push(ordinal);
        let consequence_id = format!("local:{ordinal}");
        let terms = consequence_object
            .get("consequence_terms")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let terms_json = serde_json::to_string(terms).map_err(|error| error.to_string())?;
        let primary = terms.first().and_then(Value::as_str).map(str::to_owned);
        let feature_id = match *feature_type {
            "transcript" => optional_json_string(consequence_object, "transcript_id"),
            "regulatory" => optional_json_string(consequence_object, "regulatory_feature_id"),
            "motif" => optional_json_string(consequence_object, "motif_feature_id"),
            _ => None,
        };
        let mut enriched_object = consequence_object.clone();
        enriched_object.insert("feature_type".into(), Value::String((*feature_type).into()));
        if let Some(feature_id) = &feature_id {
            enriched_object.insert("feature_id".into(), Value::String(feature_id.clone()));
        }
        let raw_json =
            serde_json::to_string(&enriched_object).map_err(|error| error.to_string())?;
        consequences.schema_version.push(SCHEMA_VERSION);
        consequences.consequence_id.push(consequence_id.clone());
        consequences.allele_id.push(id.clone());
        consequences.ordinal.push(ordinal as i64);
        consequences.feature_type.push((*feature_type).into());
        consequences.feature_id.push(feature_id);
        consequences.transcript_id.push(
            (*feature_type == "transcript")
                .then(|| optional_json_string(consequence_object, "transcript_id"))
                .flatten(),
        );
        consequences
            .gene_id
            .push(optional_json_string(consequence_object, "gene_id"));
        consequences
            .gene_symbol
            .push(optional_json_string(consequence_object, "gene_symbol"));
        consequences
            .biotype
            .push(optional_json_string(consequence_object, "biotype"));
        consequences.consequence_terms_json.push(terms_json);
        consequences.primary_consequence.push(primary);
        consequences
            .impact
            .push(optional_json_string(consequence_object, "impact"));
        consequences
            .canonical
            .push(json_bool(consequence_object.get("canonical")));
        consequences
            .mane_select
            .push(optional_json_string(consequence_object, "mane_select"));
        consequences.mane_plus_clinical.push(optional_json_string(
            consequence_object,
            "mane_plus_clinical",
        ));
        consequences
            .protein_id
            .push(optional_json_string(consequence_object, "protein_id"));
        consequences
            .exon
            .push(optional_json_string(consequence_object, "exon"));
        consequences
            .intron
            .push(optional_json_string(consequence_object, "intron"));
        consequences
            .hgvsg
            .push(optional_json_string(consequence_object, "hgvsg"));
        consequences
            .hgvsc
            .push(optional_json_string(consequence_object, "hgvsc"));
        consequences
            .hgvsp
            .push(optional_json_string(consequence_object, "hgvsp"));
        consequences
            .distance
            .push(optional_json_i64(consequence_object, "distance"));
        consequences
            .strand
            .push(optional_json_i64(consequence_object, "strand").map(|value| value as i32));
        consequences.consequence_json.push(raw_json);
        consequences.selected.push(false);

        for (key, value) in consequence_object {
            if !CONSEQUENCE_FIELDS.contains(&key.as_str()) {
                let source_id = structured_source_alias(source_aliases, key).unwrap_or(key);
                if crate::evidence_resolution::is_record_list(source_id, value) {
                    merge_record_list(
                        &mut record_lists,
                        &mut conflicting_record_lists,
                        &id,
                        source_id,
                        value,
                    );
                    continue;
                }
                let declared_scope = source_evidence_scope(source_id);
                let (scope, linked_consequence) = match declared_scope {
                    Some(SourceEvidenceScope::Allele) => {
                        merge_allele_evidence(
                            &mut allele_evidence,
                            &mut conflicting_allele_evidence,
                            &id,
                            source_id,
                            value,
                        );
                        continue;
                    }
                    Some(SourceEvidenceScope::Transcript) => {
                        let linked = explicit_source_transcript(value).and_then(|transcript| {
                            matching_transcript_consequence(
                                &structured_consequences,
                                alternate,
                                transcript,
                            )
                        });
                        let linked = linked.map(|index| format!("local:{index}")).or_else(|| {
                            explicit_source_transcript(value)
                                .is_none()
                                .then(|| consequence_id.clone())
                        });
                        if let Some(linked) = linked {
                            ("transcript", Some(linked))
                        } else {
                            ("unresolved_transcript", None)
                        }
                    }
                    Some(SourceEvidenceScope::Feature) => {
                        (*feature_type, Some(consequence_id.clone()))
                    }
                    Some(SourceEvidenceScope::Gene) => ("gene", Some(consequence_id.clone())),
                    None => (*feature_type, Some(consequence_id.clone())),
                };
                let identity = (
                    id.clone(),
                    source_id.clone(),
                    scope.to_owned(),
                    linked_consequence.clone(),
                );
                if !linked_evidence_written.insert(identity) {
                    continue;
                }
                let context = EvidenceContext {
                    allele_id: &id,
                    consequence_id: linked_consequence.as_deref(),
                    scope,
                    source_id,
                };
                append_evidence_tree(&mut evidence, &mut catalog, &context, "", value)?;
            }
        }
    }
    let mut seen_supplementary_alleles = BTreeSet::new();
    for mut allele_object in supplementary_alleles.unwrap_or_default() {
        let alternate = allele_object
            .remove("allele")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| {
                format!(
                    "structured supplementary allele on record {} has no allele",
                    record.line_number
                )
            })?;
        if !annocat_core::vcf::is_variant_alternate(&alternate) {
            continue;
        }
        if !real_alternates.contains(&alternate.as_str()) {
            return Err(format!(
                "structured supplementary allele {alternate} on record {} is not a variant alternate",
                record.line_number
            ));
        }
        if !seen_supplementary_alleles.insert(alternate.clone()) {
            return Err(format!(
                "structured record {} has duplicate supplementary allele {alternate}",
                record.line_number
            ));
        }
        let id = record
            .canonical_alleles
            .get(&alternate)
            .map(canonical_allele_id)
            .unwrap_or_else(|| allele_id(&seq_region_name, start, reference, &alternate));
        for (key, value) in &allele_object {
            let source_id = structured_source_alias(source_aliases, key).unwrap_or(key);
            if crate::evidence_resolution::is_record_list(source_id, value) {
                merge_record_list(
                    &mut record_lists,
                    &mut conflicting_record_lists,
                    &id,
                    source_id,
                    value,
                );
                continue;
            }
            if source_evidence_scope(source_id) == Some(SourceEvidenceScope::Allele) {
                merge_allele_evidence(
                    &mut allele_evidence,
                    &mut conflicting_allele_evidence,
                    &id,
                    source_id,
                    value,
                );
                continue;
            }
            append_scoped_source_evidence(
                &mut evidence,
                &mut catalog,
                &mut linked_evidence_written,
                &structured_consequences,
                &id,
                &alternate,
                source_id,
                value,
            )?;
        }
    }
    for (id, alternate, source_id, value) in deferred_scoped_evidence {
        append_scoped_source_evidence(
            &mut evidence,
            &mut catalog,
            &mut linked_evidence_written,
            &structured_consequences,
            &id,
            &alternate,
            &source_id,
            &value,
        )?;
    }
    let selected_consequences = consequence_indices
        .into_iter()
        .filter_map(|(allele_id, indices)| {
            best_structured_consequence_index(&structured_consequences, &indices)
                .map(|index| (allele_id, index))
        })
        .collect::<HashMap<_, _>>();
    for (allele_id, ordinal) in &selected_consequences {
        let consequence_id = format!("local:{ordinal}");
        if let Some(index) = consequences
            .allele_id
            .iter()
            .zip(&consequences.consequence_id)
            .position(|(allele, consequence)| allele == allele_id && consequence == &consequence_id)
        {
            consequences.selected[index] = true;
        }
    }
    for ((allele_id, source_id), value) in record_lists {
        if conflicting_record_lists.contains(&(allele_id.clone(), source_id.clone())) {
            continue;
        }
        let Some(selected_index) = selected_consequences.get(&allele_id).copied() else {
            continue;
        };
        let Some(resolved) = crate::evidence_resolution::resolve_record_list(
            &source_id,
            &value,
            &structured_consequences[selected_index].1,
        )?
        else {
            continue;
        };
        let raw_context = EvidenceContext {
            allele_id: &allele_id,
            consequence_id: None,
            scope: "source_records",
            source_id: &source_id,
        };
        let mut hidden_catalog = BTreeMap::new();
        append_evidence_tree(
            &mut evidence,
            &mut hidden_catalog,
            &raw_context,
            &resolved.raw_field_path,
            &resolved.raw_value,
        )?;
        for field in resolved.fields {
            let (scope, consequence_id) = match field.scope {
                crate::evidence_resolution::ResolvedRecordScope::Allele => ("allele", None),
                crate::evidence_resolution::ResolvedRecordScope::Selected => {
                    ("selected", Some(format!("local:{selected_index}")))
                }
            };
            let context = EvidenceContext {
                allele_id: &allele_id,
                consequence_id: consequence_id.as_deref(),
                scope,
                source_id: &source_id,
            };
            append_evidence_tree(
                &mut evidence,
                &mut catalog,
                &context,
                &field.field_path,
                &field.value,
            )?;
        }
    }
    for ((id, source_id, field_path), value) in &allele_evidence {
        let context = EvidenceContext {
            allele_id: id,
            consequence_id: None,
            scope: "allele",
            source_id,
        };
        append_evidence_tree(&mut evidence, &mut catalog, &context, field_path, value)?;
    }
    materialize_selected_evidence(
        &mut evidence,
        &structured_consequences,
        &selected_consequences,
    );

    Ok(ParsedStructuredRecord {
        is_variant: true,
        consequences,
        evidence,
        catalog,
    })
}

fn parse_structured_chunk(
    pool: &ThreadPool,
    records: &[StructuredRecord],
    source_aliases: &BTreeMap<String, String>,
) -> Result<Vec<ParsedStructuredRecord>, String> {
    pool.install(|| {
        records
            .par_iter()
            .map(|record| parse_structured_record(record, source_aliases))
            .collect::<Result<Vec<_>, _>>()
    })
}

fn merge_catalog(
    target: &mut BTreeMap<(String, String, String), CatalogEntry>,
    source: BTreeMap<(String, String, String), CatalogEntry>,
) {
    for (key, entry) in source {
        let target_entry = target.entry(key).or_default();
        target_entry.types.extend(entry.types);
        target_entry.occurrences = target_entry.occurrences.saturating_add(entry.occurrences);
        target_entry.observed_categories_complete &= entry.observed_categories_complete;
        for value in entry.observed_categories.into_values() {
            insert_observed_category(target_entry, &value);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_and_write_structured_chunk(
    pool: &ThreadPool,
    records: &mut Vec<StructuredRecord>,
    consequence_writer: &mut ArrowWriter<File>,
    evidence_writer: &mut ArrowWriter<File>,
    consequence_batch: &mut ConsequenceBatch,
    evidence_batch: &mut EvidenceBatch,
    catalog: &mut BTreeMap<(String, String, String), CatalogEntry>,
    counts: &mut StructuredCounts,
    source_aliases: &BTreeMap<String, String>,
) -> Result<(), String> {
    let parsed = parse_structured_chunk(pool, records, source_aliases)?;
    for mut record in parsed {
        counts.records += u64::from(record.is_variant);
        counts.evidence = counts.evidence.saturating_add(record.evidence.len() as u64);
        record.rebase_consequence_ids(&mut counts.consequences)?;
        merge_catalog(catalog, record.catalog);
        consequence_batch.extend(record.consequences);
        evidence_batch.extend(record.evidence);

        if consequence_batch.len() >= VARIANT_CHUNK_RECORDS {
            let batch = std::mem::take(consequence_batch).into_record_batch()?;
            write_batch(consequence_writer, batch, "transcript consequences")?;
        }
        if evidence_batch.len() >= VARIANT_CHUNK_RECORDS {
            let batch = std::mem::take(evidence_batch).into_record_batch()?;
            write_batch(evidence_writer, batch, "source evidence")?;
        }
    }
    records.clear();
    Ok(())
}

#[cfg(test)]
pub fn convert_structured(
    ndjson: &Path,
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    catalog_json: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<StructuredSummary, String> {
    convert_structured_mode(
        ndjson,
        consequences_parquet,
        evidence_parquet,
        catalog_json,
        None,
        None,
        &BTreeMap::new(),
        cancelled,
        &mut progress,
    )
}

pub fn convert_structured_with_canonical_vcf_and_sources(
    ndjson: &Path,
    canonical_vcf: &Path,
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    catalog_json: &Path,
    fasta: &Path,
    source_ids: &[String],
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<StructuredSummary, String> {
    let aliases = structured_source_aliases(source_ids)?;
    convert_structured_mode(
        ndjson,
        consequences_parquet,
        evidence_parquet,
        catalog_json,
        Some(canonical_vcf),
        Some(fasta),
        &aliases,
        cancelled,
        &mut progress,
    )
}

fn structured_source_aliases(source_ids: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut aliases = BTreeMap::new();
    for source_id in source_ids {
        let source = annocat_core::source_catalog::source(source_id)
            .ok_or_else(|| format!("unknown annotation source: {source_id}"))?;
        let Some(raw_id) = source.fastvep_source.as_deref() else {
            continue;
        };
        if let Some(previous) = aliases.insert(raw_id.to_owned(), source_id.clone())
            && previous != *source_id
        {
            return Err(format!(
                "annotation sources {previous} and {source_id} share FastVEP key {raw_id}"
            ));
        }
    }
    Ok(aliases)
}

fn structured_source_alias<'a>(
    aliases: &'a BTreeMap<String, String>,
    source_id: &str,
) -> Option<&'a String> {
    aliases.get(source_id).or_else(|| {
        aliases.iter().find_map(|(raw_id, canonical_id)| {
            raw_id
                .eq_ignore_ascii_case(source_id)
                .then_some(canonical_id)
        })
    })
}

fn convert_structured_mode(
    ndjson: &Path,
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    catalog_json: &Path,
    canonical_vcf: Option<&Path>,
    fasta: Option<&Path>,
    source_aliases: &BTreeMap<String, String>,
    cancelled: impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<StructuredSummary, String> {
    let _ = fs::remove_file(consequences_parquet);
    let _ = fs::remove_file(evidence_parquet);
    let result = convert_structured_with_workers(
        ndjson,
        StructuredOutputPaths {
            consequences: consequences_parquet,
            evidence: evidence_parquet,
            catalog: catalog_json,
        },
        canonical_vcf,
        fasta,
        source_aliases,
        &cancelled,
        progress,
        structured_parser_workers(),
    );
    if result.is_err() {
        let _ = fs::remove_file(consequences_parquet);
        let _ = fs::remove_file(evidence_parquet);
        let _ = fs::remove_file(catalog_json);
    }
    result
}

fn structured_parser_workers() -> usize {
    std::thread::available_parallelism()
        .map(|workers| workers.get().saturating_sub(1).clamp(1, 8))
        .unwrap_or(1)
}

struct StructuredOutputPaths<'a> {
    consequences: &'a Path,
    evidence: &'a Path,
    catalog: &'a Path,
}

fn convert_structured_with_workers(
    ndjson: &Path,
    outputs: StructuredOutputPaths<'_>,
    canonical_vcf: Option<&Path>,
    fasta: Option<&Path>,
    source_aliases: &BTreeMap<String, String>,
    cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
    parser_workers: usize,
) -> Result<StructuredSummary, String> {
    let consequence_schema = ConsequenceBatch::default().into_record_batch()?.schema();
    let evidence_schema = EvidenceBatch::default().into_record_batch()?.schema();
    let mut consequence_writer = parquet_writer(outputs.consequences, consequence_schema)?;
    let mut evidence_writer = parquet_writer(outputs.evidence, evidence_schema)?;
    let parser_pool = ThreadPoolBuilder::new()
        .num_threads(parser_workers.max(1))
        .thread_name(|index| format!("annocat-structured-parser-{index}"))
        .build()
        .map_err(|error| format!("cannot initialize structured parser workers: {error}"))?;

    let file = fs::File::open(ndjson)
        .map_err(|error| format!("cannot open {}: {error}", ndjson.display()))?;
    let mut canonical_records = match (canonical_vcf, fasta) {
        (Some(vcf), Some(fasta)) => Some(CanonicalVcfRecords::open(vcf, fasta)?),
        (None, _) => None,
        (Some(_), None) => return Err("canonical VCF indexing requires a reference FASTA".into()),
    };
    let mut reference_source = if canonical_records.is_none() {
        fasta
            .map(IndexedReference::open)
            .transpose()
            .map_err(|error| {
                format!("cannot initialize structured allele normalization: {error}")
            })?
    } else {
        None
    };
    let mut catalog = BTreeMap::new();
    let mut counts = StructuredCounts::default();
    let mut consequence_batch = ConsequenceBatch::default();
    let mut evidence_batch = EvidenceBatch::default();
    let mut records = Vec::with_capacity(STRUCTURED_CHUNK_RECORDS);
    let mut processed_records = 0_u64;
    let mut excluded_auxiliary_records = 0_u64;
    let mut previous_bytes = 0_u64;
    let mut previous_records = 0_u64;
    let mut previous_at = Instant::now();

    for (record_index, line) in BufReader::new(file).lines().enumerate() {
        if cancelled() {
            return Err("cancelled".into());
        }
        let line = line.map_err(|error| format!("cannot read structured output: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        processed_records += 1;
        let identity: StructuredIdentity = serde_json::from_str(&line).map_err(|error| {
            format!(
                "invalid structured output identity on record {}: {error}",
                record_index + 1
            )
        })?;
        if is_excluded_auxiliary_contig(&identity.seq_region_name) {
            excluded_auxiliary_records += 1;
            continue;
        }
        let canonical_record = if let Some(records) = canonical_records.as_mut() {
            Some(records.next()?.ok_or_else(|| {
                format!(
                    "structured output record {} has no matching canonical VCF record",
                    record_index + 1
                )
            })?)
        } else {
            None
        };
        let canonical_alleles = canonical_structured_alleles(
            record_index + 1,
            &identity,
            canonical_record.as_deref(),
            reference_source.as_mut(),
        )?;
        records.push(StructuredRecord {
            line_number: record_index + 1,
            line,
            canonical_alleles,
        });
        if records.len() < STRUCTURED_CHUNK_RECORDS {
            continue;
        }
        parse_and_write_structured_chunk(
            &parser_pool,
            &mut records,
            &mut consequence_writer,
            &mut evidence_writer,
            &mut consequence_batch,
            &mut evidence_batch,
            &mut catalog,
            &mut counts,
            source_aliases,
        )?;
        if cancelled() {
            return Err("cancelled".into());
        }
        if processed_records.saturating_sub(previous_records) >= 10_000 {
            report_structured_progress(
                outputs.consequences,
                outputs.evidence,
                processed_records,
                progress,
                &mut previous_bytes,
                &mut previous_records,
                &mut previous_at,
            );
        }
    }
    if !records.is_empty() {
        parse_and_write_structured_chunk(
            &parser_pool,
            &mut records,
            &mut consequence_writer,
            &mut evidence_writer,
            &mut consequence_batch,
            &mut evidence_batch,
            &mut catalog,
            &mut counts,
            source_aliases,
        )?;
    }
    if canonical_records
        .as_mut()
        .map(CanonicalVcfRecords::next)
        .transpose()?
        .flatten()
        .is_some()
    {
        return Err("canonical VCF contains more records than the structured output".into());
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    if counts.consequences == 0 {
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
    let output_bytes = fs::metadata(outputs.consequences)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        + fs::metadata(outputs.evidence)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    progress(processed_records, true, output_bytes, 0.0, 0.0);

    write_structured_catalog(outputs.catalog, &catalog)?;
    let source_value_counts = source_value_counts(&catalog);
    Ok(StructuredSummary {
        records: counts.records,
        excluded_auxiliary_records,
        consequences: counts.consequences,
        evidence: counts.evidence,
        fields: catalog.len(),
        sources: catalog_sources(&catalog),
        source_value_counts,
    })
}

fn report_structured_progress(
    consequences_parquet: &Path,
    evidence_parquet: &Path,
    processed_records: u64,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
    previous_bytes: &mut u64,
    previous_records: &mut u64,
    previous_at: &mut Instant,
) {
    let bytes = [consequences_parquet, evidence_parquet]
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    let now = Instant::now();
    let elapsed = now.duration_since(*previous_at).as_secs_f64();
    let bytes_per_second = if elapsed > 0.0 {
        bytes.saturating_sub(*previous_bytes) as f64 / elapsed
    } else {
        0.0
    };
    let records_per_second = if elapsed > 0.0 {
        processed_records.saturating_sub(*previous_records) as f64 / elapsed
    } else {
        0.0
    };
    progress(
        processed_records,
        true,
        bytes,
        bytes_per_second,
        records_per_second,
    );
    *previous_bytes = bytes;
    *previous_records = processed_records;
    *previous_at = now;
}

fn write_structured_catalog(
    catalog_json: &Path,
    catalog: &BTreeMap<(String, String, String), CatalogEntry>,
) -> Result<(), String> {
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
            if let Some(contract) = categorical_contract_for_field(source_id, field_path)? {
                field["categorical"] = json!({
                    "matchMode": contract.match_mode,
                    "observedValues": entry.observed_categories.values().collect::<Vec<_>>(),
                    "observedValuesComplete": entry.observed_categories_complete,
                });
            }
            if let Some(biological_scope) = (scope == "selected" || source_id == "dbnsfp")
                .then(|| crate::evidence_resolution::record_field_scope(source_id, field_path))
                .flatten()
            {
                let selected = scope == "selected";
                field["biologicalScope"] = Value::String(biological_scope.into());
                field["physicalScope"] =
                    Value::String(if selected { "selected" } else { scope }.into());
                field["storageEncoding"] = Value::String("scalar".into());
                field["rawStorageEncoding"] = Value::String("recordList".into());
                field["resolutionPolicy"] =
                    Value::String(if selected { "materializedSelected" } else { "direct" }.into());
                if selected {
                    field["selectionOrigin"] = Value::String("report".into());
                }
            } else if let Some(group) = crate::evidence_resolution::bundled_alignment_group(
                scope,
                source_id,
                field_path,
            ) {
                field["alignmentGroup"] = Value::String(group);
                field["biologicalScope"] = Value::String("transcript".into());
                field["physicalScope"] = Value::String("selected".into());
                field["storageEncoding"] = Value::String("scalar".into());
                field["rawStorageEncoding"] =
                    Value::String("parallelTranscriptVector".into());
                field["resolutionPolicy"] = Value::String("materializedSelected".into());
                field["selectionOrigin"] = Value::String("report".into());
            } else {
                let biological_scope = annocat_core::source_catalog::source(source_id)
                    .map_or(scope.as_str(), |source| source.evidence_scope.as_str());
                let resolution_policy = match scope.as_str() {
                    "allele" | "variant" => "direct",
                    value if value.starts_with("unresolved_") => "unresolved",
                    _ => "materializedSelected",
                };
                field["biologicalScope"] = Value::String(biological_scope.into());
                field["physicalScope"] = Value::String(
                    if resolution_policy == "materializedSelected" {
                        "selected"
                    } else {
                        scope
                    }
                    .into(),
                );
                field["storageEncoding"] = Value::String("scalar".into());
                field["resolutionPolicy"] = Value::String(resolution_policy.into());
                if resolution_policy == "materializedSelected" {
                    field["selectionOrigin"] = Value::String("report".into());
                }
            }
            Ok(field)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let alignment_groups = crate::evidence_resolution::catalog_alignment_groups(&fields);
    let record_resolution_contracts = crate::evidence_resolution::record_resolution_contracts();
    fs::write(
        catalog_json,
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": SCHEMA_VERSION,
            "fields": fields,
            "alignmentGroups": alignment_groups,
            "recordResolutionContracts": record_resolution_contracts
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write field catalog: {error}"))
}

fn source_value_counts(
    catalog: &BTreeMap<(String, String, String), CatalogEntry>,
) -> BTreeMap<String, u64> {
    catalog.iter().fold(
        BTreeMap::<String, u64>::new(),
        |mut counts, ((_, source, _), entry)| {
            *counts.entry(source.clone()).or_default() += entry.occurrences;
            counts
        },
    )
}

fn catalog_sources(catalog: &BTreeMap<(String, String, String), CatalogEntry>) -> Vec<String> {
    catalog
        .keys()
        .map(|(_, source, _)| source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
pub fn convert_vcf(
    vcf: &Path,
    parquet: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    convert_vcf_mode(vcf, parquet, true, None, cancelled, &mut progress)
}

pub fn convert_vcf_with_reference(
    vcf: &Path,
    parquet: &Path,
    fasta: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    convert_vcf_mode(vcf, parquet, true, Some(fasta), cancelled, &mut progress)
}

pub fn convert_input_vcf(
    vcf: &Path,
    parquet: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    convert_vcf_mode(vcf, parquet, false, None, cancelled, &mut progress)
}

fn convert_vcf_mode(
    vcf: &Path,
    parquet: &Path,
    require_csq: bool,
    fasta: Option<&Path>,
    cancelled: impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    let _ = fs::remove_file(parquet);
    let result = convert_vcf_inner(vcf, parquet, require_csq, fasta, &cancelled, progress);
    if result.is_err() {
        let _ = fs::remove_file(parquet);
    }
    result
}

fn convert_vcf_inner(
    vcf: &Path,
    parquet: &Path,
    require_csq: bool,
    fasta: Option<&Path>,
    cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(u64, bool, u64, f64, f64),
) -> Result<CanonicalSummary, String> {
    let schema = VariantBatch::default().into_record_batch()?.schema();
    let mut writer = parquet_writer(parquet, schema)?;
    let mut reference_source = fasta
        .map(IndexedReference::open)
        .transpose()
        .map_err(|error| format!("cannot initialize GRCh38 allele normalization: {error}"))?;
    let parser_workers = std::thread::available_parallelism()
        .map(|workers| workers.get().min(4))
        .unwrap_or(1);
    let parser_pool = ThreadPoolBuilder::new()
        .num_threads(parser_workers)
        .thread_name(|index| format!("annocat-result-parser-{index}"))
        .build()
        .map_err(|error| format!("cannot initialize result parser workers: {error}"))?;

    let input_content_digest = (!require_csq).then(|| Rc::new(RefCell::new(Sha256::new())));
    let source = super::csq::open(vcf)?;
    let reader: Box<dyn BufRead> = if let Some(digest) = input_content_digest.as_ref() {
        Box::new(BufReader::new(ContentHashReader {
            inner: source,
            digest: Rc::clone(digest),
        }))
    } else {
        source
    };
    let mut csq_fields: Option<Vec<String>> = None;
    let mut sample_names = Vec::new();
    let mut sample_names_json = "[]".to_owned();
    let mut record_number = 0_i64;
    let mut processed_records = 0_u64;
    let mut excluded_auxiliary_records = 0_u64;
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
            processed_records += 1;
            let Some(canonical_alleles) =
                canonical_alleles_for_vcf_line(line_index + 1, &line, reference_source.as_mut())?
            else {
                excluded_auxiliary_records += 1;
                continue;
            };
            record_number += 1;
            records.push(VariantRecord {
                line_number: line_index + 1,
                record_number,
                line,
                canonical_alleles,
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
                    processed_records.saturating_sub(previous_records) as f64 / elapsed
                } else {
                    0.0
                };
                progress(
                    processed_records,
                    true,
                    bytes,
                    bytes_per_second,
                    records_per_second,
                );
                previous_bytes = bytes;
                previous_records = processed_records;
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
        return Err("the AnnoCAT result contains no allele rows".into());
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    writer
        .close()
        .map_err(|error| format!("cannot finish the AnnoCAT result: {error}"))?;
    progress(
        processed_records,
        true,
        fs::metadata(parquet).map(|value| value.len()).unwrap_or(0),
        0.0,
        0.0,
    );
    validate(parquet, rows)?;
    let input_content_sha256 = input_content_digest.map(|digest| {
        let digest = digest.borrow().clone();
        format!("{:x}", digest.finalize())
    });
    Ok(CanonicalSummary {
        rows,
        records: record_number as u64,
        excluded_auxiliary_records,
        samples: sample_names,
        input_content_sha256,
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
        .map_err(|error| format!("cannot validate the AnnoCAT result: {error}"))?;
    if rows as u64 != expected_rows {
        return Err(format!(
            "the AnnoCAT result has {rows} rows; expected {expected_rows}"
        ));
    }
    if minimum_schema != maximum_schema || !(1..=SCHEMA_VERSION).contains(&minimum_schema) {
        return Err("the AnnoCAT result contains an unsupported schema version".into());
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
    let current_selection_contract = report_uses_current_selection_contract(variants)?;
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
        return Err("result consequence table contains no rows".into());
    }
    if consequence_rows > 0
        && (consequence_min != consequence_max
            || !consequence_min.is_some_and(|version| (1..=SCHEMA_VERSION).contains(&version)))
    {
        return Err("result consequence table has an invalid schema version or no rows".into());
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
        .map_err(|error| format!("result consequence schema is incompatible: {error}"))?;
    let selected_column = connection
        .prepare("SELECT selected FROM read_parquet(?) LIMIT 0")
        .and_then(|mut statement| statement.exists(params![consequence_path.as_ref()]))
        .is_ok();
    if selected_column && consequence_rows > 0 {
        let invalid_selected_counts: i64 = connection
            .query_row(
                "SELECT count(*) FROM (
                   SELECT allele_id
                   FROM read_parquet(?)
                   GROUP BY allele_id
                   HAVING count(*) FILTER (WHERE selected)<>1
                 )",
                params![consequence_path.as_ref()],
                |row| row.get(0),
            )
            .map_err(|error| format!("cannot validate selected consequences: {error}"))?;
        let mismatched_selected: i64 = if current_selection_contract {
            connection
                .query_row(
                    "SELECT count(*)
                 FROM read_parquet(?) v
                 JOIN read_parquet(?) c USING (allele_id)
                 WHERE c.selected AND (
                   CASE
                     WHEN upper(trim(coalesce(v.transcript_id, ''))) IN
                       ('', '.', '-', 'NA', 'N/A', 'NONE', 'NULL') THEN ''
                     ELSE trim(v.transcript_id)
                   END
                     <> CASE
                       WHEN upper(trim(coalesce(c.transcript_id, c.feature_id, ''))) IN
                         ('', '.', '-', 'NA', 'N/A', 'NONE', 'NULL') THEN ''
                       ELSE trim(coalesce(c.transcript_id, c.feature_id))
                     END
                   OR CASE
                     WHEN upper(trim(coalesce(v.gene_id, ''))) IN
                       ('', '.', '-', 'NA', 'N/A', 'NONE', 'NULL') THEN ''
                     ELSE trim(v.gene_id)
                   END
                     <> CASE
                       WHEN upper(trim(coalesce(c.gene_id, ''))) IN
                         ('', '.', '-', 'NA', 'N/A', 'NONE', 'NULL') THEN ''
                       ELSE trim(c.gene_id)
                     END
                   OR split_part(trim(coalesce(v.consequence, '')), '&', 1)
                     <> trim(coalesce(c.primary_consequence, ''))
                 )",
                    params![
                        variants.to_string_lossy().as_ref(),
                        consequence_path.as_ref()
                    ],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    format!("cannot validate VCF and structured consequence agreement: {error}")
                })?
        } else {
            0
        };
        if invalid_selected_counts != 0 {
            return Err(format!(
                "structured consequences select an invalid number of representatives for \
                 {invalid_selected_counts} alleles"
            ));
        }
        if mismatched_selected != 0 {
            return Err(format!(
                "variant and structured representative consequences disagree for \
                 {mismatched_selected} alleles"
            ));
        }
    }

    let (evidence_rows, evidence_min, evidence_max): (i64, Option<i32>, Option<i32>) = connection
        .query_row(
            "SELECT count(*), min(schema_version), max(schema_version) FROM read_parquet(?)",
            params![evidence_path.as_ref()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("cannot validate evidence table: {error}"))?;
    if evidence_rows > 0
        && (evidence_min != evidence_max
            || !evidence_min.is_some_and(|version| (1..=SCHEMA_VERSION).contains(&version)))
    {
        return Err("result evidence table has an invalid schema version".into());
    }
    connection
        .prepare(
            "SELECT allele_id, consequence_id, scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?) LIMIT 0",
        )
        .and_then(|mut statement| statement.exists(params![evidence_path.as_ref()]))
        .map_err(|error| format!("result evidence schema is incompatible: {error}"))?;

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
    if current_selection_contract && (orphan_consequences != 0 || orphan_evidence != 0) {
        return Err(
            "the AnnoCAT result contains consequence or evidence rows for unknown alleles".into(),
        );
    }
    let duplicate_selected: i64 = connection
        .query_row(
            "SELECT count(*) FROM (
               SELECT allele_id, source_id, field_path
               FROM read_parquet(?)
               WHERE scope='selected'
               GROUP BY allele_id, source_id, field_path
               HAVING count(*)>1
             )",
            params![evidence_path.as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot validate selected evidence uniqueness: {error}"))?;
    let orphan_selected_links: i64 = connection
        .query_row(
            "SELECT count(*) FROM read_parquet(?) e
             WHERE e.scope='selected' AND e.consequence_id IS NOT NULL
               AND NOT EXISTS (
                 SELECT 1 FROM read_parquet(?) c
                 WHERE c.allele_id=e.allele_id
                   AND c.consequence_id=e.consequence_id
               )",
            params![evidence_path.as_ref(), consequence_path.as_ref()],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot validate selected evidence linkage: {error}"))?;
    if duplicate_selected != 0 || orphan_selected_links != 0 {
        return Err(format!(
            "the AnnoCAT result contains {duplicate_selected} duplicate selected evidence fields \
             and {orphan_selected_links} unlinked selected evidence rows"
        ));
    }

    let metadata = fs::metadata(catalog)
        .map_err(|error| format!("result field catalog is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("result field catalog has an invalid size".into());
    }
    crate::evidence_resolution::validate_catalog(catalog)?;
    let catalog_value: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid result field catalog: {error}"))?;
    let catalog_schema = catalog_value["schemaVersion"].as_i64();
    if !catalog_schema.is_some_and(|version| (1..=i64::from(SCHEMA_VERSION)).contains(&version))
        || !catalog_value["fields"].is_array()
    {
        return Err("result field catalog has an unsupported schema".into());
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
    Json,
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
        "zygosity" => Some(("v.zygosity", FilterValueKind::Text)),
        "gene" => Some((
            "coalesce(v.gene_symbol, v.gene_id, v.transcript_id)",
            FilterValueKind::Text,
        )),
        "geneId" => Some(("v.gene_id", FilterValueKind::Text)),
        "transcriptId" => Some(("v.transcript_id", FilterValueKind::Text)),
        "consequence" => Some(("v.consequence", FilterValueKind::Text)),
        "impact" => Some(("v.impact", FilterValueKind::Text)),
        "canonical" => Some(("v.canonical", FilterValueKind::Boolean)),
        "maneSelect" => Some(("v.mane_select", FilterValueKind::Text)),
        _ => None,
    }
}

fn validate_closed_core_category(column: &str, values: &[String]) -> Result<(), String> {
    let allowed: &[&str] = match column {
        "impact" => &["HIGH", "MODERATE", "LOW", "MODIFIER"],
        "zygosity" => &[
            "Reference",
            "Other alternate",
            "Heterozygous",
            "Homozygous alternate",
            "Haploid alternate",
            "Mixed alternate",
            "Partially called",
            "Not called",
            "Invalid genotype",
            "Multiple sample calls",
        ],
        _ => return Ok(()),
    };
    if let Some(value) = values.iter().find(|value| {
        let key = categorical_value_key(value);
        !allowed
            .iter()
            .any(|allowed| categorical_value_key(allowed) == key)
    }) {
        return Err(format!(
            "unsupported {column} filter value: {value}; choose a listed value"
        ));
    }
    Ok(())
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

fn comma_numeric_filter_values(value: &str) -> Result<Vec<f64>, String> {
    comma_filter_values(value)?
        .into_iter()
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| "numeric filter list must contain only finite numbers".to_string())
        })
        .collect()
}

fn text_contains_list_sql(
    expression: &str,
    operator: &str,
    value: &str,
) -> Result<(String, Vec<SqlValue>), String> {
    let values = comma_filter_values(value)?;
    let conditions = std::iter::repeat_n(
        format!("contains(lower(coalesce(CAST({expression} AS VARCHAR), '')), ?)"),
        values.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    Ok((
        format!(
            "{}({conditions})",
            if operator == "not_in" { "NOT " } else { "" }
        ),
        values.into_iter().map(Into::into).collect(),
    ))
}

fn json_text_member_sql(
    expression: &str,
    operator: &str,
    value: &str,
) -> Result<(String, Vec<SqlValue>), String> {
    let negative = matches!(operator, "not_equals" | "not_in");
    let values = if matches!(operator, "equals" | "not_equals") {
        vec![
            bounded_page_text(value, "rule value", 32 * 1024)?
                .trim()
                .to_ascii_lowercase()
                .replace(['_', '-'], " "),
        ]
    } else {
        comma_filter_values(value)?
            .into_iter()
            .map(|value| value.replace(['_', '-'], " "))
            .collect()
    };
    let placeholders = std::iter::repeat_n("?", values.len())
        .collect::<Vec<_>>()
        .join(",");
    Ok((
        format!(
            "{}EXISTS (
                SELECT 1
                FROM unnest(json_extract_string(coalesce({expression}, '[]'), '$[*]')) AS json_items(value)
                WHERE replace(replace(lower(trim(json_items.value)), '_', ' '), '-', ' ')
                    IN ({placeholders})
            )",
            if negative { "NOT " } else { "" }
        ),
        values.into_iter().map(Into::into).collect(),
    ))
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
        (FilterValueKind::Text, operator @ ("in" | "not_in")) => {
            let values = comma_filter_values(value)?;
            let placeholders = std::iter::repeat_n("?", values.len())
                .collect::<Vec<_>>()
                .join(",");
            Ok((
                format!(
                    "lower(coalesce(CAST({expression} AS VARCHAR), '')) {}IN ({placeholders})",
                    if operator == "not_in" { "NOT " } else { "" }
                ),
                values.into_iter().map(Into::into).collect(),
            ))
        }
        (FilterValueKind::Json, operator @ ("equals" | "not_equals" | "in" | "not_in")) => {
            json_text_member_sql(expression, operator, value)
        }
        (FilterValueKind::Json, operator @ ("contains" | "not_contains")) => Ok((
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
            FilterValueKind::Text | FilterValueKind::Json | FilterValueKind::Boolean,
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
        (FilterValueKind::Number, operator @ ("in" | "not_in")) => {
            let values = comma_numeric_filter_values(value)?;
            let placeholders = std::iter::repeat_n("?", values.len())
                .collect::<Vec<_>>()
                .join(",");
            Ok((
                format!(
                    "CAST({expression} AS DOUBLE) {}IN ({placeholders})",
                    if operator == "not_in" { "NOT " } else { "" }
                ),
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

struct CategoricalSelection {
    values: Vec<String>,
    include_missing: bool,
}

fn categorical_selection(
    values: &Option<Vec<String>>,
    include_missing: Option<bool>,
) -> Result<Option<CategoricalSelection>, String> {
    if values.is_none() && include_missing.is_none() {
        return Ok(None);
    }
    let mut selected = BTreeMap::new();
    for value in values.as_deref().unwrap_or_default() {
        let value = bounded_page_text(
            value,
            "categorical filter value",
            MAX_CATEGORICAL_VALUE_BYTES,
        )?
        .trim();
        if value.is_empty() {
            return Err("categorical filter values cannot be empty".into());
        }
        selected
            .entry(categorical_value_key(value))
            .or_insert_with(|| value.to_owned());
    }
    if selected.len() > MAX_CATEGORICAL_VALUES {
        return Err(format!(
            "at most {MAX_CATEGORICAL_VALUES} categorical values can be selected"
        ));
    }
    let include_missing = include_missing.unwrap_or(false);
    if selected.is_empty() && !include_missing {
        return Err("select at least one categorical value or include missing values".into());
    }
    Ok(Some(CategoricalSelection {
        values: selected.into_values().collect(),
        include_missing,
    }))
}

fn normalized_categorical_match_sql(
    expression: &str,
    kind: FilterValueKind,
    values: &[String],
) -> (String, Vec<SqlValue>) {
    if values.is_empty() {
        return ("FALSE".into(), Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", values.len())
        .collect::<Vec<_>>()
        .join(",");
    let normalize =
        |value: &str| format!("replace(replace(lower(trim({value})), '_', ' '), '-', ' ')");
    let condition = if kind == FilterValueKind::Json {
        format!(
            "EXISTS (
                SELECT 1
                FROM unnest(json_extract_string(coalesce({expression}, '[]'), '$[*]')) AS category_items(value)
                WHERE {} IN ({placeholders})
            )",
            normalize("category_items.value")
        )
    } else {
        format!(
            "{} IN ({placeholders})",
            normalize(&format!("CAST({expression} AS VARCHAR)"))
        )
    };
    (
        condition,
        values
            .iter()
            .map(|value| categorical_value_key(value).into())
            .collect(),
    )
}

fn delimited_categorical_match_sql(
    expression: &str,
    delimiter: char,
    values: &[String],
) -> (String, Vec<SqlValue>) {
    if values.is_empty() {
        return ("FALSE".into(), Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", values.len())
        .collect::<Vec<_>>()
        .join(",");
    (
        format!(
            "EXISTS (
                SELECT 1
                FROM unnest(string_split(coalesce(CAST({expression} AS VARCHAR), ''), '{delimiter}')) AS category_items(value)
                WHERE replace(replace(lower(trim(category_items.value)), '_', ' '), '-', ' ')
                    IN ({placeholders})
            )"
        ),
        values
            .iter()
            .map(|value| categorical_value_key(value).into())
            .collect(),
    )
}

fn categorical_present_sql(expression: &str, kind: FilterValueKind) -> String {
    if kind == FilterValueKind::Json {
        format!("json_array_length(coalesce({expression}, '[]')) > 0")
    } else {
        format!(
            "nullif(trim(CAST({expression} AS VARCHAR)), '') IS NOT NULL
             AND trim(CAST({expression} AS VARCHAR)) <> '.'"
        )
    }
}

fn categorical_condition_sql(
    present: &str,
    matched: &str,
    operator: &str,
    include_missing: bool,
) -> Result<String, String> {
    match (operator, include_missing) {
        ("in", false) => Ok(format!("({present}) AND ({matched})")),
        ("in", true) => Ok(format!("NOT ({present}) OR ({matched})")),
        ("not_in", false) => Ok(format!("({present}) AND NOT ({matched})")),
        ("not_in", true) => Ok(format!("NOT ({matched})")),
        _ => Err("categorical filters use 'is any of' or 'is none of'".into()),
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
        let operator = rule.operator.trim();
        let categorical = categorical_selection(&rule.values, rule.include_missing)?;
        let (condition, values) = if let Some(selection) = categorical {
            validate_closed_core_category(rule.column.trim(), &selection.values)?;
            let delimiter = match rule.column.trim() {
                "consequence" => Some('&'),
                "filter" => Some(';'),
                "impact" | "zygosity" => None,
                _ => return Err(format!("{} is not a categorical filter", rule.column)),
            };
            let (matched, values) = delimiter.map_or_else(
                || normalized_categorical_match_sql(expression, kind, &selection.values),
                |delimiter| {
                    delimited_categorical_match_sql(expression, delimiter, &selection.values)
                },
            );
            let present = categorical_present_sql(expression, kind);
            (
                categorical_condition_sql(&present, &matched, operator, selection.include_missing)?,
                values,
            )
        } else if rule.column.trim() == "consequence" && matches!(operator, "in" | "not_in") {
            text_contains_list_sql(expression, operator, &rule.value)?
        } else {
            comparison_sql(expression, kind, operator, &rule.value)?
        };
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
    let placeholders = std::iter::repeat_n("?", parameters.len())
        .collect::<Vec<_>>()
        .join(", ");
    Ok((
        format!(" AND v.allele_id NOT IN ({placeholders})"),
        parameters,
    ))
}

const CORE_PAGE_WHERE_SQL: &str = "v.alternate NOT IN ('.', '', '<NON_REF>', '<*>')
         AND (? = '' OR lower(v.chromosome) = lower(?))
         AND (CAST(? AS BIGINT) IS NULL OR v.position >= CAST(? AS BIGINT))
         AND (CAST(? AS BIGINT) IS NULL OR v.position <= CAST(? AS BIGINT))
         AND (? = '' OR lower(v.reference) = lower(?))
         AND (? = '' OR lower(v.alternate) = lower(?))
         AND (? = '' OR contains(lower(coalesce(v.variant_id, '')), lower(?)))
         AND (? = '' OR contains(lower(concat_ws(' ', coalesce(v.gene_symbol, ''),
             coalesce(v.gene_id, ''))), lower(?)))
         AND (? = '' OR contains(lower(coalesce(v.transcript_id, '')), lower(?)))
         AND (? = '' OR contains(lower(coalesce(v.consequence, '')), lower(?)))
         AND (? = '' OR upper(coalesce(v.impact, '')) = upper(?))
         AND (CAST(? AS DOUBLE) IS NULL OR v.quality >= CAST(? AS DOUBLE))
         AND (CAST(? AS DOUBLE) IS NULL OR v.quality <= CAST(? AS DOUBLE))
         AND (? = '' OR lower(v.filter) = lower(?))
         AND (CAST(? AS BOOLEAN) IS NULL OR v.canonical = CAST(? AS BOOLEAN))";

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
    prepare_sample_call_projection(parquet, request)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    register_report_variants(&connection, parquet)?;
    let query = PageQuery {
        variants: parquet,
        evidence,
        evidence_files: None,
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
    let evidence_files = query.evidence_files;
    let catalog = query.catalog;
    let offset = query.offset;
    let limit = query.limit;
    let request = query.request;
    let candidate_ids = query.candidate_ids;
    let limit = limit.clamp(1, 500);
    let core_filters = validated_core_page_filters(request)?;
    let match_cache_key = matched_row_cache_key(query, &core_filters)?;
    let mut cached_rows = match_cache_key.as_deref().and_then(cached_result_rows);
    if let Some(rows) = &cached_rows {
        install_matched_rows(connection, rows)?;
    }
    let (
        core_rule_sql,
        core_rule_params,
        evidence_rule_sql,
        evidence_rule_params,
        excluded_sql,
        excluded_params,
    ) = if cached_rows.is_some() {
        (
            String::new(),
            Vec::new(),
            String::new(),
            Vec::new(),
            String::new(),
            Vec::new(),
        )
    } else {
        let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
        let (evidence_rule_sql, evidence_rule_params) =
            evidence_filter_rules_sql(evidence, evidence_files, catalog, request)?;
        let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
        let (search_sql, mut search_params) = displayed_field_search_sql(
            connection,
            evidence,
            evidence_files,
            catalog,
            request,
            &core_filters.search,
        )?;
        search_params.extend(excluded_params);
        (
            core_rule_sql,
            core_rule_params,
            evidence_rule_sql,
            evidence_rule_params,
            format!("{search_sql}{excluded_sql}"),
            search_params,
        )
    };
    let page_sorts = page_sort_specs(evidence, evidence_files, catalog, request)?;
    let primary_sort = page_sorts
        .first()
        .expect("every result page has an input-order sort");
    let sort_key = primary_sort.key.clone();
    let direction = primary_sort.direction.as_str();
    let can_use_parquet_input_order = report_uses_current_selection_contract(parquet)?;
    let candidate_sql = candidate_ids
        .map(|_| " AND v.allele_id IN (SELECT allele_id FROM candidate_alleles)")
        .unwrap_or_default();
    let filtered_where_sql = format!(
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
    if cached_rows.is_none()
        && request
            .known_total
            .is_some_and(|total| total <= MATCHED_ROW_CACHE_LIMIT as u64)
        && match_cache_key.is_some()
    {
        let rows = bounded_matched_rows(
            connection,
            path.as_ref(),
            request,
            &core_filters,
            &filtered_where_sql,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
        )?;
        if rows.len() <= MATCHED_ROW_CACHE_LIMIT as usize {
            let key = match_cache_key
                .as_ref()
                .expect("bounded filtered query has a cache key");
            remember_result_rows(key.clone(), rows);
            cached_rows = cached_result_rows(key);
            install_matched_rows(
                connection,
                cached_rows
                    .as_deref()
                    .expect("remembered filtered query is available"),
            )?;
        }
    }
    let where_sql = if cached_rows.is_some() {
        format!(
            "{CORE_PAGE_WHERE_SQL}{candidate_sql} AND EXISTS (
                SELECT 1 FROM matched_result_rows matched
                WHERE matched.record_number=v.record_number AND matched.alt_index=v.alt_index
            )"
        )
    } else {
        filtered_where_sql
    };
    let optimized_evidence_sort = (page_sorts.len() == 1)
        .then_some(primary_sort.evidence.as_ref())
        .flatten();
    if cached_rows.is_none()
        && request.known_total.is_none()
        && !request.exact_total
        && optimized_evidence_sort.is_none()
    {
        let order_sql = if page_sorts.len() == 1
            && sort_key == "input"
            && direction == "ASC"
            && can_use_parquet_input_order
        {
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
             FROM annocat_variants(?) v
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
        select_params.push((limit.saturating_add(1) as i64).into());
        select_params.push((offset as i64).into());
        let mut rows = query_result_rows(connection, &sql, &select_params)?;
        let has_more = rows.len() > limit as usize;
        if has_more {
            rows.truncate(limit as usize);
        }
        let total = (!has_more && (offset == 0 || !rows.is_empty()))
            .then(|| {
                i64::try_from(offset.saturating_add(rows.len() as u64))
                    .map_err(|_| "result total is too large")
            })
            .transpose()?;
        return Ok(ResultPage {
            schema_version: SCHEMA_VERSION,
            offset,
            limit,
            total,
            has_more,
            search: core_filters.search,
            sort: sort_key,
            direction: direction.to_ascii_lowercase(),
            rows,
        });
    }
    let total = if let Some(rows) = &cached_rows {
        i64::try_from(rows.len()).map_err(|_| "cached result total is too large")?
    } else if let Some(total) = request.known_total {
        i64::try_from(total).map_err(|_| "known result total is too large")?
    } else if request.exact_total && match_cache_key.is_some() {
        let rows = bounded_matched_rows(
            connection,
            path.as_ref(),
            request,
            &core_filters,
            &where_sql,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
        )?;
        if rows.len() <= MATCHED_ROW_CACHE_LIMIT as usize {
            let total = rows.len() as i64;
            remember_result_rows(
                match_cache_key
                    .clone()
                    .expect("exact filtered query has a cache key"),
                rows,
            );
            total
        } else {
            count_result_rows(
                connection,
                path.as_ref(),
                request,
                &core_filters,
                &where_sql,
                &core_rule_params,
                &evidence_rule_params,
                &excluded_params,
            )?
        }
    } else {
        count_result_rows(
            connection,
            path.as_ref(),
            request,
            &core_filters,
            &where_sql,
            &core_rule_params,
            &evidence_rule_params,
            &excluded_params,
        )?
    };
    if total == 0 {
        return Ok(ResultPage {
            schema_version: SCHEMA_VERSION,
            offset,
            limit,
            total: Some(total),
            has_more: false,
            search: core_filters.search,
            sort: sort_key,
            direction: direction.to_ascii_lowercase(),
            rows: Vec::new(),
        });
    }
    let field_first_sort = optimized_evidence_sort.is_some()
        && candidate_ids.is_none()
        && page_request_is_unfiltered(request, &core_filters);
    let rows = if cached_rows.is_some() && page_sorts.iter().all(|sort| sort.evidence.is_none()) {
        cached_core_sorted_page_rows(connection, path.as_ref(), &page_sorts, offset, limit)?
    } else if cached_rows.is_some() && optimized_evidence_sort.is_some() {
        cached_evidence_sorted_page_rows(
            connection,
            path.as_ref(),
            optimized_evidence_sort.expect("cached evidence sort requires a sort specification"),
            direction,
            offset,
            limit,
        )?
    } else if field_first_sort {
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
        let order_sql = if page_sorts.len() == 1
            && sort_key == "input"
            && direction == "ASC"
            && can_use_parquet_input_order
        {
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
             FROM annocat_variants(?) v
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
        total: Some(total),
        has_more: offset.saturating_add(rows.len() as u64) < total as u64,
        search: core_filters.search,
        sort: sort_key,
        direction: direction.to_ascii_lowercase(),
        rows,
    })
}

const RESULT_PAGE_COLUMNS: &str =
    "v.allele_id, v.chromosome, v.position, v.reference, v.alternate, v.variant_id,
     v.quality, v.filter, v.gene_symbol, v.gene_id, v.transcript_id, v.consequence,
     v.impact, v.canonical, v.mane_select, v.record_number, v.alt_index,
     v.alternate_count, v.format, v.samples_json";
const FILTERED_EVIDENCE_SORT_THRESHOLD: i64 = 100_000;

fn cached_sort_terms(page_sorts: &[PageSortSpec], prefix: &str) -> String {
    page_sorts
        .iter()
        .enumerate()
        .map(|(index, sort)| format!("{prefix}sort_{index} {} NULLS LAST", sort.direction))
        .chain([
            format!("{prefix}record_number ASC"),
            format!("{prefix}alt_index ASC"),
        ])
        .collect::<Vec<_>>()
        .join(", ")
}

fn cached_core_sorted_page_rows(
    connection: &Connection,
    path: &str,
    page_sorts: &[PageSortSpec],
    offset: u64,
    limit: u64,
) -> Result<Vec<Value>, String> {
    let sort_columns = page_sorts
        .iter()
        .enumerate()
        .map(|(index, sort)| format!("{} AS sort_{index}", sort.expression))
        .collect::<Vec<_>>()
        .join(", ");
    let selected_order = cached_sort_terms(page_sorts, "");
    let output_order = cached_sort_terms(page_sorts, "selected.");
    let sql = format!(
        "WITH selected AS (
           SELECT record_number, alt_index, {sort_columns}
           FROM matched_result_rows
           ORDER BY {selected_order}
           LIMIT ? OFFSET ?
         )
         SELECT {RESULT_PAGE_COLUMNS}
         FROM annocat_variants(?) v
         JOIN selected USING(record_number, alt_index)
         ORDER BY {output_order}"
    );
    query_result_rows(
        connection,
        &sql,
        &[
            (limit as i64).into(),
            (offset as i64).into(),
            path.to_owned().into(),
        ],
    )
}

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

fn matched_row_cache_key(
    query: &PageQuery<'_>,
    filters: &CorePageFilters,
) -> Result<Option<String>, String> {
    if query.candidate_ids.is_some() || page_request_is_unfiltered(query.request, filters) {
        return Ok(None);
    }
    let mut request = query.request.clone();
    request.sort.clear();
    request.direction.clear();
    request.sort_evidence = None;
    request.sorts.clear();
    request.known_total = None;
    request.exact_total = false;
    request.query_session.clear();
    request.request_generation = 0;
    request.evidence_columns.sort_unstable();

    let mut digest = Sha256::new();
    digest.update(
        serde_json::to_vec(&request)
            .map_err(|error| format!("cannot identify result query: {error}"))?,
    );
    let mut paths = vec![query.variants.to_path_buf()];
    if let Some(files) = query.evidence_files {
        paths.extend(files.iter().cloned());
    } else if let Some(evidence) = query.evidence {
        paths.extend(visible_evidence_files(evidence)?);
    }
    paths.extend(query.catalog.map(Path::to_path_buf));
    paths.sort();
    paths.dedup();
    for path in paths {
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "cannot inspect result query input {}: {error}",
                path.display()
            )
        })?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        digest.update(path.to_string_lossy().as_bytes());
        digest.update(metadata.len().to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    Ok(Some(format!("{:x}", digest.finalize())))
}

fn cached_result_rows(key: &str) -> Option<Arc<Vec<CachedResultRow>>> {
    let mut cache = MATCHED_ROW_CACHE
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = cache.iter().position(|(candidate, _)| candidate == key)?;
    let entry = cache.remove(index)?;
    let rows = Arc::clone(&entry.1);
    cache.push_front(entry);
    Some(rows)
}

fn remember_result_rows(key: String, rows: Vec<CachedResultRow>) {
    let mut cache = MATCHED_ROW_CACHE
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(index) = cache.iter().position(|(candidate, _)| candidate == &key) {
        cache.remove(index);
    }
    cache.push_front((key, Arc::new(rows)));
    cache.truncate(MATCHED_ROW_CACHE_ENTRIES);
}

fn install_matched_rows(connection: &Connection, rows: &[CachedResultRow]) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TEMP TABLE matched_result_rows(
                record_number BIGINT NOT NULL,
                alt_index INTEGER NOT NULL,
                allele_id VARCHAR NOT NULL,
                chromosome VARCHAR NOT NULL,
                position BIGINT NOT NULL,
                reference VARCHAR NOT NULL,
                alternate VARCHAR NOT NULL,
                variant_id VARCHAR,
                quality DOUBLE,
                filter VARCHAR NOT NULL,
                gene_symbol VARCHAR,
                gene_id VARCHAR,
                transcript_id VARCHAR,
                consequence VARCHAR,
                impact VARCHAR,
                canonical BOOLEAN NOT NULL,
                mane_select VARCHAR,
                alternate_count INTEGER NOT NULL,
                format VARCHAR,
                samples_json VARCHAR NOT NULL,
                zygosity VARCHAR,
                zygosity_sort INTEGER,
                PRIMARY KEY(record_number, alt_index)
            )",
        )
        .map_err(|error| format!("cannot create cached result query: {error}"))?;
    let mut appender = connection
        .appender("matched_result_rows")
        .map_err(|error| format!("cannot prepare cached result query: {error}"))?;
    for row in rows {
        let values = vec![
            SqlValue::BigInt(row.record_number),
            SqlValue::Int(row.alt_index),
            row.allele_id.clone().into(),
            row.chromosome.clone().into(),
            SqlValue::BigInt(row.position),
            row.reference.clone().into(),
            row.alternate.clone().into(),
            row.variant_id.clone().map_or(SqlValue::Null, Into::into),
            row.quality.map_or(SqlValue::Null, SqlValue::Double),
            row.filter.clone().into(),
            row.gene_symbol.clone().map_or(SqlValue::Null, Into::into),
            row.gene_id.clone().map_or(SqlValue::Null, Into::into),
            row.transcript_id.clone().map_or(SqlValue::Null, Into::into),
            row.consequence.clone().map_or(SqlValue::Null, Into::into),
            row.impact.clone().map_or(SqlValue::Null, Into::into),
            SqlValue::Boolean(row.canonical),
            row.mane_select.clone().map_or(SqlValue::Null, Into::into),
            SqlValue::Int(row.alternate_count),
            row.format.clone().map_or(SqlValue::Null, Into::into),
            row.samples_json.clone().into(),
            row.zygosity.clone().map_or(SqlValue::Null, Into::into),
            row.zygosity_sort.map_or(SqlValue::Null, SqlValue::Int),
        ];
        appender
            .append_row(appender_params_from_iter(values))
            .map_err(|error| format!("cannot populate cached result query: {error}"))?;
    }
    appender
        .flush()
        .map_err(|error| format!("cannot finish cached result query: {error}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn bounded_matched_rows(
    connection: &Connection,
    path: &str,
    request: &PageRequest,
    filters: &CorePageFilters,
    where_sql: &str,
    core_rule_params: &[SqlValue],
    evidence_rule_params: &[SqlValue],
    excluded_params: &[SqlValue],
) -> Result<Vec<CachedResultRow>, String> {
    let sql = format!(
        "SELECT v.record_number, v.alt_index, v.allele_id, v.chromosome, v.position,
                v.reference, v.alternate, v.variant_id, v.quality, v.filter,
                v.gene_symbol, v.gene_id, v.transcript_id, v.consequence, v.impact,
                v.canonical, v.mane_select, v.alternate_count, v.format, v.samples_json
                , v.zygosity, v.zygosity_sort
         FROM annocat_variants(?) v
         WHERE {where_sql}
         LIMIT {}",
        MATCHED_ROW_CACHE_LIMIT + 1
    );
    let parameters = filtered_page_params(
        path,
        request,
        filters,
        core_rule_params,
        evidence_rule_params,
        excluded_params,
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("cannot prepare bounded result query: {error}"))?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok(CachedResultRow {
                record_number: row.get(0)?,
                alt_index: row.get(1)?,
                allele_id: row.get(2)?,
                chromosome: row.get(3)?,
                position: row.get(4)?,
                reference: row.get(5)?,
                alternate: row.get(6)?,
                variant_id: row.get(7)?,
                quality: row.get(8)?,
                filter: row.get(9)?,
                gene_symbol: row.get(10)?,
                gene_id: row.get(11)?,
                transcript_id: row.get(12)?,
                consequence: row.get(13)?,
                impact: row.get(14)?,
                canonical: row.get(15)?,
                mane_select: row.get(16)?,
                alternate_count: row.get(17)?,
                format: row.get(18)?,
                samples_json: row.get(19)?,
                zygosity: row.get(20)?,
                zygosity_sort: row.get(21)?,
            })
        })
        .map_err(|error| format!("cannot read bounded result query: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot collect bounded result query: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn count_result_rows(
    connection: &Connection,
    path: &str,
    request: &PageRequest,
    filters: &CorePageFilters,
    where_sql: &str,
    core_rule_params: &[SqlValue],
    evidence_rule_params: &[SqlValue],
    excluded_params: &[SqlValue],
) -> Result<i64, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT count(*) FROM annocat_variants(?) v WHERE {where_sql}"
        ))
        .map_err(|error| format!("cannot prepare result count: {error}"))?;
    let parameters = filtered_page_params(
        path,
        request,
        filters,
        core_rule_params,
        evidence_rule_params,
        excluded_params,
    );
    statement
        .query_row(params_from_iter(parameters.iter()), |row| row.get(0))
        .map_err(|error| format!("cannot count result page: {error}"))
}

fn query_result_rows(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
) -> Result<Vec<Value>, String> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("cannot prepare result page: {error}"))?;
    let mapped = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok(json!({
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
                "zygosity": table_zygosity(
                    row.get::<_, Option<String>>(18)?.as_deref(),
                    &row.get::<_, String>(19)?,
                    row.get::<_, i32>(16)?,
                    row.get::<_, i32>(17)?,
                ),
            }))
        })
        .map_err(|error| format!("cannot read result page: {error}"))?;
    mapped
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[derive(Clone)]
struct SelectedEvidenceColumn {
    index: usize,
    scope: String,
    biological_scope: String,
    equivalent_scopes: Vec<String>,
    source_id: String,
    field_path: String,
    value_type: String,
    resolution: EvidenceResolutionStrategy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EvidenceResolutionStrategy {
    Allele,
    AlleleGeneDirect,
    DerivedMaximum,
    GeneDirect,
    MaterializedSelected,
    LegacyAlleleRecovery,
    SelectedConsequence,
    AlignedTranscriptVector,
    SourceSelected,
}

fn uses_resolution_sidecar(strategy: EvidenceResolutionStrategy) -> bool {
    matches!(
        strategy,
        EvidenceResolutionStrategy::AlignedTranscriptVector
            | EvidenceResolutionStrategy::DerivedMaximum
            | EvidenceResolutionStrategy::LegacyAlleleRecovery
            | EvidenceResolutionStrategy::SelectedConsequence
    )
}

fn resolution_kind_condition(strategy: EvidenceResolutionStrategy, alias: &str) -> String {
    let kinds = match strategy {
        EvidenceResolutionStrategy::AlignedTranscriptVector => {
            "('exact_transcript', 'stable_id_match', 'policy_selected')"
        }
        EvidenceResolutionStrategy::DerivedMaximum => "('derived_maximum')",
        EvidenceResolutionStrategy::LegacyAlleleRecovery => {
            "('direct_allele', 'legacy_allele_scope_recovered')"
        }
        EvidenceResolutionStrategy::SelectedConsequence => {
            "('exact_transcript', 'stable_id_match', 'exact_gene', 'policy_selected')"
        }
        _ => "('')",
    };
    format!("{alias}.resolution_kind IN {kinds}")
}

fn resolved_value_is_usable(strategy: EvidenceResolutionStrategy, kind: &str) -> bool {
    match strategy {
        EvidenceResolutionStrategy::AlignedTranscriptVector => {
            matches!(
                kind,
                "exact_transcript" | "stable_id_match" | "policy_selected"
            )
        }
        EvidenceResolutionStrategy::DerivedMaximum => kind == "derived_maximum",
        EvidenceResolutionStrategy::LegacyAlleleRecovery => {
            matches!(kind, "direct_allele" | "legacy_allele_scope_recovered")
        }
        EvidenceResolutionStrategy::SelectedConsequence => matches!(
            kind,
            "exact_transcript" | "stable_id_match" | "exact_gene" | "policy_selected"
        ),
        _ => false,
    }
}

struct EvidenceSortSpec {
    evidence: String,
    evidence_read: String,
    evidence_parameters: Vec<SqlValue>,
    field: SelectedEvidenceColumn,
    value_expression: &'static str,
}

struct PageSortSpec {
    key: String,
    direction: String,
    expression: String,
    parameters: Vec<SqlValue>,
    evidence: Option<EvidenceSortSpec>,
}

fn evidence_scope_condition(field: &SelectedEvidenceColumn, alias: &str) -> String {
    let column = if alias.is_empty() {
        "scope".to_owned()
    } else {
        format!("{alias}.scope")
    };
    let placeholders = std::iter::repeat_n("?", field.equivalent_scopes.len())
        .collect::<Vec<_>>()
        .join(",");
    format!("{column} IN ({placeholders})")
}

fn append_evidence_scope_parameters(
    parameters: &mut Vec<SqlValue>,
    field: &SelectedEvidenceColumn,
) {
    parameters.extend(field.equivalent_scopes.iter().cloned().map(Into::into));
}

fn evidence_field_condition(
    field: &SelectedEvidenceColumn,
    alias: &str,
    evidence: &Path,
) -> String {
    if is_query_projection(evidence) {
        return format!("{alias}.field_index = ?");
    }
    format!(
        "{} AND {alias}.source_id = ? AND {alias}.field_path = ?",
        evidence_scope_condition(field, alias)
    )
}

fn append_evidence_field_parameters(
    parameters: &mut Vec<SqlValue>,
    field: &SelectedEvidenceColumn,
    evidence: &Path,
) -> Result<(), String> {
    if is_query_projection(evidence) {
        parameters.push((field.index as i64).into());
        return Ok(());
    }
    append_evidence_scope_parameters(parameters, field);
    parameters.push(field.source_id.clone().into());
    parameters.push(field.field_path.clone().into());
    Ok(())
}

fn evidence_sort_parameters(sort: &EvidenceSortSpec) -> Vec<SqlValue> {
    let mut parameters = sort.evidence_parameters.clone();
    if sort.field.resolution == EvidenceResolutionStrategy::GeneDirect
        || (!is_query_projection(Path::new(&sort.evidence))
            && (sort.field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
                || uses_resolution_sidecar(sort.field.resolution)))
    {
        parameters.push(sort.field.source_id.clone().into());
        parameters.push(sort.field.field_path.clone().into());
    } else {
        append_evidence_field_parameters(&mut parameters, &sort.field, Path::new(&sort.evidence))
            .expect("evidence sort paths were validated when the sort was created");
    }
    parameters
}

fn evidence_sort_sql(sort: &EvidenceSortSpec, sql: String) -> String {
    sql.replace("read_parquet(?)", &sort.evidence_read)
}

fn evidence_sort_join(sort: &EvidenceSortSpec, variant: &str, evidence: &str) -> String {
    if sort.field.resolution == EvidenceResolutionStrategy::GeneDirect {
        format!("upper({variant}.gene_symbol)={evidence}.gene_symbol")
    } else if is_query_projection(Path::new(&sort.evidence)) {
        format!(
            "{variant}.record_number={evidence}.record_number AND \
             {variant}.alt_index={evidence}.alt_index"
        )
    } else {
        format!("{variant}.allele_id={evidence}.allele_id")
    }
}

fn evidence_sort_expression(sort: &EvidenceSortSpec) -> String {
    if sort.field.resolution == EvidenceResolutionStrategy::GeneDirect {
        if sort.field.field_path == "phenotypeRelevance" {
            return evidence_sort_sql(sort, "(SELECT coalesce(
                        max(CASE WHEN ev_sort.field_path='phenotypeRelevance'
                                 THEN coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE)) END),
                        max(CASE WHEN ev_sort.field_path='selectedConditionMatches'
                                      AND coalesce(ev_sort.integer_value, try_cast(ev_sort.string_value AS BIGINT), 0) > 0
                                 THEN 1.0 END))
                     FROM read_parquet(?) ev_sort
                     WHERE upper(ev_sort.gene_symbol)=upper(v.gene_symbol)
                       AND ev_sort.source_id=?
                       AND ev_sort.field_path IN (?, 'selectedConditionMatches'))"
                .into());
        }
        return evidence_sort_sql(
            sort,
            format!(
                "(SELECT {} FROM read_parquet(?) ev_sort
              WHERE upper(ev_sort.gene_symbol)=upper(v.gene_symbol)
                AND ev_sort.source_id=? AND ev_sort.field_path=?
              LIMIT 1)",
                sort.value_expression
            ),
        );
    }
    if sort.field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
        && !is_query_projection(Path::new(&sort.evidence))
    {
        return evidence_sort_sql(
            sort,
            format!(
                "(SELECT {} FROM read_parquet(?) ev_sort
              WHERE ev_sort.allele_id = v.allele_id
                AND ev_sort.source_id=? AND ev_sort.field_path=?
              LIMIT 1)",
                sort.value_expression
            ),
        );
    }
    let field_condition =
        evidence_field_condition(&sort.field, "ev_sort", Path::new(&sort.evidence));
    if is_query_projection(Path::new(&sort.evidence)) {
        return evidence_sort_sql(
            sort,
            format!(
                "(SELECT {} FROM read_parquet(?) ev_sort
              WHERE ev_sort.record_number = v.record_number
                AND ev_sort.alt_index = v.alt_index AND {}
              LIMIT 1)",
                sort.value_expression, field_condition
            ),
        );
    }
    if uses_resolution_sidecar(sort.field.resolution) {
        let resolution = resolution_kind_condition(sort.field.resolution, "ev_sort");
        return evidence_sort_sql(
            sort,
            format!(
                "(SELECT {} FROM read_parquet(?) ev_sort
              WHERE ev_sort.allele_id = v.allele_id
                AND ev_sort.source_id = ? AND ev_sort.field_path = ?
                AND {}
              LIMIT 1)",
                sort.value_expression, resolution
            ),
        );
    }
    evidence_sort_sql(
        sort,
        format!(
            "(SELECT {} FROM read_parquet(?) ev_sort
          WHERE ev_sort.allele_id = v.allele_id AND {}
          ORDER BY ev_sort.consequence_id NULLS FIRST LIMIT 1)",
            sort.value_expression, field_condition
        ),
    )
}

fn evidence_sort_cte(sort: &EvidenceSortSpec) -> String {
    if sort.field.resolution == EvidenceResolutionStrategy::GeneDirect {
        if sort.field.field_path == "phenotypeRelevance" {
            return evidence_sort_sql(sort, "WITH scored_evidence AS (
                       SELECT upper(gene_symbol) AS gene_symbol,
                              coalesce(
                                max(CASE WHEN ev_sort.field_path='phenotypeRelevance'
                                         THEN coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE)) END),
                                max(CASE WHEN ev_sort.field_path='selectedConditionMatches'
                                              AND coalesce(ev_sort.integer_value, try_cast(ev_sort.string_value AS BIGINT), 0) > 0
                                         THEN 1.0 END)) AS sort_value
                       FROM read_parquet(?) ev_sort
                       WHERE ev_sort.source_id=?
                         AND ev_sort.field_path IN (?, 'selectedConditionMatches')
                       GROUP BY upper(gene_symbol)
                       HAVING sort_value IS NOT NULL
                     )"
                .into());
        }
        return evidence_sort_sql(
            sort,
            format!(
                "WITH scored_evidence AS (
               SELECT upper(gene_symbol) AS gene_symbol, {} AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE ev_sort.source_id=? AND ev_sort.field_path=?
                 AND {} IS NOT NULL
             )",
                sort.value_expression, sort.value_expression
            ),
        );
    }
    if sort.field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
        && !is_query_projection(Path::new(&sort.evidence))
    {
        return evidence_sort_sql(
            sort,
            format!(
                "WITH scored_evidence AS (
               SELECT allele_id, first({}) AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE ev_sort.source_id=? AND ev_sort.field_path=?
               GROUP BY allele_id
               HAVING sort_value IS NOT NULL
             )",
                sort.value_expression
            ),
        );
    }
    let field_condition =
        evidence_field_condition(&sort.field, "ev_sort", Path::new(&sort.evidence));
    if is_query_projection(Path::new(&sort.evidence)) {
        return evidence_sort_sql(
            sort,
            format!(
                "WITH evidence_values AS (
               SELECT record_number, alt_index,
                      first({}) AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE {}
               GROUP BY record_number, alt_index
             ), scored_evidence AS (
               SELECT record_number, alt_index, sort_value
               FROM evidence_values WHERE sort_value IS NOT NULL
             )",
                sort.value_expression, field_condition
            ),
        );
    }
    if uses_resolution_sidecar(sort.field.resolution) {
        let resolution = resolution_kind_condition(sort.field.resolution, "ev_sort");
        return evidence_sort_sql(
            sort,
            format!(
                "WITH scored_evidence AS (
               SELECT allele_id, {} AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE ev_sort.source_id = ? AND ev_sort.field_path = ?
                 AND {}
                 AND {} IS NOT NULL
             )",
                sort.value_expression, resolution, sort.value_expression
            ),
        );
    }
    let selection = format!(
        "first({} ORDER BY consequence_id NULLS FIRST)",
        sort.value_expression
    );
    evidence_sort_sql(
        sort,
        format!(
            "WITH evidence_values AS (
           SELECT allele_id,
                  {selection} AS sort_value
           FROM read_parquet(?) ev_sort
           WHERE {}
           GROUP BY allele_id
         ), scored_evidence AS (
           SELECT allele_id, sort_value FROM evidence_values WHERE sort_value IS NOT NULL
         )",
            field_condition
        ),
    )
}

fn cached_evidence_sorted_page_rows(
    connection: &Connection,
    path: &str,
    sort: &EvidenceSortSpec,
    direction: &str,
    offset: u64,
    limit: u64,
) -> Result<Vec<Value>, String> {
    let cte = evidence_sort_cte(sort);
    let join = evidence_sort_join(sort, "matched", "ev_order");
    let sql = format!(
        "{cte}, selected AS (
           SELECT matched.record_number, matched.alt_index, ev_order.sort_value
           FROM matched_result_rows matched
           LEFT JOIN scored_evidence ev_order ON {join}
           ORDER BY ev_order.sort_value {direction} NULLS LAST,
                    matched.record_number ASC, matched.alt_index ASC
           LIMIT ? OFFSET ?
         )
         SELECT {RESULT_PAGE_COLUMNS}
         FROM annocat_variants(?) v
         JOIN selected USING(record_number, alt_index)
         ORDER BY selected.sort_value {direction} NULLS LAST,
                  selected.record_number ASC, selected.alt_index ASC"
    );
    let mut parameters = evidence_sort_parameters(sort);
    parameters.push((limit as i64).into());
    parameters.push((offset as i64).into());
    parameters.push(path.to_owned().into());
    query_result_rows(connection, &sql, &parameters)
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
         JOIN annocat_variants(?) v ON {}
         WHERE {where_sql}
         ORDER BY ev_order.sort_value {direction}, v.record_number ASC, v.alt_index ASC
         LIMIT ? OFFSET ?",
        evidence_sort_join(sort, "v", "ev_order")
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
             JOIN annocat_variants(?) v ON {}
             WHERE {where_sql}",
            evidence_sort_join(sort, "v", "ev_order")
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
         FROM annocat_variants(?) v
         WHERE {where_sql}
           AND NOT EXISTS (
             SELECT 1 FROM scored_evidence ev_order WHERE {}
           )
         ORDER BY v.record_number ASC, v.alt_index ASC
         LIMIT ? OFFSET ?",
        evidence_sort_join(sort, "v", "ev_order")
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
    if sort.field.resolution == EvidenceResolutionStrategy::GeneDirect {
        let sql = evidence_sort_sql(
            sort,
            format!(
                "WITH matched_variants AS MATERIALIZED (
               SELECT v.* FROM annocat_variants(?) v WHERE {where_sql}
             ), evidence_values AS (
               SELECT upper(ev_sort.gene_symbol) AS gene_symbol,
                      first({}) AS sort_value
               FROM read_parquet(?) ev_sort
               WHERE ev_sort.source_id=? AND ev_sort.field_path=?
               GROUP BY upper(ev_sort.gene_symbol)
             )
             SELECT {RESULT_PAGE_COLUMNS}
             FROM matched_variants v
             LEFT JOIN evidence_values ev_order
               ON ev_order.gene_symbol=upper(v.gene_symbol)
             ORDER BY ev_order.sort_value {direction} NULLS LAST,
                      v.record_number ASC, v.alt_index ASC
             LIMIT ? OFFSET ?",
                sort.value_expression
            ),
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
        return query_result_rows(connection, &sql, &parameters);
    }
    if is_query_projection(Path::new(&sort.evidence)) {
        let sql = evidence_sort_sql(
            sort,
            format!(
                "WITH matched_variants AS MATERIALIZED (
               SELECT v.* FROM annocat_variants(?) v WHERE {where_sql}
             ), evidence_values AS (
               SELECT ev_sort.record_number, ev_sort.alt_index,
                      first({}) AS sort_value
               FROM read_parquet(?) ev_sort
               JOIN matched_variants matched USING(record_number, alt_index)
               WHERE {}
               GROUP BY ev_sort.record_number, ev_sort.alt_index
             )
             SELECT {RESULT_PAGE_COLUMNS}
             FROM matched_variants v
             LEFT JOIN evidence_values ev_order USING(record_number, alt_index)
             ORDER BY ev_order.sort_value {direction} NULLS LAST,
                      v.record_number ASC, v.alt_index ASC
             LIMIT ? OFFSET ?",
                sort.value_expression,
                evidence_field_condition(&sort.field, "ev_sort", Path::new(&sort.evidence))
            ),
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
        return query_result_rows(connection, &sql, &parameters);
    }
    let (selection, evidence_where) = if sort.field.resolution
        == EvidenceResolutionStrategy::AlleleGeneDirect
        && !is_query_projection(Path::new(&sort.evidence))
    {
        (
            format!("first({})", sort.value_expression),
            "ev_sort.source_id = ? AND ev_sort.field_path = ?".into(),
        )
    } else if uses_resolution_sidecar(sort.field.resolution) {
        (
            format!("first({})", sort.value_expression),
            format!(
                "ev_sort.source_id = ? AND ev_sort.field_path = ? AND {}",
                resolution_kind_condition(sort.field.resolution, "ev_sort")
            ),
        )
    } else {
        (
            format!(
                "first({} ORDER BY ev_sort.consequence_id NULLS FIRST)",
                sort.value_expression
            ),
            evidence_field_condition(&sort.field, "ev_sort", Path::new(&sort.evidence)),
        )
    };
    let sql = evidence_sort_sql(
        sort,
        format!(
            "WITH matched_variants AS MATERIALIZED (
           SELECT v.* FROM annocat_variants(?) v WHERE {where_sql}
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
        ),
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

fn evidence_field_can_match_search(field: &SelectedEvidenceColumn, search: &str) -> bool {
    if evidence_field_is_numeric(field) {
        return search
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+' | b'e' | b'E'));
    }
    if field.value_type == "boolean" {
        let search = search.to_ascii_lowercase();
        return "true".contains(&search) || "false".contains(&search);
    }
    true
}

fn evidence_search_value_expression(field: &SelectedEvidenceColumn, alias: &str) -> String {
    match field.value_type.as_str() {
        "string" => format!("coalesce({alias}.string_value, '')"),
        "integer" => format!("coalesce(CAST({alias}.integer_value AS VARCHAR), '')"),
        "number" => format!("coalesce(CAST({alias}.number_value AS VARCHAR), '')"),
        "boolean" => format!("coalesce(CAST({alias}.boolean_value AS VARCHAR), '')"),
        "json" => format!("coalesce({alias}.json_value, '')"),
        _ => format!(
            "coalesce({alias}.string_value, CAST({alias}.integer_value AS VARCHAR), \
             CAST({alias}.number_value AS VARCHAR), CAST({alias}.boolean_value AS VARCHAR), \
             {alias}.json_value, '')"
        ),
    }
}

fn resolved_search_value_expression(field: &SelectedEvidenceColumn, alias: &str) -> String {
    match field.value_type.as_str() {
        "integer" | "number" => {
            format!("coalesce(CAST({alias}.resolved_number AS VARCHAR), '')")
        }
        "mixed" => format!(
            "coalesce({alias}.resolved_string, CAST({alias}.resolved_number AS VARCHAR), '')"
        ),
        _ => format!("coalesce({alias}.resolved_string, '')"),
    }
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

fn canonical_evidence_path(evidence: &Path) -> PathBuf {
    if is_composite_evidence(evidence)
        && let Some(run_directory) = evidence.parent().and_then(Path::parent)
    {
        let canonical = run_directory.join("evidence.parquet");
        if canonical.is_file() {
            return canonical;
        }
    }
    if is_query_projection(evidence)
        && let Some(run_directory) = evidence.parent()
    {
        let canonical = run_directory.join("evidence.parquet");
        if canonical.is_file() {
            return canonical;
        }
    }
    evidence.to_path_buf()
}

fn is_composite_evidence(evidence: &Path) -> bool {
    evidence.file_name().and_then(|name| name.to_str()) == Some("*.parquet")
        && evidence
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("query-evidence")
}

fn is_query_projection(evidence: &Path) -> bool {
    evidence
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(QUERY_PROJECTION_PREFIX) && name.ends_with(".parquet"))
}

fn request_uses_zygosity(request: &PageRequest) -> bool {
    request
        .filter_rules
        .iter()
        .any(|rule| rule.column.trim() == "zygosity")
        || request.sort.trim() == "zygosity"
        || request
            .sorts
            .iter()
            .any(|sort| sort.column.trim() == "zygosity")
}

fn parquet_row_count(path: &Path) -> Result<i64, String> {
    let file =
        File::open(path).map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    Ok(reader.metadata().file_metadata().num_rows())
}

fn sample_call_projection_path(variants: &Path) -> Result<PathBuf, String> {
    let metadata = fs::metadata(variants)
        .map_err(|error| format!("cannot inspect {}: {error}", variants.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    let root = variants
        .parent()
        .ok_or("result variant table has no directory")?;
    Ok(root.join(format!(
        "{SAMPLE_CALL_PROJECTION_PREFIX}{:x}-{modified:x}.parquet",
        metadata.len()
    )))
}

fn sample_call_projection_is_valid(path: &Path, variants: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(reader) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    let schema = reader.schema();
    ["record_number", "alt_index", "zygosity", "zygosity_sort"]
        .iter()
        .all(|name| schema.field_with_name(name).is_ok())
        && parquet_row_count(variants)
            .is_ok_and(|rows| rows == reader.metadata().file_metadata().num_rows())
}

fn remove_stale_sample_call_projections(variants: &Path, keep: &Path) {
    let Some(root) = variants.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path != keep
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(SAMPLE_CALL_PROJECTION_PREFIX) && name.ends_with(".parquet")
                })
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn available_sample_call_projection(variants: &Path) -> Option<PathBuf> {
    let path = sample_call_projection_path(variants).ok()?;
    sample_call_projection_is_valid(&path, variants).then_some(path)
}

fn build_sample_call_projection(variants: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(variants)
        .map_err(|error| format!("cannot read {}: {error}", variants.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|error| format!("cannot read result sample calls: {error}"))?;
    let schema = builder.schema();
    let has_alternate_count = schema.field_with_name("alternate_count").is_ok();
    let mut columns = vec!["record_number", "alt_index", "format", "samples_json"];
    if has_alternate_count {
        columns.push("alternate_count");
    }
    let indices = columns
        .into_iter()
        .map(|name| {
            schema
                .index_of(name)
                .map_err(|_| format!("result sample calls have no {name} column"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let projection = ProjectionMask::roots(builder.parquet_schema(), indices);
    let reader = builder
        .with_projection(projection)
        .with_batch_size(VARIANT_CHUNK_RECORDS)
        .build()
        .map_err(|error| format!("cannot stream result sample calls: {error}"))?;

    let partial = crate::library_metadata::unique_temporary_path(destination)?;
    let projection_schema = SampleCallProjectionBatch::default()
        .into_record_batch()?
        .schema();
    let mut writer = parquet_writer(&partial, projection_schema)?;
    let mut pending_legacy_record = Vec::<LegacySampleCallRow>::new();
    for batch in reader {
        let batch = batch.map_err(|error| format!("cannot read result sample calls: {error}"))?;
        let record_numbers = batch
            .column_by_name("record_number")
            .and_then(|array| array.as_any().downcast_ref::<Int64Array>())
            .ok_or("result sample calls have an invalid record_number column")?;
        let alt_indices = batch
            .column_by_name("alt_index")
            .and_then(|array| array.as_any().downcast_ref::<Int32Array>())
            .ok_or("result sample calls have an invalid alt_index column")?;
        let alternate_counts = batch.column_by_name("alternate_count").map(|array| {
            array
                .as_any()
                .downcast_ref::<Int32Array>()
                .ok_or("result sample calls have an invalid alternate_count column")
        });
        let alternate_counts = alternate_counts.transpose()?;
        let formats = batch
            .column_by_name("format")
            .and_then(|array| array.as_any().downcast_ref::<StringArray>())
            .ok_or("result sample calls have an invalid format column")?;
        let samples = batch
            .column_by_name("samples_json")
            .and_then(|array| array.as_any().downcast_ref::<StringArray>())
            .ok_or("result sample calls have an invalid samples_json column")?;
        let mut output = SampleCallProjectionBatch::default();
        for row in 0..batch.num_rows() {
            let format = (!formats.is_null(row)).then(|| formats.value(row));
            let record_number = record_numbers.value(row);
            let alt_index = alt_indices.value(row);
            if let Some(alternate_counts) = alternate_counts {
                let (zygosity, zygosity_sort) = zygosity_from_samples_json(
                    format,
                    samples.value(row),
                    alt_index,
                    alternate_counts.value(row),
                );
                output.record_number.push(record_number);
                output.alt_index.push(alt_index);
                output.zygosity.push(zygosity);
                output.zygosity_sort.push(zygosity_sort);
                continue;
            }
            if pending_legacy_record
                .first()
                .is_some_and(|pending| pending.record_number != record_number)
            {
                if pending_legacy_record[0].record_number > record_number {
                    return Err("legacy result sample calls are not ordered by record".into());
                }
                append_legacy_sample_call_group(&mut output, &mut pending_legacy_record)?;
            }
            pending_legacy_record.push(LegacySampleCallRow {
                record_number,
                alt_index,
                format: format.map(str::to_owned),
                samples_json: samples.value(row).to_owned(),
            });
        }
        if output.len() > 0 {
            writer
                .write(&output.into_record_batch()?)
                .map_err(|error| format!("cannot write sample-call projection: {error}"))?;
        }
    }
    if !pending_legacy_record.is_empty() {
        let mut output = SampleCallProjectionBatch::default();
        append_legacy_sample_call_group(&mut output, &mut pending_legacy_record)?;
        writer
            .write(&output.into_record_batch()?)
            .map_err(|error| format!("cannot write sample-call projection: {error}"))?;
    }
    writer
        .close()
        .map_err(|error| format!("cannot finish sample-call projection: {error}"))?;
    if !sample_call_projection_is_valid(&partial, variants) {
        let _ = fs::remove_file(&partial);
        return Err("sample-call projection failed validation".into());
    }
    crate::library_metadata::publish_cache_file(&partial, destination, |path| {
        sample_call_projection_is_valid(path, variants)
    })
}

fn prepare_sample_call_projection(
    variants: &Path,
    request: &PageRequest,
) -> Result<Option<PathBuf>, String> {
    // Reject invalid closed-category values before legacy compatibility work starts.
    let _ = core_filter_rules_sql(request)?;
    if !request_uses_zygosity(request)
        || (parquet_has_column(variants, "zygosity")?
            && parquet_has_column(variants, "zygosity_sort")?)
    {
        return Ok(None);
    }
    if let Some(path) = available_sample_call_projection(variants) {
        return Ok(Some(path));
    }
    let _guard = SAMPLE_CALL_PROJECTION_BUILD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "sample-call projection lock failed")?;
    let destination = sample_call_projection_path(variants)?;
    if !sample_call_projection_is_valid(&destination, variants) {
        build_sample_call_projection(variants, &destination)?;
    }
    remove_stale_sample_call_projections(variants, &destination);
    Ok(Some(destination))
}

fn categorical_subquery_sql(
    read_sql: &str,
    base_condition: &str,
    expression: &str,
    kind: FilterValueKind,
    operator: &str,
    selection: &CategoricalSelection,
) -> Result<(String, Vec<SqlValue>), String> {
    let (matched, mut parameters) =
        normalized_categorical_match_sql(expression, kind, &selection.values);
    let aggregate = match operator {
        "in" => format!("bool_or({matched})"),
        "not_in" => format!("NOT bool_or({matched})"),
        _ => return Err("categorical filters use 'is any of' or 'is none of'".into()),
    };
    let present = categorical_present_sql(expression, kind);
    parameters.push(selection.include_missing.into());
    Ok((
        format!(
            "COALESCE((
                SELECT {aggregate}
                FROM {read_sql}
                WHERE ({base_condition}) AND ({present})
            ), CAST(? AS BOOLEAN))"
        ),
        parameters,
    ))
}

fn evidence_filter_rules_sql(
    evidence: Option<&Path>,
    evidence_files: Option<&[PathBuf]>,
    catalog: Option<&Path>,
    request: &PageRequest,
) -> Result<(String, Vec<SqlValue>), String> {
    if request.evidence_filters.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    if request.evidence_filters.len() > 24 {
        return Err("at most 24 evidence filter rules can be applied at once".into());
    }
    let evidence = evidence.ok_or("this AnnoCAT result has no evidence table")?;
    let catalog = catalog.ok_or("this AnnoCAT result has no field catalog")?;
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
            "json" => FilterValueKind::Json,
            _ if evidence_field_is_numeric(&field) => FilterValueKind::Number,
            _ => FilterValueKind::Text,
        };
        let categorical = categorical_selection(&filter.values, filter.include_missing)?;
        if categorical.is_some()
            && categorical_contract_for_field(&field.source_id, &field.field_path)?.is_none()
        {
            return Err(format!(
                "{} is not a categorical annotation field",
                field.field_path
            ));
        }
        if field.resolution == EvidenceResolutionStrategy::GeneDirect
            || (field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
                && !is_query_projection(evidence))
        {
            let gene_evidence =
                gene_evidence_path(catalog)?.ok_or("phenotype gene evidence is not ready")?;
            let expression = match kind {
                FilterValueKind::Number => {
                    "coalesce(ge.number_value, CAST(ge.integer_value AS DOUBLE))"
                }
                FilterValueKind::Boolean => "ge.boolean_value",
                FilterValueKind::Json => "ge.json_value",
                FilterValueKind::Text => {
                    "coalesce(ge.string_value, CAST(ge.integer_value AS VARCHAR), CAST(ge.number_value AS VARCHAR), CAST(ge.boolean_value AS VARCHAR), ge.json_value, '')"
                }
            };
            if let Some(selection) = categorical.as_ref() {
                let identity = if field.resolution == EvidenceResolutionStrategy::GeneDirect {
                    "upper(ge.gene_symbol)=upper(v.gene_symbol)"
                } else {
                    "ge.allele_id=v.allele_id"
                };
                let (condition, values) = categorical_subquery_sql(
                    "read_parquet(?) ge",
                    &format!("{identity} AND ge.source_id=? AND ge.field_path=?"),
                    expression,
                    kind,
                    &filter.operator,
                    selection,
                )?;
                sql.push_str(" AND (");
                sql.push_str(&condition);
                sql.push(')');
                parameters.push(gene_evidence.to_string_lossy().into_owned().into());
                parameters.push(field.source_id.into());
                parameters.push(field.field_path.into());
                parameters.extend(values);
                continue;
            }
            let negative = matches!(
                filter.operator.as_str(),
                "not_equals" | "not_contains" | "not_in"
            );
            let positive = match filter.operator.as_str() {
                "not_equals" => "equals",
                "not_contains" => "contains",
                "not_in" => "in",
                operator => operator,
            };
            let (condition, values) = comparison_sql(expression, kind, positive, &filter.value)?;
            sql.push_str(if negative {
                " AND NOT EXISTS ("
            } else {
                " AND EXISTS ("
            });
            sql.push_str("SELECT 1 FROM read_parquet(?) ge WHERE ");
            sql.push_str(
                if field.resolution == EvidenceResolutionStrategy::GeneDirect {
                    "upper(ge.gene_symbol)=upper(v.gene_symbol)"
                } else {
                    "ge.allele_id=v.allele_id"
                },
            );
            sql.push_str(" AND ge.source_id=? AND ge.field_path=? AND (");
            sql.push_str(&condition);
            sql.push_str(") LIMIT 1)");
            parameters.push(gene_evidence.to_string_lossy().into_owned().into());
            parameters.push(field.source_id.into());
            parameters.push(field.field_path.into());
            parameters.extend(values);
            continue;
        }
        if uses_resolution_sidecar(field.resolution) && !is_query_projection(evidence) {
            let resolved =
                crate::evidence_resolution::available_path(&canonical_evidence_path(evidence))
                    .ok_or("transcript evidence index is not ready")?;
            let expression = match kind {
                FilterValueKind::Number => "er.resolved_number",
                FilterValueKind::Text | FilterValueKind::Json | FilterValueKind::Boolean => {
                    "er.resolved_string"
                }
            };
            if let Some(selection) = categorical.as_ref() {
                let (condition, values) = categorical_subquery_sql(
                    "read_parquet(?) er",
                    &format!(
                        "er.allele_id = v.allele_id AND er.source_id=? AND er.field_path=? AND {}",
                        resolution_kind_condition(field.resolution, "er")
                    ),
                    expression,
                    kind,
                    &filter.operator,
                    selection,
                )?;
                sql.push_str(" AND (");
                sql.push_str(&condition);
                sql.push(')');
                parameters.push(resolved.to_string_lossy().into_owned().into());
                parameters.push(field.source_id.into());
                parameters.push(field.field_path.into());
                parameters.extend(values);
                continue;
            }
            let negative = matches!(
                filter.operator.as_str(),
                "not_equals" | "not_contains" | "not_in"
            );
            let positive_operator = match filter.operator.as_str() {
                "not_equals" => "equals",
                "not_contains" => "contains",
                "not_in" => "in",
                operator => operator,
            };
            let (condition, values) =
                comparison_sql(expression, kind, positive_operator, &filter.value)?;
            sql.push_str(if negative {
                " AND NOT EXISTS ("
            } else {
                " AND EXISTS ("
            });
            sql.push_str(&format!(
                "SELECT 1 FROM read_parquet(?) er
                   WHERE er.allele_id = v.allele_id
                     AND er.source_id = ? AND er.field_path = ?
                     AND {} AND (",
                resolution_kind_condition(field.resolution, "er")
            ));
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
            FilterValueKind::Json => "ev.json_value",
            FilterValueKind::Text => {
                "coalesce(ev.string_value, CAST(ev.integer_value AS VARCHAR), CAST(ev.number_value AS VARCHAR), CAST(ev.boolean_value AS VARCHAR), ev.json_value, '')"
            }
        };
        if let Some(selection) = categorical.as_ref() {
            let (read_sql, mut read_parameters) =
                evidence_read_for_fields(evidence, evidence_files, [field.index]);
            let identity = if is_query_projection(evidence) {
                "ev.record_number = v.record_number AND ev.alt_index = v.alt_index"
            } else {
                "ev.allele_id = v.allele_id"
            };
            let field_condition = evidence_field_condition(&field, "ev", evidence);
            let (condition, values) = categorical_subquery_sql(
                &format!("{read_sql} ev"),
                &format!("{identity} AND {field_condition}"),
                value_expression,
                kind,
                &filter.operator,
                selection,
            )?;
            sql.push_str(" AND (");
            sql.push_str(&condition);
            sql.push(')');
            parameters.append(&mut read_parameters);
            append_evidence_field_parameters(&mut parameters, &field, evidence)?;
            parameters.extend(values);
            continue;
        }
        let negative = matches!(
            filter.operator.as_str(),
            "not_equals" | "not_contains" | "not_in"
        );
        let positive_operator = match filter.operator.as_str() {
            "not_equals" => "equals",
            "not_contains" => "contains",
            "not_in" => "in",
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
        let (read_sql, read_parameters) =
            evidence_read_for_fields(evidence, evidence_files, [field.index]);
        let identity = if is_query_projection(evidence) {
            "ev.record_number = v.record_number AND ev.alt_index = v.alt_index"
        } else {
            "ev.allele_id = v.allele_id"
        };
        sql.push_str(&format!(
            "SELECT 1 FROM {read_sql} ev
             WHERE {identity} AND {} AND (",
            evidence_field_condition(&field, "ev", evidence)
        ));
        sql.push_str(&condition);
        sql.push_str(") LIMIT 1)");
        parameters.extend(read_parameters);
        append_evidence_field_parameters(&mut parameters, &field, evidence)?;
        parameters.extend(values);
    }
    Ok((sql, parameters))
}

fn displayed_field_search_sql(
    connection: &Connection,
    evidence: Option<&Path>,
    evidence_files: Option<&[PathBuf]>,
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
            .filter(|field| evidence_field_can_match_search(field, search))
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            connection
                .execute_batch(
                    "CREATE TEMP TABLE displayed_evidence_search(allele_id VARCHAR PRIMARY KEY)",
                )
                .map_err(|error| format!("cannot create evidence search table: {error}"))?;
            if is_query_projection(evidence) {
                connection
                    .execute_batch(
                        "CREATE TEMP TABLE displayed_projection_search(
                            record_number BIGINT,
                            alt_index INTEGER,
                            PRIMARY KEY(record_number, alt_index)
                        )",
                    )
                    .map_err(|error| {
                        format!("cannot create projected evidence search table: {error}")
                    })?;
            }
            let (gene_fields, fields): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .partition(|field| field.resolution == EvidenceResolutionStrategy::GeneDirect);
            let (allele_gene_fields, fields): (Vec<_>, Vec<_>) =
                fields.into_iter().partition(|field| {
                    field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
                        && !is_query_projection(evidence)
                });
            let (resolved_fields, raw): (Vec<_>, Vec<_>) = fields.into_iter().partition(|field| {
                uses_resolution_sidecar(field.resolution) && !is_query_projection(evidence)
            });
            if !gene_fields.is_empty() {
                connection
                    .execute_batch(
                        "CREATE TEMP TABLE displayed_gene_search(gene_symbol VARCHAR PRIMARY KEY)",
                    )
                    .map_err(|error| format!("cannot create phenotype search table: {error}"))?;
                let gene_evidence =
                    gene_evidence_path(catalog)?.ok_or("phenotype gene evidence is not ready")?;
                let conditions = gene_fields
                    .iter()
                    .map(|field| {
                        let field_condition =
                            evidence_field_condition(field, "gene_search", &gene_evidence);
                        let value = evidence_search_value_expression(field, "gene_search");
                        format!(
                            "(({field_condition}) AND contains(replace(replace(lower({value}), \
                             '_', ' '), '-', ' '), lower(?)))"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let mut search_parameters =
                    vec![SqlValue::from(gene_evidence.to_string_lossy().into_owned())];
                for field in &gene_fields {
                    append_evidence_field_parameters(
                        &mut search_parameters,
                        field,
                        &gene_evidence,
                    )?;
                    search_parameters.push(search.to_owned().into());
                }
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO displayed_gene_search
                             SELECT DISTINCT upper(gene_symbol)
                             FROM read_parquet(?) gene_search
                             WHERE {conditions}"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| format!("cannot search phenotype evidence fields: {error}"))?;
            }
            if !allele_gene_fields.is_empty() {
                let gene_evidence =
                    gene_evidence_path(catalog)?.ok_or("gene match evidence is not ready")?;
                let conditions = allele_gene_fields
                    .iter()
                    .map(|_| {
                        "(match_search.source_id=? AND match_search.field_path=? AND \
                         contains(replace(replace(lower(coalesce(match_search.string_value, \
                         cast(match_search.integer_value AS VARCHAR), \
                         cast(match_search.number_value AS VARCHAR), \
                         cast(match_search.boolean_value AS VARCHAR), match_search.json_value, '')), \
                         '_', ' '), '-', ' '), lower(?)))"
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let mut search_parameters =
                    vec![SqlValue::from(gene_evidence.to_string_lossy().into_owned())];
                for field in allele_gene_fields {
                    search_parameters.push(field.source_id.into());
                    search_parameters.push(field.field_path.into());
                    search_parameters.push(search.to_owned().into());
                }
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO displayed_evidence_search
                             SELECT DISTINCT match_search.allele_id
                             FROM read_parquet(?) match_search
                             WHERE match_search.allele_id IS NOT NULL AND ({conditions})"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| format!("cannot search gene match fields: {error}"))?;
            }
            if !raw.is_empty() {
                let (read_sql, mut search_parameters) = evidence_read_for_fields(
                    evidence,
                    evidence_files,
                    raw.iter().map(|field| field.index),
                );
                let conditions = raw
                    .iter()
                    .map(|field| {
                        let field_condition =
                            evidence_field_condition(field, "ev_search", evidence);
                        let value = evidence_search_value_expression(field, "ev_search");
                        format!(
                            "(({field_condition}) AND contains(replace(replace(lower({value}), \
                             '_', ' '), '-', ' '), lower(?)))"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ");
                for field in &raw {
                    append_evidence_field_parameters(&mut search_parameters, &field, evidence)?;
                    search_parameters.push(search.to_owned().into());
                }
                let (table, identity) = if is_query_projection(evidence) {
                    ("displayed_projection_search", "record_number, alt_index")
                } else {
                    ("displayed_evidence_search", "allele_id")
                };
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO {table}
                             SELECT DISTINCT {identity}
                             FROM {read_sql} ev_search
                             WHERE {conditions}"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| format!("cannot search displayed evidence fields: {error}"))?;
            }
            if !resolved_fields.is_empty() {
                let resolved =
                    crate::evidence_resolution::available_path(&canonical_evidence_path(evidence))
                        .ok_or("transcript evidence index is not ready")?;
                let conditions = resolved_fields
                    .iter()
                    .map(|field| {
                        let value = resolved_search_value_expression(field, "er_search");
                        format!(
                            "(er_search.source_id = ? AND er_search.field_path = ? AND {} \
                             AND contains(replace(replace(lower({value}), '_', ' '), '-', ' '), \
                             lower(?)))",
                            resolution_kind_condition(field.resolution, "er_search")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let mut search_parameters =
                    vec![SqlValue::from(resolved.to_string_lossy().into_owned())];
                for field in resolved_fields {
                    search_parameters.push(field.source_id.into());
                    search_parameters.push(field.field_path.into());
                    search_parameters.push(search.to_owned().into());
                }
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO displayed_evidence_search
                             SELECT DISTINCT er_search.allele_id
                             FROM read_parquet(?) er_search
                             WHERE {conditions}"
                        ),
                        params_from_iter(search_parameters.iter()),
                    )
                    .map_err(|error| format!("cannot search resolved evidence fields: {error}"))?;
            }
            sql.push_str(" OR v.allele_id IN (SELECT allele_id FROM displayed_evidence_search)");
            if is_query_projection(evidence) {
                sql.push_str(
                    " OR EXISTS (SELECT 1 FROM displayed_projection_search projection_search
                       WHERE projection_search.record_number=v.record_number
                         AND projection_search.alt_index=v.alt_index)",
                );
            }
            if !gene_fields.is_empty() {
                sql.push_str(
                    " OR upper(v.gene_symbol) IN (SELECT gene_symbol FROM displayed_gene_search)",
                );
            }
        }
    }
    sql.push(')');
    Ok((sql, parameters))
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

fn requested_evidence_indices(request: &PageRequest) -> Vec<usize> {
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
    indices
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
    let indices = requested_evidence_indices(request);
    if indices.is_empty() {
        return Ok(None);
    }
    let selected = selected_evidence_columns(catalog, &indices)?;
    let requested = requested_resolution_fields(selected);
    if requested.is_empty() {
        return Ok(None);
    }
    crate::evidence_resolution::prepare(
        variants,
        &canonical_evidence_path(evidence),
        catalog,
        &requested,
    )
}

fn requested_resolution_fields(
    selected: impl IntoIterator<Item = SelectedEvidenceColumn>,
) -> Vec<crate::evidence_resolution::RequestedField> {
    selected
        .into_iter()
        .filter_map(|field| {
            let kind = match field.resolution {
                EvidenceResolutionStrategy::DerivedMaximum => {
                    crate::evidence_resolution::RequestedResolutionKind::DerivedMaximum
                }
                EvidenceResolutionStrategy::SelectedConsequence => {
                    crate::evidence_resolution::RequestedResolutionKind::SelectedFeature
                }
                EvidenceResolutionStrategy::AlignedTranscriptVector => {
                    crate::evidence_resolution::RequestedResolutionKind::AlignedTranscriptVector
                }
                EvidenceResolutionStrategy::LegacyAlleleRecovery => {
                    crate::evidence_resolution::RequestedResolutionKind::LegacyAllele
                }
                _ => return None,
            };
            Some(crate::evidence_resolution::RequestedField {
                scope: field.scope,
                biological_scope: field.biological_scope,
                source_id: field.source_id,
                field_path: field.field_path,
                kind,
            })
        })
        .collect()
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
    prepare_sample_call_projection(variants, request)?;
    prepare_requested_evidence_resolution(variants, evidence, catalog, request)?;
    with_query_evidence(
        evidence,
        catalog,
        request,
        |query_evidence, query_evidence_files| {
            page_json_with_evidence_once(
                variants,
                query_evidence,
                query_evidence_files,
                catalog,
                offset,
                limit,
                request,
                candidate_ids,
            )
        },
    )
}

fn page_json_with_evidence_once(
    variants: &Path,
    evidence: Option<&Path>,
    evidence_files: Option<&[PathBuf]>,
    catalog: Option<&Path>,
    offset: u64,
    limit: u64,
    request: &PageRequest,
    candidate_ids: Option<&[String]>,
) -> Result<String, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    register_report_variants(&connection, variants)?;
    let query = PageQuery {
        variants,
        evidence,
        evidence_files,
        catalog,
        offset,
        limit,
        request,
        candidate_ids,
    };
    serde_json::to_string(&page_with_evidence_result(&connection, &query)?)
        .map_err(|error| error.to_string())
}

type TranscriptEvidenceResolution = (String, Option<String>, String, i16, i16);

fn page_with_evidence_result(
    connection: &Connection,
    query: &PageQuery<'_>,
) -> Result<ResultPage, String> {
    let evidence = query.evidence;
    let evidence_files = query.evidence_files;
    let catalog = query.catalog;
    let request = query.request;
    if request.evidence_columns.is_empty() {
        return page_result_internal(connection, query);
    }
    let evidence = evidence.ok_or("this AnnoCAT result has no evidence table")?;
    let catalog = catalog.ok_or("this AnnoCAT result has no field catalog")?;
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
    let row_identities = rows
        .iter()
        .filter_map(|row| {
            Some((
                row["recordNumber"].as_i64()?,
                i32::try_from(row["altIndex"].as_i64()?).ok()?,
                row["alleleId"].as_str()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    let mut values: HashMap<(String, usize), Vec<String>> = HashMap::new();
    let mut fallback_values: HashMap<(String, usize), Vec<String>> = HashMap::new();
    if is_query_projection(evidence) {
        let direct = selected
            .iter()
            .filter(|field| query_projection_field_is_eligible(field))
            .collect::<Vec<_>>();
        if !direct.is_empty() {
            let (read_sql, mut parameters) = evidence_read(evidence, evidence_files);
            let row_placeholders = std::iter::repeat_n("(?, ?)", row_identities.len())
                .collect::<Vec<_>>()
                .join(",");
            let field_placeholders = std::iter::repeat_n("?", direct.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT record_number, alt_index, field_index,
                        coalesce(string_value, cast(integer_value AS VARCHAR),
                                 cast(number_value AS VARCHAR),
                                 cast(boolean_value AS VARCHAR), json_value)
                 FROM {read_sql}
                 WHERE (record_number, alt_index) IN ({row_placeholders})
                   AND field_index IN ({field_placeholders})
                 ORDER BY record_number, alt_index, field_index"
            );
            for (record_number, alt_index, _) in &row_identities {
                parameters.push((*record_number).into());
                parameters.push((*alt_index as i64).into());
            }
            parameters.extend(direct.iter().map(|field| (field.index as i64).into()));
            let allele_by_row = row_identities
                .iter()
                .map(|(record_number, alt_index, allele_id)| {
                    ((*record_number, *alt_index), allele_id.as_str())
                })
                .collect::<HashMap<_, _>>();
            let mut statement = connection
                .prepare(&sql)
                .map_err(|error| format!("cannot prepare query projection columns: {error}"))?;
            let mapped = statement
                .query_map(params_from_iter(parameters.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i64>(2)? as usize,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|error| format!("cannot read query projection columns: {error}"))?;
            for row in mapped {
                let (record_number, alt_index, index, value) =
                    row.map_err(|error| error.to_string())?;
                let Some(value) = value else { continue };
                let Some(allele_id) = allele_by_row.get(&(record_number, alt_index)) else {
                    continue;
                };
                let entry = values.entry(((*allele_id).to_owned(), index)).or_default();
                if !entry.contains(&value) {
                    entry.push(value);
                }
            }
        }
    } else {
        let (read_sql, mut parameters) = evidence_read(evidence, evidence_files);
        let field_conditions = selected
            .iter()
            .map(|field| format!("({})", evidence_field_condition(field, "ev", evidence)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT allele_id, scope, source_id, field_path,
                    coalesce(string_value, cast(integer_value AS VARCHAR),
                             cast(number_value AS VARCHAR),
                             cast(boolean_value AS VARCHAR), json_value)
             FROM {read_sql} ev
             WHERE allele_id IN ({allele_placeholders}) AND ({field_conditions})
             ORDER BY allele_id, scope, source_id, field_path,
                      consequence_id NULLS FIRST"
        );
        parameters.extend(allele_ids.iter().cloned().map(Into::into));
        for field in &selected {
            append_evidence_field_parameters(&mut parameters, field, evidence)?;
        }
        let exact_lookup = selected
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
        let fallback_lookup = selected
            .iter()
            .flat_map(|field| {
                field
                    .equivalent_scopes
                    .iter()
                    .filter(|scope| **scope != field.scope)
                    .map(|scope| {
                        (
                            (
                                scope.clone(),
                                field.source_id.clone(),
                                field.field_path.clone(),
                            ),
                            field.index,
                        )
                    })
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
        for row in mapped {
            let (allele_id, scope, source_id, field_path, value) =
                row.map_err(|error| error.to_string())?;
            let Some(value) = value else { continue };
            let identity = (scope, source_id, field_path);
            let (index, target) = if let Some(index) = exact_lookup.get(&identity) {
                (*index, &mut values)
            } else if let Some(index) = fallback_lookup.get(&identity) {
                (*index, &mut fallback_values)
            } else {
                continue;
            };
            let entry = target.entry((allele_id, index)).or_default();
            if !entry.contains(&value) {
                entry.push(value);
            }
        }
    }

    let gene_fields = selected
        .iter()
        .filter(|field| field.resolution == EvidenceResolutionStrategy::GeneDirect)
        .collect::<Vec<_>>();
    if !gene_fields.is_empty() {
        let gene_evidence =
            gene_evidence_path(catalog)?.ok_or("phenotype gene evidence is not ready")?;
        let symbols = rows
            .iter()
            .filter_map(|row| row["geneSymbol"].as_str())
            .map(|symbol| symbol.to_ascii_uppercase())
            .collect::<HashSet<_>>();
        if !symbols.is_empty() {
            let symbol_placeholders = std::iter::repeat_n("?", symbols.len())
                .collect::<Vec<_>>()
                .join(",");
            let field_conditions =
                std::iter::repeat_n("(source_id=? AND field_path=?)", gene_fields.len())
                    .collect::<Vec<_>>()
                    .join(" OR ");
            let sql = format!(
                "SELECT upper(gene_symbol), source_id, field_path,
                        coalesce(string_value, cast(integer_value AS VARCHAR),
                                 cast(number_value AS VARCHAR),
                                 cast(boolean_value AS VARCHAR), json_value)
                 FROM read_parquet(?)
                 WHERE upper(gene_symbol) IN ({symbol_placeholders})
                   AND ({field_conditions})"
            );
            let mut parameters = vec![SqlValue::from(gene_evidence.to_string_lossy().into_owned())];
            parameters.extend(symbols.iter().cloned().map(Into::into));
            for field in &gene_fields {
                parameters.push(field.source_id.clone().into());
                parameters.push(field.field_path.clone().into());
            }
            let lookup = gene_fields
                .iter()
                .map(|field| {
                    (
                        (field.source_id.as_str(), field.field_path.as_str()),
                        field.index,
                    )
                })
                .collect::<HashMap<_, _>>();
            let allele_symbols = rows
                .iter()
                .filter_map(|row| {
                    Some((
                        row["geneSymbol"].as_str()?.to_ascii_uppercase(),
                        row["alleleId"].as_str()?.to_owned(),
                    ))
                })
                .fold(
                    HashMap::<String, Vec<String>>::new(),
                    |mut map, (symbol, allele)| {
                        map.entry(symbol).or_default().push(allele);
                        map
                    },
                );
            let mut statement = connection
                .prepare(&sql)
                .map_err(|error| format!("cannot prepare phenotype evidence columns: {error}"))?;
            let mapped = statement
                .query_map(params_from_iter(parameters.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|error| format!("cannot read phenotype evidence columns: {error}"))?;
            for row in mapped {
                let (symbol, source_id, field_path, value) =
                    row.map_err(|error| error.to_string())?;
                let (Some(index), Some(value), Some(alleles)) = (
                    lookup.get(&(source_id.as_str(), field_path.as_str())),
                    value,
                    allele_symbols.get(&symbol),
                ) else {
                    continue;
                };
                for allele_id in alleles {
                    values.insert((allele_id.clone(), *index), vec![value.clone()]);
                }
            }
        }
    }

    let allele_gene_fields = selected
        .iter()
        .filter(|field| {
            field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
                && !is_query_projection(evidence)
        })
        .collect::<Vec<_>>();
    if !allele_gene_fields.is_empty() {
        let gene_evidence =
            gene_evidence_path(catalog)?.ok_or("gene match evidence is not ready")?;
        let field_conditions =
            std::iter::repeat_n("(source_id=? AND field_path=?)", allele_gene_fields.len())
                .collect::<Vec<_>>()
                .join(" OR ");
        let sql = format!(
            "SELECT allele_id, source_id, field_path,
                    coalesce(string_value, cast(integer_value AS VARCHAR),
                             cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                             json_value)
             FROM read_parquet(?)
             WHERE allele_id IN ({allele_placeholders})
               AND ({field_conditions})"
        );
        let mut parameters = vec![SqlValue::from(gene_evidence.to_string_lossy().into_owned())];
        parameters.extend(allele_ids.iter().cloned().map(Into::into));
        for field in &allele_gene_fields {
            parameters.push(field.source_id.clone().into());
            parameters.push(field.field_path.clone().into());
        }
        let lookup = allele_gene_fields
            .iter()
            .map(|field| {
                (
                    (field.source_id.as_str(), field.field_path.as_str()),
                    field.index,
                )
            })
            .collect::<HashMap<_, _>>();
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("cannot prepare gene match columns: {error}"))?;
        let mapped = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| format!("cannot read gene match columns: {error}"))?;
        for row in mapped {
            let (allele_id, source_id, field_path, value) =
                row.map_err(|error| error.to_string())?;
            if let (Some(index), Some(value)) = (
                lookup.get(&(source_id.as_str(), field_path.as_str())),
                value,
            ) {
                values.entry((allele_id, *index)).or_default().push(value);
            }
        }
    }

    let resolved_fields = selected
        .iter()
        .filter(|field| uses_resolution_sidecar(field.resolution))
        .collect::<Vec<_>>();
    let mut resolutions: HashMap<(String, usize), TranscriptEvidenceResolution> = HashMap::new();
    if !resolved_fields.is_empty() {
        let resolved =
            crate::evidence_resolution::available_path(&canonical_evidence_path(evidence))
                .ok_or("transcript evidence index is not ready")?;
        let conditions =
            std::iter::repeat_n("(source_id = ? AND field_path = ?)", resolved_fields.len())
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
        for field in &resolved_fields {
            parameters.push(field.source_id.clone().into());
            parameters.push(field.field_path.clone().into());
        }
        let resolved_lookup = resolved_fields
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
            if let Some(index) = resolved_lookup.get(&(source_id, field_path)) {
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
                if resolved_value_is_usable(field.resolution, kind) {
                    if let Some(resolved) = resolved {
                        object.insert(field.index.to_string(), Value::String(resolved.clone()));
                    }
                    continue;
                }
                if field.resolution == EvidenceResolutionStrategy::AlignedTranscriptVector
                    && matches!(kind.as_str(), "exact_missing" | "not_reported")
                {
                    continue;
                }
                if uses_resolution_sidecar(field.resolution) {
                    continue;
                }
            }
            let key = (allele_id.to_owned(), field.index);
            let exact_values = values.get(&key);
            let Some(field_values) = exact_values.or_else(|| fallback_values.get(&key)) else {
                continue;
            };
            match field.resolution {
                EvidenceResolutionStrategy::SelectedConsequence => {
                    resolution_metadata.insert(
                        field.index.to_string(),
                        json!({"kind": "exact_consequence"}),
                    );
                }
                EvidenceResolutionStrategy::MaterializedSelected => {
                    resolution_metadata.insert(
                        field.index.to_string(),
                        json!({"kind": "exact_consequence"}),
                    );
                }
                EvidenceResolutionStrategy::SourceSelected => {
                    resolution_metadata
                        .insert(field.index.to_string(), json!({"kind": "source_selected"}));
                }
                EvidenceResolutionStrategy::LegacyAlleleRecovery if exact_values.is_none() => {
                    resolution_metadata.insert(
                        field.index.to_string(),
                        json!({"kind": "exact_consequence"}),
                    );
                }
                _ => {}
            }
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
            evidence_files: None,
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
            evidence_files: None,
            catalog,
            offset,
            limit,
            request,
            candidate_ids: Some(candidate_ids),
        },
    )
}

fn page_json_with_details_query(query_key: &str, query: PageQuery<'_>) -> Result<String, String> {
    prepare_sample_call_projection(query.variants, query.request)?;
    prepare_requested_evidence_resolution(
        query.variants,
        query.evidence,
        query.catalog,
        query.request,
    )?;
    with_query_evidence(
        query.evidence,
        query.catalog,
        query.request,
        |evidence, evidence_files| {
            page_json_with_details_query_once(
                query_key,
                PageQuery {
                    evidence,
                    evidence_files,
                    ..query
                },
            )
        },
    )
}

fn page_json_with_details_query_once(
    query_key: &str,
    query: PageQuery<'_>,
) -> Result<String, String> {
    let session_key = if query.request.query_session.is_empty() {
        query_key.to_owned()
    } else {
        format!("{query_key}:{}", query.request.query_session)
    };
    let (connection, _guard) =
        cancellable_page_connection(&session_key, query.request.request_generation)?;
    register_report_variants(&connection, query.variants)?;
    let page = page_with_evidence_result(&connection, &query)?;
    serde_json::to_string(&page).map_err(|error| error.to_string())
}

fn selected_evidence_columns(
    catalog: &Path,
    indices: &[usize],
) -> Result<Vec<SelectedEvidenceColumn>, String> {
    let current_selection_contract = report_uses_current_selection_contract(catalog)?;
    let catalog = query_field_catalog(catalog)?;
    let fields = catalog["fields"]
        .as_array()
        .ok_or("field catalog has no fields array")?;
    let current_record_contracts = crate::evidence_resolution::record_resolution_contracts();
    indices
        .iter()
        .map(|index| {
            let field = fields
                .get(*index)
                .ok_or_else(|| format!("evidence column {index} is outside the field catalog"))?;
            let logical_scope = field["scope"]
                .as_str()
                .ok_or("evidence field has no scope")?;
            let source_id = field["sourceId"]
                .as_str()
                .ok_or("evidence field has no source ID")?;
            let field_path = field["fieldPath"]
                .as_str()
                .ok_or("evidence field has no field path")?;
            let biological_scope = field["biologicalScope"].as_str().unwrap_or(logical_scope);
            let physical_scope = field["physicalScope"].as_str().unwrap_or(logical_scope);
            let mut equivalent_scopes = vec![physical_scope.to_owned()];
            if physical_scope == logical_scope
                && logical_scope == "allele"
                && fields.iter().any(|candidate| {
                    candidate["scope"] == "transcript"
                        && candidate["sourceId"] == source_id
                        && candidate["fieldPath"] == field_path
                })
            {
                equivalent_scopes.push("transcript".to_owned());
            }
            let aligned = field["alignmentGroup"].as_str().is_some()
                || crate::evidence_resolution::bundled_alignment_group(
                    logical_scope,
                    source_id,
                    field_path,
                )
                .is_some();
            let record_aligned =
                crate::evidence_resolution::record_field_is_aligned(source_id, field_path);
            let record_contract_is_current =
                current_record_contracts
                    .get(source_id)
                    .is_some_and(|contract_id| {
                        catalog["recordResolutionContracts"][source_id].as_str()
                            == Some(contract_id.as_str())
                    });
            let resolution_policy = field["resolutionPolicy"].as_str();
            let resolution = if resolution_policy == Some("derivedSpliceAiMaximum") {
                EvidenceResolutionStrategy::DerivedMaximum
            } else if resolution_policy == Some("alleleGeneDirect") {
                EvidenceResolutionStrategy::AlleleGeneDirect
            } else if resolution_policy == Some("geneDirect")
                || field["storageRelation"].as_str() == Some("geneEvidence")
            {
                EvidenceResolutionStrategy::GeneDirect
            } else if physical_scope == "selected"
                && field["selectionOrigin"].as_str() == Some("provider")
            {
                EvidenceResolutionStrategy::SourceSelected
            } else if physical_scope == "selected" && record_aligned && !record_contract_is_current
            {
                EvidenceResolutionStrategy::AlignedTranscriptVector
            } else if physical_scope == "selected" && current_selection_contract {
                EvidenceResolutionStrategy::MaterializedSelected
            } else if physical_scope == "selected" {
                EvidenceResolutionStrategy::SelectedConsequence
            } else if aligned || resolution_policy == Some("alignedTranscriptVector") {
                EvidenceResolutionStrategy::AlignedTranscriptVector
            } else if matches!(
                resolution_policy,
                Some("sourceSelectedCodingRecord" | "providerSelected")
            ) {
                EvidenceResolutionStrategy::SourceSelected
            } else if matches!(
                resolution_policy,
                Some("selectedFeature" | "materializedSelected")
            ) {
                EvidenceResolutionStrategy::SelectedConsequence
            } else if matches!(resolution_policy, Some("directAllele" | "direct")) {
                EvidenceResolutionStrategy::Allele
            } else if logical_scope == "allele" && equivalent_scopes.len() > 1 {
                EvidenceResolutionStrategy::LegacyAlleleRecovery
            } else if logical_scope != "allele" && logical_scope != "variant" {
                EvidenceResolutionStrategy::SelectedConsequence
            } else {
                EvidenceResolutionStrategy::Allele
            };
            Ok(SelectedEvidenceColumn {
                index: *index,
                scope: if resolution == EvidenceResolutionStrategy::SelectedConsequence
                    && physical_scope == "selected"
                {
                    logical_scope.to_owned()
                } else {
                    physical_scope.to_owned()
                },
                biological_scope: biological_scope.to_owned(),
                equivalent_scopes,
                source_id: source_id.to_owned(),
                field_path: field_path.to_owned(),
                value_type: field["valueType"]
                    .as_str()
                    .ok_or("evidence field has no value type")?
                    .to_owned(),
                resolution,
            })
        })
        .collect()
}

fn query_field_catalog(catalog: &Path) -> Result<Value, String> {
    let metadata =
        fs::metadata(catalog).map_err(|error| format!("field catalog is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("field catalog has an invalid size".into());
    }
    let mut catalog: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid field catalog: {error}"))?;
    append_legacy_spliceai_maximum(&mut catalog)?;
    enrich_categorical_contracts(&mut catalog)?;
    Ok(catalog)
}

pub(crate) fn field_catalog_json(catalog: &Path) -> Result<String, String> {
    serde_json::to_string(&query_field_catalog(catalog)?).map_err(|error| error.to_string())
}

pub(crate) fn categorical_filter_values_json(
    variants: &Path,
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    core_column: Option<&str>,
    evidence_index: Option<usize>,
) -> Result<String, String> {
    if core_column.is_some() == evidence_index.is_some() {
        return Err("specify one core column or one annotation field".into());
    }
    let connection = Connection::open_in_memory()
        .map_err(|error| format!("cannot initialize categorical value discovery: {error}"))?;
    let (query, parameters) = if let Some(column) = core_column {
        let (expression, delimiter) = match column {
            "impact" => ("coalesce(CAST(v.impact AS VARCHAR), '')", None),
            "consequence" => ("coalesce(CAST(v.consequence AS VARCHAR), '')", Some('&')),
            "filter" => ("coalesce(CAST(v.filter AS VARCHAR), '')", Some(';')),
            _ => return Err(format!("{column} is not a categorical core column")),
        };
        let values = delimiter.map_or_else(
            || format!("SELECT {expression} AS value FROM read_parquet(?) v"),
            |delimiter| {
                format!(
                    "SELECT category.value
                     FROM read_parquet(?) v,
                     unnest(string_split({expression}, '{delimiter}')) AS category(value)"
                )
            },
        );
        (
            format!(
                "SELECT DISTINCT trim(value) AS value FROM ({values}) discovered
                 WHERE nullif(trim(value), '') IS NOT NULL AND trim(value) <> '.'
                 ORDER BY lower(value), value LIMIT {}",
                MAX_CATEGORICAL_VALUES + 1
            ),
            vec![variants.to_string_lossy().into_owned().into()],
        )
    } else {
        let evidence = evidence.ok_or("this AnnoCAT result has no evidence table")?;
        let catalog = catalog.ok_or("this AnnoCAT result has no field catalog")?;
        let field = selected_evidence_columns(catalog, &[evidence_index.unwrap()])?
            .into_iter()
            .next()
            .ok_or("annotation field is missing")?;
        if categorical_contract_for_field(&field.source_id, &field.field_path)?.is_none() {
            return Err(format!(
                "{} is not a categorical annotation field",
                field.field_path
            ));
        }
        if field.resolution == EvidenceResolutionStrategy::GeneDirect
            || field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect
            || uses_resolution_sidecar(field.resolution)
        {
            return Err("this field has fixed choices and does not need value discovery".into());
        }
        let evidence = canonical_evidence_path(evidence);
        let kind = if field.value_type == "json" {
            FilterValueKind::Json
        } else {
            FilterValueKind::Text
        };
        let expression = if kind == FilterValueKind::Json {
            "ev.json_value"
        } else {
            "coalesce(ev.string_value, CAST(ev.integer_value AS VARCHAR), CAST(ev.number_value AS VARCHAR), CAST(ev.boolean_value AS VARCHAR), '')"
        };
        let field_condition = evidence_field_condition(&field, "ev", &evidence);
        let values = if kind == FilterValueKind::Json {
            format!(
                "SELECT category.value
                 FROM read_parquet(?) ev,
                 unnest(json_extract_string(coalesce({expression}, '[]'), '$[*]')) AS category(value)
                 WHERE {field_condition}"
            )
        } else {
            format!(
                "SELECT {expression} AS value FROM read_parquet(?) ev
                 WHERE {field_condition}"
            )
        };
        let mut parameters = vec![evidence.to_string_lossy().into_owned().into()];
        append_evidence_field_parameters(&mut parameters, &field, &evidence)?;
        (
            format!(
                "SELECT DISTINCT trim(value) AS value FROM ({values}) discovered
                 WHERE nullif(trim(value), '') IS NOT NULL AND trim(value) <> '.'
                 ORDER BY lower(value), value LIMIT {}",
                MAX_CATEGORICAL_VALUES + 1
            ),
            parameters,
        )
    };
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| format!("cannot prepare categorical value discovery: {error}"))?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("cannot discover categorical values: {error}"))?;
    let mut values = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot read categorical values: {error}"))?;
    let complete = values.len() <= MAX_CATEGORICAL_VALUES;
    values.truncate(MAX_CATEGORICAL_VALUES);
    serde_json::to_string(&json!({ "values": values, "complete": complete }))
        .map_err(|error| error.to_string())
}

fn append_legacy_spliceai_maximum(catalog: &mut Value) -> Result<(), String> {
    let fields = catalog["fields"]
        .as_array_mut()
        .ok_or("field catalog has no fields array")?;
    let components = ["dsag", "dsal", "dsdg", "dsdl"];
    let mut present = BTreeMap::<String, BTreeSet<String>>::new();
    let mut has_maximum = HashSet::<String>::new();
    for field in fields.iter() {
        let Some(source_id) = field["sourceId"].as_str() else {
            continue;
        };
        if !source_id.eq_ignore_ascii_case("spliceai") {
            continue;
        }
        let Some(field_path) = field["fieldPath"].as_str() else {
            continue;
        };
        let normalized = normalized_evidence_key(field_path);
        if normalized == "maxdeltascore" {
            has_maximum.insert(source_id.to_owned());
        } else if components.contains(&normalized.as_str()) {
            present
                .entry(source_id.to_owned())
                .or_default()
                .insert(normalized);
        }
    }
    for (source_id, available) in present {
        if available.len() != components.len() || has_maximum.contains(&source_id) {
            continue;
        }
        fields.push(json!({
            "scope": "selected",
            "sourceId": source_id,
            "fieldPath": "maxDeltaScore",
            "valueType": "number",
            "observedTypes": ["number"],
            "occurrences": 0,
            "biologicalScope": "gene",
            "physicalScope": "selected",
            "storageEncoding": "derivedScalar",
            "resolutionPolicy": "derivedSpliceAiMaximum"
        }));
    }
    Ok(())
}

fn query_projection_field_is_eligible(field: &SelectedEvidenceColumn) -> bool {
    field.resolution != EvidenceResolutionStrategy::GeneDirect
}

fn catalog_source_is(source_id: &str, expected: &str) -> bool {
    let source_id = source_id.to_ascii_lowercase();
    let expected = expected.to_ascii_lowercase();
    source_id == expected
        || source_id
            .strip_prefix(&expected)
            .is_some_and(|suffix| matches!(suffix.as_bytes().first(), Some(b'-' | b'@')))
}

fn catalog_field_leaf(field: &Value) -> String {
    field["fieldPath"]
        .as_str()
        .unwrap_or_default()
        .split(['.', '[', ']'])
        .filter(|part| !part.is_empty())
        .next_back()
        .map(normalized_evidence_key)
        .unwrap_or_default()
}

fn recommended_query_projection_indices(catalog: &Value) -> Result<Vec<usize>, String> {
    let fields = catalog["fields"]
        .as_array()
        .ok_or("field catalog has no fields array")?;
    let entries = fields
        .iter()
        .enumerate()
        .filter(|(index, field)| {
            if field["selectable"].as_bool() == Some(false) {
                return false;
            }
            let source_id = field["sourceId"].as_str().unwrap_or_default();
            let field_path = field["fieldPath"].as_str().unwrap_or_default();
            if catalog_source_is(source_id, "spliceai") && catalog_field_leaf(field) == "gene" {
                return false;
            }
            field["scope"].as_str() != Some("transcript")
                || !fields
                    .iter()
                    .enumerate()
                    .any(|(candidate_index, candidate)| {
                        candidate_index != *index
                            && candidate["scope"].as_str() == Some("allele")
                            && candidate["sourceId"].as_str() == Some(source_id)
                            && candidate["fieldPath"].as_str() == Some(field_path)
                    })
        })
        .collect::<Vec<_>>();
    let find = |source: &dyn Fn(&str) -> bool, leaves: &[&str], scope: Option<&str>| {
        entries.iter().find_map(|(index, field)| {
            let source_id = field["sourceId"].as_str()?;
            (source(source_id)
                && scope.is_none_or(|scope| field["scope"].as_str() == Some(scope))
                && leaves.contains(&catalog_field_leaf(field).as_str()))
            .then_some(*index)
        })
    };
    let source =
        |expected: &'static str| move |candidate: &str| catalog_source_is(candidate, expected);
    let favor = source("favor-online");
    let mut selected = Vec::new();
    let mut push = |index: Option<usize>| {
        if let Some(index) = index
            && !selected.contains(&index)
        {
            selected.push(index);
        }
    };
    push(
        find(&source("clinvar"), &["significance"], Some("allele"))
            .or_else(|| find(&favor, &["clinicalsignificance"], None)),
    );
    push(
        find(
            &|candidate| candidate.to_ascii_lowercase().contains("gnomad"),
            &["allaf", "af", "allelefrequency"],
            None,
        )
        .or_else(|| find(&favor, &["gnomadaf"], None)),
    );
    push(
        find(&source("phylop"), &["score", "value"], None)
            .or_else(|| {
                find(
                    &source("dbnsfp"),
                    &["phylop100way", "phylop100wayscore"],
                    None,
                )
            })
            .or_else(|| find(&favor, &["codingphylop100way"], None))
            .or_else(|| find(&favor, &["apcconservation"], None)),
    );
    push(
        find(&source("cadd"), &["phred"], None)
            .or_else(|| find(&source("dbnsfp"), &["caddphred"], None))
            .or_else(|| find(&favor, &["codingcaddphred", "caddphred"], None)),
    );
    push(
        find(&source("revel"), &["score"], None)
            .or_else(|| find(&source("dbnsfp"), &["revelscore"], None))
            .or_else(|| find(&favor, &["codingrevelscore", "revel"], None)),
    );
    push(
        find(&source("dbnsfp"), &["alphamissensescore"], None)
            .or_else(|| find(&favor, &["codingalphamissensescore", "alphamissense"], None)),
    );
    push(
        find(&source("spliceai"), &["maxdeltascore"], None)
            .or_else(|| find(&favor, &["spliceaidsmax"], None)),
    );
    Ok(selected)
}

pub(crate) fn prepare_recommended_query_projections(
    variants: &Path,
    evidence: &Path,
    catalog: &Path,
) -> Result<usize, String> {
    let indices = recommended_query_projection_indices(&query_field_catalog(catalog)?)?;
    let fields = selected_evidence_columns(catalog, &indices)?
        .into_iter()
        .filter(query_projection_field_is_eligible)
        .collect::<Vec<_>>();
    if fields.is_empty() {
        return Ok(0);
    }
    let requested = requested_resolution_fields(fields.iter().cloned());
    if !requested.is_empty() {
        crate::evidence_resolution::prepare(
            variants,
            &canonical_evidence_path(evidence),
            catalog,
            &requested,
        )?;
    }
    prepare_query_projection(evidence, catalog, &fields)?;
    Ok(fields.len())
}

fn query_projection_source(
    evidence: &Path,
    catalog: &Path,
    field: &SelectedEvidenceColumn,
) -> Result<PathBuf, String> {
    if uses_resolution_sidecar(field.resolution) {
        return crate::evidence_resolution::available_path(&canonical_evidence_path(evidence))
            .ok_or_else(|| "transcript evidence index is not ready".into());
    }
    if field.resolution == EvidenceResolutionStrategy::AlleleGeneDirect {
        return gene_evidence_path(catalog)?
            .ok_or_else(|| "gene match evidence is not ready".into());
    }
    Ok(evidence.to_path_buf())
}

fn visible_evidence_files(evidence: &Path) -> Result<Vec<PathBuf>, String> {
    let name = evidence
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("result evidence has an invalid file name")?;
    let mut files = if let Some((prefix, suffix)) = name.split_once('*') {
        let directory = evidence
            .parent()
            .ok_or("evidence wildcard has no directory")?;
        fs::read_dir(directory)
            .map_err(|error| format!("cannot inspect supplemental evidence: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.starts_with(prefix) && value.ends_with(suffix))
            })
            .collect::<Vec<_>>()
    } else {
        vec![evidence.to_path_buf()]
    };
    files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    if files.is_empty() || files.iter().any(|path| !path.is_file()) {
        return Err("result evidence is missing".into());
    }
    Ok(files)
}

fn query_projection_identities(
    field: &SelectedEvidenceColumn,
) -> BTreeSet<(usize, String, String, String)> {
    field
        .equivalent_scopes
        .iter()
        .cloned()
        .map(|scope| {
            (
                field.index,
                scope,
                field.source_id.clone(),
                field.field_path.clone(),
            )
        })
        .collect()
}

fn query_projection_fingerprint(
    evidence: &Path,
    catalog: &Path,
    field: &SelectedEvidenceColumn,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(QUERY_PROJECTION_CONTRACT.as_bytes());
    for (index, scope, source_id, field_path) in query_projection_identities(field) {
        digest.update((index as u64).to_le_bytes());
        for value in [scope, source_id, field_path] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
    }
    let source = query_projection_source(evidence, catalog, field)?;
    let mut source_files = visible_evidence_files(&source)?;
    source_files.push(
        catalog
            .parent()
            .ok_or("field catalog has no result folder")?
            .join("variants.parquet"),
    );
    for path in source_files {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect result evidence: {error}"))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("result evidence has an invalid file name")?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update(metadata.len().to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize())[..16].to_owned())
}

fn query_projection_field_path(root: &Path, fingerprint: &str, field_index: usize) -> PathBuf {
    root.join(format!(
        "{QUERY_PROJECTION_PREFIX}{field_index}-{fingerprint}.parquet"
    ))
}

fn remove_stale_query_projection_field(root: &Path, field_index: usize, keep: &Path) {
    let prefix = format!("{QUERY_PROJECTION_PREFIX}{field_index}-");
    if let Ok(entries) = fs::read_dir(root) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let stale = path != keep
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".parquet"));
            if stale {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn remove_legacy_query_projections(root: &Path) {
    if let Ok(entries) = fs::read_dir(root) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let legacy = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    LEGACY_QUERY_PROJECTION_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                        && name.ends_with(".parquet")
                });
            if legacy {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn query_projection_is_valid(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(reader) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    [
        "record_number",
        "alt_index",
        "field_index",
        "string_value",
        "integer_value",
        "number_value",
        "boolean_value",
        "json_value",
    ]
    .iter()
    .all(|name| reader.schema().index_of(name).is_ok())
}

fn build_query_projection(
    evidence: &Path,
    catalog: &Path,
    field: &SelectedEvidenceColumn,
    destination: &Path,
) -> Result<(), String> {
    let fields = query_projection_identities(field);
    if fields.is_empty() {
        return Err("field catalog has no direct evidence fields".into());
    }

    let partial = crate::library_metadata::unique_temporary_path(destination)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "SET threads=4;
             SET preserve_insertion_order=false;",
        )
        .map_err(|error| format!("cannot configure query projection: {error}"))?;
    let escape = |value: &str| value.replace('\'', "''");
    let resolved = uses_resolution_sidecar(field.resolution);
    let conditions = if resolved {
        format!(
            "ev.source_id='{}' AND ev.field_path='{}' AND {}",
            escape(&field.source_id),
            escape(&field.field_path),
            resolution_kind_condition(field.resolution, "ev")
        )
    } else {
        fields
            .into_iter()
            .map(|(_, scope, source_id, field_path)| {
                format!(
                    "(ev.scope='{}' AND ev.source_id='{}' AND ev.field_path='{}')",
                    escape(&scope),
                    escape(&source_id),
                    escape(&field_path)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    let source = query_projection_source(evidence, catalog, field)?;
    let variants = catalog
        .parent()
        .ok_or("field catalog has no result folder")?
        .join("variants.parquet");
    if !variants.is_file() {
        return Err("result variant table is missing".into());
    }
    let evidence_sql = source.to_string_lossy().replace('\'', "''");
    let variants_sql = variants.to_string_lossy().replace('\'', "''");
    let partial_sql = partial.to_string_lossy().replace('\'', "''");
    let values = if resolved {
        "ev.resolved_string AS string_value,
         CAST(NULL AS BIGINT) AS integer_value,
         ev.resolved_number AS number_value,
         try_cast(ev.resolved_string AS BOOLEAN) AS boolean_value,
         CAST(NULL AS VARCHAR) AS json_value"
    } else {
        "ev.string_value,
         ev.integer_value,
         ev.number_value,
         ev.boolean_value,
         ev.json_value"
    };
    connection
        .execute_batch(&format!(
            "COPY (
                 SELECT v.record_number,
                        v.alt_index,
                        CAST({} AS INTEGER) AS field_index,
                        {values}
                 FROM read_parquet('{evidence_sql}') ev
                 JOIN read_parquet('{variants_sql}') v USING(allele_id)
                 WHERE {conditions}
             ) TO '{partial_sql}'
             (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 100000)",
            field.index
        ))
        .map_err(|error| format!("cannot build query projection: {error}"))?;
    if !query_projection_is_valid(&partial) {
        let _ = fs::remove_file(&partial);
        return Err("query projection failed validation".into());
    }
    crate::library_metadata::publish_cache_file(&partial, destination, query_projection_is_valid)
}

fn prepare_query_projection(
    evidence: &Path,
    catalog: &Path,
    fields: &[SelectedEvidenceColumn],
) -> Result<Option<Vec<PathBuf>>, String> {
    if fields.is_empty() {
        return Ok(None);
    }
    let root = catalog
        .parent()
        .ok_or("field catalog has no result folder")?;
    let projection_files = || {
        fields
            .iter()
            .map(|field| {
                let fingerprint = query_projection_fingerprint(evidence, catalog, field)?;
                Ok(query_projection_field_path(root, &fingerprint, field.index))
            })
            .collect::<Result<Vec<_>, String>>()
    };
    let existing = projection_files()?;
    if existing.iter().all(|path| query_projection_is_valid(path)) {
        remove_legacy_query_projections(root);
        return Ok(Some(existing));
    }
    // ponytail: one process-wide build is enough; use per-result locks only if builds contend.
    let _guard = QUERY_PROJECTION_BUILD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "query projection lock failed")?;
    for field in fields {
        let fingerprint = query_projection_fingerprint(evidence, catalog, field)?;
        let destination = query_projection_field_path(root, &fingerprint, field.index);
        if !query_projection_is_valid(&destination) {
            build_query_projection(evidence, catalog, field, &destination)?;
        }
        remove_stale_query_projection_field(root, field.index, &destination);
    }
    remove_legacy_query_projections(root);
    Ok(Some(projection_files()?))
}

fn available_query_projection(
    evidence: &Path,
    catalog: &Path,
    fields: &[SelectedEvidenceColumn],
) -> Option<Vec<PathBuf>> {
    let root = catalog.parent()?;
    let paths = fields
        .iter()
        .map(|field| {
            query_projection_fingerprint(evidence, catalog, field)
                .ok()
                .map(|fingerprint| query_projection_field_path(root, &fingerprint, field.index))
        })
        .collect::<Option<Vec<_>>>()?;
    paths
        .iter()
        .all(|path| query_projection_is_valid(path))
        .then_some(paths)
}

fn remove_query_projection(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn request_query_projection_fields(
    catalog: &Path,
    request: &PageRequest,
) -> Result<Option<Vec<SelectedEvidenceColumn>>, String> {
    let indices = requested_evidence_indices(request);
    if indices.is_empty() {
        return Ok(None);
    }
    let fields = selected_evidence_columns(catalog, &indices)?
        .into_iter()
        .filter(query_projection_field_is_eligible)
        .collect::<Vec<_>>();
    Ok((!fields.is_empty()).then_some(fields))
}

fn request_requires_query_projection(
    catalog: &Path,
    request: &PageRequest,
) -> Result<bool, String> {
    let mut indices = request
        .evidence_filters
        .iter()
        .map(|filter| filter.index)
        .collect::<Vec<_>>();
    if !request.search.trim().is_empty() {
        indices.extend(request.evidence_columns.iter().copied());
    }
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
    if indices.is_empty() {
        return Ok(false);
    }
    Ok(selected_evidence_columns(catalog, &indices)?
        .iter()
        .any(query_projection_field_is_eligible))
}

pub fn query_projection_ready(
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
) -> Result<bool, String> {
    let (Some(evidence), Some(catalog)) = (evidence, catalog) else {
        return Ok(true);
    };
    if !request_requires_query_projection(catalog, request)? {
        return Ok(true);
    }
    let Some(fields) = request_query_projection_fields(catalog, request)? else {
        return Ok(true);
    };
    Ok(available_query_projection(evidence, catalog, &fields).is_some())
}

fn with_query_evidence<T>(
    evidence: Option<&Path>,
    catalog: Option<&Path>,
    request: &PageRequest,
    mut operation: impl FnMut(Option<&Path>, Option<&[PathBuf]>) -> Result<T, String>,
) -> Result<T, String> {
    let (Some(evidence), Some(catalog)) = (evidence, catalog) else {
        return operation(evidence, None);
    };
    let projection = request_query_projection_fields(catalog, request)
        .ok()
        .flatten()
        .and_then(|fields| {
            available_query_projection(evidence, catalog, &fields).or_else(|| {
                request_requires_query_projection(catalog, request)
                    .ok()
                    .filter(|required| *required)
                    .and_then(|_| {
                        prepare_query_projection(evidence, catalog, &fields)
                            .ok()
                            .flatten()
                    })
            })
        });
    let Some(projection) = projection else {
        return operation(Some(evidence), None);
    };
    with_projection_fallback(evidence, &projection, operation)
}

fn with_projection_fallback<T>(
    evidence: &Path,
    projection: &[PathBuf],
    mut operation: impl FnMut(Option<&Path>, Option<&[PathBuf]>) -> Result<T, String>,
) -> Result<T, String> {
    let marker = projection
        .first()
        .ok_or("query projection has no field files")?;
    match operation(Some(marker), Some(projection)) {
        Ok(value) => Ok(value),
        Err(_) => match operation(Some(evidence), None) {
            Ok(value) => {
                remove_query_projection(projection);
                Ok(value)
            }
            Err(error) => Err(error),
        },
    }
}

fn gene_evidence_path(catalog: &Path) -> Result<Option<PathBuf>, String> {
    let value: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid field catalog: {error}"))?;
    let Some(name) = value["geneEvidenceFile"].as_str() else {
        return Ok(None);
    };
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err("field catalog has an invalid gene evidence file".into());
    }
    let path = catalog
        .parent()
        .ok_or("field catalog has no directory")?
        .join(name);
    if !path.is_file() {
        return Err("phenotype gene evidence file is missing".into());
    }
    Ok(Some(path))
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
    prepare_sample_call_projection(parquet, request)?;
    prepare_requested_evidence_resolution(parquet, evidence, catalog, request)?;
    with_query_evidence(evidence, catalog, request, |query_evidence, query_files| {
        export_filtered_rows_with_details_once(
            parquet,
            query_evidence,
            query_files,
            catalog,
            destination,
            request,
            columns,
        )
    })
}

fn export_filtered_rows_with_details_once(
    parquet: &Path,
    evidence: Option<&Path>,
    evidence_files: Option<&[PathBuf]>,
    catalog: Option<&Path>,
    destination: &Path,
    request: &PageRequest,
    columns: &[String],
) -> Result<u64, String> {
    let filters = validated_core_page_filters(request)?;
    let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
    let (evidence_rule_sql, evidence_rule_params) =
        evidence_filter_rules_sql(evidence, evidence_files, catalog, request)?;
    let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
    let requested = export_columns(columns)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    register_report_variants(&connection, parquet)?;
    let (search_sql, mut search_params) = displayed_field_search_sql(
        &connection,
        evidence,
        evidence_files,
        catalog,
        request,
        &filters.search,
    )?;
    let where_sql = format!(
        "{CORE_PAGE_WHERE_SQL}{core_rule_sql}{evidence_rule_sql}{search_sql}{excluded_sql}"
    );
    search_params.extend(excluded_params);
    let path = parquet.to_string_lossy();
    let mut statement = connection
        .prepare(&format!(
            "SELECT chromosome, position, reference, alternate, variant_id, quality, filter,
                    gene_symbol, gene_id, transcript_id, consequence, impact, canonical, mane_select,
                    alt_index, alternate_count, format, samples_json
             FROM annocat_variants(?) v WHERE {where_sql}
             ORDER BY record_number ASC, alt_index ASC"
        ))
        .map_err(|error| format!("cannot prepare filtered row export: {error}"))?;
    let mut params = core_page_params(path.as_ref(), request, &filters);
    params.extend(core_rule_params);
    params.extend(evidence_rule_params);
    params.extend(search_params);
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
                zygosity: table_zygosity(
                    row.get::<_, Option<String>>(16)
                        .map_err(|error| error.to_string())?
                        .as_deref(),
                    &row.get::<_, String>(17)
                        .map_err(|error| error.to_string())?,
                    row.get(14).map_err(|error| error.to_string())?,
                    row.get(15).map_err(|error| error.to_string())?,
                ),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportGeneOccurrence {
    pub allele_id: String,
    pub gene_symbol: String,
    pub gene_id: String,
}

pub(crate) fn report_gene_occurrences(
    parquet: &Path,
    selected_symbols: &HashSet<String>,
    selected_gene_ids: &BTreeSet<String>,
) -> Result<Vec<ReportGeneOccurrence>, String> {
    if selected_symbols.is_empty() && selected_gene_ids.is_empty() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TEMP TABLE selected_genes(symbol VARCHAR PRIMARY KEY);
             CREATE TEMP TABLE selected_gene_ids(gene_id VARCHAR PRIMARY KEY);",
        )
        .map_err(|error| format!("cannot prepare selected result genes: {error}"))?;
    {
        let mut appender = connection
            .appender("selected_genes")
            .map_err(|error| format!("cannot prepare selected result genes: {error}"))?;
        for symbol in selected_symbols {
            appender
                .append_row([symbol.as_str()])
                .map_err(|error| format!("cannot select result gene: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot select result genes: {error}"))?;
    }
    {
        let mut appender = connection
            .appender("selected_gene_ids")
            .map_err(|error| format!("cannot prepare selected result gene identifiers: {error}"))?;
        for gene_id in selected_gene_ids {
            appender
                .append_row([gene_id.as_str()])
                .map_err(|error| format!("cannot select result gene identifier: {error}"))?;
        }
        appender
            .flush()
            .map_err(|error| format!("cannot select result gene identifiers: {error}"))?;
    }
    let consequences = parquet.with_file_name("consequences.parquet");
    let (sql, path) = if consequences.is_file() {
        (
            "SELECT allele_id, upper(trim(c.gene_symbol)) AS symbol,
                    min(coalesce(nullif(trim(c.gene_id), ''), '')) AS gene_id
             FROM read_parquet(?) c
             LEFT JOIN selected_genes s ON s.symbol=upper(trim(c.gene_symbol))
             LEFT JOIN selected_gene_ids i ON i.gene_id=upper(trim(c.gene_id))
             WHERE allele_id IS NOT NULL AND trim(allele_id) <> ''
               AND (s.symbol IS NOT NULL OR i.gene_id IS NOT NULL)
             GROUP BY allele_id, upper(trim(c.gene_symbol))
             ORDER BY allele_id, symbol",
            consequences,
        )
    } else {
        register_report_variants(&connection, parquet)?;
        (
            "SELECT allele_id, upper(trim(c.gene_symbol)) AS symbol,
                    min(coalesce(nullif(trim(c.gene_id), ''), '')) AS gene_id
             FROM annocat_variants(?) c
             LEFT JOIN selected_genes s ON s.symbol=upper(trim(c.gene_symbol))
             LEFT JOIN selected_gene_ids i ON i.gene_id=upper(trim(c.gene_id))
             WHERE allele_id IS NOT NULL AND trim(allele_id) <> ''
               AND (s.symbol IS NOT NULL OR i.gene_id IS NOT NULL)
             GROUP BY allele_id, upper(trim(c.gene_symbol))
             ORDER BY allele_id, symbol",
            parquet.to_path_buf(),
        )
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("cannot prepare result gene lookup: {error}"))?;
    let mut rows = statement
        .query(params![path.to_string_lossy().as_ref()])
        .map_err(|error| format!("cannot read result genes: {error}"))?;
    let mut genes = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot read result genes: {error}"))?
    {
        genes.push(ReportGeneOccurrence {
            allele_id: row.get(0).map_err(|error| error.to_string())?,
            gene_symbol: row.get(1).map_err(|error| error.to_string())?,
            gene_id: row.get(2).map_err(|error| error.to_string())?,
        });
    }
    Ok(genes)
}

pub(crate) fn report_gene_identities_from_occurrences(
    occurrences: &[ReportGeneOccurrence],
) -> Vec<(String, String)> {
    let mut identities = BTreeMap::<String, HashSet<String>>::new();
    for occurrence in occurrences {
        identities
            .entry(occurrence.gene_symbol.clone())
            .or_default()
            .insert(occurrence.gene_id.clone());
    }
    identities
        .into_iter()
        .filter_map(|(symbol, ids)| {
            let nonempty = ids
                .into_iter()
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>();
            (nonempty.len() == 1).then(|| (symbol, nonempty.into_iter().next().unwrap()))
        })
        .collect()
}

pub(crate) fn report_gene_identities(parquet: &Path) -> Result<Vec<(String, String)>, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Vec<(String, String)>>>>> = OnceLock::new();
    let path = parquet
        .canonicalize()
        .unwrap_or_else(|_| parquet.to_path_buf());
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "result gene cache is unavailable")?;
    if let Some(genes) = cache.get(&path).cloned() {
        return Ok((*genes).clone());
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let consequences = parquet.with_file_name("consequences.parquet");
    let (sql, source) = if consequences.is_file() {
        (
            "SELECT upper(trim(gene_symbol)) AS symbol,
                    coalesce(nullif(trim(gene_id), ''), '') AS gene_id
             FROM read_parquet(?)
             WHERE gene_symbol IS NOT NULL AND trim(gene_symbol) <> ''
             GROUP BY symbol, gene_id
             ORDER BY symbol, gene_id",
            consequences,
        )
    } else {
        register_report_variants(&connection, parquet)?;
        (
            "SELECT upper(trim(gene_symbol)) AS symbol,
                    coalesce(nullif(trim(gene_id), ''), '') AS gene_id
             FROM annocat_variants(?)
             WHERE gene_symbol IS NOT NULL AND trim(gene_symbol) <> ''
             GROUP BY symbol, gene_id
             ORDER BY symbol, gene_id",
            parquet.to_path_buf(),
        )
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("cannot prepare result gene dictionary: {error}"))?;
    let mut rows = statement
        .query(params![source.to_string_lossy().as_ref()])
        .map_err(|error| format!("cannot read result gene dictionary: {error}"))?;
    let mut identities = BTreeMap::<String, HashSet<String>>::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot read result gene dictionary: {error}"))?
    {
        identities
            .entry(row.get(0).map_err(|error| error.to_string())?)
            .or_default()
            .insert(row.get(1).map_err(|error| error.to_string())?);
    }
    let identities: Vec<(String, String)> = identities
        .into_iter()
        .filter_map(|(symbol, ids)| {
            let mut nonempty = ids.into_iter().filter(|id| !id.is_empty());
            let id = nonempty.next()?;
            nonempty.next().is_none().then_some((symbol, id))
        })
        .collect();
    let genes = Arc::new(identities);
    cache.insert(path, genes.clone());
    Ok((*genes).clone())
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
    prepare_sample_call_projection(parquet, request)?;
    prepare_requested_evidence_resolution(parquet, evidence, catalog, request)?;
    with_query_evidence(evidence, catalog, request, |query_evidence, query_files| {
        export_filtered_genes_with_details_once(
            parquet,
            query_evidence,
            query_files,
            catalog,
            destination,
            request,
        )
    })
}

fn export_filtered_genes_with_details_once(
    parquet: &Path,
    evidence: Option<&Path>,
    evidence_files: Option<&[PathBuf]>,
    catalog: Option<&Path>,
    destination: &Path,
    request: &PageRequest,
) -> Result<u64, String> {
    let filters = validated_core_page_filters(request)?;
    let (core_rule_sql, core_rule_params) = core_filter_rules_sql(request)?;
    let (evidence_rule_sql, evidence_rule_params) =
        evidence_filter_rules_sql(evidence, evidence_files, catalog, request)?;
    let (excluded_sql, excluded_params) = excluded_alleles_sql(request)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    register_report_variants(&connection, parquet)?;
    let (search_sql, mut search_params) = displayed_field_search_sql(
        &connection,
        evidence,
        evidence_files,
        catalog,
        request,
        &filters.search,
    )?;
    let where_sql = format!(
        "{CORE_PAGE_WHERE_SQL}{core_rule_sql}{evidence_rule_sql}{search_sql}{excluded_sql}"
    );
    search_params.extend(excluded_params);
    let path = parquet.to_string_lossy();
    let mut statement = connection
        .prepare(&format!(
            "SELECT DISTINCT trim(gene_symbol) AS gene
             FROM annocat_variants(?) v WHERE {where_sql}
               AND gene_symbol IS NOT NULL AND trim(gene_symbol) <> ''
             ORDER BY upper(gene), gene"
        ))
        .map_err(|error| format!("cannot prepare filtered gene export: {error}"))?;
    let mut params = core_page_params(path.as_ref(), request, &filters);
    params.extend(core_rule_params);
    params.extend(evidence_rule_params);
    params.extend(search_params);
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
    Zygosity,
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
            Self::Zygosity => "Zygosity",
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
            Self::Zygosity => row.zygosity.clone(),
            Self::VariantId => row.variant_id.clone().unwrap_or_default(),
            Self::Quality => row
                .quality
                .map(|value| value.to_string())
                .unwrap_or_default(),
            Self::Filter => row.filter.clone(),
            Self::Gene => row
                .gene
                .clone()
                .or_else(|| row.gene_id.clone())
                .or_else(|| row.transcript_id.clone())
                .unwrap_or_default(),
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
    zygosity: String,
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
            "zygosity" => Ok(ExportColumn::Zygosity),
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
    evidence_files: Option<&[PathBuf]>,
    catalog: Option<&Path>,
    index: usize,
) -> Result<EvidenceSortSpec, String> {
    let evidence = evidence.ok_or("this AnnoCAT result has no evidence table")?;
    let catalog = catalog.ok_or("this AnnoCAT result has no field catalog")?;
    let mut selected = selected_evidence_columns(catalog, &[index])?;
    let field = selected.pop().ok_or("unknown evidence sort column")?;
    let projected = is_query_projection(evidence);
    let resolved = uses_resolution_sidecar(field.resolution);
    let evidence_path = if projected {
        evidence.to_path_buf()
    } else if matches!(
        field.resolution,
        EvidenceResolutionStrategy::GeneDirect | EvidenceResolutionStrategy::AlleleGeneDirect
    ) {
        gene_evidence_path(catalog)?.ok_or("phenotype gene evidence is not ready")?
    } else if resolved {
        crate::evidence_resolution::available_path(&canonical_evidence_path(evidence))
            .ok_or("transcript evidence index is not ready")?
    } else {
        evidence.to_path_buf()
    };
    let value_expression = match (projected, field.value_type.as_str()) {
        (true, "integer" | "number") => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE))"
        }
        (true, "boolean") => "ev_sort.boolean_value",
        (true, _) if evidence_field_is_numeric(&field) => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE))"
        }
        (true, _) => {
            "coalesce(ev_sort.string_value, CAST(ev_sort.integer_value AS VARCHAR), CAST(ev_sort.number_value AS VARCHAR), CAST(ev_sort.boolean_value AS VARCHAR), ev_sort.json_value)"
        }
        (false, "integer" | "number") if resolved => "ev_sort.resolved_number",
        (false, "integer" | "number") => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE), try_cast(ev_sort.string_value AS DOUBLE))"
        }
        (false, "boolean") if resolved => "ev_sort.resolved_string",
        (false, "boolean") => "ev_sort.boolean_value",
        (false, _) if resolved && evidence_field_is_numeric(&field) => {
            "ev_sort.resolved_number"
        }
        (false, _) if resolved => "ev_sort.resolved_string",
        (false, _) if evidence_field_is_numeric(&field) => {
            "coalesce(ev_sort.number_value, CAST(ev_sort.integer_value AS DOUBLE),
                      try_cast(nullif(trim(split_part(ev_sort.string_value, ';', 1)), '.') AS DOUBLE))"
        }
        (false, _) => {
            "coalesce(ev_sort.string_value, CAST(ev_sort.integer_value AS VARCHAR), CAST(ev_sort.number_value AS VARCHAR), CAST(ev_sort.boolean_value AS VARCHAR), ev_sort.json_value)"
        }
    };
    let direct_files = (projected
        || (!resolved
            && !matches!(
                field.resolution,
                EvidenceResolutionStrategy::GeneDirect
                    | EvidenceResolutionStrategy::AlleleGeneDirect
            )))
    .then_some(evidence_files)
    .flatten();
    let (evidence_read, evidence_parameters) =
        evidence_read_for_fields(&evidence_path, direct_files, [field.index]);
    Ok(EvidenceSortSpec {
        evidence: evidence_path.to_string_lossy().into_owned(),
        evidence_read,
        evidence_parameters,
        field,
        value_expression,
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
    evidence_files: Option<&[PathBuf]>,
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
                let evidence_sort = evidence_sort_spec(evidence, evidence_files, catalog, index)?;
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
        "zygosity" => Ok(("zygosity", "zygosity_sort")),
        "gene" => Ok(("gene", "coalesce(gene_symbol, gene_id, transcript_id)")),
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

fn read_evidence_rows(
    connection: &Connection,
    evidence_parquet: &Path,
    allele_id: &str,
    consequences: &[(String, String)],
) -> Result<Vec<Value>, String> {
    let evidence_path = evidence_parquet.to_string_lossy();
    let mut statement = connection
        .prepare(
            "SELECT consequence_id, scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?)
             WHERE allele_id = ?
             ORDER BY scope, source_id, field_path, consequence_id LIMIT 6001",
        )
        .map_err(|error| format!("cannot prepare evidence detail query: {error}"))?;
    let rows = statement
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
    crate::detail_lookup::resolve_record_evidence(rows, consequences)
}

fn supplemental_evidence_rows(evidence: &Path, allele_id: &str) -> Result<Vec<Value>, String> {
    let directory = evidence
        .parent()
        .ok_or("composite evidence has no directory")?;
    let mut paths = fs::read_dir(directory)
        .map_err(|error| format!("cannot inspect supplemental evidence: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("parquet")
                && path.file_name().and_then(|name| name.to_str()) != Some("canonical.parquet")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".annocat-"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for path in paths {
        rows.extend(read_evidence_rows(&connection, &path, allele_id, &[])?);
    }
    Ok(rows)
}

fn gene_evidence_rows(catalog: &Path, gene_symbol: &str) -> Result<Vec<Value>, String> {
    let Some(evidence) = gene_evidence_path(catalog)? else {
        return Ok(Vec::new());
    };
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT scope, source_id, field_path, value_type,
                    string_value, integer_value, number_value, boolean_value, json_value
             FROM read_parquet(?)
             WHERE upper(gene_symbol)=upper(?)
             ORDER BY source_id, field_path LIMIT 101",
        )
        .map_err(|error| format!("cannot prepare gene evidence detail query: {error}"))?;
    statement
        .query_map(
            params![evidence.to_string_lossy().as_ref(), gene_symbol],
            |row| {
                let value_type = row.get::<_, String>(3)?;
                let value = match value_type.as_str() {
                    "string" => row.get::<_, Option<String>>(4)?.map(Value::String),
                    "integer" => row
                        .get::<_, Option<i64>>(5)?
                        .map(|value| Value::Number(value.into())),
                    "number" => row
                        .get::<_, Option<f64>>(6)?
                        .and_then(serde_json::Number::from_f64)
                        .map(Value::Number),
                    "boolean" => row.get::<_, Option<bool>>(7)?.map(Value::Bool),
                    "json" => row
                        .get::<_, Option<String>>(8)?
                        .map(|value| serde_json::from_str(&value).unwrap_or(Value::String(value))),
                    _ => None,
                }
                .unwrap_or(Value::Null);
                Ok(json!({
                    "consequenceId": Value::Null,
                    "scope": row.get::<_, String>(0)?,
                    "sourceId": row.get::<_, String>(1)?,
                    "fieldPath": row.get::<_, String>(2)?,
                    "valueType": value_type,
                    "value": value,
                }))
            },
        )
        .map_err(|error| format!("cannot read gene evidence details: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn append_gene_evidence(
    detail: &mut Value,
    catalog: Option<&Path>,
    gene_symbol: Option<&str>,
) -> Result<(), String> {
    let (Some(catalog), Some(gene_symbol)) =
        (catalog, gene_symbol.filter(|value| !value.is_empty()))
    else {
        return Ok(());
    };
    let rows = gene_evidence_rows(catalog, gene_symbol)?;
    if rows.is_empty() {
        return Ok(());
    }
    let object = detail
        .as_object_mut()
        .ok_or("variant detail response is not an object")?;
    let available = 5000usize.saturating_sub(
        object
            .get("evidence")
            .and_then(Value::as_array)
            .ok_or("variant detail response has no evidence array")?
            .len(),
    );
    let truncated = rows.len() > available;
    object
        .get_mut("evidence")
        .and_then(Value::as_array_mut)
        .ok_or("variant detail response has no evidence array")?
        .extend(rows.into_iter().take(available));
    if truncated {
        object.insert("evidenceTruncated".into(), Value::Bool(true));
    }
    Ok(())
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
    let evidence_rows =
        read_evidence_rows(&connection, evidence_parquet, allele_id, &consequence_rows)?;
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
        None,
        allele_id,
        None,
        None,
    )
}

pub fn complete_detail_json_at(
    variants_parquet: &Path,
    consequences_parquet: Option<&Path>,
    evidence_parquet: Option<&Path>,
    field_catalog: Option<&Path>,
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
        let canonical_evidence = canonical_evidence_path(evidence);
        if let Ok(Some(mut indexed)) = crate::detail_lookup::lookup(
            variants_parquet,
            consequences,
            &canonical_evidence,
            allele_id,
            record_number,
            alt_index,
        ) {
            let supplemental = if is_composite_evidence(evidence) {
                supplemental_evidence_rows(evidence, allele_id).ok()
            } else {
                Some(Vec::new())
            };
            if let Some(supplemental) = supplemental {
                indexed.evidence.extend(supplemental);
                apply_representative_override_to_variant(
                    variants_parquet,
                    allele_id,
                    &mut indexed.variant,
                )?;
                let embedded_consequences = indexed
                    .variant
                    .get("fallbackConsequences")
                    .and_then(Value::as_array)
                    .is_some_and(|items| !items.is_empty());
                if !indexed.consequences.is_empty() || !embedded_consequences {
                    let mut detail =
                        detail_value(allele_id, indexed.consequences, indexed.evidence);
                    detail
                        .as_object_mut()
                        .ok_or("variant detail response is not an object")?
                        .insert("variant".into(), indexed.variant);
                    let gene_symbol = detail["variant"]["geneSymbol"].as_str().map(str::to_owned);
                    append_gene_evidence(&mut detail, field_catalog, gene_symbol.as_deref())?;
                    return serialize_complete_detail(detail);
                }
            }
        }
    }
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    register_report_variants(&connection, variants_parquet)?;
    let path = variants_parquet.to_string_lossy();
    let context = connection
        .query_row(
            "SELECT record_number, chromosome, position, reference, alternate, alt_index, variant_id, quality, filter,
                    gene_symbol, gene_id, transcript_id, consequence, impact, canonical,
                    mane_select, format, samples_json, consequences_json
             FROM annocat_variants(?) WHERE allele_id = ? LIMIT 1",
            params![path.as_ref(), allele_id],
            |row| {
                let samples_json = row.get::<_, String>(17)?;
                let consequences_json = row.get::<_, String>(18)?;
                Ok(json!({
                    "recordNumber": row.get::<_, i64>(0)?,
                    "chromosome": row.get::<_, String>(1)?,
                    "position": row.get::<_, i64>(2)?,
                    "reference": row.get::<_, String>(3)?,
                    "alternate": row.get::<_, String>(4)?,
                    "altIndex": row.get::<_, i32>(5)?,
                    "variantId": row.get::<_, Option<String>>(6)?,
                    "quality": row.get::<_, Option<f64>>(7)?,
                    "filter": row.get::<_, String>(8)?,
                    "geneSymbol": row.get::<_, Option<String>>(9)?,
                    "geneId": row.get::<_, Option<String>>(10)?,
                    "transcriptId": row.get::<_, Option<String>>(11)?,
                    "consequence": row.get::<_, Option<String>>(12)?,
                    "impact": row.get::<_, Option<String>>(13)?,
                    "canonical": row.get::<_, bool>(14)?,
                    "maneSelect": row.get::<_, Option<String>>(15)?,
                    "format": row.get::<_, Option<String>>(16)?,
                    "samples": serde_json::from_str::<Value>(&samples_json).unwrap_or(Value::Array(Vec::new())),
                    "fallbackConsequences": serde_json::from_str::<Value>(&consequences_json).unwrap_or(Value::Array(Vec::new())),
                }))
            },
        )
        .map_err(|error| format!("cannot read variant details: {error}"))?;
    let context_record = context["recordNumber"]
        .as_i64()
        .ok_or("variant detail response has no record number")?;
    let context_alt = context["altIndex"]
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or("variant detail response has no ALT index")?;
    let mut context = context;
    context
        .as_object_mut()
        .ok_or("variant detail response is not an object")?
        .insert(
            "alternateCount".into(),
            variant_alternate_count(variants_parquet, context_record, context_alt)?.into(),
        );
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
    let gene_symbol = detail["variant"]["geneSymbol"].as_str().map(str::to_owned);
    append_gene_evidence(&mut detail, field_catalog, gene_symbol.as_deref())?;
    serialize_complete_detail(detail)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawSampleCall {
    name: String,
    value: String,
}

fn zygosity_from_samples_json(
    format: Option<&str>,
    samples_json: &str,
    alt_index: i32,
    alternate_count: i32,
) -> (Option<String>, Option<i32>) {
    let samples = serde_json::from_str::<Vec<RawSampleCall>>(samples_json).unwrap_or_default();
    zygosity_label_and_sort(
        samples.first().map(|sample| sample.name.as_str()),
        format,
        samples.first().map(|sample| sample.value.as_str()),
        samples.len(),
        usize::try_from(alt_index).unwrap_or_default(),
        usize::try_from(alternate_count).unwrap_or_default(),
    )
}

fn table_zygosity(
    format: Option<&str>,
    samples_json: &str,
    alt_index: i32,
    alternate_count: i32,
) -> String {
    let samples = serde_json::from_str::<Vec<RawSampleCall>>(samples_json).unwrap_or_default();
    if samples.is_empty() {
        return "Not available".into();
    }
    if samples.len() > 1 {
        return format!("{} sample calls", samples.len());
    }
    let call = annocat_core::sample_call::parse_sample_call(
        &samples[0].name,
        format,
        &samples[0].value,
        usize::try_from(alt_index).unwrap_or_default(),
        usize::try_from(alternate_count).unwrap_or_default(),
    );
    use annocat_core::sample_call::GenotypeRelation;
    match call.genotype_relation {
        GenotypeRelation::Reference => "Reference".into(),
        GenotypeRelation::OtherAlternate => "Other alternate".into(),
        GenotypeRelation::Heterozygous => "Heterozygous".into(),
        GenotypeRelation::HomozygousAlternate => "Homozygous alternate".into(),
        GenotypeRelation::HaploidAlternate => "Haploid alternate".into(),
        GenotypeRelation::MixedAlternate => {
            format!("{} of {} copies", call.selected_alt_copy_count, call.ploidy)
        }
        GenotypeRelation::PartiallyCalled => "Partially called".into(),
        GenotypeRelation::NotCalled => "Not called".into(),
        GenotypeRelation::Unavailable => "Not available".into(),
        GenotypeRelation::Invalid => "Invalid genotype".into(),
    }
}

pub(crate) fn parquet_has_column(path: &Path, name: &str) -> Result<bool, String> {
    let file =
        File::open(path).map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|error| format!("cannot inspect Parquet schema: {error}"))?;
    Ok(reader.schema().field_with_name(name).is_ok())
}

fn variant_alternate_count(
    variants_parquet: &Path,
    record_number: i64,
    alt_index: i32,
) -> Result<i32, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let path = variants_parquet.to_string_lossy();
    let alternate_count = if parquet_has_column(variants_parquet, "alternate_count")? {
        connection.query_row(
            "SELECT alternate_count FROM read_parquet(?)
             WHERE record_number = ? AND alt_index = ? LIMIT 1",
            params![path.as_ref(), record_number, alt_index],
            |row| row.get(0),
        )
    } else {
        connection.query_row(
            "SELECT max(alt_index) FROM read_parquet(?) WHERE record_number = ?",
            params![path.as_ref(), record_number],
            |row| row.get(0),
        )
    }
    .map_err(|error| format!("cannot determine record ALT count: {error}"))?;
    if alternate_count < 1 {
        return Err("variant record has an invalid ALT count".into());
    }
    Ok(alternate_count)
}

fn serialize_complete_detail(mut detail: Value) -> Result<String, String> {
    let variant = detail
        .get_mut("variant")
        .and_then(Value::as_object_mut)
        .ok_or("variant detail response has no variant context")?;
    let alt_index = variant
        .get("altIndex")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or("variant detail response has an invalid alternate-allele index")?;
    let alternate_count = variant
        .get("alternateCount")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value >= alt_index)
        .ok_or("variant detail response has an invalid alternate-allele count")?;
    let format = variant.get("format").and_then(Value::as_str);
    let samples = serde_json::from_value::<Vec<RawSampleCall>>(
        variant
            .get("samples")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("variant sample calls are invalid: {error}"))?;
    let sample_calls = samples
        .iter()
        .map(|sample| {
            annocat_core::sample_call::parse_sample_call(
                &sample.name,
                format,
                &sample.value,
                alt_index,
                alternate_count,
            )
        })
        .collect::<Vec<_>>();
    variant.insert(
        "sampleCalls".into(),
        serde_json::to_value(sample_calls).map_err(|error| error.to_string())?,
    );
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
    "alleles",
    "allele_string",
    "end",
    "id",
    "input",
    "intergenic_consequences",
    "most_severe_consequence",
    "motif_feature_consequences",
    "regulatory_feature_consequences",
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
    "feature_id",
    "feature_type",
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
    "motif_feature_id",
    "polyphen_prediction",
    "polyphen_score",
    "protein_end",
    "protein_id",
    "protein_start",
    "regulatory_feature_id",
    "sift_prediction",
    "sift_score",
    "source",
    "strand",
    "symbol_source",
    "translated_length",
    "translation_length",
    "protein_length",
    "transcript_length",
    "feature_length",
    "length",
    "transcript_id",
    "transcription_factors",
    "tsl",
    "variant_allele",
];

struct EvidenceContext<'a> {
    allele_id: &'a str,
    consequence_id: Option<&'a str>,
    scope: &'a str,
    source_id: &'a str,
}

fn merge_record_list(
    record_lists: &mut BTreeMap<(String, String), Value>,
    conflicts: &mut BTreeSet<(String, String)>,
    allele_id: &str,
    source_id: &str,
    value: &Value,
) {
    let key = (allele_id.to_owned(), source_id.to_owned());
    if conflicts.contains(&key) {
        return;
    }
    if record_lists
        .get(&key)
        .is_some_and(|previous| previous != value)
    {
        record_lists.remove(&key);
        conflicts.insert(key);
    } else {
        record_lists.entry(key).or_insert_with(|| value.clone());
    }
}

fn merge_allele_evidence(
    evidence: &mut BTreeMap<(String, String, String), Value>,
    conflicts: &mut BTreeSet<(String, String, String)>,
    allele_id: &str,
    source_id: &str,
    value: &Value,
) {
    let mut leaves = Vec::new();
    collect_evidence_leaves(value, "", &mut leaves);
    for (field_path, value) in leaves {
        let key = (allele_id.to_owned(), source_id.to_owned(), field_path);
        if conflicts.contains(&key) {
            continue;
        }
        if evidence
            .get(&key)
            .is_some_and(|previous| previous != &value)
        {
            evidence.remove(&key);
            conflicts.insert(key);
        } else {
            evidence.entry(key).or_insert(value);
        }
    }
}

fn collect_evidence_leaves(value: &Value, path: &str, leaves: &mut Vec<(String, Value)>) {
    if let Value::Object(object) = value {
        for (key, child) in object {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            collect_evidence_leaves(child, &child_path, leaves);
        }
    } else if !value.is_null() {
        leaves.push((
            if path.is_empty() { "value" } else { path }.to_owned(),
            value.clone(),
        ));
    }
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
        if path.is_empty()
            && context.source_id.eq_ignore_ascii_case("spliceai")
            && !object
                .keys()
                .any(|key| normalized_evidence_key(key) == "maxdeltascore")
            && let Some(maximum) = spliceai_maximum_delta(object)
        {
            rows += append_evidence_tree(
                batch,
                catalog,
                context,
                "maxDeltaScore",
                &Value::from(maximum),
            )?;
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
    if let Some(contract) = categorical_contract_for_field(context.source_id, field_path)? {
        collect_observed_categories(entry, contract, value);
    }
    Ok(1)
}

fn normalized_evidence_key(value: &str) -> String {
    value
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn categorical_value_key(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bundled_categorical_contracts() -> &'static Result<Vec<CategoricalContract>, String> {
    static CONTRACTS: OnceLock<Result<Vec<CategoricalContract>, String>> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        let supplementary: Value = serde_json::from_str(include_str!(
            "../../../config/supplementary-source-fields.json"
        ))
        .map_err(|error| format!("invalid categorical field contract: {error}"))?;
        let mut contracts = Vec::new();
        for source in supplementary["sources"]
            .as_array()
            .ok_or("supplementary field contract has no sources")?
        {
            let Some(source_id) = source["resourceId"].as_str() else {
                continue;
            };
            for field in source["categoricalFields"].as_array().into_iter().flatten() {
                let field_name = field["field"]
                    .as_str()
                    .ok_or("categorical field contract has no field name")?;
                let match_mode = field["matchMode"]
                    .as_str()
                    .ok_or("categorical field contract has no match mode")?;
                let parser = match field["parser"].as_str().unwrap_or("scalar") {
                    "scalar" => CategoricalParser::Scalar,
                    "json" => CategoricalParser::Json,
                    value => return Err(format!("unknown categorical parser: {value}")),
                };
                contracts.push(CategoricalContract {
                    source_id: source_id.to_owned(),
                    field_name: field_name.to_owned(),
                    match_mode: match_mode.to_owned(),
                    parser,
                    values: field["values"].as_array().cloned().unwrap_or_default(),
                    discover_observed: field["discoverObserved"].as_bool().unwrap_or(false),
                });
            }
        }

        let calibrations: Value =
            serde_json::from_str(include_str!("../../../config/evidence-calibrations.json"))
                .map_err(|error| format!("invalid categorical prediction contract: {error}"))?;
        for field in calibrations["categoricalPredictions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(source_id) = field["sourceId"].as_str() else {
                continue;
            };
            let Some(field_name) = field["fieldName"].as_str() else {
                continue;
            };
            contracts.push(CategoricalContract {
                source_id: source_id.to_owned(),
                field_name: field_name.to_owned(),
                match_mode: "scalar".into(),
                parser: CategoricalParser::Scalar,
                values: field["codes"].as_array().cloned().unwrap_or_default(),
                discover_observed: false,
            });
        }
        Ok(contracts)
    })
}

fn categorical_contract_for_field(
    source_id: &str,
    field_path: &str,
) -> Result<Option<&'static CategoricalContract>, String> {
    let contracts = bundled_categorical_contracts()
        .as_ref()
        .map_err(Clone::clone)?;
    let field_key = normalized_evidence_key(field_path.rsplit('.').next().unwrap_or(field_path));
    Ok(contracts.iter().find(|contract| {
        catalog_source_is(source_id, &contract.source_id)
            && field_key == normalized_evidence_key(&contract.field_name)
    }))
}

fn insert_observed_category(entry: &mut CatalogEntry, value: &str) {
    let value = value.trim();
    if value.is_empty() || value == "." || value.len() > MAX_CATEGORICAL_VALUE_BYTES {
        return;
    }
    let key = categorical_value_key(value);
    if key.is_empty() {
        return;
    }
    entry
        .observed_categories
        .entry(key)
        .and_modify(|existing| {
            if value < existing.as_str() {
                *existing = value.to_owned();
            }
        })
        .or_insert_with(|| value.to_owned());
    if entry.observed_categories.len() > MAX_CATEGORICAL_VALUES {
        entry.observed_categories.pop_last();
        entry.observed_categories_complete = false;
    }
}

fn collect_observed_categories(
    entry: &mut CatalogEntry,
    contract: &CategoricalContract,
    value: &Value,
) {
    if !contract.discover_observed {
        return;
    }
    match (contract.parser, value) {
        (CategoricalParser::Json, Value::Array(values)) => {
            for value in values.iter().filter_map(Value::as_str) {
                insert_observed_category(entry, value);
            }
        }
        (CategoricalParser::Scalar, Value::String(value)) => {
            insert_observed_category(entry, value);
        }
        _ => {}
    }
}

fn enrich_categorical_contracts(catalog: &mut Value) -> Result<(), String> {
    let fields = catalog["fields"]
        .as_array_mut()
        .ok_or("field catalog has no fields array")?;
    for field in fields {
        let (Some(source_id), Some(field_path)) =
            (field["sourceId"].as_str(), field["fieldPath"].as_str())
        else {
            continue;
        };
        let Some(contract) = categorical_contract_for_field(source_id, field_path)? else {
            continue;
        };
        let observed = field["categorical"].clone();
        field["categorical"] = json!({
            "matchMode": contract.match_mode,
            "values": contract.values,
            "canDiscover": contract.discover_observed,
            "observedValues": observed["observedValues"].clone(),
            "observedValuesComplete": observed["observedValuesComplete"].as_bool().unwrap_or(false),
        });
    }
    Ok(())
}

fn spliceai_maximum_delta(object: &Map<String, Value>) -> Option<f64> {
    object
        .iter()
        .filter(|(key, _)| {
            matches!(
                normalized_evidence_key(key).as_str(),
                "dsag" | "dsal" | "dsdg" | "dsdl"
            )
        })
        .filter_map(|(_, value)| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .filter(|value| (0.0..=1.0).contains(value))
        .reduce(f64::max)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceEvidenceScope {
    Allele,
    Transcript,
    Feature,
    Gene,
}

impl SourceEvidenceScope {
    fn unresolved_scope(self) -> &'static str {
        match self {
            Self::Allele => "allele",
            Self::Transcript => "unresolved_transcript",
            Self::Feature => "unresolved_feature",
            Self::Gene => "unresolved_gene",
        }
    }
}

fn source_evidence_scope(source_id: &str) -> Option<SourceEvidenceScope> {
    match annocat_core::source_catalog::source(source_id)?
        .evidence_scope
        .as_str()
    {
        "allele" => Some(SourceEvidenceScope::Allele),
        "transcript" => Some(SourceEvidenceScope::Transcript),
        "feature" => Some(SourceEvidenceScope::Feature),
        "gene" => Some(SourceEvidenceScope::Gene),
        _ => None,
    }
}

fn materialize_selected_evidence(
    evidence: &mut EvidenceBatch,
    consequences: &[(&str, Map<String, Value>)],
    selected: &HashMap<String, usize>,
) {
    let raw_rows = evidence.len();
    let mut scalar_matches =
        BTreeMap::<(String, String, String), (u8, usize, EvidenceValue, bool)>::new();
    for index in 0..raw_rows {
        if evidence.scope[index] == "selected" {
            continue;
        }
        let Some(selected_index) = selected.get(&evidence.allele_id[index]).copied() else {
            continue;
        };
        let Some(linked_index) = evidence.consequence_id[index]
            .as_deref()
            .and_then(local_consequence_index)
        else {
            continue;
        };
        let source_scope = source_evidence_scope(&evidence.source_id[index]);
        let match_rank = match source_scope {
            Some(SourceEvidenceScope::Allele) => None,
            Some(SourceEvidenceScope::Transcript) => (linked_index == selected_index).then_some(0),
            Some(SourceEvidenceScope::Gene) => {
                (linked_index == selected_index).then_some(0).or_else(|| {
                    same_gene(
                        &consequences[linked_index].1,
                        &consequences[selected_index].1,
                    )
                    .then_some(1)
                })
            }
            Some(SourceEvidenceScope::Feature)
                if annocat_core::source_catalog::feature_identity(&evidence.source_id[index])
                    == Some("gene") =>
            {
                (linked_index == selected_index).then_some(0).or_else(|| {
                    same_gene(
                        &consequences[linked_index].1,
                        &consequences[selected_index].1,
                    )
                    .then_some(1)
                })
            }
            Some(SourceEvidenceScope::Feature) | None => {
                (linked_index == selected_index).then_some(0)
            }
        };
        let Some(match_rank) = match_rank else {
            continue;
        };
        let Some(value) = evidence.value(index) else {
            continue;
        };
        let key = (
            evidence.allele_id[index].clone(),
            evidence.source_id[index].clone(),
            evidence.field_path[index].clone(),
        );
        match scalar_matches.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((match_rank, index, value, false));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get_mut();
                if match_rank < current.0 {
                    *current = (match_rank, index, value, false);
                } else if match_rank == current.0 {
                    current.3 |= current.2 != value;
                }
            }
        }
    }
    for ((allele_id, _, _), (_, index, _, conflict)) in scalar_matches {
        if !conflict {
            evidence.push_selected_copy(index, &format!("local:{}", selected[&allele_id]));
        }
    }

    let mut aligned_rows = BTreeMap::<(String, String, String, String), (usize, bool)>::new();
    for index in 0..raw_rows {
        let scope = &evidence.scope[index];
        if scope == "selected" {
            continue;
        }
        let source = &evidence.source_id[index];
        let field = &evidence.field_path[index];
        if crate::evidence_resolution::bundled_alignment_group(scope, source, field).is_some() {
            aligned_rows
                .entry((
                    evidence.allele_id[index].clone(),
                    scope.clone(),
                    source.clone(),
                    field.clone(),
                ))
                .and_modify(|(_, conflict)| *conflict = true)
                .or_insert((index, false));
        }
    }
    for ((allele_id, scope, source, field), (index, conflict)) in aligned_rows.clone() {
        if conflict {
            continue;
        }
        let Some(selected_index) = selected.get(&allele_id).copied() else {
            continue;
        };
        let Some(selected_transcript) = consequence_text(
            &consequences[selected_index].1,
            &["transcript_id", "Feature"],
        ) else {
            continue;
        };
        let Some(key_field) = crate::evidence_resolution::alignment_key_field(&scope, &source)
        else {
            continue;
        };
        let Some((key_index, false)) = aligned_rows
            .get(&(allele_id.clone(), scope.clone(), source.clone(), key_field))
            .copied()
        else {
            continue;
        };
        let Some(transcripts) = evidence.text_value(key_index) else {
            continue;
        };
        let Some(values) = evidence.text_value(index) else {
            continue;
        };
        let Some(value) = crate::evidence_resolution::select_aligned_value(
            &scope,
            &source,
            &field,
            &transcripts,
            &values,
            selected_transcript,
        ) else {
            continue;
        };
        evidence.push_selected_string(
            &allele_id,
            &format!("local:{selected_index}"),
            &source,
            &field,
            value,
        );
    }
}

fn local_consequence_index(value: &str) -> Option<usize> {
    value.strip_prefix("local:")?.parse().ok()
}

fn same_gene(left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    consequence_text(left, &["gene_id", "Gene"])
        .zip(consequence_text(right, &["gene_id", "Gene"]))
        .is_some_and(|(left, right)| left == right)
}

fn explicit_source_transcript(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    ["transcriptId", "transcript_id"]
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn explicit_source_gene(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    ["gene", "geneSymbol", "gene_symbol", "geneId", "gene_id"]
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn stable_transcript_id(value: &str) -> &str {
    value.split_once('.').map_or(value, |(stable, _)| stable)
}

fn matching_transcript_consequence(
    consequences: &[(&str, Map<String, Value>)],
    alternate: &str,
    transcript: &str,
) -> Option<usize> {
    let exact = consequences
        .iter()
        .enumerate()
        .filter(|(_, (feature_type, object))| {
            *feature_type == "transcript"
                && object.get("variant_allele").and_then(Value::as_str) == Some(alternate)
                && consequence_text(object, &["transcript_id", "Feature"]) == Some(transcript)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return exact.first().copied();
    }
    if transcript.contains('.') {
        return None;
    }
    let stable = consequences
        .iter()
        .enumerate()
        .filter(|(_, (feature_type, object))| {
            *feature_type == "transcript"
                && object.get("variant_allele").and_then(Value::as_str) == Some(alternate)
                && consequence_text(object, &["transcript_id", "Feature"])
                    .is_some_and(|candidate| stable_transcript_id(candidate) == transcript)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    (stable.len() == 1).then(|| stable[0])
}

fn matching_gene_consequence(
    consequences: &[(&str, Map<String, Value>)],
    alternate: &str,
    gene: &str,
) -> Option<usize> {
    let matches = consequences
        .iter()
        .enumerate()
        .filter(|(_, (_, object))| {
            object.get("variant_allele").and_then(Value::as_str) == Some(alternate)
                && ["gene_id", "gene_symbol", "Gene"]
                    .iter()
                    .any(|field| object.get(*field).and_then(Value::as_str) == Some(gene))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    best_structured_consequence_index(consequences, &matches)
}

#[allow(clippy::too_many_arguments)]
fn append_scoped_source_evidence(
    evidence: &mut EvidenceBatch,
    catalog: &mut BTreeMap<(String, String, String), CatalogEntry>,
    linked_evidence_written: &mut BTreeSet<(String, String, String, Option<String>)>,
    consequences: &[(&str, Map<String, Value>)],
    allele_id: &str,
    alternate: &str,
    source_id: &str,
    value: &Value,
) -> Result<(), String> {
    let declared_scope = source_evidence_scope(source_id);
    let linked_index = match declared_scope {
        Some(SourceEvidenceScope::Transcript) => {
            explicit_source_transcript(value).and_then(|transcript| {
                matching_transcript_consequence(consequences, alternate, transcript)
            })
        }
        Some(SourceEvidenceScope::Gene) => explicit_source_gene(value)
            .and_then(|gene| matching_gene_consequence(consequences, alternate, gene)),
        Some(SourceEvidenceScope::Feature)
            if annocat_core::source_catalog::feature_identity(source_id) == Some("gene") =>
        {
            explicit_source_gene(value)
                .and_then(|gene| matching_gene_consequence(consequences, alternate, gene))
        }
        Some(SourceEvidenceScope::Feature) => {
            explicit_source_transcript(value).and_then(|transcript| {
                matching_transcript_consequence(consequences, alternate, transcript)
            })
        }
        Some(SourceEvidenceScope::Allele) => None,
        None => None,
    };
    let linked_consequence = linked_index.map(|index| format!("local:{index}"));
    let scope = linked_index
        .map(|index| consequences[index].0)
        .unwrap_or_else(|| {
            declared_scope
                .map(SourceEvidenceScope::unresolved_scope)
                .unwrap_or("unresolved_feature")
        });
    if !linked_evidence_written.insert((
        allele_id.to_owned(),
        source_id.to_owned(),
        scope.to_owned(),
        linked_consequence.clone(),
    )) {
        return Ok(());
    }
    let context = EvidenceContext {
        allele_id,
        consequence_id: linked_consequence.as_deref(),
        scope,
        source_id,
    };
    append_evidence_tree(evidence, catalog, &context, "", value)?;
    Ok(())
}

fn consequence_impact(term: &str) -> &'static str {
    match term {
        "transcript_ablation"
        | "splice_acceptor_variant"
        | "splice_donor_variant"
        | "stop_gained"
        | "frameshift_variant"
        | "stop_lost"
        | "start_lost"
        | "transcript_amplification" => "HIGH",
        "inframe_insertion"
        | "inframe_deletion"
        | "missense_variant"
        | "protein_altering_variant" => "MODERATE",
        "splice_donor_5th_base_variant"
        | "splice_region_variant"
        | "splice_polypyrimidine_tract_variant"
        | "incomplete_terminal_codon_variant"
        | "start_retained_variant"
        | "stop_retained_variant"
        | "synonymous_variant" => "LOW",
        _ => "MODIFIER",
    }
}

fn consequence_text<'a>(consequence: &'a Map<String, Value>, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        consequence
            .get(*name)
            .and_then(Value::as_str)
            .and_then(valid_annotation_text)
    })
}

fn valid_annotation_text(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && !matches!(
            value.to_ascii_uppercase().as_str(),
            "." | "-" | "NA" | "N/A" | "NONE" | "NULL"
        ))
    .then_some(value)
}

fn consequence_truthy(consequence: &Map<String, Value>, names: &[&str]) -> bool {
    names.iter().any(|name| {
        consequence.get(*name).is_some_and(|value| {
            value.as_bool().unwrap_or_else(|| {
                value.as_i64().is_some_and(|number| number != 0)
                    || value.as_str().is_some_and(|text| {
                        matches!(
                            text.to_ascii_uppercase().as_str(),
                            "YES" | "Y" | "TRUE" | "1"
                        )
                    })
            })
        })
    })
}

fn valid_mane_value(value: &Value) -> bool {
    if value.as_bool() == Some(true) || value.as_i64() == Some(1) {
        return true;
    }
    let Some(value) = value.as_str().and_then(valid_annotation_text) else {
        return false;
    };
    if matches!(
        value.to_ascii_uppercase().as_str(),
        "YES" | "Y" | "TRUE" | "1"
    ) {
        return true;
    }
    let suffix = ["ENST", "NM_", "NR_", "XM_", "XR_"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix));
    let Some(suffix) = suffix else {
        return false;
    };
    let mut parts = suffix.split('.');
    parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && parts
            .next()
            .is_none_or(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && parts.next().is_none()
}

fn has_valid_mane(consequence: &Map<String, Value>, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| consequence.get(*name).is_some_and(valid_mane_value))
}

fn mane_rank(consequence: &Map<String, Value>) -> u8 {
    if has_valid_mane(consequence, &["MANE_SELECT", "mane_select", "MANE", "mane"]) {
        0
    } else if has_valid_mane(consequence, &["MANE_PLUS_CLINICAL", "mane_plus_clinical"]) {
        1
    } else {
        2
    }
}

fn appris_rank(consequence: &Map<String, Value>) -> u8 {
    let Some(value) = consequence_text(consequence, &["APPRIS", "appris"]) else {
        return 8;
    };
    let value = value.to_ascii_lowercase().replace(['_', ':'], "");
    if let Some(rank) = value
        .strip_prefix('p')
        .and_then(|value| value.parse::<u8>().ok())
    {
        return rank.saturating_sub(1).min(4);
    }
    if let Some(rank) = value
        .strip_prefix('a')
        .and_then(|value| value.parse::<u8>().ok())
    {
        return 5 + rank.saturating_sub(1).min(1);
    }
    for (prefix, offset) in [("principal", 0), ("alternative", 5)] {
        if let Some(suffix) = value.strip_prefix(prefix) {
            return suffix
                .chars()
                .find_map(|value| value.to_digit(10))
                .map(|value| offset + value.saturating_sub(1) as u8)
                .unwrap_or(offset);
        }
    }
    7
}

fn tsl_rank(consequence: &Map<String, Value>) -> u16 {
    consequence_text(consequence, &["TSL", "tsl"])
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(u16::MAX)
}

fn consequence_severity_rank(consequence: &Map<String, Value>) -> u8 {
    let terms = consequence
        .get("Consequence")
        .and_then(Value::as_str)
        .map(|value| value.split('&').collect::<Vec<_>>())
        .or_else(|| {
            consequence
                .get("consequence_terms")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
        })
        .unwrap_or_default();
    terms
        .into_iter()
        .map(|term| match term {
            "transcript_ablation" => 0,
            "splice_acceptor_variant" => 1,
            "splice_donor_variant" => 2,
            "stop_gained" => 3,
            "frameshift_variant" => 4,
            "stop_lost" => 5,
            "start_lost" => 6,
            "transcript_amplification" => 7,
            "feature_elongation" => 8,
            "feature_truncation" => 9,
            "inframe_insertion" => 10,
            "inframe_deletion" => 11,
            "missense_variant" => 12,
            "protein_altering_variant" => 13,
            "splice_region_variant" => 14,
            "splice_donor_5th_base_variant" => 15,
            "splice_donor_region_variant" => 16,
            "splice_polypyrimidine_tract_variant" => 17,
            "incomplete_terminal_codon_variant" => 18,
            "start_retained_variant" => 19,
            "stop_retained_variant" => 20,
            "synonymous_variant" => 21,
            "coding_sequence_variant" => 22,
            "mature_miRNA_variant" => 23,
            "5_prime_UTR_variant" => 24,
            "3_prime_UTR_variant" => 25,
            "non_coding_transcript_exon_variant" => 26,
            "intron_variant" => 27,
            "NMD_transcript_variant" => 28,
            "non_coding_transcript_variant" => 29,
            "coding_transcript_variant" => 30,
            "upstream_gene_variant" => 31,
            "downstream_gene_variant" => 32,
            "TFBS_ablation" => 33,
            "TFBS_amplification" => 34,
            "TF_binding_site_variant" => 35,
            "regulatory_region_ablation" => 36,
            "regulatory_region_amplification" => 37,
            "regulatory_region_variant" => 38,
            "intergenic_variant" => 39,
            "sequence_variant" => 40,
            "copy_number_change" => 41,
            "copy_number_increase" => 42,
            "copy_number_decrease" => 43,
            "short_tandem_repeat_change" => 44,
            "short_tandem_repeat_expansion" => 45,
            "short_tandem_repeat_contraction" => 46,
            "unidirectional_gene_fusion" => 47,
            "transcript_variant" => 48,
            _ => u8::MAX,
        })
        .min()
        .unwrap_or(u8::MAX)
}

fn consequence_length(consequence: &Map<String, Value>) -> i64 {
    [
        "translated_length",
        "translation_length",
        "protein_length",
        "transcript_length",
        "feature_length",
        "length",
    ]
    .iter()
    .find_map(|name| {
        consequence.get(*name).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
    })
    .unwrap_or(0)
}

fn canonical_rank(consequence: &Map<String, Value>) -> u8 {
    u8::from(!consequence_truthy(
        consequence,
        &["CANONICAL", "canonical"],
    ))
}

fn protein_coding_rank(consequence: &Map<String, Value>) -> u8 {
    u8::from(
        consequence_text(consequence, &["BIOTYPE", "biotype"])
            .is_none_or(|value| !value.eq_ignore_ascii_case("protein_coding")),
    )
}

fn ccds_rank(consequence: &Map<String, Value>) -> u8 {
    u8::from(consequence_text(consequence, &["CCDS", "ccds"]).is_none())
}

fn optional_stable_id<'a>(consequence: &'a Map<String, Value>, names: &[&str]) -> (u8, &'a str) {
    consequence_text(consequence, names)
        .map(stable_transcript_id)
        .map_or((1, ""), |value| (0, value))
}

#[derive(Clone, Copy)]
struct ConsequenceCandidate<'a> {
    index: usize,
    ordinal: usize,
    feature_type: &'static str,
    consequence: &'a Map<String, Value>,
}

fn compare_gene_fallback(
    left: &ConsequenceCandidate<'_>,
    right: &ConsequenceCandidate<'_>,
) -> Ordering {
    canonical_rank(left.consequence)
        .cmp(&canonical_rank(right.consequence))
        .then_with(|| appris_rank(left.consequence).cmp(&appris_rank(right.consequence)))
        .then_with(|| tsl_rank(left.consequence).cmp(&tsl_rank(right.consequence)))
        .then_with(|| {
            protein_coding_rank(left.consequence).cmp(&protein_coding_rank(right.consequence))
        })
        .then_with(|| ccds_rank(left.consequence).cmp(&ccds_rank(right.consequence)))
        .then_with(|| {
            consequence_severity_rank(left.consequence)
                .cmp(&consequence_severity_rank(right.consequence))
        })
        .then_with(|| {
            consequence_length(right.consequence).cmp(&consequence_length(left.consequence))
        })
        .then_with(|| {
            optional_stable_id(
                left.consequence,
                &[
                    "Feature",
                    "transcript_id",
                    "feature_id",
                    "regulatory_feature_id",
                    "motif_feature_id",
                ],
            )
            .cmp(&optional_stable_id(
                right.consequence,
                &[
                    "Feature",
                    "transcript_id",
                    "feature_id",
                    "regulatory_feature_id",
                    "motif_feature_id",
                ],
            ))
        })
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

fn compare_gene_representatives(
    left: &ConsequenceCandidate<'_>,
    right: &ConsequenceCandidate<'_>,
) -> Ordering {
    mane_rank(left.consequence)
        .cmp(&mane_rank(right.consequence))
        .then_with(|| compare_gene_fallback(left, right))
}

fn normalized_feature_type(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "transcript" => "transcript",
        "regulatoryfeature" | "regulatory" => "regulatory",
        "motiffeature" | "motif" => "motif",
        "intergenic" => "intergenic",
        _ => "unresolved",
    }
}

fn consequence_feature_type(
    consequence: &Map<String, Value>,
    declared: Option<&str>,
) -> &'static str {
    let feature_type = declared
        .or_else(|| consequence_text(consequence, &["Feature_type", "feature_type"]))
        .map(normalized_feature_type)
        .unwrap_or("unresolved");
    if feature_type == "unresolved"
        && consequence_text(consequence, &["Feature", "transcript_id"]).is_some()
    {
        "transcript"
    } else {
        feature_type
    }
}

fn normalized_gene_symbol(consequence: &Map<String, Value>) -> Option<String> {
    consequence_text(consequence, &["SYMBOL", "gene_symbol"])
        .map(|value| value.to_ascii_uppercase())
}

fn consequence_group_key(
    candidate: &ConsequenceCandidate<'_>,
    symbol_gene_ids: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    if candidate.feature_type == "transcript" {
        if let Some(gene_id) =
            consequence_text(candidate.consequence, &["Gene", "gene_id"]).map(stable_transcript_id)
        {
            return format!("gene:{gene_id}");
        }
        if let Some(symbol) = normalized_gene_symbol(candidate.consequence) {
            if let Some(ids) = symbol_gene_ids.get(&symbol)
                && ids.len() == 1
            {
                return format!("gene:{}", ids.first().expect("one gene ID"));
            }
            if symbol_gene_ids.get(&symbol).is_none_or(BTreeSet::is_empty) {
                return format!("symbol:{symbol}");
            }
        }
        return format!("unresolved-transcript:{}", candidate.ordinal);
    }
    let feature_id = consequence_text(
        candidate.consequence,
        &[
            "Feature",
            "feature_id",
            "regulatory_feature_id",
            "motif_feature_id",
        ],
    )
    .map(stable_transcript_id);
    feature_id.map_or_else(
        || {
            format!(
                "unresolved-feature:{}:{}",
                candidate.feature_type, candidate.ordinal
            )
        },
        |feature_id| format!("feature:{}:{feature_id}", candidate.feature_type),
    )
}

fn compare_allele_candidates(
    left: &ConsequenceCandidate<'_>,
    right: &ConsequenceCandidate<'_>,
) -> Ordering {
    consequence_severity_rank(left.consequence)
        .cmp(&consequence_severity_rank(right.consequence))
        .then_with(|| mane_rank(left.consequence).cmp(&mane_rank(right.consequence)))
        .then_with(|| canonical_rank(left.consequence).cmp(&canonical_rank(right.consequence)))
        .then_with(|| appris_rank(left.consequence).cmp(&appris_rank(right.consequence)))
        .then_with(|| tsl_rank(left.consequence).cmp(&tsl_rank(right.consequence)))
        .then_with(|| {
            protein_coding_rank(left.consequence).cmp(&protein_coding_rank(right.consequence))
        })
        .then_with(|| ccds_rank(left.consequence).cmp(&ccds_rank(right.consequence)))
        .then_with(|| {
            consequence_length(right.consequence).cmp(&consequence_length(left.consequence))
        })
        .then_with(|| {
            optional_stable_id(left.consequence, &["Gene", "gene_id"])
                .cmp(&optional_stable_id(right.consequence, &["Gene", "gene_id"]))
        })
        .then_with(|| {
            optional_stable_id(
                left.consequence,
                &[
                    "Feature",
                    "transcript_id",
                    "feature_id",
                    "regulatory_feature_id",
                    "motif_feature_id",
                ],
            )
            .cmp(&optional_stable_id(
                right.consequence,
                &[
                    "Feature",
                    "transcript_id",
                    "feature_id",
                    "regulatory_feature_id",
                    "motif_feature_id",
                ],
            ))
        })
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

fn select_representative(candidates: &[ConsequenceCandidate<'_>]) -> Option<usize> {
    selected_gene_candidates(candidates)
        .iter()
        .min_by(|left, right| compare_allele_candidates(left, right))
        .map(|candidate| candidate.index)
}

fn selected_gene_candidates<'a>(
    candidates: &'a [ConsequenceCandidate<'a>],
) -> Vec<ConsequenceCandidate<'a>> {
    let mut symbol_gene_ids = BTreeMap::<String, BTreeSet<String>>::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.feature_type == "transcript")
    {
        if let (Some(symbol), Some(gene_id)) = (
            normalized_gene_symbol(candidate.consequence),
            consequence_text(candidate.consequence, &["Gene", "gene_id"]).map(stable_transcript_id),
        ) {
            symbol_gene_ids
                .entry(symbol)
                .or_default()
                .insert(gene_id.to_owned());
        }
    }
    let mut groups = BTreeMap::<String, Vec<ConsequenceCandidate<'_>>>::new();
    for candidate in candidates {
        groups
            .entry(consequence_group_key(candidate, &symbol_gene_ids))
            .or_default()
            .push(*candidate);
    }
    let mut gene_candidates = Vec::with_capacity(groups.len());
    for group in groups.values() {
        if let Some(candidate) = group.iter().copied().min_by(compare_gene_representatives) {
            gene_candidates.push(candidate);
        }
    }
    gene_candidates
}

fn best_structured_consequence_index(
    entries: &[(&str, Map<String, Value>)],
    indices: &[usize],
) -> Option<usize> {
    let candidates = indices
        .iter()
        .copied()
        .map(|index| ConsequenceCandidate {
            index,
            ordinal: index,
            feature_type: consequence_feature_type(&entries[index].1, Some(entries[index].0)),
            consequence: &entries[index].1,
        })
        .collect::<Vec<_>>();
    select_representative(&candidates)
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
    let candidates = entries
        .iter()
        .enumerate()
        .map(|(index, consequence)| ConsequenceCandidate {
            index,
            ordinal: index,
            feature_type: consequence_feature_type(consequence, None),
            consequence,
        })
        .collect::<Vec<_>>();
    select_representative(&candidates).and_then(|index| entries.get(index))
}

fn representative_fields(
    feature_type: &str,
    consequence: &Map<String, Value>,
) -> RepresentativeFields {
    let consequence_term = consequence
        .get("consequence_terms")
        .and_then(Value::as_array)
        .and_then(|terms| terms.first())
        .and_then(Value::as_str)
        .or_else(|| {
            consequence_text(consequence, &["primary_consequence", "Consequence"])
                .and_then(|value| value.split('&').next())
        })
        .map(str::to_owned);
    let feature_type = consequence_feature_type(consequence, Some(feature_type));
    let transcript_id = if feature_type == "transcript" {
        consequence_text(consequence, &["transcript_id", "Feature"])
    } else {
        consequence_text(
            consequence,
            &[
                "feature_id",
                "regulatory_feature_id",
                "motif_feature_id",
                "Feature",
            ],
        )
    }
    .map(str::to_owned);
    let mane_select = has_valid_mane(consequence, &["MANE_SELECT", "mane_select", "MANE", "mane"])
        .then(|| consequence_text(consequence, &["MANE_SELECT", "mane_select", "MANE", "mane"]))
        .flatten()
        .map(str::to_owned);
    RepresentativeFields {
        gene_symbol: consequence_text(consequence, &["SYMBOL", "gene_symbol"]).map(str::to_owned),
        gene_id: consequence_text(consequence, &["Gene", "gene_id"]).map(str::to_owned),
        transcript_id,
        impact: consequence_text(consequence, &["IMPACT", "impact"])
            .map(str::to_owned)
            .or_else(|| {
                consequence_term
                    .as_deref()
                    .map(consequence_impact)
                    .map(str::to_owned)
            }),
        consequence: consequence_term,
        canonical: consequence_truthy(consequence, &["CANONICAL", "canonical"]),
        mane_select,
    }
}

fn flush_representative_override(
    fingerprint: &str,
    allele_id: &str,
    entries: &[(String, Map<String, Value>)],
    existing: &RepresentativeFields,
    output: &mut RepresentativeOverrideBatch,
) {
    let candidates = entries
        .iter()
        .enumerate()
        .map(
            |(index, (feature_type, consequence))| ConsequenceCandidate {
                index,
                ordinal: index,
                feature_type: consequence_feature_type(consequence, Some(feature_type)),
                consequence,
            },
        )
        .collect::<Vec<_>>();
    let Some(selected) = select_representative(&candidates) else {
        return;
    };
    let fields = representative_fields(&entries[selected].0, &entries[selected].1);
    if &fields != existing {
        output.push(fingerprint, allele_id, fields);
    }
}

fn representative_override_fingerprint(
    variants: &Path,
    consequences: &Path,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(REPRESENTATIVE_SELECTION_CONTRACT.as_bytes());
    for path in [variants, consequences] {
        let metadata = fs::metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        digest.update(metadata.len().to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize())[..16].to_owned())
}

fn representative_override_is_valid(path: &Path, fingerprint: &str) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(reader) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    for name in [
        "schema_version",
        "input_fingerprint",
        "selection_contract",
        "allele_id",
        "gene_symbol",
        "gene_id",
        "transcript_id",
        "consequence",
        "impact",
        "canonical",
        "mane_select",
    ] {
        if reader.schema().index_of(name).is_err() {
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
                      AND selection_contract=?
                    ), true)
             FROM read_parquet(?)",
            params![
                SCHEMA_VERSION,
                fingerprint,
                REPRESENTATIVE_SELECTION_CONTRACT,
                path.to_string_lossy().as_ref()
            ],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

fn build_representative_override(
    variants: &Path,
    consequences: &Path,
    fingerprint: &str,
    destination: &Path,
) -> Result<(), String> {
    let partial = crate::library_metadata::unique_temporary_path(destination)?;
    let temporary =
        crate::library_metadata::unique_temporary_path(&destination.with_extension("duckdb-tmp"))?;
    fs::create_dir_all(&temporary)
        .map_err(|error| format!("cannot create representative cache workspace: {error}"))?;
    let _temporary = TemporaryDirectory(temporary.clone());
    let temporary = temporary.to_string_lossy().replace('\'', "''");
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(&format!(
            "SET temp_directory='{temporary}';
             SET threads=1;
             SET preserve_insertion_order=false;
             SET memory_limit='1GB';"
        ))
        .map_err(|error| format!("cannot configure representative repair: {error}"))?;
    let consequences_path = consequences.to_string_lossy().into_owned();
    let has_feature_type = connection
        .query_row(
            "SELECT count(*) > 0 FROM parquet_schema(?) WHERE name='feature_type'",
            params![consequences_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("cannot inspect legacy consequence schema: {error}"))?;
    let feature_type = if has_feature_type {
        "c.feature_type"
    } else {
        "NULL::VARCHAR"
    };
    let mut statement = connection
        .prepare(&format!(
            "WITH repair_alleles AS (
                 SELECT allele_id
                 FROM read_parquet(?)
                 GROUP BY allele_id
                 HAVING count(*) > 1
             )
             SELECT c.allele_id, {feature_type}, c.consequence_json,
                    v.gene_symbol, v.gene_id, v.transcript_id, v.consequence,
                    v.impact, v.canonical, v.mane_select
             FROM read_parquet(?) c
             JOIN repair_alleles USING (allele_id)
             JOIN read_parquet(?) v USING (allele_id)
             ORDER BY c.allele_id, c.ordinal"
        ))
        .map_err(|error| format!("cannot prepare representative repair: {error}"))?;
    let mut rows = statement
        .query(params![
            consequences.to_string_lossy().as_ref(),
            consequences.to_string_lossy().as_ref(),
            variants.to_string_lossy().as_ref()
        ])
        .map_err(|error| format!("cannot read representative repair inputs: {error}"))?;
    let schema = RepresentativeOverrideBatch::default()
        .into_record_batch()?
        .schema();
    let mut writer = parquet_writer(&partial, schema)?;
    let mut batch = RepresentativeOverrideBatch::default();
    let mut current_allele = None::<String>;
    let mut current_entries = Vec::<(String, Map<String, Value>)>::new();
    let mut current_existing = RepresentativeFields::default();
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("cannot stream representative repair: {error}"))?
    {
        let allele_id = row.get::<_, String>(0).map_err(|error| error.to_string())?;
        if current_allele
            .as_deref()
            .is_some_and(|current| current != allele_id)
        {
            flush_representative_override(
                fingerprint,
                current_allele.as_deref().expect("current allele exists"),
                &current_entries,
                &current_existing,
                &mut batch,
            );
            current_entries.clear();
            if batch.len() >= 4_096 {
                let record_batch = std::mem::take(&mut batch).into_record_batch()?;
                write_batch(&mut writer, record_batch, "legacy representative overrides")?;
            }
        }
        if current_allele.as_deref() != Some(&allele_id) {
            current_allele = Some(allele_id.clone());
            current_existing = RepresentativeFields {
                gene_symbol: row.get(3).map_err(|error| error.to_string())?,
                gene_id: row.get(4).map_err(|error| error.to_string())?,
                transcript_id: row.get(5).map_err(|error| error.to_string())?,
                consequence: row.get(6).map_err(|error| error.to_string())?,
                impact: row.get(7).map_err(|error| error.to_string())?,
                canonical: row.get(8).map_err(|error| error.to_string())?,
                mane_select: row.get(9).map_err(|error| error.to_string())?,
            };
        }
        let feature_type = row
            .get::<_, Option<String>>(1)
            .map_err(|error| error.to_string())?;
        let consequence_json = row.get::<_, String>(2).map_err(|error| error.to_string())?;
        let consequence = serde_json::from_str::<Map<String, Value>>(&consequence_json)
            .map_err(|error| format!("legacy consequence JSON is invalid: {error}"))?;
        let feature_type =
            consequence_feature_type(&consequence, feature_type.as_deref()).to_owned();
        current_entries.push((feature_type, consequence));
    }
    if let Some(allele_id) = current_allele {
        flush_representative_override(
            fingerprint,
            &allele_id,
            &current_entries,
            &current_existing,
            &mut batch,
        );
    }
    let record_batch = batch.into_record_batch()?;
    write_batch(&mut writer, record_batch, "legacy representative overrides")?;
    writer
        .close()
        .map_err(|error| format!("cannot finish representative repair: {error}"))?;
    if !representative_override_is_valid(&partial, fingerprint) {
        let _ = fs::remove_file(&partial);
        return Err("representative repair failed validation".into());
    }
    crate::library_metadata::publish_cache_file(&partial, destination, |path| {
        representative_override_is_valid(path, fingerprint)
    })
}

pub(crate) fn legacy_representative_override(variants: &Path) -> Result<Option<PathBuf>, String> {
    if report_uses_current_selection_contract(variants)? {
        return Ok(None);
    }
    let root = variants
        .parent()
        .ok_or("variant table has no result folder")?;
    let consequences = root.join("consequences.parquet");
    if !consequences.is_file() {
        return Ok(None);
    }
    let fingerprint = representative_override_fingerprint(variants, &consequences)?;
    let destination = root.join(format!(".annocat-representatives-{fingerprint}.parquet"));
    if representative_override_is_valid(&destination, &fingerprint) {
        return Ok(Some(destination));
    }
    let _guard = REPRESENTATIVE_OVERRIDE_BUILD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "representative repair lock failed")?;
    if !representative_override_is_valid(&destination, &fingerprint) {
        build_representative_override(variants, &consequences, &fingerprint, &destination)?;
    }
    Ok(Some(destination))
}

fn report_uses_current_selection_contract(report_file: &Path) -> Result<bool, String> {
    let root = report_file
        .parent()
        .ok_or("result file has no result folder")?;
    let manifest = root.join("manifest.json");
    if !manifest.is_file() {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(
        &fs::read(&manifest).map_err(|error| format!("cannot read result manifest: {error}"))?,
    )
    .map_err(|error| format!("result manifest is invalid: {error}"))?;
    match value["representativeSelectionContract"].as_str() {
        Some(REPRESENTATIVE_SELECTION_CONTRACT) => Ok(true),
        Some(_) => Err("result uses an unsupported representative-selection contract".into()),
        None => Ok(false),
    }
}

pub(crate) fn resolved_variants_relation(variants: &Path) -> Result<String, String> {
    let overrides = legacy_representative_override(variants)?;
    let has_alternate_count = parquet_has_column(variants, "alternate_count")?;
    let variants = format!("'{}'", variants.to_string_lossy().replace('\'', "''"));
    let variants = if has_alternate_count {
        format!("read_parquet({variants})")
    } else {
        format!(
            "(SELECT v.*, max(alt_index) OVER (PARTITION BY record_number) AS alternate_count
              FROM read_parquet({variants}) v)"
        )
    };
    let Some(overrides) = overrides else {
        return Ok(variants);
    };
    let overrides = format!("'{}'", overrides.to_string_lossy().replace('\'', "''"));
    Ok(format!(
        "(SELECT v.* REPLACE (
            CASE WHEN o.allele_id IS NULL THEN v.gene_symbol ELSE o.gene_symbol END AS gene_symbol,
            CASE WHEN o.allele_id IS NULL THEN v.gene_id ELSE o.gene_id END AS gene_id,
            CASE WHEN o.allele_id IS NULL THEN v.transcript_id ELSE o.transcript_id END AS transcript_id,
            CASE WHEN o.allele_id IS NULL THEN v.consequence ELSE o.consequence END AS consequence,
            CASE WHEN o.allele_id IS NULL THEN v.impact ELSE o.impact END AS impact,
            CASE WHEN o.allele_id IS NULL THEN v.canonical ELSE o.canonical END AS canonical,
            CASE WHEN o.allele_id IS NULL THEN v.mane_select ELSE o.mane_select END AS mane_select
          )
          FROM {variants} v
          LEFT JOIN read_parquet({overrides}) o USING (allele_id))"
    ))
}

fn register_sample_call_macros(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE OR REPLACE TEMP MACRO annocat_sample_gt(format_value, samples_value) AS (
               CASE
                 WHEN format_value IS NULL
                   OR coalesce(json_array_length(try_cast(samples_value AS JSON)), 0) <> 1
                 THEN NULL
                 ELSE list_extract(
                   string_split(
                     json_extract_string(try_cast(samples_value AS JSON), '$[0].value'), ':'
                   ),
                   list_position(string_split(format_value, ':'), 'GT')
                 )
               END
             );
             CREATE OR REPLACE TEMP MACRO annocat_gt_alleles(format_value, samples_value) AS (
               string_split(replace(annocat_sample_gt(format_value, samples_value), '|', '/'), '/')
             );
             CREATE OR REPLACE TEMP MACRO annocat_zygosity_sort(
               format_value, samples_value, alt_index_value, alternate_count_value
             ) AS (
               CASE
                 WHEN annocat_sample_gt(format_value, samples_value) IS NULL
                   OR NOT regexp_full_match(
                     annocat_sample_gt(format_value, samples_value),
                     '(\\.|[0-9]+)([|/](\\.|[0-9]+))*'
                   )
                   OR alt_index_value < 1
                   OR alt_index_value > alternate_count_value
                   OR coalesce(list_max(list_transform(
                     list_filter(
                       annocat_gt_alleles(format_value, samples_value),
                       allele -> allele <> '.'
                     ),
                     allele -> try_cast(allele AS INTEGER)
                   )), 0) > alternate_count_value
                 THEN NULL
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele <> '.'
                 )) = 0
                 THEN NULL
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele = '.'
                 )) > 0
                 THEN NULL
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele = cast(alt_index_value AS VARCHAR)
                 )) = 0
                 THEN CASE
                   WHEN list_count(list_filter(
                     annocat_gt_alleles(format_value, samples_value),
                     allele -> allele = '0'
                   )) = list_count(annocat_gt_alleles(format_value, samples_value))
                   THEN 0
                   ELSE 1
                 END
                 WHEN list_count(annocat_gt_alleles(format_value, samples_value)) = 1
                 THEN 3
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele = cast(alt_index_value AS VARCHAR)
                 )) = list_count(annocat_gt_alleles(format_value, samples_value))
                 THEN 5
                 WHEN list_count(annocat_gt_alleles(format_value, samples_value)) = 2
                 THEN 2
                 ELSE 4
               END
             );
             CREATE OR REPLACE TEMP MACRO annocat_zygosity_label(
               format_value, samples_value, alt_index_value, alternate_count_value
             ) AS (
               CASE
                 WHEN coalesce(json_array_length(try_cast(samples_value AS JSON)), 0) = 0
                 THEN NULL
                 WHEN json_array_length(try_cast(samples_value AS JSON)) > 1
                 THEN 'Multiple sample calls'
                 WHEN annocat_sample_gt(format_value, samples_value) IS NULL
                 THEN NULL
                 WHEN NOT regexp_full_match(
                   annocat_sample_gt(format_value, samples_value),
                   '(\\.|[0-9]+)([|/](\\.|[0-9]+))*'
                 )
                   OR alt_index_value < 1
                   OR alt_index_value > alternate_count_value
                   OR coalesce(list_max(list_transform(
                     list_filter(
                       annocat_gt_alleles(format_value, samples_value),
                       allele -> allele <> '.'
                     ),
                     allele -> try_cast(allele AS INTEGER)
                   )), 0) > alternate_count_value
                 THEN 'Invalid genotype'
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele <> '.'
                 )) = 0
                 THEN 'Not called'
                 WHEN list_count(list_filter(
                   annocat_gt_alleles(format_value, samples_value),
                   allele -> allele = '.'
                 )) > 0
                 THEN 'Partially called'
                 ELSE CASE annocat_zygosity_sort(
                   format_value, samples_value, alt_index_value, alternate_count_value
                 )
                   WHEN 0 THEN 'Reference'
                   WHEN 1 THEN 'Other alternate'
                   WHEN 2 THEN 'Heterozygous'
                   WHEN 3 THEN 'Haploid alternate'
                   WHEN 4 THEN 'Mixed alternate'
                   WHEN 5 THEN 'Homozygous alternate'
                 END
               END
             )",
        )
        .map_err(|error| format!("cannot prepare sample-call result helpers: {error}"))
}

fn register_report_variants(connection: &Connection, variants: &Path) -> Result<(), String> {
    register_sample_call_macros(connection)?;
    let mut relation = resolved_variants_relation(variants)?;
    let has_stored_zygosity =
        parquet_has_column(variants, "zygosity")? && parquet_has_column(variants, "zygosity_sort")?;
    if !has_stored_zygosity {
        relation = if let Some(projection) = available_sample_call_projection(variants) {
            let projection = projection.to_string_lossy().replace('\'', "''");
            format!(
                "(SELECT v.*, calls.zygosity, calls.zygosity_sort
                  FROM {relation} v
                  LEFT JOIN read_parquet('{projection}') calls
                    USING(record_number, alt_index))"
            )
        } else {
            format!(
                "(SELECT v.*,
                         annocat_zygosity_label(
                           v.format, v.samples_json, v.alt_index, v.alternate_count
                         ) AS zygosity,
                         annocat_zygosity_sort(
                           v.format, v.samples_json, v.alt_index, v.alternate_count
                         ) AS zygosity_sort
                  FROM {relation} v)"
            )
        };
    }
    connection
        .execute_batch(&format!(
            "CREATE OR REPLACE TEMP MACRO annocat_variants(path) AS TABLE
             SELECT * FROM {relation} WHERE path IS NOT NULL"
        ))
        .map_err(|error| format!("cannot prepare representative result view: {error}"))
}

fn apply_representative_override_to_variant(
    variants: &Path,
    allele_id: &str,
    variant: &mut Value,
) -> Result<(), String> {
    let relation = resolved_variants_relation(variants)?;
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT gene_symbol, gene_id, transcript_id, consequence, impact, canonical,
                    mane_select
             FROM {relation} WHERE allele_id=? LIMIT 1"
        ))
        .map_err(|error| format!("cannot prepare representative detail lookup: {error}"))?;
    let fields = statement
        .query_row(params![allele_id], |row| {
            Ok(RepresentativeFields {
                gene_symbol: row.get(0)?,
                gene_id: row.get(1)?,
                transcript_id: row.get(2)?,
                consequence: row.get(3)?,
                impact: row.get(4)?,
                canonical: row.get(5)?,
                mane_select: row.get(6)?,
            })
        })
        .map_err(|error| format!("cannot read representative detail: {error}"))?;
    let object = variant
        .as_object_mut()
        .ok_or("variant detail context is not an object")?;
    for (name, value) in [
        ("geneSymbol", fields.gene_symbol),
        ("geneId", fields.gene_id),
        ("transcriptId", fields.transcript_id),
        ("consequence", fields.consequence),
        ("impact", fields.impact),
        ("maneSelect", fields.mane_select),
    ] {
        object.insert(name.into(), value.map_or(Value::Null, Value::String));
    }
    object.insert("canonical".into(), Value::Bool(fields.canonical));
    Ok(())
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

fn canonical_allele_id(allele: &CanonicalAllele) -> String {
    allele_id(
        &allele.chromosome,
        i64::try_from(allele.position).expect("canonical allele positions fit in i64"),
        &allele.reference,
        &allele.alternate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn categorical_filters_preserve_exact_values_and_missing_semantics() {
        let selection = categorical_selection(
            &Some(vec![
                " Pathogenic ".into(),
                "pathogenic".into(),
                "A, B".into(),
            ]),
            Some(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selection.values, ["A, B", "Pathogenic"]);
        assert!(!selection.include_missing);

        let (matched, parameters) =
            normalized_categorical_match_sql("value", FilterValueKind::Json, &selection.values);
        assert_eq!(
            parameters,
            [
                SqlValue::Text("a, b".into()),
                SqlValue::Text("pathogenic".into())
            ]
        );
        let present = categorical_present_sql("value", FilterValueKind::Json);
        let condition = categorical_condition_sql(&present, &matched, "in", false).unwrap();
        let connection = Connection::open_in_memory().unwrap();
        let query = format!(
            "SELECT value FROM (VALUES ('[\"Pathogenic\"]'), ('[\"Uncertain_pathogenicity\"]'),
             ('[\"A, B\"]'), (NULL)) categories(value) WHERE {condition} ORDER BY value"
        );
        let mut statement = connection.prepare(&query).unwrap();
        let matches = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(matches, ["[\"A, B\"]", "[\"Pathogenic\"]"]);
        assert_eq!(
            categorical_condition_sql("present", "matched", "not_in", true).unwrap(),
            "NOT (matched)"
        );

        let contract = CategoricalContract {
            source_id: "clinvar".into(),
            field_name: "significance".into(),
            match_mode: "set".into(),
            parser: CategoricalParser::Json,
            values: Vec::new(),
            discover_observed: true,
        };
        let mut entry = CatalogEntry::default();
        collect_observed_categories(
            &mut entry,
            &contract,
            &serde_json::json!(["Pathogenic", "pathogenic", "Likely_pathogenic"]),
        );
        assert_eq!(
            entry.observed_categories.into_values().collect::<Vec<_>>(),
            ["Likely_pathogenic", "Pathogenic"]
        );
    }

    #[test]
    fn projected_field_reads_do_not_open_unrelated_sidecars() {
        let files = vec![
            PathBuf::from(".annocat-query-v3-3-a.parquet"),
            PathBuf::from(".annocat-query-v3-8-b.parquet"),
        ];
        let (sql, parameters) = evidence_read_for_fields(&files[0], Some(&files), [8]);
        assert!(sql.contains(".annocat-query-v3-8-b.parquet"));
        assert!(!sql.contains(".annocat-query-v3-3-a.parquet"));
        assert!(parameters.is_empty());
    }

    #[test]
    fn matched_row_cache_keys_ignore_sort_but_not_search() {
        let root = std::env::temp_dir().join(format!(
            "annocat-match-cache-key-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let variants = root.join("variants.parquet");
        fs::write(&variants, b"test").unwrap();
        let mut request = PageRequest {
            search: "stop".into(),
            sort: "position".into(),
            direction: "asc".into(),
            exact_total: true,
            query_session: "count".into(),
            request_generation: 1,
            ..PageRequest::default()
        };
        let filters = validated_core_page_filters(&request).unwrap();
        let first = matched_row_cache_key(
            &PageQuery {
                variants: &variants,
                evidence: None,
                evidence_files: None,
                catalog: None,
                offset: 0,
                limit: 200,
                request: &request,
                candidate_ids: None,
            },
            &filters,
        )
        .unwrap();
        request.sort = "gene".into();
        request.direction = "desc".into();
        request.exact_total = false;
        request.known_total = Some(1_230);
        request.query_session = "page".into();
        request.request_generation = 2;
        let filters = validated_core_page_filters(&request).unwrap();
        let second = matched_row_cache_key(
            &PageQuery {
                variants: &variants,
                evidence: None,
                evidence_files: None,
                catalog: None,
                offset: 0,
                limit: 200,
                request: &request,
                candidate_ids: None,
            },
            &filters,
        )
        .unwrap();
        assert_eq!(first, second);
        request.search = "frameshift".into();
        let filters = validated_core_page_filters(&request).unwrap();
        let changed = matched_row_cache_key(
            &PageQuery {
                variants: &variants,
                evidence: None,
                evidence_files: None,
                catalog: None,
                offset: 0,
                limit: 200,
                request: &request,
                candidate_ids: None,
            },
            &filters,
        )
        .unwrap();
        assert_ne!(second, changed);

        let evidence_dir = root.join("query-evidence");
        fs::create_dir_all(&evidence_dir).unwrap();
        fs::write(evidence_dir.join("core.parquet"), b"test").unwrap();
        let evidence_glob = evidence_dir.join("*.parquet");
        request.search = "stop".into();
        let filters = validated_core_page_filters(&request).unwrap();
        assert!(
            matched_row_cache_key(
                &PageQuery {
                    variants: &variants,
                    evidence: Some(&evidence_glob),
                    evidence_files: None,
                    catalog: None,
                    offset: 0,
                    limit: 200,
                    request: &request,
                    candidate_ids: None,
                },
                &filters,
            )
            .unwrap()
            .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gene_operations_fall_back_to_stable_identifiers() {
        assert_eq!(
            core_filter_column("gene").unwrap().0,
            "coalesce(v.gene_symbol, v.gene_id, v.transcript_id)"
        );
        assert_eq!(
            page_sort_expression("gene").unwrap().1,
            "coalesce(gene_symbol, gene_id, transcript_id)"
        );
    }

    #[test]
    fn gene_occurrences_materialize_only_selected_identities() {
        let root = std::env::temp_dir().join(format!(
            "annocat-selected-gene-occurrences-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let consequences = root.join("consequences.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE consequences(allele_id VARCHAR, gene_symbol VARCHAR, gene_id VARCHAR);
                 INSERT INTO consequences VALUES
                   ('allele-1', 'GENE1', 'ENSG1'),
                   ('allele-1', 'GENE2', 'ENSG2'),
                   ('allele-2', 'GENE2', 'ENSG2');
                 COPY consequences TO '{}' (FORMAT PARQUET);",
                consequences.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        let selected = HashSet::from(["GENE1".to_owned()]);
        assert_eq!(
            report_gene_occurrences(&root.join("variants.parquet"), &selected, &BTreeSet::new())
                .unwrap(),
            [ReportGeneOccurrence {
                allele_id: "allele-1".into(),
                gene_symbol: "GENE1".into(),
                gene_id: "ENSG1".into(),
            }]
        );
        assert_eq!(
            report_gene_occurrences(
                &root.join("variants.parquet"),
                &HashSet::new(),
                &BTreeSet::from(["ENSG2".to_owned()]),
            )
            .unwrap(),
            [
                ReportGeneOccurrence {
                    allele_id: "allele-1".into(),
                    gene_symbol: "GENE2".into(),
                    gene_id: "ENSG2".into(),
                },
                ReportGeneOccurrence {
                    allele_id: "allele-2".into(),
                    gene_symbol: "GENE2".into(),
                    gene_id: "ENSG2".into(),
                },
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn core_page_filters_bind_with_joined_gene_evidence() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE variants(
                    chromosome VARCHAR, position BIGINT, reference VARCHAR, alternate VARCHAR,
                    variant_id VARCHAR, gene_symbol VARCHAR, gene_id VARCHAR,
                    transcript_id VARCHAR, consequence VARCHAR, impact VARCHAR,
                    quality DOUBLE, filter VARCHAR, canonical BOOLEAN
                 );
                 CREATE TABLE scored_evidence(gene_symbol VARCHAR);",
            )
            .unwrap();
        let sql = format!(
            "SELECT v.gene_symbol
             FROM scored_evidence ev_order
             JOIN variants v ON upper(v.gene_symbol)=ev_order.gene_symbol
             WHERE {CORE_PAGE_WHERE_SQL}"
        );
        connection.prepare(&sql).unwrap();
    }

    #[test]
    fn result_page_includes_condition_only_gene_evidence() {
        let root = std::env::temp_dir().join(format!(
            "annocat-condition-page-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t200\t.\tC\tT\t50\tPASS\tCSQ=T|missense_variant|MODERATE|GENE2|ENSG2|ENST2|C/T|YES\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();

        let evidence = root.join("evidence.parquet");
        let gene_evidence = root.join("gene-evidence.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE evidence(allele_id VARCHAR, consequence_id VARCHAR, scope VARCHAR,
                    source_id VARCHAR, field_path VARCHAR, value_type VARCHAR,
                    string_value VARCHAR, integer_value BIGINT, number_value DOUBLE,
                    boolean_value BOOLEAN, json_value VARCHAR);
                 COPY evidence TO '{}' (FORMAT PARQUET);
                 CREATE TABLE gene_evidence(gene_id VARCHAR, gene_symbol VARCHAR, scope VARCHAR,
                    source_id VARCHAR, field_path VARCHAR, value_type VARCHAR,
                    string_value VARCHAR, integer_value BIGINT, number_value DOUBLE,
                    boolean_value BOOLEAN, json_value VARCHAR);
                 INSERT INTO gene_evidence VALUES
                    ('ENSG1', 'GENE1', 'gene', 'hpo', 'selectedConditionMatches', 'integer', NULL, 1, NULL, NULL, NULL),
                    ('ENSG1', 'GENE1', 'gene', 'hpo', 'matchedSelectedConditions', 'text', 'MONDO:0005277 migraine disorder', NULL, NULL, NULL, NULL),
                    ('ENSG1', 'GENE1', 'gene', 'hpo', 'selectedConditionRelation', 'text', 'Condition subtype', NULL, NULL, NULL, NULL),
                    ('ENSG1', 'GENE1', 'gene', 'hpo', 'phenotypeEvidenceDetails', 'json', NULL, NULL, NULL, NULL, '{{\"conditionLinks\":[{{\"selectedConditionId\":\"MONDO:0005277\",\"selectedCondition\":\"migraine disorder\",\"relation\":\"Condition subtype\"}}]}}');
                 COPY gene_evidence TO '{}' (FORMAT PARQUET);",
                evidence.to_string_lossy().replace('\'', "''"),
                gene_evidence.to_string_lossy().replace('\'', "''"),
            ))
            .unwrap();
        let catalog = root.join("field-catalog.json");
        let fields = [
            ("phenotypeRelevance", "number"),
            ("selectedConditionMatches", "integer"),
            ("matchedSelectedConditions", "text"),
            ("selectedConditionRelation", "text"),
            ("phenotypeEvidenceDetails", "json"),
        ]
        .into_iter()
        .map(|(path, value_type)| {
            json!({
                "scope": "gene",
                "physicalScope": "gene",
                "biologicalScope": "gene",
                "sourceId": "hpo",
                "fieldPath": path,
                "valueType": value_type,
                "storageRelation": "geneEvidence",
                "resolutionPolicy": "geneDirect"
            })
        })
        .collect::<Vec<_>>();
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "schemaVersion": 1,
                "geneEvidenceFile": "gene-evidence.parquet",
                "fields": fields
            }))
            .unwrap(),
        )
        .unwrap();
        let page: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![0, 1, 2, 3, 4],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        let row = &page["rows"][0];
        assert_eq!(row["geneSymbol"], "GENE1");
        assert_eq!(row["evidence"]["1"], "1");
        assert_eq!(row["evidence"]["2"], "MONDO:0005277 migraine disorder");
        assert_eq!(row["evidence"]["3"], "Condition subtype");
        assert!(
            row["evidence"]["4"]
                .as_str()
                .unwrap()
                .contains("conditionLinks")
        );
        let sorted: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![0, 1, 2, 3, 4],
                    sort_evidence: Some(0),
                    direction: "asc".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sorted["rows"][0]["geneSymbol"], "GENE1");
        assert_eq!(sorted["rows"][1]["geneSymbol"], "GENE2");

        let counted: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    gene: "GENE".into(),
                    evidence_columns: vec![0, 1, 2, 3, 4],
                    exact_total: true,
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(counted["total"], 2);
        let cached_sorted: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    gene: "GENE".into(),
                    evidence_columns: vec![0, 1, 2, 3, 4],
                    sort_evidence: Some(0),
                    direction: "asc".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cached_sorted["rows"], sorted["rows"]);
        fs::remove_dir_all(root).unwrap();
    }

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
        let genes = report_gene_identities(&parquet).unwrap();
        assert!(!genes.is_empty());
        assert!(genes.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let page: Value =
            serde_json::from_str(&page_json(&parquet, 0, 3, &PageRequest::default()).unwrap())
                .unwrap();
        assert!(page["total"].is_null());
        assert_eq!(page["hasMore"], true);
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
        assert_eq!(next_page["hasMore"], true);
        assert_eq!(next_page["rows"].as_array().unwrap().len(), 3);
        let final_page: Value =
            serde_json::from_str(&page_json(&parquet, 6, 3, &PageRequest::default()).unwrap())
                .unwrap();
        assert_eq!(final_page["total"], 8);
        assert_eq!(final_page["hasMore"], false);
        assert_eq!(final_page["rows"].as_array().unwrap().len(), 2);
        let counted_page: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                3,
                &PageRequest {
                    exact_total: true,
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(counted_page["total"], 8);
        assert_eq!(counted_page["hasMore"], true);
        let position_sort = vec![PageSortRequest {
            column: "position".into(),
            direction: "desc".into(),
        }];
        let sorted_page: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                3,
                &PageRequest {
                    sorts: position_sort.clone(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        let sorted_all: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                100,
                &PageRequest {
                    exact_total: true,
                    sorts: position_sort,
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            sorted_page["rows"].as_array().unwrap(),
            &sorted_all["rows"].as_array().unwrap()[..3]
        );

        let chromosome = page["rows"][0]["chromosome"].as_str().unwrap().to_owned();
        let filtered_sort = PageRequest {
            chromosome: chromosome.clone(),
            sorts: vec![PageSortRequest {
                column: "position".into(),
                direction: "desc".into(),
            }],
            ..PageRequest::default()
        };
        let canonical_sorted: Value =
            serde_json::from_str(&page_json(&parquet, 0, 100, &filtered_sort).unwrap()).unwrap();
        let filtered_count: Value = serde_json::from_str(
            &page_json(
                &parquet,
                0,
                100,
                &PageRequest {
                    chromosome,
                    exact_total: true,
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        let cached_sorted: Value =
            serde_json::from_str(&page_json(&parquet, 0, 100, &filtered_sort).unwrap()).unwrap();
        assert_eq!(cached_sorted["total"], filtered_count["total"]);
        assert_eq!(cached_sorted["rows"], canonical_sorted["rows"]);

        let remembered_search = PageRequest {
            search: "variant".into(),
            known_total: Some(8),
            sorts: vec![PageSortRequest {
                column: "position".into(),
                direction: "desc".into(),
            }],
            ..PageRequest::default()
        };
        let remembered_filters = validated_core_page_filters(&remembered_search).unwrap();
        let remembered_key = matched_row_cache_key(
            &PageQuery {
                variants: &parquet,
                evidence: None,
                evidence_files: None,
                catalog: None,
                offset: 0,
                limit: 100,
                request: &remembered_search,
                candidate_ids: None,
            },
            &remembered_filters,
        )
        .unwrap()
        .unwrap();
        assert!(cached_result_rows(&remembered_key).is_none());
        let remembered_page: Value =
            serde_json::from_str(&page_json(&parquet, 0, 100, &remembered_search).unwrap())
                .unwrap();
        assert_eq!(remembered_page["total"], 8);
        assert!(cached_result_rows(&remembered_key).is_some());

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
    fn report_gene_lookup_omits_symbols_with_ambiguous_stable_ids() {
        let root = std::env::temp_dir().join(format!(
            "annocat-report-genes-{}-{}",
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
            "##fileformat=VCFv4.2\n\
             ##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n\
             #CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n\
             1\t101\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|COLLIDE|ENSG1|ENST1|A/G||\n\
             1\t102\t.\tC\tT\t50\tPASS\tCSQ=T|missense_variant|MODERATE|COLLIDE|ENSG2|ENST2|C/T||\n",
        )
        .unwrap();
        let parquet = root.join("variants.parquet");
        convert_vcf(&input, &parquet, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(
            report_gene_identities(&parquet).unwrap(),
            Vec::<(String, String)>::new()
        );
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
                        values: None,
                        include_missing: None,
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
            search: "stop".into(),
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
        assert_eq!(
            detail["variant"]["sampleCalls"][0]["allelePresence"],
            "carried"
        );
        assert_eq!(
            detail["variant"]["sampleCalls"][0]["selectedAltCopyCount"],
            1
        );
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
        assert_eq!(
            summary.input_content_sha256,
            Some(format!("{:x}", Sha256::digest(fs::read(&input).unwrap())))
        );

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
    fn validation_accepts_full_vcf_terms_and_missing_placeholders() {
        let root = std::env::temp_dir().join(format!(
            "annocat-consequence-validation-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            concat!(
                "##fileformat=VCFv4.2\n",
                "##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature_type|Feature\">\n",
                "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n",
                "1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|intron_variant&non_coding_transcript_variant|MODIFIER|GENE1|ENSG1|Transcript|ENST1\n",
                "1\t200\t.\tA\tG\t50\tPASS\tCSQ=G|intergenic_variant|MODIFIER|-|-|Intergenic|-\n"
            ),
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();

        let structured = root.join("fastvep.ndjson");
        fs::write(
            &structured,
            concat!(
                r#"{"allele_string":"A/G","start":100,"seq_region_name":"1","transcript_consequences":[{"variant_allele":"G","consequence_terms":["intron_variant","non_coding_transcript_variant"],"impact":"MODIFIER","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST1"}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":200,"seq_region_name":"1","intergenic_consequences":[{"variant_allele":"G","consequence_terms":["intergenic_variant"],"impact":"MODIFIER","gene_symbol":"-","gene_id":"-"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        convert_structured(
            &structured,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();

        validate_report_tables(&variants, &consequences, &evidence, &catalog, 2).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiallelic_records_keep_an_allele_without_a_csq_entry() {
        let root = std::env::temp_dir().join(format!(
            "annocat-parquet-partial-csq-{}-{}",
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
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tC,AT\t50\tPASS\tCSQ=C|missense_variant|MODERATE|GENE_C|ENSGC|ENSTC|A/C\n",
        )
        .unwrap();
        let parquet = root.join("variants.parquet");
        let summary = convert_vcf(&input, &parquet, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(summary.rows, 2);

        let connection = Connection::open_in_memory().unwrap();
        let path = parquet.to_string_lossy();
        let rows: (i64, i64, i64) = connection
            .query_row(
                "SELECT count(*), count(gene_symbol), count(consequence)
                 FROM read_parquet(?)",
                params![path.as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(rows, (2, 1, 1));
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
    fn structured_consequences_keep_feature_types_and_allele_identity() {
        let record = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "A/G/T",
                "start": 100,
                "seq_region_name": "1",
                "most_severe_consequence": "stop_gained",
                "transcript_consequences": [{
                    "variant_allele": "G",
                    "transcript_id": "ENST1",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }],
                "regulatory_feature_consequences": [{
                    "variant_allele": "T",
                    "regulatory_feature_id": "ENSR1",
                    "consequence_terms": ["regulatory_region_variant"],
                    "impact": "MODIFIER"
                }]
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };
        let parsed = parse_structured_record(&record, &BTreeMap::new()).unwrap();
        assert_eq!(parsed.consequences.len(), 2);
        assert_eq!(
            parsed.consequences.feature_type,
            ["transcript", "regulatory"]
        );
        assert_eq!(
            parsed.consequences.feature_id,
            [Some("ENST1".into()), Some("ENSR1".into())]
        );
        assert_ne!(
            parsed.consequences.allele_id[0],
            parsed.consequences.allele_id[1]
        );
        assert!(
            parsed.consequences.consequence_json[1].contains("\"feature_type\":\"regulatory\"")
        );
    }

    #[test]
    fn structured_allele_sources_follow_declared_scope_identity() {
        let record = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "A/G",
                "start": 100,
                "seq_region_name": "1",
                "alleles": [{
                    "allele": "G",
                    "gnomad": {"allAf": 0.001},
                    "revel": {
                        "score": 0.9,
                        "transcriptId": "ENST2"
                    },
                    "spliceai": {
                        "gene": "GENE2",
                        "dsAg": 0.8,
                        "dsAl": 0.1,
                        "dsDg": 0.2,
                        "dsDl": 0.3,
                        "dpAg": -49
                    }
                }],
                "transcript_consequences": [{
                    "variant_allele": "G",
                    "transcript_id": "ENST1",
                    "gene_id": "ENSG1",
                    "gene_symbol": "GENE1",
                    "canonical": 1,
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }, {
                    "variant_allele": "G",
                    "transcript_id": "ENST2",
                    "gene_id": "ENSG2",
                    "gene_symbol": "GENE2",
                    "mane_select": "NM_2",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }]
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };

        let parsed = parse_structured_record(&record, &BTreeMap::new()).unwrap();
        let gnomad = parsed
            .evidence
            .source_id
            .iter()
            .enumerate()
            .find_map(|(index, source)| {
                (source == "gnomad" && parsed.evidence.field_path[index] == "allAf")
                    .then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.scope[gnomad], "allele");
        assert_eq!(parsed.evidence.consequence_id[gnomad], None);
        assert_eq!(parsed.evidence.number_value[gnomad], Some(0.001));

        let revel = parsed
            .evidence
            .source_id
            .iter()
            .enumerate()
            .find_map(|(index, source)| {
                (source == "revel" && parsed.evidence.field_path[index] == "score").then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.scope[revel], "transcript");
        assert_eq!(
            parsed.evidence.consequence_id[revel].as_deref(),
            Some("local:1")
        );

        let spliceai = parsed
            .evidence
            .source_id
            .iter()
            .enumerate()
            .find_map(|(index, source)| {
                (source == "spliceai" && parsed.evidence.field_path[index] == "dsAg")
                    .then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.scope[spliceai], "transcript");
        assert_eq!(
            parsed.evidence.consequence_id[spliceai].as_deref(),
            Some("local:1")
        );
        let maximum = parsed
            .evidence
            .field_path
            .iter()
            .enumerate()
            .find_map(|(index, field)| {
                (parsed.evidence.source_id[index] == "spliceai"
                    && field == "maxDeltaScore"
                    && parsed.evidence.scope[index] == "selected")
                    .then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.number_value[maximum], Some(0.8));
        let position = parsed
            .evidence
            .field_path
            .iter()
            .enumerate()
            .find_map(|(index, field)| {
                (parsed.evidence.source_id[index] == "spliceai"
                    && field == "dpAg"
                    && parsed.evidence.scope[index] == "transcript")
                    .then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.integer_value[position], Some(-49));
        assert!(
            parsed
                .consequences
                .consequence_json
                .iter()
                .all(|value| !value.contains("gnomad")
                    && !value.contains("revel")
                    && !value.contains("spliceai"))
        );
    }

    #[test]
    fn structured_record_lists_resolve_the_selected_gene_and_transcript() {
        let record = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "A/G",
                "start": 100,
                "seq_region_name": "1",
                "spliceai": [
                    {"gene": "GENE1", "dsAg": 0.1, "dpAg": 8},
                    {"gene": "GENE2", "dsAg": 0.9, "dpAg": -49}
                ],
                "revel": [
                    {"transcriptId": "ENST1", "aaRef": "R", "aaAlt": "H", "score": 0.2},
                    {"transcriptId": "ENST2", "aaRef": "Arg", "aaAlt": "His", "score": 0.8}
                ],
                "transcript_consequences": [{
                    "variant_allele": "G",
                    "transcript_id": "ENST1",
                    "gene_symbol": "GENE1",
                    "canonical": 1,
                    "amino_acids": "R/H",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }, {
                    "variant_allele": "G",
                    "transcript_id": "ENST2.4",
                    "gene_symbol": "GENE2",
                    "mane_select": "NM_2",
                    "amino_acids": "R/H",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }]
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };

        let parsed = parse_structured_record(&record, &BTreeMap::new()).unwrap();
        let mut selected_fields = HashSet::new();
        for index in 0..parsed.evidence.len() {
            if parsed.evidence.scope[index] == "selected" {
                assert!(selected_fields.insert((
                    parsed.evidence.source_id[index].clone(),
                    parsed.evidence.field_path[index].clone(),
                )));
            }
        }
        let selected_value = |source: &str, field: &str| {
            parsed
                .evidence
                .source_id
                .iter()
                .enumerate()
                .find_map(|(index, candidate)| {
                    (candidate == source
                        && parsed.evidence.field_path[index] == field
                        && parsed.evidence.scope[index] == "selected")
                        .then_some(
                            parsed.evidence.number_value[index]
                                .or(parsed.evidence.integer_value[index].map(|value| value as f64)),
                        )
                })
                .flatten()
        };
        assert_eq!(selected_value("spliceai", "dsAg"), Some(0.9));
        assert_eq!(selected_value("spliceai", "dpAg"), Some(-49.0));
        assert_eq!(selected_value("revel", "score"), Some(0.8));
        for source in ["spliceai", "revel"] {
            assert_eq!(
                parsed
                    .evidence
                    .source_id
                    .iter()
                    .enumerate()
                    .filter(|(index, candidate)| {
                        *candidate == source
                            && parsed.evidence.scope[*index] == "source_records"
                            && parsed.evidence.field_path[*index] == "__recordList"
                    })
                    .count(),
                1
            );
        }
    }

    #[test]
    fn structured_top_level_spliceai_links_to_its_gene() {
        let record = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "A/G",
                "start": 100,
                "seq_region_name": "1",
                "spliceAI": {
                    "gene": "GENE2",
                    "dsAg": 0.1,
                    "dsAl": 0.7,
                    "dsDg": 0.2,
                    "dsDl": 0.3,
                    "dpAg": -49
                },
                "transcript_consequences": [{
                    "variant_allele": "G",
                    "transcript_id": "ENST1",
                    "gene_symbol": "GENE1",
                    "canonical": 1,
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }, {
                    "variant_allele": "G",
                    "transcript_id": "ENST2",
                    "gene_symbol": "GENE2",
                    "mane_select": "NM_2",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }]
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };
        let aliases = structured_source_aliases(&["spliceai".into()]).unwrap();
        let parsed = parse_structured_record(&record, &aliases).unwrap();
        for field in ["dsAg", "dsAl", "dsDg", "dsDl", "dpAg", "maxDeltaScore"] {
            let index = parsed
                .evidence
                .field_path
                .iter()
                .enumerate()
                .find_map(|(index, candidate)| {
                    (parsed.evidence.source_id[index] == "spliceai"
                        && candidate == field
                        && parsed.evidence.scope[index] == "transcript")
                        .then_some(index)
                })
                .unwrap();
            assert_eq!(
                parsed.evidence.consequence_id[index].as_deref(),
                Some("local:1")
            );
        }
        let selected_maximum = parsed
            .evidence
            .field_path
            .iter()
            .enumerate()
            .find_map(|(index, field)| {
                (parsed.evidence.source_id[index] == "spliceai"
                    && field == "maxDeltaScore"
                    && parsed.evidence.scope[index] == "selected")
                    .then_some(index)
            })
            .unwrap();
        assert_eq!(parsed.evidence.number_value[selected_maximum], Some(0.7));
    }

    #[test]
    fn legacy_top_level_spliceai_columns_resolve_by_gene() {
        let root = std::env::temp_dir().join(format!(
            "annocat-legacy-spliceai-{}-{}",
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
                r#"{"allele_string":"A/G","start":100,"seq_region_name":"1","spliceai":{"gene":"GENE2","dsAg":0.1,"dsAl":0.7,"dsDg":0.2,"dsDl":0.3,"dpAg":-49},"transcript_consequences":[{"variant_allele":"G","transcript_id":"ENST2","gene_id":"ENSG2","gene_symbol":"GENE2","mane_select":"NM_2","consequence_terms":["missense_variant"],"impact":"MODERATE"}]}"#,
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

        let legacy_evidence = root.join("legacy-evidence.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                   SELECT * REPLACE (
                     CASE WHEN source_id='spliceai' THEN 'unresolved_feature' ELSE scope END AS scope,
                     CASE WHEN source_id='spliceai' THEN NULL ELSE consequence_id END AS consequence_id
                   )
                   FROM read_parquet('{}')
                   WHERE NOT (source_id='spliceai' AND scope='selected')
                     AND NOT (source_id='spliceai' AND field_path='maxDeltaScore')
                 ) TO '{}' (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 4096)",
                evidence.to_string_lossy().replace('\'', "''"),
                legacy_evidence.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        let mut catalog_value: Value =
            serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        catalog_value["fields"]
            .as_array_mut()
            .unwrap()
            .retain(|field| {
                field["sourceId"] != "spliceai" || field["fieldPath"] != "maxDeltaScore"
            });
        for field in catalog_value["fields"].as_array_mut().unwrap() {
            if field["sourceId"] == "spliceai" {
                field["scope"] = Value::String("unresolved_feature".into());
                field["biologicalScope"] = Value::String("feature".into());
                field["physicalScope"] = Value::String("unresolved_feature".into());
                field["resolutionPolicy"] = Value::String("unresolved".into());
                field.as_object_mut().unwrap().remove("selectionOrigin");
            }
        }
        fs::write(&catalog, serde_json::to_vec(&catalog_value).unwrap()).unwrap();
        let query_catalog = query_field_catalog(&catalog).unwrap();
        let query_fields = query_catalog["fields"].as_array().unwrap();
        let score_index = query_fields
            .iter()
            .position(|field| field["sourceId"] == "spliceai" && field["fieldPath"] == "dsAl")
            .unwrap();
        let position_index = query_fields
            .iter()
            .position(|field| field["sourceId"] == "spliceai" && field["fieldPath"] == "dpAg")
            .unwrap();
        let maximum_index = query_fields
            .iter()
            .position(|field| {
                field["sourceId"] == "spliceai" && field["fieldPath"] == "maxDeltaScore"
            })
            .unwrap();
        assert_eq!(
            query_fields[maximum_index]["resolutionPolicy"],
            "derivedSpliceAiMaximum"
        );
        let stored_catalog: Value = serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        assert!(
            stored_catalog["fields"]
                .as_array()
                .unwrap()
                .iter()
                .all(|field| {
                    field["sourceId"] != "spliceai" || field["fieldPath"] != "maxDeltaScore"
                })
        );

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE2|ENSG2|ENST2|A/G|YES|ENST2\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let request = PageRequest {
            evidence_columns: vec![score_index, position_index, maximum_index],
            sort_evidence: Some(maximum_index),
            direction: "desc".into(),
            ..PageRequest::default()
        };
        let page: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&legacy_evidence),
                Some(&catalog),
                0,
                10,
                &request,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            page["rows"][0]["evidence"][score_index.to_string()],
            "0.7",
            "{page}"
        );
        assert_eq!(
            page["rows"][0]["evidence"][position_index.to_string()],
            "-49"
        );
        assert_eq!(
            page["rows"][0]["evidence"][maximum_index.to_string()],
            "0.7"
        );
        assert_eq!(
            page["rows"][0]["evidenceResolution"][score_index.to_string()]["kind"],
            "exact_gene"
        );
        assert_eq!(
            page["rows"][0]["evidenceResolution"][maximum_index.to_string()]["kind"],
            "derived_maximum"
        );
        let projection_fields = request_query_projection_fields(&catalog, &request)
            .unwrap()
            .unwrap();
        prepare_query_projection(&legacy_evidence, &catalog, &projection_fields).unwrap();
        let maximum_projection_prefix = format!("{QUERY_PROJECTION_PREFIX}{maximum_index}-");
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with(&maximum_projection_prefix))
                })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_spanning_deletion_placeholder_does_not_hide_real_allele() {
        let record = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "G/*/T",
                "start": 100,
                "seq_region_name": "1",
                "alleles": [{
                    "allele": "*",
                    "gnomad": {"allAf": 0.9}
                }, {
                    "allele": "T",
                    "gnomad": {"allAf": 0.001}
                }],
                "transcript_consequences": [{
                    "variant_allele": "*",
                    "transcript_id": "ENST_STAR",
                    "consequence_terms": ["downstream_gene_variant"]
                }, {
                    "variant_allele": "T",
                    "transcript_id": "ENST_REAL",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE"
                }]
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };

        let parsed = parse_structured_record(&record, &BTreeMap::new()).unwrap();
        let expected_allele = allele_id("1", 100, "G", "T");
        assert_eq!(parsed.consequences.len(), 1);
        assert_eq!(parsed.consequences.allele_id, [expected_allele.clone()]);
        assert_eq!(
            parsed.consequences.primary_consequence,
            [Some("missense_variant".into())]
        );
        assert!(
            parsed
                .evidence
                .allele_id
                .iter()
                .all(|allele| allele == &expected_allele)
        );
        assert!(parsed.evidence.number_value.contains(&Some(0.001)));
        assert!(!parsed.evidence.number_value.contains(&Some(0.9)));
    }

    #[test]
    fn record_level_consequence_is_not_broadcast_across_multiple_alts() {
        let multiallelic = StructuredRecord {
            line_number: 1,
            line: json!({
                "allele_string": "A/G/T",
                "start": 100,
                "seq_region_name": "1",
                "most_severe_consequence": "stop_gained"
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };
        assert_eq!(
            parse_structured_record(&multiallelic, &BTreeMap::new())
                .unwrap()
                .consequences
                .len(),
            0
        );

        let biallelic = StructuredRecord {
            line_number: 2,
            line: json!({
                "allele_string": "A/G",
                "start": 101,
                "seq_region_name": "1",
                "most_severe_consequence": "stop_gained"
            })
            .to_string(),
            canonical_alleles: BTreeMap::new(),
        };
        let parsed = parse_structured_record(&biallelic, &BTreeMap::new()).unwrap();
        assert_eq!(parsed.consequences.len(), 1);
        assert_eq!(parsed.consequences.feature_type[0], "unresolved");
        assert_eq!(parsed.consequences.impact[0].as_deref(), Some("HIGH"));
    }

    #[test]
    fn structured_records_reuse_canonical_vcf_alleles_by_alt_index() {
        let canonical = vec![
            CanonicalAllele {
                chromosome: "1".into(),
                position: 99,
                reference: "AA".into(),
                alternate: "A".into(),
            },
            CanonicalAllele {
                chromosome: "1".into(),
                position: 100,
                reference: "A".into(),
                alternate: "T".into(),
            },
        ];
        let line = json!({
            "allele_string": "A/G/T",
            "start": 100,
            "seq_region_name": "1"
        })
        .to_string();
        let identity: StructuredIdentity = serde_json::from_str(&line).unwrap();
        let mapped = canonical_structured_alleles(1, &identity, Some(&canonical), None).unwrap();

        assert_eq!(mapped["G"], canonical[0]);
        assert_eq!(mapped["T"], canonical[1]);
    }

    struct MissingReference;

    impl ReferenceSequence for MissingReference {
        fn base(&mut self, chromosome: &str, _position: u64) -> Result<u8, NormalizeError> {
            Err(NormalizeError::MissingChromosome(chromosome.into()))
        }

        fn sequence(
            &mut self,
            chromosome: &str,
            _position: u64,
            _length: usize,
        ) -> Result<Vec<u8>, NormalizeError> {
            Err(NormalizeError::MissingChromosome(chromosome.into()))
        }
    }

    #[test]
    fn missing_auxiliary_contig_is_preserved_but_main_chromosome_is_rejected() {
        let mut reference = MissingReference;
        let auxiliary =
            canonicalize_or_preserve_auxiliary(&mut reference, "Un_KI_missing", 52, "C", "A")
                .unwrap();
        assert_eq!(
            auxiliary,
            CanonicalAllele {
                chromosome: "Un_KI_missing".into(),
                position: 52,
                reference: "C".into(),
                alternate: "A".into(),
            }
        );
        assert!(matches!(
            canonicalize_or_preserve_auxiliary(&mut reference, "chr3", 52, "C", "A"),
            Err(NormalizeError::MissingChromosome(_))
        ));
    }

    #[test]
    fn nonhuman_auxiliary_records_are_excluded_but_human_contigs_remain() {
        assert!(
            canonical_alleles_for_vcf_line(1, "Un_KN707607v1_decoy\t52\t.\tC\tA", None)
                .unwrap()
                .is_none()
        );
        assert!(
            canonical_alleles_for_vcf_line(2, "hs37d5\t52\t.\tC\tA", None)
                .unwrap()
                .is_none()
        );
        assert!(
            canonical_alleles_for_vcf_line(3, "EBV\t52\t.\tC\tA", None)
                .unwrap()
                .is_none()
        );
        assert!(
            canonical_alleles_for_vcf_line(4, "chrUn_KI270442v1\t52\t.\tC\tA", None)
                .unwrap()
                .is_some()
        );
        assert!(
            canonical_alleles_for_vcf_line(5, "22_KI270879v1_alt\t52\t.\tC\tA", None)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn variant_and_evidence_tables_both_omit_decoy_records() {
        let root = std::env::temp_dir().join(format!(
            "annocat-decoy-filter-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\nUn_KN707607v1_decoy\t52\t.\tC\tA\t50\tPASS\tCSQ=A|missense_variant|MODERATE|DECOY|ENSG0|ENST0\n1\t52\t.\tC\tA\t50\tPASS\tCSQ=A|missense_variant|MODERATE|GENE1|ENSG1|ENST1\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        let canonical = convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        assert_eq!(canonical.records, 1);
        assert_eq!(canonical.excluded_auxiliary_records, 1);

        let ndjson = root.join("fastvep.ndjson");
        fs::write(
            &ndjson,
            concat!(
                r#"{"allele_string":"C/A","start":52,"end":52,"seq_region_name":"Un_KN707607v1_decoy","transcript_consequences":[{"variant_allele":"A","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"DECOY","gene_id":"ENSG0","transcript_id":"ENST0"}]}"#,
                "\n",
                r#"{"allele_string":"C/A","start":52,"end":52,"seq_region_name":"1","transcript_consequences":[{"variant_allele":"A","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST1"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        let structured = convert_structured(
            &ndjson,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(structured.records, 1);
        assert_eq!(structured.excluded_auxiliary_records, 1);
        assert_eq!(structured.consequences, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_selection_uses_mane_plus_clinical_before_canonical() {
        let canonical = Map::from_iter([
            ("Feature".into(), Value::String("ENST_CANONICAL".into())),
            ("CANONICAL".into(), Value::String("YES".into())),
            ("BIOTYPE".into(), Value::String("protein_coding".into())),
            ("IMPACT".into(), Value::String("HIGH".into())),
        ]);
        let mane_plus = Map::from_iter([
            ("Feature".into(), Value::String("ENST00000000002".into())),
            (
                "MANE_PLUS_CLINICAL".into(),
                Value::String("ENST00000000002.1".into()),
            ),
            ("BIOTYPE".into(), Value::String("protein_coding".into())),
            ("IMPACT".into(), Value::String("MODERATE".into())),
        ]);
        assert_eq!(
            best_consequence(&[canonical, mane_plus])
                .unwrap()
                .get("Feature")
                .and_then(Value::as_str),
            Some("ENST00000000002")
        );
    }

    #[test]
    fn transcript_selection_uses_mane_select_before_other_preferences() {
        let mane_select = Map::from_iter([
            ("Feature".into(), Value::String("ENST_MANE".into())),
            ("MANE_SELECT".into(), Value::String("NM_000001.1".into())),
            ("BIOTYPE".into(), Value::String("protein_coding".into())),
            ("IMPACT".into(), Value::String("MODERATE".into())),
        ]);
        let mane_plus = Map::from_iter([
            ("Feature".into(), Value::String("ENST00000000002".into())),
            (
                "MANE_PLUS_CLINICAL".into(),
                Value::String("ENST00000000002.1".into()),
            ),
            ("BIOTYPE".into(), Value::String("protein_coding".into())),
            ("IMPACT".into(), Value::String("HIGH".into())),
        ]);
        let canonical = Map::from_iter([
            ("Feature".into(), Value::String("ENST_CANONICAL".into())),
            ("CANONICAL".into(), Value::String("YES".into())),
            ("BIOTYPE".into(), Value::String("protein_coding".into())),
            ("IMPACT".into(), Value::String("HIGH".into())),
        ]);
        assert_eq!(
            best_consequence(&[canonical, mane_plus, mane_select])
                .unwrap()
                .get("Feature")
                .and_then(Value::as_str),
            Some("ENST_MANE")
        );
    }

    #[test]
    fn overlapping_gene_severity_selects_muc6_stop_gain_over_ap2a2_downstream() {
        let ap2a2 = json!({
            "Allele": "A",
            "Consequence": "downstream_gene_variant",
            "SYMBOL": "AP2A2",
            "Gene": "ENSG00000183020",
            "Feature_type": "Transcript",
            "Feature": "ENST00000396630",
            "MANE_SELECT": "NM_012305.4"
        })
        .as_object()
        .unwrap()
        .clone();
        let muc6 = json!({
            "Allele": "A",
            "Consequence": "stop_gained",
            "SYMBOL": "MUC6",
            "Gene": "ENSG00000184956",
            "Feature_type": "Transcript",
            "Feature": "ENST00000421673",
            "MANE_SELECT": "NM_005961.3"
        })
        .as_object()
        .unwrap()
        .clone();
        for entries in [vec![ap2a2.clone(), muc6.clone()], vec![muc6.clone(), ap2a2]] {
            assert_eq!(
                consequence_text(best_consequence(&entries).unwrap(), &["SYMBOL"]),
                Some("MUC6")
            );
        }
    }

    #[test]
    fn same_gene_fallback_prefers_canonical_before_consequence_severity() {
        let canonical = json!({
            "Consequence": "intron_variant",
            "SYMBOL": "GENE1",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000001",
            "CANONICAL": "YES"
        })
        .as_object()
        .unwrap()
        .clone();
        let severe = json!({
            "Consequence": "stop_gained",
            "SYMBOL": "GENE1",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000002"
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            consequence_text(
                best_consequence(&[severe, canonical]).unwrap(),
                &["Feature"]
            ),
            Some("ENST00000000001")
        );
    }

    #[test]
    fn same_gene_prefers_mane_select_before_mane_plus_clinical() {
        let mane_select = json!({
            "Consequence": "downstream_gene_variant",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000001",
            "MANE_SELECT": "NM_000001.1"
        })
        .as_object()
        .unwrap()
        .clone();
        let mane_plus = json!({
            "Consequence": "stop_gained",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000002",
            "MANE_PLUS_CLINICAL": "NM_000002.1"
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            consequence_text(
                best_consequence(&[mane_select, mane_plus]).unwrap(),
                &["Feature"]
            ),
            Some("ENST00000000001")
        );
    }

    #[test]
    fn malformed_mane_value_does_not_outrank_valid_fallback() {
        let malformed = json!({
            "Consequence": "stop_gained",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000002",
            "MANE_SELECT": "-"
        })
        .as_object()
        .unwrap()
        .clone();
        let canonical = json!({
            "Consequence": "intron_variant",
            "Gene": "ENSG00000000001",
            "Feature_type": "Transcript",
            "Feature": "ENST00000000001",
            "CANONICAL": true
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            consequence_text(
                best_consequence(&[malformed, canonical]).unwrap(),
                &["Feature"]
            ),
            Some("ENST00000000001")
        );
    }

    #[test]
    fn vcf_and_structured_keys_choose_the_same_representative() {
        let vcf_entries = vec![
            json!({
                "Consequence": "downstream_gene_variant",
                "SYMBOL": "AP2A2",
                "Gene": "ENSG00000183020",
                "Feature_type": "Transcript",
                "Feature": "ENST00000396630",
                "MANE_SELECT": "NM_012305.4"
            })
            .as_object()
            .unwrap()
            .clone(),
            json!({
                "Consequence": "stop_gained",
                "SYMBOL": "MUC6",
                "Gene": "ENSG00000184956",
                "Feature_type": "Transcript",
                "Feature": "ENST00000421673",
                "MANE_SELECT": "NM_005961.3"
            })
            .as_object()
            .unwrap()
            .clone(),
        ];
        let structured_entries = vec![
            (
                "transcript",
                json!({
                    "consequence_terms": ["downstream_gene_variant"],
                    "gene_symbol": "AP2A2",
                    "gene_id": "ENSG00000183020",
                    "transcript_id": "ENST00000396630",
                    "mane_select": "NM_012305.4"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            (
                "transcript",
                json!({
                    "consequence_terms": ["stop_gained"],
                    "gene_symbol": "MUC6",
                    "gene_id": "ENSG00000184956",
                    "transcript_id": "ENST00000421673",
                    "mane_select": "NM_005961.3"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        ];
        let selected = best_structured_consequence_index(&structured_entries, &[0, 1]).unwrap();
        assert_eq!(
            consequence_text(best_consequence(&vcf_entries).unwrap(), &["Feature"]),
            consequence_text(&structured_entries[selected].1, &["transcript_id"])
        );
    }

    #[test]
    fn legacy_report_repairs_overlapping_gene_representative_and_corrupt_cache() {
        let root = std::env::temp_dir().join(format!(
            "annocat-legacy-representative-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature_type|Feature|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n11\t101001\t.\tA\tG\t50\tPASS\tCSQ=G|downstream_gene_variant|MODIFIER|AP2A2|ENSG00000183020|Transcript|ENST00000396630|NM_012305.4\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let structured = root.join("fastvep.ndjson");
        fs::write(
            &structured,
            concat!(
                r#"{"allele_string":"A/G","start":101001,"end":101001,"seq_region_name":"11","transcript_consequences":[{"variant_allele":"G","consequence_terms":["downstream_gene_variant"],"impact":"MODIFIER","gene_symbol":"AP2A2","gene_id":"ENSG00000183020","transcript_id":"ENST00000396630","mane_select":"NM_012305.4"},{"variant_allele":"G","consequence_terms":["stop_gained"],"impact":"HIGH","gene_symbol":"MUC6","gene_id":"ENSG00000184956","transcript_id":"ENST00000421673","mane_select":"NM_005961.3"}]}"#,
                "\n",
                r#"{"allele_string":"A/T","start":101002,"end":101002,"seq_region_name":"11","transcript_consequences":[{"variant_allele":"T","consequence_terms":["intron_variant"],"impact":"MODIFIER","gene_symbol":"LEGACY","gene_id":"ENSGLEGACY","transcript_id":"ENSTLEGACY"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let consequences = root.join("consequences.parquet");
        let evidence = root.join("evidence.parquet");
        let catalog = root.join("field-catalog.json");
        convert_structured(
            &structured,
            &consequences,
            &evidence,
            &catalog,
            || false,
            |_, _, _, _, _| {},
        )
        .unwrap();
        validate_report_tables(&variants, &consequences, &evidence, &catalog, 1).unwrap();

        let page = |search: &str| {
            serde_json::from_str::<Value>(
                &page_json(
                    &variants,
                    0,
                    10,
                    &PageRequest {
                        search: search.into(),
                        exact_total: true,
                        ..PageRequest::default()
                    },
                )
                .unwrap(),
            )
            .unwrap()
        };
        let repaired = page("stop");
        assert_eq!(repaired["total"], 1);
        assert_eq!(repaired["rows"][0]["geneSymbol"], "MUC6");
        assert_eq!(repaired["rows"][0]["consequence"], "stop_gained");
        assert_eq!(page("AP2A2")["total"], 0);

        let override_path = legacy_representative_override(&variants)
            .unwrap()
            .expect("legacy report has a derived override");
        fs::write(&override_path, b"corrupt").unwrap();
        assert_eq!(page("stop")["rows"][0]["geneSymbol"], "MUC6");
        assert!(representative_override_is_valid(
            &override_path,
            &representative_override_fingerprint(&variants, &consequences).unwrap()
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn consequence_severity_matches_the_pinned_fastvep_order() {
        let terms = [
            "transcript_ablation",
            "splice_acceptor_variant",
            "splice_donor_variant",
            "stop_gained",
            "frameshift_variant",
            "stop_lost",
            "start_lost",
            "transcript_amplification",
            "feature_elongation",
            "feature_truncation",
            "inframe_insertion",
            "inframe_deletion",
            "missense_variant",
            "protein_altering_variant",
            "splice_region_variant",
            "splice_donor_5th_base_variant",
            "splice_donor_region_variant",
            "splice_polypyrimidine_tract_variant",
            "incomplete_terminal_codon_variant",
            "start_retained_variant",
            "stop_retained_variant",
            "synonymous_variant",
            "coding_sequence_variant",
            "mature_miRNA_variant",
            "5_prime_UTR_variant",
            "3_prime_UTR_variant",
            "non_coding_transcript_exon_variant",
            "intron_variant",
            "NMD_transcript_variant",
            "non_coding_transcript_variant",
            "coding_transcript_variant",
            "upstream_gene_variant",
            "downstream_gene_variant",
            "TFBS_ablation",
            "TFBS_amplification",
            "TF_binding_site_variant",
            "regulatory_region_ablation",
            "regulatory_region_amplification",
            "regulatory_region_variant",
            "intergenic_variant",
            "sequence_variant",
            "copy_number_change",
            "copy_number_increase",
            "copy_number_decrease",
            "short_tandem_repeat_change",
            "short_tandem_repeat_expansion",
            "short_tandem_repeat_contraction",
            "unidirectional_gene_fusion",
            "transcript_variant",
        ];
        for (expected, term) in terms.into_iter().enumerate() {
            let consequence = json!({"Consequence": term}).as_object().unwrap().clone();
            assert_eq!(consequence_severity_rank(&consequence), expected as u8);
        }
    }

    #[test]
    fn representative_ranking_accepts_vep_feature_and_appris_encodings() {
        let principal = Map::from_iter([
            ("Feature".into(), Value::String("ENST_P1".into())),
            ("Feature_type".into(), Value::String("Transcript".into())),
            ("APPRIS".into(), Value::String("P1".into())),
        ]);
        let alternative = Map::from_iter([
            ("Feature".into(), Value::String("ENST_A1".into())),
            ("Feature_type".into(), Value::String("Transcript".into())),
            ("APPRIS".into(), Value::String("A1".into())),
        ]);
        let regulatory = Map::from_iter([
            ("Feature".into(), Value::String("ENSR1".into())),
            (
                "Feature_type".into(),
                Value::String("RegulatoryFeature".into()),
            ),
            ("IMPACT".into(), Value::String("HIGH".into())),
        ]);
        assert_eq!(
            best_consequence(&[regulatory, alternative, principal])
                .unwrap()
                .get("Feature")
                .and_then(Value::as_str),
            Some("ENST_P1")
        );
    }

    #[test]
    fn structured_conversion_is_deterministic_across_worker_counts() {
        use std::io::Write as _;

        let root = std::env::temp_dir().join(format!(
            "annocat-structured-parallel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("fastvep.ndjson");
        let mut input_writer = BufWriter::new(File::create(&input).unwrap());
        for index in 0..(STRUCTURED_CHUNK_RECORDS * 2 + 17) {
            let position = 100_000 + index as i64;
            let transcript_a = format!("ENST{index:011}");
            let transcript_b = format!("ENST{:011}", index + 1_000_000);
            writeln!(
                input_writer,
                "{}",
                json!({
                    "allele_string": "A/G",
                    "start": position,
                    "end": position,
                    "seq_region_name": "1",
                    "most_severe_consequence": "missense_variant",
                    "variant_type": "Snv",
                    "clinvar": {"classification": "Uncertain_significance"},
                    "transcript_consequences": [
                        {
                            "variant_allele": "G",
                            "consequence_terms": ["missense_variant"],
                            "impact": "MODERATE",
                            "gene_symbol": "GENE1",
                            "gene_id": "ENSG00000000001",
                            "transcript_id": transcript_a,
                            "canonical": 1,
                            "cadd": {"phred": 12.5}
                        },
                        {
                            "variant_allele": "G",
                            "consequence_terms": ["intron_variant"],
                            "impact": "MODIFIER",
                            "gene_symbol": "GENE1",
                            "gene_id": "ENSG00000000001",
                            "transcript_id": transcript_b,
                            "cadd": {"phred": 12.5},
                            "custom": {"score": index}
                        }
                    ]
                })
            )
            .unwrap();
        }
        input_writer.flush().unwrap();

        let single_consequences = root.join("single-consequences.parquet");
        let single_evidence = root.join("single-evidence.parquet");
        let single_catalog = root.join("single-catalog.json");
        let parallel_consequences = root.join("parallel-consequences.parquet");
        let parallel_evidence = root.join("parallel-evidence.parquet");
        let parallel_catalog = root.join("parallel-catalog.json");
        let mut single_progress = |_, _, _, _, _| {};
        let single = convert_structured_with_workers(
            &input,
            StructuredOutputPaths {
                consequences: &single_consequences,
                evidence: &single_evidence,
                catalog: &single_catalog,
            },
            None,
            None,
            &BTreeMap::new(),
            &|| false,
            &mut single_progress,
            1,
        )
        .unwrap();
        let mut parallel_progress = |_, _, _, _, _| {};
        let parallel = convert_structured_with_workers(
            &input,
            StructuredOutputPaths {
                consequences: &parallel_consequences,
                evidence: &parallel_evidence,
                catalog: &parallel_catalog,
            },
            None,
            None,
            &BTreeMap::new(),
            &|| false,
            &mut parallel_progress,
            8,
        )
        .unwrap();

        assert_eq!(parallel, single);
        assert_eq!(
            fs::read(parallel_consequences).unwrap(),
            fs::read(single_consequences).unwrap()
        );
        assert_eq!(
            fs::read(parallel_evidence).unwrap(),
            fs::read(single_evidence).unwrap()
        );
        assert_eq!(
            fs::read(parallel_catalog).unwrap(),
            fs::read(single_catalog).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
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
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"start_lost","variant_type":"Snv","transcript_consequences":[{"variant_allele":"G","consequence_terms":["start_lost"],"impact":"HIGH","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000641515","biotype":"protein_coding","canonical":1,"mane_select":"ENST00000641515.2","hgvsg":"1:g.65565A>G","hgvsc":"ENST00000641515.2:c.1A>G","hgvsp":"ENSP00000493376.1:p.Met1Val","cadd":{"raw":1.25,"phred":12.5},"clinvar":"Likely_benign","custom_source":{"labels":["one","two"],"score":"0.25"}},{"variant_allele":"G","consequence_terms":["downstream_gene_variant"],"impact":"MODIFIER","gene_id":"ENSG00000290826","transcript_id":"ENST00000832531","biotype":"lncRNA","distance":2039,"cadd":{"raw":1.25,"phred":12.5},"custom_source":{"labels":["one","two"],"score":"0.25"}}]}"#,
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
        assert_eq!(summary.evidence, 9);
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
        let custom_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?)
                 WHERE source_id='custom_source' AND scope='transcript'",
                params![evidence.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            custom_rows, 4,
            "unknown equal-valued source objects remain feature scoped"
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
        assert_eq!(catalog_value["fields"].as_array().unwrap().len(), 8);
        let id = allele_id("1", 65565, "A", "G");
        let detail: Value =
            serde_json::from_str(&detail_json(&consequences, &evidence, &id).unwrap()).unwrap();
        assert_eq!(detail["evidence"].as_array().unwrap().len(), 8);
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
                None,
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(indexed_detail["variant"]["geneSymbol"], "OR4F5");
        assert_eq!(indexed_detail["variant"]["alternateCount"], 1);
        assert_eq!(indexed_detail["consequences"].as_array().unwrap().len(), 1);
        assert_eq!(indexed_detail["evidence"].as_array().unwrap().len(), 8);
        let query_evidence = root.join("query-evidence");
        fs::create_dir(&query_evidence).unwrap();
        fs::copy(&evidence, query_evidence.join("canonical.parquet")).unwrap();
        fs::copy(&evidence, query_evidence.join("favor.parquet")).unwrap();
        let composite_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&query_evidence.join("*.parquet")),
                None,
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(composite_detail["evidence"].as_array().unwrap().len(), 16);
        let legacy = root.join("legacy");
        fs::create_dir(&legacy).unwrap();
        let legacy_variants = legacy.join("variants.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                    SELECT * EXCLUDE (alternate_count) FROM read_parquet('{}')
                 ) TO '{}' (FORMAT PARQUET)",
                variants.to_string_lossy().replace('\'', "''"),
                legacy_variants.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        let legacy_consequences = legacy.join("consequences.parquet");
        let legacy_evidence = legacy.join("evidence.parquet");
        fs::copy(&consequences, &legacy_consequences).unwrap();
        fs::copy(&evidence, &legacy_evidence).unwrap();
        let legacy_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &legacy_variants,
                Some(&legacy_consequences),
                Some(&legacy_evidence),
                None,
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(legacy_detail["variant"]["alternateCount"], 1);
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
                None,
                &id,
                Some(record_number),
                Some(alt_index),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(fallback_detail["consequences"].as_array().unwrap().len(), 1);
        assert_eq!(fallback_detail["evidence"].as_array().unwrap().len(), 8);
        fs::write(&detail_index, b"not a valid index").unwrap();
        assert!(
            complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&evidence),
                None,
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
                        values: None,
                        include_missing: None,
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
                        values: None,
                        include_missing: None,
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(clinvar_filtered["total"], 1);
        for (operator, total) in [("equals", 1), ("in", 1), ("not_equals", 0), ("not_in", 0)] {
            let exact: Value = serde_json::from_str(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &PageRequest {
                        evidence_filters: vec![EvidenceFilterRequest {
                            index: clinvar_index,
                            operator: operator.into(),
                            value: "Pathogenic".into(),
                            value2: String::new(),
                            values: None,
                            include_missing: None,
                        }],
                        ..PageRequest::default()
                    },
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(exact["total"], total, "operator {operator}");
        }
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
        let text_search_with_numeric_field: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    search: "path".into(),
                    evidence_columns: vec![clinvar_index, revel_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(text_search_with_numeric_field["total"], 1);
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
        assert_eq!(numeric_search["total"], 1);
        let hidden_numeric_search: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    search: "0.31".into(),
                    evidence_columns: vec![clinvar_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(hidden_numeric_search["total"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn allele_sources_are_normalized_once_at_ingestion() {
        let root = std::env::temp_dir().join(format!(
            "annocat-evidence-scope-fallback-{}-{}",
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
                r#"{"allele_string":"A/G","start":65564,"end":65564,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"gnomad":{"allAf":0}}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"gnomad":{"allAf":0.25}},{"variant_allele":"G","consequence_terms":["intron_variant"],"impact":"MODIFIER","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","gnomad":{"allAf":0.25}}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65566,"end":65566,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"gnomad":{"allAf":0.1,"alleleCount":4}},{"variant_allele":"G","consequence_terms":["intron_variant"],"impact":"MODIFIER","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","gnomad":{"allAf":0.2,"alleleCount":4}}]}"#,
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
        let fields = catalog_value["fields"].as_array().unwrap();
        let allele_index = fields
            .iter()
            .position(|field| {
                field["scope"] == "allele"
                    && field["sourceId"] == "gnomad"
                    && field["fieldPath"] == "allAf"
            })
            .unwrap();
        let count_index = fields
            .iter()
            .position(|field| {
                field["scope"] == "allele"
                    && field["sourceId"] == "gnomad"
                    && field["fieldPath"] == "alleleCount"
            })
            .unwrap();
        assert!(!fields.iter().any(|field| {
            field["scope"] == "transcript"
                && field["sourceId"] == "gnomad"
                && field["fieldPath"] == "allAf"
        }));

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t65564\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000001|A/G|YES|\n1\t65565\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000001|A/G|YES|\n1\t65566\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000001|A/G|YES|\n",
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
                    evidence_columns: vec![allele_index, count_index],
                    sort_evidence: Some(allele_index),
                    direction: "desc".into(),
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["rows"][0]["position"], 65565);
        assert_eq!(
            page["rows"][0]["evidence"][allele_index.to_string()]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            0.25
        );
        assert_eq!(page["rows"][1]["position"], 65564);
        assert_eq!(
            page["rows"][1]["evidence"][allele_index.to_string()]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            0.0
        );
        let conflicting = page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["position"] == 65566)
            .unwrap();
        assert!(
            conflicting["evidence"]
                .get(allele_index.to_string())
                .is_none()
        );
        assert_eq!(
            conflicting["evidence"][count_index.to_string()]
                .as_str()
                .unwrap(),
            "4"
        );
        let filtered: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&evidence),
                Some(&catalog),
                0,
                10,
                &PageRequest {
                    evidence_filters: vec![EvidenceFilterRequest {
                        index: allele_index,
                        operator: "gte".into(),
                        value: "0".into(),
                        value2: String::new(),
                        values: None,
                        include_missing: None,
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(filtered["total"], 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_source_aliases_preserve_logical_gnomad_identity() {
        assert_eq!(
            structured_source_aliases(&["gnomad-genomes".into()])
                .unwrap()
                .get("gnomad")
                .map(String::as_str),
            Some("gnomad-genomes")
        );
        assert!(
            structured_source_aliases(&["gnomad".into(), "gnomad-genomes".into()])
                .unwrap_err()
                .contains("share FastVEP key gnomad")
        );
    }

    #[test]
    fn structured_source_aliases_ignore_fastvep_key_case() {
        let aliases = structured_source_aliases(&["spliceai".into()]).unwrap();
        assert_eq!(
            structured_source_alias(&aliases, "spliceAI").map(String::as_str),
            Some("spliceai")
        );
    }

    #[test]
    fn text_search_skips_numeric_evidence_fields() {
        let score = SelectedEvidenceColumn {
            index: 0,
            scope: "allele".into(),
            biological_scope: "allele".into(),
            equivalent_scopes: vec!["allele".into()],
            source_id: "cadd".into(),
            field_path: "phred".into(),
            value_type: "number".into(),
            resolution: EvidenceResolutionStrategy::Allele,
        };
        assert!(!evidence_field_can_match_search(&score, "stop"));
        assert!(evidence_field_can_match_search(&score, "10.9"));
        let score_expression = evidence_search_value_expression(&score, "ev");
        assert!(score_expression.contains("number_value"));
        assert!(!score_expression.contains("string_value"));

        let text = SelectedEvidenceColumn {
            value_type: "string".into(),
            field_path: "significance".into(),
            ..score
        };
        let text_expression = evidence_search_value_expression(&text, "ev");
        assert!(text_expression.contains("string_value"));
        assert!(!text_expression.contains("number_value"));
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
    fn exact_list_filters_keep_text_and_numbers_typed() {
        let (text_sql, text_values) = comparison_sql(
            "v.gene_symbol",
            FilterValueKind::Text,
            "not_in",
            "BRCA1, TP53",
        )
        .unwrap();
        assert!(text_sql.contains("NOT IN (?,?)"));
        assert_eq!(text_values.len(), 2);

        let (number_sql, number_values) =
            comparison_sql("v.position", FilterValueKind::Number, "in", "10, 20.5").unwrap();
        assert!(number_sql.contains("CAST(v.position AS DOUBLE) IN (?,?)"));
        assert_eq!(number_values.len(), 2);
        assert!(
            comparison_sql("v.position", FilterValueKind::Number, "in", "10, nope")
                .unwrap_err()
                .contains("finite numbers")
        );
    }

    #[test]
    fn consequence_lists_match_each_partial_term() {
        let (sql, values) = text_contains_list_sql(
            "v.consequence",
            "not_in",
            "intron, upstream, downstream, prime",
        )
        .unwrap();
        assert!(sql.starts_with("NOT ("));
        assert_eq!(sql.matches("contains(").count(), 4);
        assert_eq!(values.len(), 4);
    }

    #[test]
    fn table_zygosity_uses_the_selected_alternate_allele() {
        let samples = r#"[{"name":"CASE","value":"1/2:20,8,3"}]"#;
        assert_eq!(table_zygosity(Some("GT:AD"), samples, 1, 2), "Heterozygous");
        assert_eq!(table_zygosity(Some("GT:AD"), samples, 2, 2), "Heterozygous");
        assert_eq!(
            table_zygosity(Some("GT"), r#"[{"name":"CASE","value":"0/1"}]"#, 2, 2),
            "Other alternate"
        );
    }

    #[test]
    fn zygosity_sort_uses_the_selected_alternate_allele() {
        let connection = Connection::open_in_memory().unwrap();
        register_sample_call_macros(&connection).unwrap();
        let sort = page_sort_expression("zygosity").unwrap();
        assert_eq!(sort.0, "zygosity");

        let rank = |format: Option<&str>, samples: &str, alt_index, alternate_count| {
            connection
                .query_row(
                    "SELECT annocat_zygosity_sort(?, ?, ?, ?)",
                    params![format, samples, alt_index, alternate_count],
                    |row| row.get::<_, Option<i32>>(0),
                )
                .unwrap()
        };
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"0/0"}]"#, 1, 2),
            Some(0)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"0/2"}]"#, 1, 2),
            Some(1)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"1/2"}]"#, 1, 2),
            Some(2)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"1"}]"#, 1, 2),
            Some(3)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"0/1/2"}]"#, 1, 2),
            Some(4)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"1/1"}]"#, 1, 2),
            Some(5)
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"./."}]"#, 1, 2),
            None
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"0/."}]"#, 1, 2),
            None
        );
        assert_eq!(
            rank(Some("GT"), r#"[{"name":"S","value":"0/3"}]"#, 1, 2),
            None
        );
        assert_eq!(
            rank(
                Some("GT"),
                r#"[{"name":"S1","value":"0/1"},{"name":"S2","value":"1/1"}]"#,
                1,
                2,
            ),
            None
        );
    }

    #[test]
    fn zygosity_filter_labels_use_the_selected_alternate_allele() {
        let connection = Connection::open_in_memory().unwrap();
        register_sample_call_macros(&connection).unwrap();
        let label = |format: Option<&str>, samples: &str, alt_index, alternate_count| {
            connection
                .query_row(
                    "SELECT annocat_zygosity_label(?, ?, ?, ?)",
                    params![format, samples, alt_index, alternate_count],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
        };
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"0/1"}]"#, 1, 2),
            Some("Heterozygous".into())
        );
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"0/2"}]"#, 1, 2),
            Some("Other alternate".into())
        );
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"1/1"}]"#, 1, 2),
            Some("Homozygous alternate".into())
        );
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"./."}]"#, 1, 2),
            Some("Not called".into())
        );
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"0/."}]"#, 1, 2),
            Some("Partially called".into())
        );
        assert_eq!(
            label(Some("GT"), r#"[{"name":"S","value":"x/y"}]"#, 1, 2),
            Some("Invalid genotype".into())
        );
        assert_eq!(
            label(
                Some("GT"),
                r#"[{"name":"S1","value":"0/1"},{"name":"S2","value":"1/1"}]"#,
                1,
                2,
            ),
            Some("Multiple sample calls".into())
        );
        assert_eq!(label(Some("GT"), "[]", 1, 2), None);
    }

    #[test]
    fn zygosity_is_a_categorical_core_filter() {
        let request = PageRequest {
            filter_rules: vec![CoreFilterRuleRequest {
                column: "zygosity".into(),
                operator: "in".into(),
                value: String::new(),
                values: Some(vec!["Heterozygous".into()]),
                include_missing: Some(false),
            }],
            ..PageRequest::default()
        };
        let (sql, values) = core_filter_rules_sql(&request).unwrap();
        assert!(sql.contains("v.zygosity"));
        assert!(!sql.contains("annocat_zygosity_label"));
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn zygosity_filter_rejects_unknown_values_before_querying() {
        let request = PageRequest {
            filter_rules: vec![CoreFilterRuleRequest {
                column: "zygosity".into(),
                operator: "in".into(),
                value: String::new(),
                values: Some(vec!["haploid".into()]),
                include_missing: Some(false),
            }],
            ..PageRequest::default()
        };
        assert_eq!(
            core_filter_rules_sql(&request).unwrap_err(),
            "unsupported zygosity filter value: haploid; choose a listed value"
        );
    }

    #[test]
    fn legacy_zygosity_filter_builds_one_bounded_projection_after_validation() {
        let root = std::env::temp_dir().join(format!(
            "annocat-legacy-zygosity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("sample.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE\">\n##FORMAT=<ID=GT,Number=1,Type=String,Description=\"Genotype\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tCASE\n1\t100\t.\tA\tG\t50\tPASS\tCSQ=G|intron_variant|MODIFIER|GENE1|ENSG00000000001|ENST00000000001|A/G\tGT\t1\n",
        )
        .unwrap();
        let current = root.join("current.parquet");
        convert_vcf(&vcf, &current, || false, |_, _, _, _, _| {}).unwrap();
        let direct: Value = serde_json::from_str(
            &page_json(
                &current,
                0,
                10,
                &PageRequest {
                    filter_rules: vec![CoreFilterRuleRequest {
                        column: "zygosity".into(),
                        operator: "in".into(),
                        value: String::new(),
                        values: Some(vec!["Haploid alternate".into()]),
                        include_missing: Some(false),
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(direct["rows"][0]["zygosity"], "Haploid alternate");
        let legacy = root.join("variants.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                    SELECT * EXCLUDE (alternate_count, zygosity, zygosity_sort)
                    FROM read_parquet('{}')
                 ) TO '{}' (FORMAT PARQUET, COMPRESSION ZSTD)",
                current.to_string_lossy().replace('\'', "''"),
                legacy.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        let sidecar_count = || {
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with(SAMPLE_CALL_PROJECTION_PREFIX))
                })
                .count()
        };
        let request = |value: &str| PageRequest {
            filter_rules: vec![CoreFilterRuleRequest {
                column: "zygosity".into(),
                operator: "in".into(),
                value: String::new(),
                values: Some(vec![value.into()]),
                include_missing: Some(false),
            }],
            ..PageRequest::default()
        };

        assert!(page_json(&legacy, 0, 10, &request("haploid")).is_err());
        assert_eq!(sidecar_count(), 0);
        let page: Value = serde_json::from_str(
            &page_json(&legacy, 0, 10, &request("Haploid alternate")).unwrap(),
        )
        .unwrap();
        assert_eq!(page["rows"].as_array().unwrap().len(), 1);
        assert_eq!(page["rows"][0]["zygosity"], "Haploid alternate");
        assert_eq!(sidecar_count(), 1);
        page_json(&legacy, 0, 10, &request("Haploid alternate")).unwrap();
        assert_eq!(sidecar_count(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favor_predictor_summaries_are_provider_selected() {
        let root = std::env::temp_dir().join(format!(
            "annocat-favor-resolution-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let catalog = root.join("field-catalog.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "fields": [
                    {
                        "scope": "feature",
                        "biologicalScope": "feature",
                        "physicalScope": "selected",
                        "sourceId": "favor-online",
                        "fieldPath": "codingRevelScore",
                        "valueType": "number",
                        "resolutionPolicy": "materializedSelected",
                        "selectionOrigin": "provider"
                    },
                    {
                        "scope": "feature",
                        "biologicalScope": "feature",
                        "physicalScope": "allele",
                        "sourceId": "favor-online",
                        "fieldPath": "revel",
                        "valueType": "number",
                        "resolutionPolicy": "providerSelected",
                        "selectionOrigin": "provider"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        for field in selected_evidence_columns(&catalog, &[0, 1]).unwrap() {
            assert!(field.resolution == EvidenceResolutionStrategy::SourceSelected);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_allele_fields_accept_only_their_equivalent_feature_scope() {
        let root = std::env::temp_dir().join(format!(
            "annocat-legacy-scope-resolution-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let catalog = root.join("field-catalog.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "fields": [
                    {
                        "scope": "allele",
                        "sourceId": "gnomad",
                        "fieldPath": "allAf",
                        "valueType": "number"
                    },
                    {
                        "scope": "transcript",
                        "sourceId": "gnomad",
                        "fieldPath": "allAf",
                        "valueType": "number"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let field = selected_evidence_columns(&catalog, &[0]).unwrap().remove(0);
        assert!(field.resolution == EvidenceResolutionStrategy::LegacyAlleleRecovery);
        assert_eq!(field.equivalent_scopes, ["allele", "transcript"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_allele_recovery_is_shared_by_display_sort_filter_and_search() {
        let root = std::env::temp_dir().join(format!(
            "annocat-legacy-allele-recovery-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t71001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t71002\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t71003\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t71004\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();

        let evidence = root.join("evidence.parquet");
        let mut batch = EvidenceBatch::default();
        for (position, scope, consequence, value) in [
            (71001, "transcript", Some("t1"), 0.25),
            (71002, "transcript", Some("t1"), 0.4),
            (71002, "transcript", Some("t2"), 0.4),
            (71003, "transcript", Some("t1"), 0.1),
            (71003, "transcript", Some("t2"), 0.2),
            (71004, "allele", None, 0.3),
            (71004, "transcript", Some("t1"), 0.9),
        ] {
            batch.schema_version.push(SCHEMA_VERSION);
            batch.allele_id.push(allele_id("1", position, "A", "G"));
            batch.consequence_id.push(consequence.map(str::to_owned));
            batch.scope.push(scope.into());
            batch.source_id.push("gnomad".into());
            batch.field_path.push("allAf".into());
            batch.value_type.push("number".into());
            batch.string_value.push(None);
            batch.integer_value.push(None);
            batch.number_value.push(Some(value));
            batch.boolean_value.push(None);
            batch.json_value.push(None);
        }
        let schema = EvidenceBatch::default()
            .into_record_batch()
            .unwrap()
            .schema();
        let mut writer = parquet_writer(&evidence, schema).unwrap();
        writer.write(&batch.into_record_batch().unwrap()).unwrap();
        writer.close().unwrap();

        let catalog = root.join("field-catalog.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "fields": [
                    {
                        "scope": "allele",
                        "sourceId": "gnomad",
                        "fieldPath": "allAf",
                        "valueType": "number"
                    },
                    {
                        "scope": "transcript",
                        "sourceId": "gnomad",
                        "fieldPath": "allAf",
                        "valueType": "number"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let page = |request: PageRequest| -> Value {
            serde_json::from_str(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &request,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let sorted = page(PageRequest {
            evidence_columns: vec![0],
            sort_evidence: Some(0),
            direction: "desc".into(),
            ..PageRequest::default()
        });
        assert_eq!(sorted["rows"][0]["position"], 71002);
        assert_eq!(sorted["rows"][1]["position"], 71004);
        assert_eq!(sorted["rows"][2]["position"], 71001);
        assert_eq!(sorted["rows"][3]["position"], 71003);
        assert_eq!(
            sorted["rows"][0]["evidenceResolution"]["0"]["kind"],
            "legacy_allele_scope_recovered"
        );
        assert_eq!(
            sorted["rows"][1]["evidenceResolution"]["0"]["kind"],
            "direct_allele"
        );
        assert!(sorted["rows"][3]["evidence"].get("0").is_none());
        assert_eq!(
            sorted["rows"][3]["evidenceResolution"]["0"]["kind"],
            "conflicting_legacy_values"
        );

        let filtered = page(PageRequest {
            evidence_filters: vec![EvidenceFilterRequest {
                index: 0,
                operator: "gt".into(),
                value: "0.35".into(),
                value2: String::new(),
                values: None,
                include_missing: None,
            }],
            ..PageRequest::default()
        });
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["rows"][0]["position"], 71002);

        let searched = page(PageRequest {
            search: "0.25".into(),
            evidence_columns: vec![0],
            ..PageRequest::default()
        });
        assert_eq!(searched["total"], 1);
        assert_eq!(searched["rows"][0]["position"], 71001);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_feature_fallback_is_source_independent() {
        let root = std::env::temp_dir().join(format!(
            "annocat-generic-feature-fallback-{}-{}",
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
            r#"{"allele_string":"A/G","start":72001,"end":72001,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","mane_select":"ENST000001.1","futurepredictor":{"score":0.6}},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","futurepredictor":{"score":0.2}}]}"#,
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
        let legacy_consequences = root.join("legacy-consequences.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                   SELECT * EXCLUDE (feature_type)
                   FROM read_parquet('{}')
                 ) TO '{}' (FORMAT PARQUET)",
                consequences.to_string_lossy().replace('\'', "''"),
                legacy_consequences.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        fs::remove_file(&consequences).unwrap();
        fs::rename(legacy_consequences, &consequences).unwrap();
        let catalog_value: Value = serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        let score_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| {
                field["sourceId"] == "futurepredictor" && field["fieldPath"] == "score"
            })
            .unwrap();
        assert_eq!(
            catalog_value["fields"][score_index]["resolutionPolicy"],
            "materializedSelected"
        );

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t72001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST_MISSING|A/G|YES\n",
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
        assert_eq!(page["rows"][0]["evidence"][score_index.to_string()], "0.6");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gene_evidence_follows_the_shared_representative_feature() {
        let root = std::env::temp_dir().join(format!(
            "annocat-gene-feature-resolution-{}-{}",
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
            r#"{"allele_string":"A/G","start":73001,"end":73001,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","mane_select":"ENST000001.1","gnomad-constraint":{"oeLoF":0.6}},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","gnomad-constraint":{"oeLoF":0.9}}]}"#,
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
                field["scope"] == "gene"
                    && field["sourceId"] == "gnomad-constraint"
                    && field["fieldPath"] == "oeLoF"
            })
            .unwrap();
        assert_eq!(
            catalog_value["fields"][score_index]["biologicalScope"],
            "gene"
        );

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t73001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST_MISSING|A/G|YES\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        let page = |request| {
            serde_json::from_str::<Value>(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &request,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let displayed = page(PageRequest {
            evidence_columns: vec![score_index],
            ..PageRequest::default()
        });
        assert_eq!(
            displayed["rows"][0]["evidence"][score_index.to_string()],
            "0.6"
        );
        let alternate_only = page(PageRequest {
            evidence_filters: vec![EvidenceFilterRequest {
                index: score_index,
                operator: "gt".into(),
                value: "0.8".into(),
                value2: String::new(),
                values: None,
                include_missing: None,
            }],
            ..PageRequest::default()
        });
        assert_eq!(alternate_only["total"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn consequence_linked_evidence_uses_one_selected_transcript_everywhere() {
        let root = std::env::temp_dir().join(format!(
            "annocat-selected-consequence-{}-{}",
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
                r#"{"allele_string":"A/G","start":70001,"end":70001,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1","revel":{"score":0.2,"transcriptId":"ENST000001"}},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","revel":{"score":0.9,"transcriptId":"ENST000002"}}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":70002,"end":70002,"seq_region_name":"1","most_severe_consequence":"missense_variant","transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1","revel":{"score":0.8,"transcriptId":"ENST000001"}},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002","revel":{"score":0.1,"transcriptId":"ENST000002"}}]}"#,
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
                field["scope"] == "transcript"
                    && field["sourceId"] == "revel"
                    && field["fieldPath"] == "score"
            })
            .unwrap();

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t70001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000001|A/G|YES|ENST000001.1\n1\t70002\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000001|A/G|YES|ENST000001.1\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&json!({
                "representativeSelectionContract": REPRESENTATIVE_SELECTION_CONTRACT
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            prepare_recommended_query_projections(&variants, &evidence, &catalog).unwrap(),
            1
        );
        let resolution_cache = crate::evidence_resolution::available_path(&evidence);
        let projection_caches = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_query_projection(path))
            .collect::<Vec<_>>();
        assert_eq!(projection_caches.len(), 1);
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".annocat-representatives-"))
        );
        let cache_state = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|path| {
                    let metadata = fs::metadata(path).unwrap();
                    (path.clone(), metadata.len(), metadata.modified().unwrap())
                })
                .collect::<Vec<_>>()
        };
        let cache_paths = resolution_cache
            .into_iter()
            .chain(projection_caches)
            .collect::<Vec<_>>();
        let prewarmed_state = cache_state(&cache_paths);

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
        assert_eq!(sorted["rows"][0]["position"], 70002);
        assert_eq!(
            sorted["rows"][0]["evidence"][score_index.to_string()],
            "0.8"
        );
        assert_eq!(
            sorted["rows"][1]["evidence"][score_index.to_string()],
            "0.2"
        );
        assert_eq!(
            sorted["rows"][0]["evidenceResolution"][score_index.to_string()]["kind"],
            "exact_consequence"
        );
        assert_eq!(
            prepare_recommended_query_projections(&variants, &evidence, &catalog).unwrap(),
            1
        );
        assert_eq!(cache_state(&cache_paths), prewarmed_state);

        let alternate_transcript_filter: Value = serde_json::from_str(
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
                        value: "0.85".into(),
                        value2: String::new(),
                        values: None,
                        include_missing: None,
                    }],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(alternate_transcript_filter["total"], 0);

        for (search, expected) in [("0.9", 0), ("0.2", 1)] {
            let page: Value = serde_json::from_str(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &PageRequest {
                        search: search.into(),
                        evidence_columns: vec![score_index],
                        ..PageRequest::default()
                    },
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(page["total"], expected);
        }
        assert!(crate::evidence_resolution::available_path(&evidence).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dbnsfp_record_lists_are_resolved_once_for_all_viewer_operations() {
        let root = std::env::temp_dir().join(format!(
            "annocat-dbnsfp-record-list-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let record = |position: u64, selected_score: &str| {
            json!({
                "allele_string": "A/G",
                "start": position,
                "end": position,
                "seq_region_name": "1",
                "most_severe_consequence": "missense_variant",
                "dbnsfp": [{
                    "Ensembl_transcriptid": "ENST000001;ENST000002",
                    "Ensembl_proteinid": "ENSP000001;ENSP000002",
                    "Uniprot_acc": "P1;P2",
                    "Uniprot_entry": "ENTRY1;ENTRY2",
                    "aaref": "R",
                    "aaalt": "H",
                    "aapos": "10;20",
                    "HGVSp_VEP": "p.Arg10His;p.Arg20His",
                    "REVEL_score": format!("0.1;{selected_score}"),
                    "AlphaMissense_score": "0.0539;0.0543",
                    "AlphaMissense_pred": "B;B",
                    "PrimateAI_score": "0.347287714481",
                    "CADD_phred": "20"
                }],
                "transcript_consequences": [{
                    "variant_allele": "G",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE",
                    "gene_symbol": "GENE1",
                    "gene_id": "ENSG1",
                    "transcript_id": "ENST000001",
                    "protein_id": "ENSP000001",
                    "amino_acids": "R/H",
                    "protein_start": 10
                }, {
                    "variant_allele": "G",
                    "consequence_terms": ["missense_variant"],
                    "impact": "MODERATE",
                    "gene_symbol": "GENE1",
                    "gene_id": "ENSG1",
                    "transcript_id": "ENST000002",
                    "protein_id": "ENSP000002",
                    "amino_acids": "R/H",
                    "protein_start": 20,
                    "hgvsp": "ENSP000002:p.Arg20His",
                    "mane_select": "ENST000002.1"
                }]
            })
        };
        let input = root.join("fastvep.ndjson");
        fs::write(
            &input,
            format!("{}\n{}\n", record(65601, "0.8"), record(65602, "0.2")),
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
        let fields = catalog_value["fields"].as_array().unwrap();
        assert!(
            fields
                .iter()
                .all(|field| field["fieldPath"] != "__recordList")
        );
        let score_index = fields
            .iter()
            .position(|field| {
                field["scope"] == "selected"
                    && field["sourceId"] == "dbnsfp"
                    && field["fieldPath"] == "REVEL_score"
            })
            .unwrap();
        let alpha_index = fields
            .iter()
            .position(|field| {
                field["scope"] == "selected"
                    && field["sourceId"] == "dbnsfp"
                    && field["fieldPath"] == "AlphaMissense_score"
            })
            .unwrap();
        assert_eq!(fields[score_index]["biologicalScope"], "transcript");
        assert_eq!(fields[score_index]["physicalScope"], "selected");
        let connection = Connection::open_in_memory().unwrap();
        let raw_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?) WHERE scope='source_records' AND field_path='__recordList'",
                params![evidence.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw_rows, 2);

        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t65601\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002|A/G|ENST000002.1\n1\t65602\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002|A/G|ENST000002.1\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&json!({
                "representativeSelectionContract": REPRESENTATIVE_SELECTION_CONTRACT
            }))
            .unwrap(),
        )
        .unwrap();

        let page = |request| {
            serde_json::from_str::<Value>(
                &page_json_with_evidence(
                    &variants,
                    Some(&evidence),
                    Some(&catalog),
                    0,
                    10,
                    &request,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let sorted = page(PageRequest {
            evidence_columns: vec![score_index, alpha_index],
            sort_evidence: Some(score_index),
            direction: "desc".into(),
            ..PageRequest::default()
        });
        assert_eq!(sorted["rows"][0]["position"], 65601);
        assert_eq!(
            sorted["rows"][0]["evidence"][score_index.to_string()],
            "0.8"
        );
        assert_eq!(
            sorted["rows"][0]["evidence"][alpha_index.to_string()],
            "0.0543"
        );
        let detail: Value = serde_json::from_str(
            &detail_json(
                &consequences,
                &evidence,
                sorted["rows"][0]["alleleId"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let detail_evidence = detail["evidence"].as_array().unwrap();
        let transcript_values = |rows: &[Value], field_path: &str| {
            rows.iter()
                .filter(|row| {
                    row["scope"] == "transcript"
                        && row["sourceId"] == "dbnsfp"
                        && row["fieldPath"] == field_path
                })
                .map(|row| {
                    (
                        row["consequenceId"].as_str().unwrap_or_default().to_owned(),
                        row["value"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let detail_revel = transcript_values(detail_evidence, "REVEL_score");
        assert_eq!(detail_revel.len(), 2);
        assert!(detail_revel.iter().any(|(_, value)| value == "0.1"));
        assert!(detail_revel.iter().any(|(_, value)| value == "0.8"));
        assert!(detail_revel.iter().all(|(id, _)| !id.is_empty()));
        assert_ne!(detail_revel[0].0, detail_revel[1].0);
        let second_detail: Value = serde_json::from_str(
            &detail_json(
                &consequences,
                &evidence,
                sorted["rows"][1]["alleleId"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let second_revel =
            transcript_values(second_detail["evidence"].as_array().unwrap(), "REVEL_score");
        assert!(second_revel.iter().any(|(_, value)| value == "0.1"));
        assert!(second_revel.iter().any(|(_, value)| value == "0.2"));
        let detail_alpha = transcript_values(detail_evidence, "AlphaMissense_score");
        assert_eq!(detail_alpha.len(), 2);
        assert!(detail_alpha.iter().any(|(_, value)| value == "0.0539"));
        assert!(detail_alpha.iter().any(|(_, value)| value == "0.0543"));
        let detail_primate = transcript_values(detail_evidence, "PrimateAI_score");
        assert_eq!(detail_primate.len(), 2);
        assert!(
            detail_primate
                .iter()
                .all(|(_, value)| value == "0.347287714481")
        );
        assert!(
            detail_evidence
                .iter()
                .filter(|row| {
                    row["sourceId"] == "dbnsfp" && row["fieldPath"] == "AlphaMissense_score"
                })
                .all(|row| row["sourceCardinality"] == "alignedVector")
        );
        assert!(
            detail_evidence
                .iter()
                .filter(|row| {
                    row["sourceId"] == "dbnsfp" && row["fieldPath"] == "PrimateAI_score"
                })
                .all(|row| row["sourceCardinality"] == "recordScalar")
        );
        assert!(
            detail_evidence.iter().all(|row| {
                row["scope"] != "source_records" && row["fieldPath"] != "__recordList"
            })
        );
        let indexed_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&evidence),
                Some(&catalog),
                sorted["rows"][0]["alleleId"].as_str().unwrap(),
                sorted["rows"][0]["recordNumber"].as_i64(),
                sorted["rows"][0]["altIndex"]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok()),
            )
            .unwrap(),
        )
        .unwrap();
        let indexed_evidence = indexed_detail["evidence"].as_array().unwrap();
        assert_eq!(
            transcript_values(indexed_evidence, "REVEL_score"),
            detail_revel
        );
        assert_eq!(
            transcript_values(indexed_evidence, "AlphaMissense_score"),
            detail_alpha
        );
        assert!(
            indexed_evidence.iter().all(|row| {
                row["scope"] != "source_records" && row["fieldPath"] != "__recordList"
            })
        );
        assert_eq!(
            page(PageRequest {
                evidence_filters: vec![EvidenceFilterRequest {
                    index: score_index,
                    operator: "gt".into(),
                    value: "0.5".into(),
                    value2: String::new(),
                    values: None,
                    include_missing: None,
                }],
                ..PageRequest::default()
            })["total"],
            1
        );
        assert_eq!(
            page(PageRequest {
                search: "0.8".into(),
                evidence_columns: vec![score_index],
                ..PageRequest::default()
            })["total"],
            1
        );
        assert!(crate::evidence_resolution::available_path(&evidence).is_none());

        let legacy_evidence = root.join("legacy-evidence.parquet");
        let source = evidence.to_string_lossy().replace('\'', "''");
        let target = legacy_evidence.to_string_lossy().replace('\'', "''");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                   SELECT * REPLACE (
                     CASE WHEN scope='selected' AND source_id='dbnsfp'
                                    AND field_path='AlphaMissense_score'
                          THEN 'string' ELSE value_type END AS value_type,
                     CASE WHEN scope='selected' AND source_id='dbnsfp'
                                    AND field_path='AlphaMissense_score'
                          THEN '0.0539;0.0543' ELSE string_value END AS string_value,
                     CASE WHEN scope='selected' AND source_id='dbnsfp'
                                    AND field_path='AlphaMissense_score'
                          THEN NULL ELSE number_value END AS number_value
                   )
                   FROM read_parquet('{source}')
                 ) TO '{target}' (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 4096)"
            ))
            .unwrap();
        let legacy_catalog = root.join("legacy-field-catalog.json");
        let mut legacy_catalog_value = catalog_value;
        legacy_catalog_value
            .as_object_mut()
            .unwrap()
            .remove("recordResolutionContracts");
        fs::write(
            &legacy_catalog,
            serde_json::to_vec(&legacy_catalog_value).unwrap(),
        )
        .unwrap();
        let legacy_page = |request| {
            serde_json::from_str::<Value>(
                &page_json_with_evidence(
                    &variants,
                    Some(&legacy_evidence),
                    Some(&legacy_catalog),
                    0,
                    10,
                    &request,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let recovered = legacy_page(PageRequest {
            evidence_columns: vec![alpha_index],
            sort_evidence: Some(alpha_index),
            direction: "desc".into(),
            ..PageRequest::default()
        });
        assert_eq!(
            recovered["rows"][0]["evidence"][alpha_index.to_string()],
            "0.0543"
        );
        assert_eq!(
            recovered["rows"][0]["evidenceResolution"][alpha_index.to_string()]["kind"],
            "exact_transcript"
        );
        assert_eq!(
            legacy_page(PageRequest {
                evidence_filters: vec![EvidenceFilterRequest {
                    index: alpha_index,
                    operator: "gt".into(),
                    value: "0.054".into(),
                    value2: String::new(),
                    values: None,
                    include_missing: None,
                }],
                ..PageRequest::default()
            })["total"],
            2
        );
        assert_eq!(
            legacy_page(PageRequest {
                search: "0.0543".into(),
                evidence_columns: vec![alpha_index],
                ..PageRequest::default()
            })["total"],
            2
        );
        assert_eq!(
            legacy_page(PageRequest {
                search: "0.0539".into(),
                evidence_columns: vec![alpha_index],
                ..PageRequest::default()
            })["total"],
            0
        );
        let legacy_detail: Value = serde_json::from_str(
            &complete_detail_json_at(
                &variants,
                Some(&consequences),
                Some(&legacy_evidence),
                Some(&legacy_catalog),
                recovered["rows"][0]["alleleId"].as_str().unwrap(),
                recovered["rows"][0]["recordNumber"].as_i64(),
                recovered["rows"][0]["altIndex"]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok()),
            )
            .unwrap(),
        )
        .unwrap();
        let legacy_alpha = transcript_values(
            legacy_detail["evidence"].as_array().unwrap(),
            "AlphaMissense_score",
        );
        assert_eq!(legacy_alpha.len(), 2, "{legacy_detail}");
        assert!(legacy_alpha.iter().any(|(_, value)| value == "0.0539"));
        assert!(legacy_alpha.iter().any(|(_, value)| value == "0.0543"));
        assert!(crate::evidence_resolution::available_path(&legacy_evidence).is_some());

        let versioned_variants = root.join("versioned-variants.parquet");
        let source = variants.to_string_lossy().replace('\'', "''");
        let target = versioned_variants.to_string_lossy().replace('\'', "''");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (
                   SELECT * REPLACE ('ENST000002.1' AS transcript_id)
                   FROM read_parquet('{source}')
                 ) TO '{target}' (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 4096)"
            ))
            .unwrap();
        fs::copy(&versioned_variants, &variants).unwrap();
        let versioned_page: Value = serde_json::from_str(
            &page_json_with_evidence(
                &variants,
                Some(&legacy_evidence),
                Some(&legacy_catalog),
                0,
                10,
                &PageRequest {
                    evidence_columns: vec![alpha_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            versioned_page["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["evidence"][alpha_index.to_string()].is_null()),
            "a legacy vector must not resolve from a version-stripped transcript ID alone: {versioned_page}"
        );
        fs::remove_dir_all(root).unwrap();
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
                r#"{"allele_string":"A/G","start":65565,"end":65565,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":" ENST000001 ; ENST000002 ","AlphaMissense_score":"0.9;0.1"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1"},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002"}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65566,"end":65566,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":"ENST000001;ENST000002","AlphaMissense_score":"46;5.1"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1"},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002"}]}"#,
                "\n",
                r#"{"allele_string":"A/G","start":65567,"end":65567,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":"ENST000001.8;ENST000002.4","AlphaMissense_score":"0.7;0.3"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000001","canonical":true,"mane_select":"ENST000001.1"},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"GENE1","gene_id":"ENSG1","transcript_id":"ENST000002"}]}"#,
                "\n",
                r#"{"allele_string":"T/G","start":65568,"end":65568,"seq_region_name":"1","most_severe_consequence":"missense_variant","dbnsfp":{"Ensembl_transcriptid":"ENST00000641515;ENST00000335137","Ensembl_proteinid":"ENSP00000493376;ENSP00000334393","Uniprot_acc":"A0A2U3U0J3;Q8NH21","AlphaMissense_score":"0.0539;0.0543","AlphaMissense_pred":"B;B","REVEL_score":".;0.053"},"transcript_consequences":[{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000641515","protein_id":"ENSP00000493376","amino_acids":"T/A","protein_start":162},{"variant_allele":"G","consequence_terms":["missense_variant"],"impact":"MODERATE","gene_symbol":"OR4F5","gene_id":"ENSG00000186092","transcript_id":"ENST00000335137","protein_id":"ENSP00000334393","amino_acids":"T/A","protein_start":141,"mane_select":"ENST00000335137.4"}]}"#,
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
        let mut catalog_value: Value =
            serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        let score_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| {
                field["sourceId"] == "dbnsfp" && field["fieldPath"] == "AlphaMissense_score"
            })
            .unwrap();
        let revel_index = catalog_value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|field| field["sourceId"] == "dbnsfp" && field["fieldPath"] == "REVEL_score")
            .unwrap();
        // Simulate a report written before selected scalar rows were cataloged.
        let score_field = catalog_value["fields"][score_index]
            .as_object_mut()
            .unwrap();
        score_field.remove("physicalScope");
        score_field.remove("selectionOrigin");
        fs::write(&catalog, serde_json::to_vec(&catalog_value).unwrap()).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL|MANE_SELECT\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t65565\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002.1|A/G|YES|ENST000002.1\n1\t65566\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002.1|A/G|YES|ENST000002.1\n1\t65567\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST000002.1|A/G|YES|ENST000002.1\n1\t65568\t.\tT\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|OR4F5|ENSG00000186092|ENST00000335137.4|T/G|YES|ENST00000335137.4\n",
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
                    evidence_columns: vec![score_index, revel_index],
                    ..PageRequest::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["rows"][0]["position"], 65565);
        assert_eq!(page["rows"][0]["transcriptId"], "ENST000001");
        assert_eq!(page["rows"][0]["evidence"][score_index.to_string()], "0.9");
        assert_eq!(
            page["rows"][0]["evidenceResolution"][score_index.to_string()]["kind"],
            "exact_transcript"
        );
        assert!(
            page["rows"][2]["evidence"]
                .get(score_index.to_string())
                .is_none()
        );
        assert_eq!(
            page["rows"][2]["evidenceResolution"][score_index.to_string()]["kind"],
            "unresolved_transcript"
        );
        let or4f5 = page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["position"] == 65568)
            .unwrap();
        assert_eq!(or4f5["evidence"][score_index.to_string()], "0.0543");
        assert_eq!(or4f5["evidence"][revel_index.to_string()], "0.053");
        assert!(crate::evidence_resolution::available_path(&evidence).is_some());
        let cache = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(".annocat-evidence-") && name.ends_with(".parquet")
                    })
            })
            .unwrap();
        fs::write(&cache, b"corrupt").unwrap();
        let rebuilt: Value = serde_json::from_str(
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
        assert_eq!(
            rebuilt["rows"][0]["evidence"][score_index.to_string()],
            "0.9"
        );
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
        assert_eq!(sorted["rows"][0]["evidence"][score_index.to_string()], "46");
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
        for (threshold, expected) in [("0.8", 2), ("0.95", 1)] {
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
                            values: None,
                            include_missing: None,
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

    #[test]
    fn exact_sidecar_reads_escape_paths_without_binding_lists() {
        let path = PathBuf::from("C:\\results\\O'Brien\\field.parquet");
        let (read, parameters) = evidence_read(&path, Some(std::slice::from_ref(&path)));
        assert_eq!(
            read,
            "read_parquet(['C:\\results\\O''Brien\\field.parquet'])"
        );
        assert!(parameters.is_empty());

        let (read, parameters) = evidence_read(&path, None);
        assert_eq!(read, "read_parquet(?)");
        assert_eq!(parameters.len(), 1);
    }

    #[test]
    fn recommended_projections_follow_the_default_viewer_fields() {
        let catalog = json!({
            "fields": [
                {"scope":"allele","sourceId":"clinvar@20260810","fieldPath":"significance"},
                {"scope":"allele","sourceId":"gnomad-genomes","fieldPath":"allAF"},
                {"scope":"allele","sourceId":"phylop","fieldPath":"value"},
                {"scope":"allele","sourceId":"cadd","fieldPath":"phred"},
                {"scope":"selected","sourceId":"revel","fieldPath":"score"},
                {"scope":"selected","sourceId":"dbnsfp","fieldPath":"AlphaMissense_score"},
                {"scope":"selected","sourceId":"spliceai","fieldPath":"maxDeltaScore"},
                {"scope":"gene","sourceId":"spliceai","fieldPath":"gene"},
                {"scope":"allele","sourceId":"clinvar","fieldPath":"review_status"},
                {"scope":"transcript","sourceId":"gnomad-genomes","fieldPath":"allAF"}
            ]
        });
        assert_eq!(
            recommended_query_projection_indices(&catalog).unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn query_projection_preserves_direct_search_filter_and_sort() {
        let root = std::env::temp_dir().join(format!(
            "annocat-query-projection-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t81001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t81002\t.\tC\tT\t50\tPASS\tCSQ=T|missense_variant|MODERATE|GENE2|ENSG2|ENST2|C/T|YES\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();

        let evidence = root.join("evidence.parquet");
        let mut batch = EvidenceBatch::default();
        for (position, score) in [(81001, 0.1), (81002, 0.9)] {
            for (field_path, value_type) in [
                ("score", "number"),
                ("payload", "json"),
                ("alternate_score", "number"),
            ] {
                batch.schema_version.push(SCHEMA_VERSION);
                batch.allele_id.push(allele_id(
                    "1",
                    position,
                    if position == 81001 { "A" } else { "C" },
                    if position == 81001 { "G" } else { "T" },
                ));
                batch.consequence_id.push(None);
                batch.scope.push("allele".into());
                batch.source_id.push("test".into());
                batch.field_path.push(field_path.into());
                batch.value_type.push(value_type.into());
                batch.string_value.push(None);
                batch.integer_value.push(None);
                batch.number_value.push(match field_path {
                    "score" => Some(score),
                    "alternate_score" => Some(1.0 - score),
                    _ => None,
                });
                batch.boolean_value.push(None);
                batch
                    .json_value
                    .push((field_path == "payload").then(|| "{\"raw\":true}".into()));
            }
        }
        let schema = EvidenceBatch::default()
            .into_record_batch()
            .unwrap()
            .schema();
        let mut writer = parquet_writer(&evidence, schema).unwrap();
        writer.write(&batch.into_record_batch().unwrap()).unwrap();
        writer.close().unwrap();

        let catalog = root.join("field-catalog.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "fields": [
                    {
                        "scope": "allele",
                        "sourceId": "test",
                        "fieldPath": "score",
                        "valueType": "number",
                        "resolutionPolicy": "directAllele"
                    },
                    {
                        "scope": "allele",
                        "sourceId": "test",
                        "fieldPath": "payload",
                        "valueType": "json",
                        "resolutionPolicy": "directAllele"
                    },
                    {
                        "scope": "allele",
                        "sourceId": "test",
                        "fieldPath": "alternate_score",
                        "valueType": "number",
                        "resolutionPolicy": "directAllele"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let search_request = PageRequest {
            search: "0.9".into(),
            evidence_columns: vec![0],
            exact_total: true,
            ..PageRequest::default()
        };
        let projection_fields = request_query_projection_fields(&catalog, &search_request)
            .unwrap()
            .unwrap();
        let legacy_projection = root.join(".annocat-query-v2-0-stale.parquet");
        fs::write(&legacy_projection, b"stale").unwrap();
        page_json_with_evidence(
            &variants,
            Some(&evidence),
            Some(&catalog),
            0,
            10,
            &PageRequest {
                evidence_columns: vec![0],
                ..PageRequest::default()
            },
        )
        .unwrap();
        assert!(available_query_projection(&evidence, &catalog, &projection_fields).is_none());
        assert!(!query_projection_ready(Some(&evidence), Some(&catalog), &search_request).unwrap());
        let projection = prepare_query_projection(&evidence, &catalog, &projection_fields)
            .unwrap()
            .unwrap();
        assert!(!legacy_projection.exists());
        let projection_schema =
            ParquetRecordBatchReaderBuilder::try_new(File::open(&projection[0]).unwrap())
                .unwrap()
                .schema()
                .clone();
        assert!(projection_schema.index_of("record_number").is_ok());
        assert!(projection_schema.index_of("alt_index").is_ok());
        assert!(projection_schema.index_of("allele_id").is_err());
        fs::write(&legacy_projection, b"stale").unwrap();
        prepare_query_projection(&evidence, &catalog, &projection_fields).unwrap();
        assert!(!legacy_projection.exists());
        page_json_with_evidence_once(
            &variants,
            Some(&projection[0]),
            Some(&projection),
            Some(&catalog),
            0,
            10,
            &search_request,
            None,
        )
        .unwrap();
        assert!(available_query_projection(&evidence, &catalog, &projection_fields).is_some());
        assert!(query_projection_ready(Some(&evidence), Some(&catalog), &search_request).unwrap());
        let projection_files = || {
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| is_query_projection(path))
                .collect::<Vec<_>>()
        };
        let first_projection_files = projection_files();
        assert_eq!(first_projection_files.len(), 1);
        let mut expanded_catalog: Value =
            serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        expanded_catalog["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "scope": "gene",
                "sourceId": "gene-profile",
                "fieldPath": "geneMatch",
                "valueType": "boolean",
                "resolutionPolicy": "alleleGeneDirect"
            }));
        fs::write(&catalog, serde_json::to_vec(&expanded_catalog).unwrap()).unwrap();
        let expanded_projection_fields = request_query_projection_fields(&catalog, &search_request)
            .unwrap()
            .unwrap();
        page_json_with_evidence(
            &variants,
            Some(&evidence),
            Some(&catalog),
            0,
            10,
            &search_request,
        )
        .unwrap();
        assert!(
            available_query_projection(&evidence, &catalog, &expanded_projection_fields).is_some()
        );
        assert_eq!(projection_files(), first_projection_files);
        let connection = Connection::open_in_memory().unwrap();
        let (projection_read, projection_parameters) =
            evidence_read(&projection[0], Some(&projection));
        let json_rows: i64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {projection_read} WHERE field_index=1"),
                params_from_iter(projection_parameters.iter()),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(json_rows, 0);

        let mixed_search_request = PageRequest {
            search: "raw".into(),
            evidence_columns: vec![0, 1],
            exact_total: true,
            ..PageRequest::default()
        };
        let mixed_projection_fields =
            request_query_projection_fields(&catalog, &mixed_search_request)
                .unwrap()
                .unwrap();
        page_json_with_evidence(
            &variants,
            Some(&evidence),
            Some(&catalog),
            0,
            10,
            &mixed_search_request,
        )
        .unwrap();
        let projection =
            available_query_projection(&evidence, &catalog, &mixed_projection_fields).unwrap();
        assert_eq!(projection_files().len(), 2);
        let (projection_read, projection_parameters) =
            evidence_read(&projection[0], Some(&projection));
        let json_rows: i64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {projection_read} WHERE field_index=1"),
                params_from_iter(projection_parameters.iter()),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(json_rows, 2);

        page_json_with_evidence(
            &variants,
            Some(&evidence),
            Some(&catalog),
            0,
            10,
            &PageRequest {
                evidence_columns: vec![2],
                sort_evidence: Some(2),
                direction: "desc".into(),
                ..PageRequest::default()
            },
        )
        .unwrap();
        assert_eq!(projection_files().len(), 3);
        assert!(first_projection_files[0].is_file());
        assert_eq!(
            available_query_projection(&evidence, &catalog, &projection_fields)
                .unwrap()
                .len(),
            1
        );

        for request in [
            mixed_search_request,
            PageRequest {
                search: "0.9".into(),
                evidence_columns: vec![0],
                exact_total: true,
                ..PageRequest::default()
            },
            PageRequest {
                evidence_columns: vec![0],
                evidence_filters: vec![EvidenceFilterRequest {
                    index: 0,
                    operator: "gt".into(),
                    value: "0.2".into(),
                    value2: String::new(),
                    values: None,
                    include_missing: None,
                }],
                exact_total: true,
                ..PageRequest::default()
            },
            PageRequest {
                evidence_columns: vec![0],
                sort_evidence: Some(0),
                direction: "desc".into(),
                ..PageRequest::default()
            },
        ] {
            let fields = request_query_projection_fields(&catalog, &request)
                .unwrap()
                .unwrap();
            let request_projection =
                available_query_projection(&evidence, &catalog, &fields).unwrap();
            let canonical = page_json_with_evidence_once(
                &variants,
                Some(&evidence),
                None,
                Some(&catalog),
                0,
                10,
                &request,
                None,
            )
            .unwrap();
            let projected = page_json_with_evidence_once(
                &variants,
                Some(&request_projection[0]),
                Some(&request_projection),
                Some(&catalog),
                0,
                10,
                &request,
                None,
            )
            .unwrap();
            assert_eq!(canonical, projected);
        }

        let export_request = PageRequest {
            search: "0.9".into(),
            evidence_columns: vec![0],
            ..PageRequest::default()
        };
        let canonical_rows = root.join("canonical-rows.csv");
        let projected_rows = root.join("projected-rows.csv");
        let columns = vec!["position".into(), "gene".into()];
        let export_fields = request_query_projection_fields(&catalog, &export_request)
            .unwrap()
            .unwrap();
        let export_projection =
            available_query_projection(&evidence, &catalog, &export_fields).unwrap();
        let canonical_count = export_filtered_rows_with_details_once(
            &variants,
            Some(&evidence),
            None,
            Some(&catalog),
            &canonical_rows,
            &export_request,
            &columns,
        )
        .unwrap();
        let projected_count = export_filtered_rows_with_details_once(
            &variants,
            Some(&export_projection[0]),
            Some(&export_projection),
            Some(&catalog),
            &projected_rows,
            &export_request,
            &columns,
        )
        .unwrap();
        assert_eq!(canonical_count, projected_count);
        assert_eq!(
            fs::read(canonical_rows).unwrap(),
            fs::read(projected_rows).unwrap()
        );

        let canonical_genes = root.join("canonical-genes.txt");
        let projected_genes = root.join("projected-genes.txt");
        let canonical_count = export_filtered_genes_with_details_once(
            &variants,
            Some(&evidence),
            None,
            Some(&catalog),
            &canonical_genes,
            &export_request,
        )
        .unwrap();
        let projected_count = export_filtered_genes_with_details_once(
            &variants,
            Some(&export_projection[0]),
            Some(&export_projection),
            Some(&catalog),
            &projected_genes,
            &export_request,
        )
        .unwrap();
        assert_eq!(canonical_count, projected_count);
        assert_eq!(
            fs::read(canonical_genes).unwrap(),
            fs::read(projected_genes).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn query_projection_preserves_allele_gene_search_filter_sort_and_values() {
        let root = std::env::temp_dir().join(format!(
            "annocat-query-gene-projection-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let vcf = root.join("input.vcf");
        fs::write(
            &vcf,
            "##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence|IMPACT|SYMBOL|Gene|Feature|UPLOADED_ALLELE|CANONICAL\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t82001\t.\tA\tG\t50\tPASS\tCSQ=G|missense_variant|MODERATE|GENE1|ENSG1|ENST1|A/G|YES\n1\t82002\t.\tC\tT\t50\tPASS\tCSQ=T|missense_variant|MODERATE|GENE2|ENSG2|ENST2|C/T|YES\n",
        )
        .unwrap();
        let variants = root.join("variants.parquet");
        convert_vcf(&vcf, &variants, || false, |_, _, _, _, _| {}).unwrap();

        let evidence = root.join("evidence.parquet");
        let gene_evidence = root.join("query-gene-evidence.parquet");
        let matched_allele = allele_id("1", 82002, "C", "T");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE evidence(allele_id VARCHAR, consequence_id VARCHAR, scope VARCHAR,
                    source_id VARCHAR, field_path VARCHAR, value_type VARCHAR,
                    string_value VARCHAR, integer_value BIGINT, number_value DOUBLE,
                    boolean_value BOOLEAN, json_value VARCHAR);
                 COPY evidence TO '{}' (FORMAT PARQUET);
                 CREATE TABLE gene_evidence(allele_id VARCHAR, gene_id VARCHAR,
                    gene_symbol VARCHAR, scope VARCHAR, source_id VARCHAR, field_path VARCHAR,
                    value_type VARCHAR, string_value VARCHAR, integer_value BIGINT,
                    number_value DOUBLE, boolean_value BOOLEAN, json_value VARCHAR);
                 INSERT INTO gene_evidence VALUES
                    ('{matched_allele}', '', 'GENE2', 'gene', 'gene-profile', 'geneMatches', 'text', 'Migraine', NULL, NULL, NULL, NULL),
                    ('{matched_allele}', '', 'GENE2', 'gene', 'gene-profile', 'geneMatch', 'boolean', NULL, NULL, NULL, true, NULL),
                    ('{matched_allele}', '', 'GENE2', 'gene', 'gene-profile', 'matchedSelectedItems', 'text', 'Migraine', NULL, NULL, NULL, NULL);
                 COPY gene_evidence TO '{}' (FORMAT PARQUET);",
                evidence.to_string_lossy().replace('\'', "''"),
                gene_evidence.to_string_lossy().replace('\'', "''"),
            ))
            .unwrap();

        let catalog = root.join("query-field-catalog.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "schemaVersion": 1,
                "geneEvidenceFile": "query-gene-evidence.parquet",
                "fields": [
                    {
                        "scope": "gene",
                        "sourceId": "gene-profile",
                        "fieldPath": "geneMatches",
                        "valueType": "text",
                        "storageRelation": "geneEvidence",
                        "resolutionPolicy": "alleleGeneDirect"
                    },
                    {
                        "scope": "gene",
                        "sourceId": "gene-profile",
                        "fieldPath": "geneMatch",
                        "valueType": "boolean",
                        "storageRelation": "geneEvidence",
                        "resolutionPolicy": "alleleGeneDirect"
                    },
                    {
                        "scope": "gene",
                        "sourceId": "gene-profile",
                        "fieldPath": "matchedSelectedItems",
                        "valueType": "text",
                        "storageRelation": "geneEvidence",
                        "resolutionPolicy": "alleleGeneDirect"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let request = PageRequest {
            search: "migraine".into(),
            evidence_columns: vec![0, 1, 2],
            evidence_filters: vec![EvidenceFilterRequest {
                index: 1,
                operator: "equals".into(),
                value: "true".into(),
                value2: String::new(),
                values: None,
                include_missing: None,
            }],
            sort_evidence: Some(0),
            direction: "desc".into(),
            exact_total: true,
            ..PageRequest::default()
        };
        let fields = request_query_projection_fields(&catalog, &request)
            .unwrap()
            .unwrap();
        let projection = prepare_query_projection(&evidence, &catalog, &fields)
            .unwrap()
            .unwrap();
        let generated: Value = serde_json::from_str(
            &page_json_with_evidence_once(
                &variants,
                Some(&projection[0]),
                Some(&projection),
                Some(&catalog),
                0,
                10,
                &request,
                None,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(generated["total"], 1);
        assert_eq!(generated["rows"][0]["evidence"]["0"], "Migraine");
        assert_eq!(generated["rows"][0]["evidence"]["1"], "true");

        assert!(available_query_projection(&evidence, &catalog, &fields).is_some());
        let canonical = page_json_with_evidence_once(
            &variants,
            Some(&evidence),
            None,
            Some(&catalog),
            0,
            10,
            &request,
            None,
        )
        .unwrap();
        let projected = page_json_with_evidence_once(
            &variants,
            Some(&projection[0]),
            Some(&projection),
            Some(&catalog),
            0,
            10,
            &request,
            None,
        )
        .unwrap();
        assert_eq!(canonical, projected);

        let (projection_read, projection_parameters) =
            evidence_read(&projection[0], Some(&projection));
        let projected_fields: i64 = connection
            .query_row(
                &format!("SELECT count(DISTINCT field_index) FROM {projection_read}"),
                params_from_iter(projection_parameters.iter()),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projected_fields, 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn query_projection_failure_retries_canonical_evidence() {
        let root = std::env::temp_dir().join(format!(
            "annocat-query-projection-fallback-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let evidence = root.join("evidence.parquet");
        let first_projection = root.join(".annocat-query-v3-0-test.parquet");
        let second_projection = root.join(".annocat-query-v3-1-test.parquet");
        let projection = vec![first_projection.clone(), second_projection.clone()];
        fs::write(&evidence, b"canonical").unwrap();
        fs::write(&first_projection, b"broken").unwrap();
        fs::write(&second_projection, b"broken").unwrap();
        let result = with_projection_fallback(&evidence, &projection, |path, files| {
            if files.is_some() {
                assert_eq!(path, Some(first_projection.as_path()));
                Err("projection read failed".into())
            } else {
                assert_eq!(path, Some(evidence.as_path()));
                Ok("canonical")
            }
        })
        .unwrap();
        assert_eq!(result, "canonical");
        assert!(!first_projection.exists());
        assert!(!second_projection.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
