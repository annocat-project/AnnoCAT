use duckdb::arrow::array::{
    Array, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use duckdb::arrow::record_batch::RecordBatch;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReaderBuilder, RowSelection, RowSelector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const INDEX_SCHEMA_VERSION: u32 = 2;
const INDEX_FILE: &str = "detail-row-groups.json";
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROW_GROUPS: usize = 100_000;
const MAX_GROUPS_PER_LOOKUP: usize = 8;
const MAX_CACHED_DETAIL_FILES: usize = 3;
const LOGICAL_RANGE_ROWS: usize = 4_096;
const BOUNDARY_CANDIDATES: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct FileStamp {
    bytes: u64,
    rows: i64,
    row_groups: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RowGroupRange {
    row_group: usize,
    row_offset: usize,
    row_count: usize,
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
    #[serde(skip)]
    variant_has_alternate_count: bool,
}

struct CachedIndex {
    path: PathBuf,
    index: Arc<DetailIndex>,
}

struct CachedBatches {
    ranges: Vec<RowGroupRange>,
    fields: Vec<String>,
    batches: Arc<Vec<RecordBatch>>,
}

struct AlleleBoundaries {
    stamp: FileStamp,
    groups: Vec<BoundaryRange>,
}

struct BoundaryRange {
    row_group: usize,
    row_offset: usize,
    row_count: usize,
    first: Vec<String>,
    last: Vec<String>,
}

pub(crate) struct IndexedDetail {
    pub variant: Value,
    pub consequences: Vec<(String, String)>,
    pub evidence: Vec<Value>,
}

static INDEX_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static INDEX_CACHE: OnceLock<Mutex<Option<CachedIndex>>> = OnceLock::new();
static BATCH_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedBatches>>> = OnceLock::new();

fn parquet_has_column(path: &Path, name: &str) -> Result<bool, String> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open result table: {error}"))?,
    )
    .map_err(|error| format!("cannot read result table metadata: {error}"))?;
    Ok(reader.schema().field_with_name(name).is_ok())
}

fn cache_index(
    path: &Path,
    variants: &Path,
    mut index: DetailIndex,
) -> Result<Arc<DetailIndex>, String> {
    index.variant_has_alternate_count = parquet_has_column(variants, "alternate_count")?;
    let index = Arc::new(index);
    if let Ok(mut cache) = INDEX_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *cache = Some(CachedIndex {
            path: path.to_owned(),
            index: Arc::clone(&index),
        });
    }
    Ok(index)
}

fn stamp(path: &Path) -> Result<FileStamp, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open result table: {error}"))?,
    )
    .map_err(|error| format!("cannot read result table metadata: {error}"))?;
    let row_groups = builder.metadata().row_groups().len();
    if row_groups > MAX_ROW_GROUPS {
        return Err("result table has too many row groups".into());
    }
    Ok(FileStamp {
        bytes: fs::metadata(path)
            .map_err(|error| format!("cannot inspect result table: {error}"))?
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
                .map_err(|_| format!("result table is missing {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectionMask::roots(builder.parquet_schema(), indices))
}

fn string_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("result table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| format!("result field {name} has the wrong type"))
}

fn i64_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("result table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| format!("result field {name} has the wrong type"))
}

fn i32_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("result table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| format!("result field {name} has the wrong type"))
}

fn f64_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("result table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| format!("result field {name} has the wrong type"))
}

fn bool_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| format!("result table is missing {name}"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| format!("result field {name} has the wrong type"))
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

struct BoundaryBuilder {
    row_group: usize,
    row_offset: usize,
    row_count: usize,
    first: Vec<String>,
    last: VecDeque<String>,
    last_allele: String,
}

struct RecordRangeBuilder {
    row_group: usize,
    row_offset: usize,
    row_count: usize,
    first_record: i64,
    last_record: i64,
}

impl RecordRangeBuilder {
    fn new(row_group: usize, row_offset: usize, record: i64) -> Self {
        Self {
            row_group,
            row_offset,
            row_count: 0,
            first_record: record,
            last_record: record,
        }
    }

    fn add(&mut self, record: i64) -> Result<(), String> {
        if record < self.last_record {
            return Err("variants are not stored in input order".into());
        }
        self.last_record = record;
        self.row_count += 1;
        Ok(())
    }

    fn finish(self) -> RowGroupRange {
        RowGroupRange {
            row_group: self.row_group,
            row_offset: self.row_offset,
            row_count: self.row_count,
            first_record: self.first_record,
            last_record: self.last_record,
        }
    }
}

impl BoundaryBuilder {
    fn new(row_group: usize, row_offset: usize, allele: &str) -> Self {
        Self {
            row_group,
            row_offset,
            row_count: 0,
            first: Vec::new(),
            last: VecDeque::new(),
            last_allele: allele.to_owned(),
        }
    }

    fn add(&mut self, allele: &str) {
        if self.first.len() < BOUNDARY_CANDIDATES {
            self.first.push(allele.to_owned());
        }
        if self.last.len() == BOUNDARY_CANDIDATES {
            self.last.pop_front();
        }
        self.last.push_back(allele.to_owned());
        self.last_allele.clear();
        self.last_allele.push_str(allele);
        self.row_count += 1;
    }

    fn should_split(&self, allele: &str) -> bool {
        self.row_count >= LOGICAL_RANGE_ROWS && self.last_allele != allele
    }

    fn finish(self) -> BoundaryRange {
        BoundaryRange {
            row_group: self.row_group,
            row_offset: self.row_offset,
            row_count: self.row_count,
            first: self.first,
            last: self.last.into_iter().collect(),
        }
    }
}

fn scan_allele_boundaries(path: &Path) -> Result<AlleleBoundaries, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open result detail table: {error}"))?,
    )
    .map_err(|error| format!("cannot read result detail metadata: {error}"))?;
    let stamp = FileStamp {
        bytes: fs::metadata(path).map_err(|error| error.to_string())?.len(),
        rows: builder.metadata().file_metadata().num_rows(),
        row_groups: builder.metadata().row_groups().len(),
    };
    if stamp.row_groups > MAX_ROW_GROUPS {
        return Err("result detail table has too many row groups".into());
    }
    let ends = row_group_ends(&builder);
    let mask = projection(&builder, &["allele_id"])?;
    let reader = builder
        .with_projection(mask)
        .with_batch_size(16_384)
        .build()
        .map_err(|error| format!("cannot scan result detail table: {error}"))?;
    let mut groups = Vec::new();
    let mut current = None::<BoundaryBuilder>;
    let mut global_row = 0usize;
    let mut group = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| format!("cannot decode result detail table: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        for row in 0..batch.num_rows() {
            while group < ends.len() && global_row >= ends[group] {
                if let Some(boundary) = current.take() {
                    groups.push(boundary.finish());
                }
                group += 1;
            }
            if group >= stamp.row_groups || alleles.is_null(row) {
                return Err("result detail row groups are inconsistent".into());
            }
            let allele = alleles.value(row);
            if current
                .as_ref()
                .is_some_and(|boundary| boundary.should_split(allele))
                && let Some(boundary) = current.take()
            {
                groups.push(boundary.finish());
            }
            let group_start = group.checked_sub(1).map_or(0, |previous| ends[previous]);
            current
                .get_or_insert_with(|| {
                    BoundaryBuilder::new(group, global_row - group_start, allele)
                })
                .add(allele);
            global_row += 1;
        }
    }
    if let Some(boundary) = current {
        groups.push(boundary.finish());
    }
    if global_row as i64 != stamp.rows {
        return Err("result detail row count changed while indexing".into());
    }
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
    let mut ranges = Vec::new();
    let mut current = None::<RecordRangeBuilder>;
    let mut records = HashMap::with_capacity(boundary_ids.len());
    let mut global_row = 0usize;
    let mut group = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| format!("cannot decode variants table: {error}"))?;
        let alleles = string_array(&batch, "allele_id")?;
        let record_numbers = i64_array(&batch, "record_number")?;
        for row in 0..batch.num_rows() {
            while group < ends.len() && global_row >= ends[group] {
                if let Some(range) = current.take() {
                    ranges.push(range.finish());
                }
                group += 1;
            }
            if group >= stamp.row_groups || alleles.is_null(row) || record_numbers.is_null(row) {
                return Err("variants row groups are inconsistent".into());
            }
            let record = record_numbers.value(row);
            if current
                .as_ref()
                .is_some_and(|range| range.row_count >= LOGICAL_RANGE_ROWS)
                && let Some(range) = current.take()
            {
                ranges.push(range.finish());
            }
            let group_start = group.checked_sub(1).map_or(0, |previous| ends[previous]);
            current
                .get_or_insert_with(|| {
                    RecordRangeBuilder::new(group, global_row - group_start, record)
                })
                .add(record)?;
            let allele = alleles.value(row);
            if boundary_ids.contains(allele) {
                records.insert(allele.to_owned(), record);
            }
            global_row += 1;
        }
    }
    if let Some(range) = current {
        ranges.push(range.finish());
    }
    if global_row as i64 != stamp.rows {
        return Err("variants row count changed while indexing".into());
    }
    Ok((
        IndexedFile {
            stamp,
            groups: ranges,
        },
        records,
    ))
}

fn resolve_detail_file(
    boundaries: AlleleBoundaries,
    records: &HashMap<String, i64>,
) -> Result<IndexedFile, String> {
    let groups = boundaries
        .groups
        .into_iter()
        .filter_map(|group| {
            let first_record = group
                .first
                .iter()
                .find_map(|allele| records.get(allele))
                .or_else(|| group.last.iter().find_map(|allele| records.get(allele)))
                .copied()?;
            let last_record = group
                .last
                .iter()
                .rev()
                .find_map(|allele| records.get(allele))
                .or_else(|| {
                    group
                        .first
                        .iter()
                        .rev()
                        .find_map(|allele| records.get(allele))
                })
                .copied()?;
            (first_record <= last_record).then_some(RowGroupRange {
                row_group: group.row_group,
                row_offset: group.row_offset,
                row_count: group.row_count,
                first_record,
                last_record,
            })
        })
        .collect();
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
        .flat_map(|group| group.first.iter().chain(&group.last).cloned())
        .collect::<HashSet<_>>();
    let (variants, records) = scan_variants(variants, &boundary_ids)?;
    Ok(DetailIndex {
        schema_version: INDEX_SCHEMA_VERSION,
        variants,
        consequences: resolve_detail_file(consequence_bounds, &records)?,
        evidence: resolve_detail_file(evidence_bounds, &records)?,
        variant_has_alternate_count: false,
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
            let rows = usize::try_from(file.stamp.rows).unwrap_or(usize::MAX);
            let maximum_ranges = rows
                .div_ceil(LOGICAL_RANGE_ROWS)
                .saturating_add(file.stamp.row_groups);
            file.groups.len() <= maximum_ranges
                && file.groups.iter().all(|group| {
                    group.row_group < file.stamp.row_groups
                        && group.row_count > 0
                        && group
                            .row_offset
                            .checked_add(group.row_count)
                            .is_some_and(|end| end <= rows)
                        && group.first_record <= group.last_record
                })
                && file.groups.windows(2).all(|groups| {
                    (groups[0].row_group < groups[1].row_group
                        || (groups[0].row_group == groups[1].row_group
                            && groups[0].row_offset < groups[1].row_offset))
                        && groups[0].first_record <= groups[1].first_record
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
) -> Result<Arc<DetailIndex>, String> {
    let directory = variants
        .parent()
        .ok_or("variants table has no parent directory")?;
    let path = directory.join(INDEX_FILE);
    if let Ok(cache) = INDEX_CACHE.get_or_init(|| Mutex::new(None)).lock()
        && let Some(cached) = cache.as_ref()
        && cached.path == path
    {
        return Ok(Arc::clone(&cached.index));
    }
    if let Ok(index) = read_index(&path)
        && index_is_valid(&index, variants, consequences, evidence)
    {
        return cache_index(&path, variants, index);
    }
    let _guard = INDEX_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "detail index build lock failed")?;
    if let Ok(index) = read_index(&path)
        && index_is_valid(&index, variants, consequences, evidence)
    {
        return cache_index(&path, variants, index);
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
    cache_index(&path, variants, index)
}

pub(crate) fn prepare(variants: &Path, consequences: &Path, evidence: &Path) -> Result<(), String> {
    if let Ok(mut cache) = BATCH_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        cache.clear();
    }
    if let Some(directory) = variants.parent()
        && let Ok(mut cache) = INDEX_CACHE.get_or_init(|| Mutex::new(None)).lock()
        && cache
            .as_ref()
            .is_some_and(|cached| cached.path == directory.join(INDEX_FILE))
    {
        *cache = None;
    }
    ensure_index(variants, consequences, evidence).map(|_| ())
}

fn groups_for_record(file: &IndexedFile, record_number: i64) -> Result<Vec<RowGroupRange>, String> {
    let first = file
        .groups
        .partition_point(|group| group.last_record < record_number);
    let groups = file.groups[first..]
        .iter()
        .take_while(|group| group.first_record <= record_number)
        .filter(|group| record_number <= group.last_record)
        .take(MAX_GROUPS_PER_LOOKUP + 1)
        .cloned()
        .collect::<Vec<_>>();
    if groups.len() > MAX_GROUPS_PER_LOOKUP {
        return Err("detail locator matched too many row groups".into());
    }
    Ok(groups)
}

fn projected_reader(
    path: &Path,
    ranges: Vec<RowGroupRange>,
    names: &[&str],
) -> Result<parquet::arrow::arrow_reader::ParquetRecordBatchReader, String> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        File::open(path).map_err(|error| format!("cannot open result table: {error}"))?,
    )
    .map_err(|error| format!("cannot read result table metadata: {error}"))?;
    let mask = projection(&builder, names)?;
    let mut row_groups = Vec::new();
    let mut selectors = Vec::new();
    let mut range = 0usize;
    while range < ranges.len() {
        let row_group = ranges[range].row_group;
        let row_group_rows = builder
            .metadata()
            .row_groups()
            .get(row_group)
            .ok_or("detail index refers to a missing row group")?
            .num_rows() as usize;
        row_groups.push(row_group);
        let mut cursor = 0usize;
        while range < ranges.len() && ranges[range].row_group == row_group {
            let selected = &ranges[range];
            if selected.row_offset < cursor {
                return Err("detail index contains overlapping row ranges".into());
            }
            if selected.row_offset > cursor {
                selectors.push(RowSelector::skip(selected.row_offset - cursor));
            }
            let end = selected
                .row_offset
                .checked_add(selected.row_count)
                .filter(|end| *end <= row_group_rows)
                .ok_or("detail index row range is invalid")?;
            selectors.push(RowSelector::select(selected.row_count));
            cursor = end;
            range += 1;
        }
        if cursor < row_group_rows {
            selectors.push(RowSelector::skip(row_group_rows - cursor));
        }
    }
    builder
        .with_row_groups(row_groups)
        .with_row_selection(RowSelection::from(selectors))
        .with_projection(mask)
        .with_batch_size(16_384)
        .build()
        .map_err(|error| format!("cannot read indexed result rows: {error}"))
}

fn projected_batches(
    path: &Path,
    ranges: Vec<RowGroupRange>,
    names: &[&str],
    error_context: &str,
) -> Result<Arc<Vec<RecordBatch>>, String> {
    if let Ok(cache) = BATCH_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        && let Some(cached) = cache.get(path)
        && cached.ranges == ranges
        && cached
            .fields
            .iter()
            .map(String::as_str)
            .eq(names.iter().copied())
    {
        return Ok(Arc::clone(&cached.batches));
    }
    let batches = projected_reader(path, ranges.clone(), names)?
        .map(|batch| batch.map_err(|error| format!("{error_context}: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    let batches = Arc::new(batches);
    if let Ok(mut cache) = BATCH_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        if cache.len() >= MAX_CACHED_DETAIL_FILES && !cache.contains_key(path) {
            cache.clear();
        }
        cache.insert(
            path.to_owned(),
            CachedBatches {
                ranges,
                fields: names.iter().map(|name| (*name).to_owned()).collect(),
                batches: Arc::clone(&batches),
            },
        );
    }
    Ok(batches)
}

fn read_variant(
    path: &Path,
    groups: Vec<RowGroupRange>,
    allele_id: &str,
    record_number: i64,
    alt_index: i32,
    has_alternate_count: bool,
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
    let mut fields = FIELDS.to_vec();
    if has_alternate_count {
        fields.push("alternate_count");
    }
    let mut variant = None;
    let mut maximum_alt_index = 0;
    for batch in
        projected_batches(path, groups, &fields, "cannot decode indexed variant rows")?.iter()
    {
        let alleles = string_array(batch, "allele_id")?;
        let records = i64_array(batch, "record_number")?;
        let alternate_indices = i32_array(batch, "alt_index")?;
        let alternate_counts = has_alternate_count
            .then(|| i32_array(batch, "alternate_count"))
            .transpose()?;
        for row in 0..batch.num_rows() {
            if !records.is_null(row)
                && records.value(row) == record_number
                && !alternate_indices.is_null(row)
            {
                maximum_alt_index = maximum_alt_index.max(alternate_indices.value(row));
            }
            if !alleles.is_null(row)
                && !records.is_null(row)
                && alleles.value(row) == allele_id
                && records.value(row) == record_number
                && !alternate_indices.is_null(row)
                && alternate_indices.value(row) == alt_index
            {
                let string = |name| string_array(batch, name);
                let samples = string("samples_json")?;
                let consequences = string("consequences_json")?;
                let quality = f64_array(&batch, "quality")?;
                let canonical = bool_array(&batch, "canonical")?;
                let alternate_count = alternate_counts
                    .map(|values| {
                        (!values.is_null(row))
                            .then(|| values.value(row))
                            .ok_or("variant record has no ALT count")
                    })
                    .transpose()?
                    .unwrap_or_default();
                let mut value = json!({
                    "chromosome": string_value(string("chromosome")?, row).unwrap_or_default(),
                    "position": i64_array(batch, "position")?.value(row),
                    "reference": string_value(string("reference")?, row).unwrap_or_default(),
                    "alternate": string_value(string("alternate")?, row).unwrap_or_default(),
                    "altIndex": i32_array(batch, "alt_index")?.value(row),
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
                });
                if has_alternate_count {
                    if alternate_count < alt_index {
                        return Err("variant record has an invalid ALT count".into());
                    }
                    value
                        .as_object_mut()
                        .ok_or("indexed variant context is not an object")?
                        .insert("alternateCount".into(), alternate_count.into());
                    return Ok(Some(value));
                }
                variant = Some(value);
            }
        }
    }
    let Some(mut variant) = variant else {
        return Ok(None);
    };
    if maximum_alt_index < alt_index {
        return Err("variant record has an invalid ALT count".into());
    }
    variant
        .as_object_mut()
        .ok_or("indexed variant context is not an object")?
        .insert("alternateCount".into(), maximum_alt_index.into());
    Ok(Some(variant))
}

fn read_consequences(
    path: &Path,
    groups: Vec<RowGroupRange>,
    allele_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut rows = Vec::new();
    for batch in projected_batches(
        path,
        groups,
        &["allele_id", "consequence_id", "ordinal", "consequence_json"],
        "cannot decode indexed consequence rows",
    )?
    .iter()
    {
        let alleles = string_array(batch, "allele_id")?;
        let ids = string_array(batch, "consequence_id")?;
        let ordinals = i64_array(batch, "ordinal")?;
        let values = string_array(batch, "consequence_json")?;
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

fn read_evidence(
    path: &Path,
    groups: Vec<RowGroupRange>,
    allele_id: &str,
    consequences: &[(String, String)],
) -> Result<Vec<Value>, String> {
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
    for batch in
        projected_batches(path, groups, FIELDS, "cannot decode indexed evidence rows")?.iter()
    {
        let alleles = string_array(batch, "allele_id")?;
        for row in 0..batch.num_rows() {
            if alleles.is_null(row) || alleles.value(row) != allele_id {
                continue;
            }
            let strings = |name| string_array(batch, name);
            let scope = strings("scope")?.value(row);
            let field_path = strings("field_path")?.value(row);
            let value_type = strings("value_type")?.value(row).to_owned();
            let value = match value_type.as_str() {
                "string" => string_value(strings("string_value")?, row).map(Value::String),
                "integer" => {
                    let values = i64_array(batch, "integer_value")?;
                    (!values.is_null(row)).then(|| Value::Number(values.value(row).into()))
                }
                "number" => {
                    let values = f64_array(batch, "number_value")?;
                    (!values.is_null(row))
                        .then(|| values.value(row))
                        .and_then(serde_json::Number::from_f64)
                        .map(Value::Number)
                }
                "boolean" => {
                    let values = bool_array(batch, "boolean_value")?;
                    (!values.is_null(row)).then(|| Value::Bool(values.value(row)))
                }
                "json" => string_value(strings("json_value")?, row)
                    .map(|value| serde_json::from_str(&value).unwrap_or(Value::String(value))),
                _ => None,
            }
            .unwrap_or(Value::Null);
            rows.push(json!({
                "consequenceId": string_value(strings("consequence_id")?, row),
                "scope": scope,
                "sourceId": strings("source_id")?.value(row),
                "fieldPath": field_path,
                "valueType": value_type,
                "value": value,
            }));
        }
    }
    let mut rows = resolve_record_evidence(rows, consequences)?;
    rows.truncate(5001);
    Ok(rows)
}

pub(crate) fn resolve_record_evidence(
    rows: Vec<Value>,
    consequences: &[(String, String)],
) -> Result<Vec<Value>, String> {
    let mut visible = Vec::new();
    let mut record_lists = Vec::new();
    for row in rows {
        if row["scope"] == "source_records" || row["fieldPath"] == "__recordList" {
            let Some(source_id) = row["sourceId"].as_str() else {
                continue;
            };
            let Some(records) = row.get("value") else {
                continue;
            };
            let records = match records {
                Value::Array(records) => Value::Array(
                    records
                        .iter()
                        .cloned()
                        .map(|record| match record {
                            Value::Object(mut record) => {
                                record.remove("sourceRecordOrdinal");
                                Value::Object(record)
                            }
                            record => record,
                        })
                        .collect(),
                ),
                records => records.clone(),
            };
            record_lists.push((source_id.to_owned(), records));
        } else {
            visible.push(row);
        }
    }
    let record_sources = record_lists
        .iter()
        .map(|(source_id, _)| source_id.as_str())
        .collect::<HashSet<_>>();
    visible.retain(|row| {
        let source_id = row["sourceId"].as_str().unwrap_or_default();
        let scope = row["scope"].as_str().unwrap_or_default();
        !(record_sources.contains(source_id)
            && matches!(scope, "selected" | "transcript")
            && crate::evidence_resolution::bundled_record_field_is_selected(
                source_id,
                row["fieldPath"].as_str().unwrap_or_default(),
            ))
    });
    let contexts = consequences
        .iter()
        .filter_map(|(consequence_id, value)| {
            serde_json::from_str::<Value>(value)
                .ok()?
                .as_object()
                .cloned()
                .map(|value| (consequence_id, value))
        })
        .collect::<Vec<_>>();
    for (source_id, records) in &record_lists {
        for (consequence_id, consequence) in &contexts {
            let Some(resolved) = crate::evidence_resolution::resolve_bundled_record_list(
                source_id,
                records,
                consequence,
            )?
            else {
                continue;
            };
            for field in resolved.fields.into_iter().filter(|field| {
                field.scope == crate::evidence_resolution::ResolvedRecordScope::Selected
            }) {
                let source_cardinality =
                    if crate::evidence_resolution::bundled_record_field_is_aligned(
                        source_id,
                        &field.field_path,
                    ) {
                        "alignedVector"
                    } else {
                        "recordScalar"
                    };
                let value_type = match &field.value {
                    Value::String(_) => "string",
                    Value::Number(value) if value.is_i64() || value.is_u64() => "integer",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    _ => "json",
                };
                visible.push(json!({
                    "consequenceId": consequence_id,
                    "scope": "transcript",
                    "sourceId": source_id,
                    "fieldPath": field.field_path,
                    "sourceCardinality": source_cardinality,
                    "valueType": value_type,
                    "value": field.value,
                }));
            }
        }
    }
    let selected = visible
        .iter()
        .filter(|row| row["scope"] == "selected")
        .map(|row| {
            (
                row["sourceId"].as_str().unwrap_or_default().to_owned(),
                row["fieldPath"].as_str().unwrap_or_default().to_owned(),
                row["consequenceId"].as_str().map(str::to_owned),
            )
        })
        .collect::<HashSet<_>>();
    visible.retain(|row| {
        row["scope"] == "selected"
            || !selected.contains(&(
                row["sourceId"].as_str().unwrap_or_default().to_owned(),
                row["fieldPath"].as_str().unwrap_or_default().to_owned(),
                row["consequenceId"].as_str().map(str::to_owned),
            ))
    });
    visible.sort_by(|left, right| {
        let key = |value: &Value| {
            ["scope", "sourceId", "fieldPath", "consequenceId"]
                .map(|field| value[field].as_str().unwrap_or_default().to_owned())
        };
        key(left).cmp(&key(right))
    });
    Ok(visible)
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
        index.variant_has_alternate_count,
    )?
    else {
        return Ok(None);
    };
    let consequences = if consequence_groups.is_empty() {
        Vec::new()
    } else {
        read_consequences(consequences, consequence_groups, allele_id)?
    };
    let evidence = if evidence_groups.is_empty() {
        Vec::new()
    } else {
        read_evidence(evidence, evidence_groups, allele_id, &consequences)?
    };
    Ok(Some(IndexedDetail {
        variant,
        consequences,
        evidence,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(row_offset: usize, first_record: i64, last_record: i64) -> RowGroupRange {
        RowGroupRange {
            row_group: 0,
            row_offset,
            row_count: LOGICAL_RANGE_ROWS,
            first_record,
            last_record,
        }
    }

    #[test]
    fn record_lookup_returns_the_matching_logical_range() {
        let file = IndexedFile {
            stamp: FileStamp {
                bytes: 1,
                rows: 8_192,
                row_groups: 1,
            },
            groups: vec![range(0, 1, 4_096), range(4_096, 4_097, 8_192)],
        };
        let selected = groups_for_record(&file, 6_000).unwrap();
        assert_eq!(selected, vec![range(4_096, 4_097, 8_192)]);
    }

    #[test]
    fn logical_ranges_do_not_split_repeated_allele_rows() {
        let mut boundary = BoundaryBuilder::new(0, 0, "allele-a");
        for _ in 0..LOGICAL_RANGE_ROWS {
            boundary.add("allele-a");
        }
        assert!(!boundary.should_split("allele-a"));
        assert!(boundary.should_split("allele-b"));
    }

    #[test]
    fn detail_resolution_uses_only_existing_consequence_contexts() {
        let rows = vec![json!({
            "scope": "source_records",
            "sourceId": "dbnsfp",
            "fieldPath": "__recordList",
            "value": [{
                "Ensembl_transcriptid": "ENST00000641515;ENST00000335137;ENST00000999999",
                "Ensembl_proteinid": "ENSP00000493376;ENSP00000334393;ENSP00000999999",
                "Uniprot_acc": "A0A2U3U0J3;Q8NH21;SOURCE_ONLY",
                "aaref": "T",
                "aaalt": "A",
                "aapos": "162;141;99",
                "HGVSp_VEP": "p.Thr162Ala;p.Thr141Ala;p.Thr99Ala",
                "AlphaMissense_score": "0.0539;0.0543;0.9",
                "REVEL_score": ".;0.053;0.9",
                "PrimateAI_score": "0.632585585117"
            }]
        })];
        let consequences = [
            (
                "first".into(),
                json!({
                    "transcript_id": "ENST00000641515",
                    "protein_id": "ENSP00000493376",
                    "amino_acids": "T/A",
                    "protein_start": 162,
                    "hgvsp": "ENSP00000493376:p.Thr162Ala"
                })
                .to_string(),
            ),
            (
                "second".into(),
                json!({
                    "transcript_id": "ENST00000335137",
                    "protein_id": "ENSP00000334393",
                    "amino_acids": "T/A",
                    "protein_start": 141,
                    "hgvsp": "ENSP00000334393:p.Thr141Ala"
                })
                .to_string(),
            ),
        ];

        let resolved = resolve_record_evidence(rows, &consequences).unwrap();
        let values = |field: &str| {
            resolved
                .iter()
                .filter(|row| row["fieldPath"] == field)
                .map(|row| row["value"].as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(values("AlphaMissense_score"), vec!["0.0539", "0.0543"]);
        assert_eq!(values("REVEL_score"), vec!["0.053"]);
        assert_eq!(
            values("PrimateAI_score"),
            vec!["0.632585585117", "0.632585585117"]
        );
        assert!(resolved.iter().all(|row| row["value"] != "0.9"));
        assert!(
            resolved
                .iter()
                .filter(|row| row["fieldPath"] == "AlphaMissense_score")
                .all(|row| row["sourceCardinality"] == "alignedVector")
        );
        assert!(
            resolved
                .iter()
                .filter(|row| row["fieldPath"] == "PrimateAI_score")
                .all(|row| row["sourceCardinality"] == "recordScalar")
        );
    }
}
