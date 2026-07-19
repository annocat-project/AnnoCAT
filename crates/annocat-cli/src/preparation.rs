use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(test)]
use std::io::Cursor;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod cache;
mod checkpoint;
mod fields;
mod state;
mod tabix;
mod transport;
#[cfg(test)]
use cache::restart_decision;
pub use cache::{initialize_partial, promote_verified};
use cache::{required_nonempty_file, restart_decision_with_legacy_upgrade, verify_partial_osa};
use checkpoint::{CHECKPOINT_SCHEMA_VERSION, read_checkpoint, write_checkpoint};
pub use checkpoint::{
    CheckpointState, PreparationCheckpoint, PreparationIdentity, RestartDecision, ShardPaths,
};
use fields::dbnsfp_schema_identity;
pub use fields::{
    DBNSFP_CURATED_SCHEMA, DbnsfpFieldSelection, SupplementaryFieldSelection,
    dbnsfp_field_configuration_json, load_dbnsfp_field_selection,
    load_supplementary_field_selection, save_dbnsfp_field_selection,
    save_supplementary_field_selection, supplementary_field_configuration_json,
    supplementary_schema_identity,
};
#[cfg(test)]
use fields::{
    DBNSFP_FIELD_SELECTION_SCHEMA_VERSION, dbnsfp_contract, dbnsfp_contract_fields,
    default_dbnsfp_field_selection, default_supplementary_field_selection,
    full_dbnsfp_field_selection,
};
pub use state::{
    LivePreparationState, cancel_live, forget_live, live_status, record_start_failure,
    running_count,
};
use state::{live_cancel, live_state, register_live_job, run_with_live_job};
use tabix::{TabixReferenceOffset, parse_reference_offsets as parse_tabix_reference_offsets};

pub const LEGACY_PREPARATION_IDENTITY_COMMIT: &str = "7038e7c17708e7d2226149e78e0bb297bcc6d1d6";
pub const STREAM_WRITER_BUFFER_BYTES: u64 = 1024 * 1024;
const PREPARATION_METADATA_ALLOWANCE_BYTES: u64 = 4 * 1024 * 1024;
const PREPARATION_SAFETY_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

fn format_decimal_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

static HYBRID_SOURCE_PARTS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceInputMode {
    HybridResumable,
    PureStreaming,
}

pub fn set_source_input_mode(value: &str) -> Result<SourceInputMode, String> {
    let mode = match value {
        "hybrid-resumable" => SourceInputMode::HybridResumable,
        "pure-streaming" | "" => SourceInputMode::PureStreaming,
        _ => return Err("source input mode must be hybrid-resumable or pure-streaming".into()),
    };
    HYBRID_SOURCE_PARTS.store(mode == SourceInputMode::HybridResumable, Ordering::SeqCst);
    Ok(mode)
}

pub fn source_input_mode() -> SourceInputMode {
    if HYBRID_SOURCE_PARTS.load(Ordering::SeqCst) {
        SourceInputMode::HybridResumable
    } else {
        SourceInputMode::PureStreaming
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparationDiskPlan {
    /// Legacy projection field. Resumable source parts are bounded by the current
    /// source unit and are reported from live filesystem state, not this estimate.
    pub source_disk_bytes: u64,
    pub writer_buffer_bytes: u64,
    pub metadata_allowance_bytes: u64,
    pub safety_reserve_bytes: u64,
    pub projected_prepared_bytes: Option<u64>,
    pub remaining_prepared_bytes: Option<u64>,
    pub required_free_bytes: Option<u64>,
}

pub fn preparation_disk_plan(
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
) -> PreparationDiskPlan {
    let projected_prepared_bytes = (network_bytes > 0 && expected_network_bytes > 0).then(|| {
        prepared_bytes
            .saturating_mul(expected_network_bytes)
            .div_ceil(network_bytes)
    });
    let remaining_prepared_bytes =
        projected_prepared_bytes.map(|projected| projected.saturating_sub(prepared_bytes));
    let fixed = STREAM_WRITER_BUFFER_BYTES
        .saturating_add(PREPARATION_METADATA_ALLOWANCE_BYTES)
        .saturating_add(PREPARATION_SAFETY_RESERVE_BYTES);
    PreparationDiskPlan {
        source_disk_bytes: 0,
        writer_buffer_bytes: STREAM_WRITER_BUFFER_BYTES,
        metadata_allowance_bytes: PREPARATION_METADATA_ALLOWANCE_BYTES,
        safety_reserve_bytes: PREPARATION_SAFETY_RESERVE_BYTES,
        projected_prepared_bytes,
        remaining_prepared_bytes,
        required_free_bytes: remaining_prepared_bytes
            .map(|remaining| remaining.saturating_add(fixed)),
    }
}

pub struct StreamingBuildRequest<'a> {
    pub fastvep_executable: &'a Path,
    pub source_type: &'a str,
    pub paths: &'a ShardPaths,
    pub identity: &'a PreparationIdentity,
    pub log_path: &'a Path,
    pub dbnsfp_fields: Option<&'a [String]>,
    pub source_fields: Option<&'a [String]>,
}

#[derive(Debug)]
pub struct StreamingBuildResult {
    pub compressed_bytes_read: u64,
    pub prepared_osa_bytes: u64,
    pub prepared_index_bytes: u64,
}

fn checkpoint_stream_complete(
    paths: &ShardPaths,
    identity: PreparationIdentity,
    result: &StreamingBuildResult,
) -> Result<(), String> {
    write_checkpoint(
        &paths.checkpoint(),
        &PreparationCheckpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            identity,
            state: CheckpointState::Preparing,
            compressed_bytes_read: result.compressed_bytes_read,
            parsed_records: 0,
            prepared_bytes: result.prepared_osa_bytes,
            prepared_index_bytes: result.prepared_index_bytes,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamingProgress {
    pub compressed_bytes_read: u64,
    pub consumed_bytes: u64,
    pub retained_bytes: u64,
    pub expected_compressed_bytes: u64,
    pub elapsed: Duration,
    pub bytes_per_second: f64,
}

struct AppendTeeReader<R, W> {
    input: R,
    output: W,
}

impl<R: Read, W: Write> Read for AppendTeeReader<R, W> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.input.read(buffer)?;
        if read == 0 {
            self.output.flush()?;
        } else {
            // Persist each chunk before exposing it to fastVEP. A crash can
            // therefore never leave the cache ahead of its resumable prefix.
            self.output.write_all(&buffer[..read])?;
        }
        Ok(read)
    }
}

fn prepare_source_part(paths: &ShardPaths, identity: &PreparationIdentity) -> Result<u64, String> {
    let identity_bytes = serde_json::to_vec_pretty(identity)
        .map_err(|error| format!("cannot encode source-part identity: {error}"))?;
    let matches =
        fs::read(paths.source_part_identity()).is_ok_and(|existing| existing == identity_bytes);
    let length = fs::metadata(paths.source_part())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if !matches || length > identity.expected_compressed_bytes {
        let _ = fs::remove_file(paths.source_part());
        let _ = fs::remove_file(paths.source_part_identity());
    }
    fs::create_dir_all(
        paths
            .source_part()
            .parent()
            .ok_or("source part has no parent directory")?,
    )
    .map_err(|error| format!("cannot create source-part directory: {error}"))?;
    if !paths.source_part_identity().is_file() {
        let temporary = paths.source_part_identity().with_extension("json.tmp");
        fs::write(&temporary, &identity_bytes)
            .map_err(|error| format!("cannot write source-part identity: {error}"))?;
        fs::rename(&temporary, paths.source_part_identity())
            .map_err(|error| format!("cannot publish source-part identity: {error}"))?;
    }
    Ok(fs::metadata(paths.source_part())
        .map(|metadata| metadata.len())
        .unwrap_or(0))
}

fn hybrid_http_reader(
    request: &StreamingBuildRequest<'_>,
) -> Result<(Box<dyn Read>, Arc<std::sync::atomic::AtomicU64>, u64), String> {
    hybrid_range_reader(
        request,
        &request.identity.source_url,
        0,
        request.identity.expected_compressed_bytes,
        request.identity.source_etag.as_deref(),
        request.identity.source_last_modified.as_deref(),
    )
}

fn hybrid_range_reader(
    request: &StreamingBuildRequest<'_>,
    source_url: &str,
    range_start: u64,
    object_bytes: u64,
    expected_etag: Option<&str>,
    expected_last_modified: Option<&str>,
) -> Result<(Box<dyn Read>, Arc<std::sync::atomic::AtomicU64>, u64), String> {
    let resumed = prepare_source_part(request.paths, request.identity)?;
    let prefix = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(request.paths.source_part())
        .map_err(|error| format!("cannot open resumable source prefix: {error}"))?;
    if resumed == request.identity.expected_compressed_bytes {
        return Ok((
            Box::new(prefix.take(resumed)),
            Arc::new(AtomicU64::new(0)),
            resumed,
        ));
    }

    let absolute_start = range_start
        .checked_add(resumed)
        .ok_or("resumable range start overflow")?;
    let absolute_end = range_start
        .checked_add(request.identity.expected_compressed_bytes)
        .and_then(|exclusive| exclusive.checked_sub(1))
        .ok_or("resumable range end overflow")?;
    if absolute_end >= object_bytes {
        return Err("resumable source range exceeds its source object".into());
    }
    let continuation_source = transport::ReconnectingRangeReader::new(
        source_url,
        &request.identity.resource_id,
        &request.identity.chromosome,
        absolute_start,
        absolute_end,
        object_bytes,
        expected_etag,
        expected_last_modified,
    )?;
    let append = fs::OpenOptions::new()
        .append(true)
        .open(request.paths.source_part())
        .map_err(|error| format!("cannot append resumable source part: {error}"))?;
    let network = Arc::new(AtomicU64::new(0));
    let continuation = AppendTeeReader {
        input: CountedReader {
            inner: continuation_source,
            count: network.clone(),
        },
        output: append,
    };
    Ok((
        Box::new(prefix.take(resumed).chain(continuation)),
        network,
        resumed,
    ))
}

/// Feed one remote chromosome into fastVEP. Hybrid mode persists every byte
/// before forwarding it so the network prefix can resume; pure mode forwards
/// directly. Validation and promotion remain separate operations.
pub fn stream_http_to_partial_osa_with_progress<F>(
    request: &StreamingBuildRequest<'_>,
    cancelled: &AtomicBool,
    progress: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut(StreamingProgress),
{
    if source_input_mode() == SourceInputMode::HybridResumable {
        return stream_http_via_resumable_part(request, cancelled, progress);
    }
    if !request.paths.partial_directory.is_dir() {
        return Err("partial shard directory has not been initialized".into());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| format!("cannot create preparation HTTP client: {error}"))?;
    let response = client
        .get(&request.identity.source_url)
        .send()
        .map_err(|error| format!("preparation download failed: {error}"))?;
    validate_source_response(&response, request.identity)?;
    let Some(expected_md5) = identity_md5(request.identity) else {
        let mut response = response;
        return stream_reader_to_partial_osa_with_progress(
            request,
            &mut response,
            cancelled,
            progress,
        );
    };

    // Rolling sources such as ClinVar resolve a dated snapshot immediately
    // before streaming. Its HEAD size is useful for planning, but the GET
    // response length may legitimately differ after a weekly update. Use the
    // GET length for bounded-copy progress and NCBI's sidecar MD5 for identity.
    let mut effective_identity = request.identity.clone();
    if let Some(bytes) = response.content_length() {
        effective_identity.expected_compressed_bytes = bytes;
    }
    let effective_request = StreamingBuildRequest {
        fastvep_executable: request.fastvep_executable,
        source_type: request.source_type,
        paths: request.paths,
        identity: &effective_identity,
        log_path: request.log_path,
        dbnsfp_fields: request.dbnsfp_fields,
        source_fields: request.source_fields,
    };
    let mut checked = Md5CountedReader {
        inner: response,
        count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        hasher: md5::Context::new(),
    };
    let result = stream_reader_to_partial_osa_with_progress(
        &effective_request,
        &mut checked,
        cancelled,
        progress,
    )?;
    let actual_md5 = format!("{:x}", checked.hasher.finalize());
    if actual_md5 != expected_md5 {
        remove_incomplete_outputs(request.paths);
        return Err("source MD5 differs from the published release checksum".into());
    }
    Ok(result)
}

fn stream_http_via_resumable_part<F>(
    request: &StreamingBuildRequest<'_>,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut(StreamingProgress),
{
    if !request.paths.partial_directory.is_dir() {
        return Err("partial shard directory has not been initialized".into());
    }
    let (reader, network, resumed) = hybrid_http_reader(request)?;
    let started = Instant::now();
    let report = |progress: &mut F, consumed: u64| {
        let downloaded = resumed.saturating_add(network.load(Ordering::Relaxed));
        let elapsed = started.elapsed();
        progress(StreamingProgress {
            compressed_bytes_read: downloaded,
            consumed_bytes: consumed,
            retained_bytes: resumed,
            expected_compressed_bytes: request.identity.expected_compressed_bytes,
            elapsed,
            bytes_per_second: if elapsed.is_zero() {
                0.0
            } else {
                network.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
            },
        });
    };
    let result = if let Some(expected_md5) = identity_md5(request.identity) {
        let mut checked = Md5CountedReader {
            inner: reader,
            count: Arc::new(AtomicU64::new(0)),
            hasher: md5::Context::new(),
        };
        let result = stream_reader_to_partial_osa_with_progress(
            request,
            &mut checked,
            cancelled,
            |state| report(&mut progress, state.consumed_bytes),
        )?;
        let actual_md5 = format!("{:x}", checked.hasher.finalize());
        if actual_md5 != expected_md5 {
            remove_incomplete_outputs(request.paths);
            let _ = fs::remove_file(request.paths.source_part());
            let _ = fs::remove_file(request.paths.source_part_identity());
            return Err("source MD5 differs from the published release checksum".into());
        }
        result
    } else {
        let mut reader = reader;
        stream_reader_to_partial_osa_with_progress(request, &mut reader, cancelled, |state| {
            report(&mut progress, state.consumed_bytes)
        })?
    };
    let retained = fs::metadata(request.paths.source_part())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if retained != request.identity.expected_compressed_bytes {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "resumable source part is incomplete: retained {retained}, expected {}",
            request.identity.expected_compressed_bytes
        ));
    }
    report(&mut progress, request.identity.expected_compressed_bytes);
    Ok(result)
}

/// Feed a bounded local source stream into fastVEP. This is shared by HTTP
/// resources and compressed members inside a verified archive, so neither path
/// needs to stage a second source copy on disk.
pub fn stream_reader_to_partial_osa_with_progress<R, F>(
    request: &StreamingBuildRequest<'_>,
    input: &mut R,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<StreamingBuildResult, String>
where
    R: Read,
    F: FnMut(StreamingProgress),
{
    if !request.paths.partial_directory.is_dir() {
        return Err("partial shard directory has not been initialized".into());
    }
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut command = Command::new(request.fastvep_executable);
    command
        .arg("sa-build")
        .arg("--source")
        .arg(request.source_type)
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    if let Some(fields) = request.dbnsfp_fields {
        command.env(
            "ANNOCAT_DBNSFP_FIELDS",
            serde_json::to_string(fields)
                .map_err(|error| format!("cannot encode dbNSFP field selection: {error}"))?,
        );
    }
    if let Some(fields) = request.source_fields {
        command.env(
            "ANNOCAT_SOURCE_FIELDS",
            serde_json::to_string(fields)
                .map_err(|error| format!("cannot encode supplementary field selection: {error}"))?,
        );
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start fastVEP preparation: {error}"))?;

    let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
    let started = Instant::now();
    let copied = copy_bounded_with_progress(input, &mut stdin, cancelled, |bytes| {
        let elapsed = started.elapsed();
        progress(StreamingProgress {
            compressed_bytes_read: bytes,
            consumed_bytes: bytes,
            retained_bytes: 0,
            expected_compressed_bytes: request.identity.expected_compressed_bytes,
            elapsed,
            bytes_per_second: if elapsed.is_zero() {
                0.0
            } else {
                bytes as f64 / elapsed.as_secs_f64()
            },
        });
    });
    drop(stdin);
    let compressed_bytes_read = match copied {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    if compressed_bytes_read != request.identity.expected_compressed_bytes {
        let _ = child.kill();
        let _ = child.wait();
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "truncated chromosome stream: received {compressed_bytes_read}, expected {}",
            request.identity.expected_compressed_bytes
        ));
    }
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!("fastVEP preparation failed with status {status}"));
    }

    Ok(StreamingBuildResult {
        compressed_bytes_read,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_osa())?,
        prepared_index_bytes: required_nonempty_file(&request.paths.partial_index())?,
    })
}

struct Crc32Reader<R> {
    inner: R,
    hasher: crc32fast::Hasher,
}

struct Md5CountedReader<R> {
    inner: R,
    count: Arc<std::sync::atomic::AtomicU64>,
    hasher: md5::Context,
}

impl<R: Read> Read for Md5CountedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(output)?;
        self.count.fetch_add(read as u64, Ordering::Relaxed);
        self.hasher.consume(&output[..read]);
        Ok(read)
    }
}

fn zip_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn zip_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[allow(clippy::too_many_arguments)]
fn stream_revel_archive_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    archive: &RevelArchive,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let url = format!(
        "https://zenodo.org/api/records/7072866/files/{}/content",
        archive.filename
    );
    let (source, hybrid_network, resumed): (Box<dyn Read>, Option<Arc<AtomicU64>>, u64) =
        if source_input_mode() == SourceInputMode::HybridResumable {
            let (reader, network, resumed) =
                hybrid_range_reader(request, &url, 0, archive.bytes, None, None)?;
            (reader, Some(network), resumed)
        } else {
            let response = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .map_err(|error| format!("cannot create REVEL client: {error}"))?
                .get(&url)
                .header(
                    reqwest::header::USER_AGENT,
                    "AnnoCat/0.1 (local variant annotation)",
                )
                .send()
                .map_err(|error| {
                    format!(
                        "REVEL chromosome {} request failed: {error}",
                        archive.chromosome
                    )
                })?;
            if !response.status().is_success() || response.content_length() != Some(archive.bytes) {
                return Err(format!(
                    "REVEL chromosome {} returned HTTP {} or an unexpected length",
                    archive.chromosome,
                    response.status()
                ));
            }
            (Box::new(response), None, 0)
        };
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create REVEL preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut child = Command::new(request.fastvep_executable)
        .arg("sa-build")
        .arg("--source")
        .arg("revel")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("cannot start fastVEP REVEL preparation: {error}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
        let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut input = Md5CountedReader {
            inner: source,
            count: count.clone(),
            hasher: md5::Context::new(),
        };
        let started = Instant::now();
        let mut csv_members = 0_u32;
        loop {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let mut signature = [0_u8; 4];
            input
                .read_exact(&mut signature)
                .map_err(|error| format!("truncated REVEL ZIP header: {error}"))?;
            match signature {
                [0x50, 0x4b, 0x03, 0x04] => {
                    let mut header = [0_u8; 26];
                    input
                        .read_exact(&mut header)
                        .map_err(|error| format!("truncated REVEL ZIP member header: {error}"))?;
                    let flags = zip_u16(&header, 2);
                    let method = zip_u16(&header, 4);
                    let crc32 = zip_u32(&header, 10);
                    let compressed = zip_u32(&header, 14) as u64;
                    let uncompressed = zip_u32(&header, 18) as u64;
                    let name_len = zip_u16(&header, 22) as usize;
                    let extra_len = zip_u16(&header, 24) as usize;
                    if flags & 0x08 != 0 {
                        return Err("REVEL ZIP uses unsupported data descriptors".into());
                    }
                    let mut name = vec![0_u8; name_len];
                    input
                        .read_exact(&mut name)
                        .map_err(|error| format!("truncated REVEL ZIP member name: {error}"))?;
                    std::io::copy(
                        &mut input.by_ref().take(extra_len as u64),
                        &mut std::io::sink(),
                    )
                    .map_err(|error| format!("cannot skip REVEL ZIP extra data: {error}"))?;
                    let name = String::from_utf8(name)
                        .map_err(|_| "REVEL ZIP member name is not UTF-8")?;
                    if name.ends_with('/') {
                        if compressed != 0 || uncompressed != 0 {
                            return Err("REVEL ZIP directory entry is not empty".into());
                        }
                        continue;
                    }
                    if !name.ends_with(".csv")
                        || method != 8
                        || compressed == 0
                        || uncompressed == 0
                    {
                        return Err(format!("unsupported REVEL ZIP member: {name}"));
                    }
                    csv_members += 1;
                    let take = input.by_ref().take(compressed);
                    let mut decoder = flate2::read::DeflateDecoder::new(take);
                    let mut crc = crc32fast::Hasher::new();
                    let mut written = 0_u64;
                    let mut buffer = [0_u8; 1024 * 1024];
                    loop {
                        if live_cancel().load(Ordering::SeqCst) {
                            return Err("cancelled".into());
                        }
                        let read = decoder.read(&mut buffer).map_err(|error| {
                            format!("cannot inflate REVEL ZIP member {name}: {error}")
                        })?;
                        if read == 0 {
                            break;
                        }
                        crc.update(&buffer[..read]);
                        written = written.saturating_add(read as u64);
                        stdin
                            .write_all(&buffer[..read])
                            .map_err(|error| format!("cannot stream REVEL to fastVEP: {error}"))?;
                        let elapsed = started.elapsed().as_secs_f64();
                        let consumed = count.load(Ordering::Relaxed);
                        let downloaded = hybrid_network
                            .as_ref()
                            .map(|network| network.load(Ordering::Relaxed))
                            .unwrap_or(consumed);
                        update_revel_progress(
                            &archive.chromosome,
                            completed,
                            base_network
                                .saturating_add(resumed)
                                .saturating_add(downloaded),
                            total_network,
                            prepared_bytes,
                            if elapsed == 0.0 {
                                0.0
                            } else {
                                downloaded as f64 / elapsed
                            },
                        );
                        update_replay_detail("REVEL", &archive.chromosome, consumed, resumed);
                    }
                    let mut remaining = decoder.into_inner();
                    if remaining.limit() != 0 {
                        std::io::copy(&mut remaining, &mut std::io::sink()).map_err(|error| {
                            format!("cannot finish REVEL ZIP member {name}: {error}")
                        })?;
                    }
                    if written != uncompressed || crc.finalize() != crc32 {
                        return Err(format!(
                            "REVEL ZIP member {name} failed size or CRC validation"
                        ));
                    }
                }
                [0x50, 0x4b, 0x01, 0x02] => {
                    std::io::copy(&mut input, &mut std::io::sink())
                        .map_err(|error| format!("cannot finish REVEL ZIP validation: {error}"))?;
                    break;
                }
                _ => return Err("REVEL ZIP has an unexpected record signature".into()),
            }
        }
        drop(stdin);
        let received = count.load(Ordering::Relaxed);
        if csv_members == 0 || received != archive.bytes {
            return Err(format!(
                "REVEL chromosome {} ZIP is incomplete",
                archive.chromosome
            ));
        }
        let actual_md5 = format!("{:x}", input.hasher.finalize());
        if actual_md5 != archive.md5 {
            return Err(format!(
                "REVEL chromosome {} MD5 mismatch",
                archive.chromosome
            ));
        }
        Ok(received)
    })();
    let received = match result {
        Ok(received) => received,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP REVEL preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "fastVEP REVEL preparation failed with status {status}"
        ));
    }
    Ok(StreamingBuildResult {
        compressed_bytes_read: received,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_osa())?,
        prepared_index_bytes: required_nonempty_file(&request.paths.partial_index())?,
    })
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(output)?;
        self.hasher.update(&output[..count]);
        Ok(count)
    }
}

/// Stream one pinned, stored ZIP member with a single continuous range
/// request. dbNSFP 4.9a stores its already-gzipped chromosome members without
/// another ZIP compression layer, so the received bytes can go directly to
/// fastVEP while CRC validation happens inline.
pub fn stream_pinned_dbnsfp_member<F>(
    request: &StreamingBuildRequest<'_>,
    archive_url: &str,
    archive_bytes: u64,
    member: &DbnsfpArchiveShard,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut(StreamingProgress),
{
    if member.compression_method != 0 || member.compressed_bytes != member.source_bytes {
        return Err("pinned dbNSFP member is not a directly streamable stored entry".into());
    }
    if source_input_mode() == SourceInputMode::HybridResumable {
        let (reader, network, resumed) = hybrid_range_reader(
            request,
            archive_url,
            member.data_offset,
            archive_bytes,
            None,
            None,
        )?;
        let started = Instant::now();
        let mut checked = Crc32Reader {
            inner: reader,
            hasher: crc32fast::Hasher::new(),
        };
        let result = stream_reader_to_partial_osa_with_progress(
            request,
            &mut checked,
            cancelled,
            |state| {
                let downloaded = resumed.saturating_add(network.load(Ordering::Relaxed));
                let elapsed = started.elapsed();
                progress(StreamingProgress {
                    compressed_bytes_read: downloaded,
                    consumed_bytes: state.consumed_bytes,
                    retained_bytes: resumed,
                    expected_compressed_bytes: member.compressed_bytes,
                    elapsed,
                    bytes_per_second: if elapsed.is_zero() {
                        0.0
                    } else {
                        network.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
                    },
                });
            },
        )?;
        let actual_crc = checked.hasher.finalize();
        if actual_crc != member.crc32 {
            remove_incomplete_outputs(request.paths);
            let _ = fs::remove_file(request.paths.source_part());
            let _ = fs::remove_file(request.paths.source_part_identity());
            return Err(format!(
                "dbNSFP chromosome {} CRC mismatch",
                member.chromosome
            ));
        }
        return Ok(result);
    }
    let end = member
        .data_offset
        .checked_add(member.compressed_bytes)
        .and_then(|exclusive| exclusive.checked_sub(1))
        .ok_or("invalid dbNSFP member byte range")?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| format!("cannot create dbNSFP range client: {error}"))?;
    let response = client
        .get(archive_url)
        .header(
            reqwest::header::RANGE,
            format!("bytes={}-{}", member.data_offset, end),
        )
        .send()
        .map_err(|error| format!("dbNSFP chromosome range request failed: {error}"))?;
    let expected_content_range = format!("bytes {}-{end}/{archive_bytes}", member.data_offset);
    let actual_content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || response.content_length() != Some(member.compressed_bytes)
        || actual_content_range != Some(expected_content_range.as_str())
    {
        return Err(format!(
            "dbNSFP chromosome range returned HTTP {} or unexpected range metadata",
            response.status()
        ));
    }
    let mut checked = Crc32Reader {
        inner: response,
        hasher: crc32fast::Hasher::new(),
    };
    let result =
        stream_reader_to_partial_osa_with_progress(request, &mut checked, cancelled, progress)?;
    let actual_crc = checked.hasher.finalize();
    if actual_crc != member.crc32 {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "dbNSFP chromosome {} CRC mismatch",
            member.chromosome
        ));
    }
    Ok(result)
}

fn validate_source_response(
    response: &reqwest::blocking::Response,
    identity: &PreparationIdentity,
) -> Result<(), String> {
    if !response.status().is_success() {
        return Err(format!(
            "preparation source returned HTTP {}",
            response.status()
        ));
    }
    if identity_md5(identity).is_none() {
        if response
            .content_length()
            .is_some_and(|bytes| bytes != identity.expected_compressed_bytes)
        {
            return Err(format!(
                "source Content-Length differs from the pinned chromosome size: expected {}, received {}",
                identity.expected_compressed_bytes,
                response.content_length().unwrap_or_default()
            ));
        }
        validate_optional_header(
            response.headers(),
            reqwest::header::ETAG,
            identity.source_etag.as_deref(),
            "ETag",
        )?;
    }
    validate_optional_header(
        response.headers(),
        reqwest::header::LAST_MODIFIED,
        identity.source_last_modified.as_deref(),
        "Last-Modified",
    )
}

fn identity_md5(identity: &PreparationIdentity) -> Option<&str> {
    identity.source_etag.as_deref()?.strip_prefix("md5:")
}

fn copy_bounded_with_progress<R: Read, W: Write, F: FnMut(u64)>(
    input: &mut R,
    output: &mut W,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<u64, String> {
    // This direct blocking copy is the backpressure boundary: when fastVEP's
    // stdin pipe is full, write_all blocks and no further HTTP bytes are read.
    let mut buffer = [0_u8; STREAM_WRITER_BUFFER_BYTES as usize];
    let mut total = 0_u64;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("chromosome stream failed after {total} bytes: {error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("fastVEP input failed after {total} bytes: {error}"))?;
        total += count as u64;
        progress(total);
    }
    output
        .flush()
        .map_err(|error| format!("cannot flush fastVEP input: {error}"))?;
    Ok(total)
}

fn validate_optional_header(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
    expected: Option<&str>,
    label: &str,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = headers
        .get(name)
        .ok_or_else(|| format!("source omitted pinned {label}"))?
        .to_str()
        .map_err(|_| format!("source returned invalid {label}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("source {label} changed"))
    }
}

fn remove_incomplete_outputs(paths: &ShardPaths) {
    let _ = fs::remove_file(paths.partial_osa());
    let _ = fs::remove_file(paths.partial_index());
}

fn safe_chromosome_component(chromosome: &str) -> Result<String, String> {
    let value = chromosome.strip_prefix("chr").unwrap_or(chromosome);
    let valid = matches!(value, "X" | "Y" | "M" | "MT" | "all")
        || value
            .parse::<u8>()
            .is_ok_and(|number| (1..=22).contains(&number));
    if valid {
        Ok(format!("chr{value}"))
    } else {
        Err(format!("unsupported chromosome: {chromosome}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbnsfpArchiveShard {
    pub chromosome: String,
    pub member_name: String,
    /// Size of the gzip member payload after ZIP decompression. These are the
    /// exact bytes sent to fastVEP stdin.
    pub source_bytes: u64,
    pub compressed_bytes: u64,
    pub data_offset: u64,
    pub compression_method: u16,
    pub crc32: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbnsfpPinnedManifest {
    pub schema_version: u16,
    pub resource_id: String,
    pub release: String,
    pub archive_url: String,
    pub archive_bytes: u64,
    pub archive_md5: String,
    pub members: Vec<DbnsfpArchiveShard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedStreamShard {
    pub chromosome: String,
    pub url: String,
    pub compressed_bytes: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedShardedSource {
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub source_type: String,
    pub selected_schema: String,
    pub shards: Vec<PinnedStreamShard>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedStreamCatalog {
    schema_version: u16,
    sources: Vec<PinnedShardedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevelArchive {
    pub chromosome: String,
    pub filename: String,
    pub bytes: u64,
    pub md5: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevelArchiveManifest {
    schema_version: u16,
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub record_url: String,
    pub archives: Vec<RevelArchive>,
}

pub fn pinned_revel_manifest() -> Result<RevelArchiveManifest, String> {
    let manifest: RevelArchiveManifest =
        serde_json::from_str(include_str!("../../../config/revel-1.3-archives.json"))
            .map_err(|error| format!("invalid pinned REVEL archive manifest: {error}"))?;
    let expected = canonical_chromosomes(false);
    if manifest.schema_version != 1
        || manifest.resource_id != "revel"
        || manifest.release != "1.3"
        || manifest.assembly != "GRCh38"
        || manifest.record_url != "https://zenodo.org/records/7072866"
        || manifest.archives.len() != expected.len()
        || manifest
            .archives
            .iter()
            .map(|item| item.chromosome.as_str())
            .ne(expected)
        || manifest.archives.iter().map(|item| item.bytes).sum::<u64>() != 667_188_638
        || manifest.archives.iter().any(|item| {
            item.bytes == 0
                || item.md5.len() != 32
                || !item.md5.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !item.filename.starts_with("revel-v1.3_segments_chrom_")
                || !item.filename.ends_with(".zip")
        })
    {
        return Err("pinned REVEL archive manifest identity is invalid".into());
    }
    Ok(manifest)
}

pub fn pinned_sharded_source(resource_id: &str) -> Result<PinnedShardedSource, String> {
    let catalog: PinnedStreamCatalog =
        serde_json::from_str(include_str!("../../../config/wgs-streams.json"))
            .map_err(|error| format!("invalid pinned WGS stream catalog: {error}"))?;
    if catalog.schema_version != 1 || catalog.sources.len() != 3 {
        return Err("pinned WGS stream catalog identity is invalid".into());
    }
    let source = catalog
        .sources
        .into_iter()
        .find(|source| source.resource_id == resource_id)
        .ok_or_else(|| format!("resource '{resource_id}' has no pinned shard stream"))?;
    let expected = (1..=22)
        .map(|number| number.to_string())
        .chain(["X", "Y"].into_iter().map(str::to_string))
        .chain((resource_id == "phylop").then(|| "M".to_string()));
    let expected = expected.collect::<Vec<_>>();
    if source.assembly != "GRCh38"
        || source.shards.len() != expected.len()
        || source
            .shards
            .iter()
            .map(|shard| &shard.chromosome)
            .ne(expected.iter())
        || source.shards.iter().any(|shard| {
            shard.compressed_bytes == 0
                || !shard.url.starts_with("https://")
                || (shard.etag.is_none() && shard.last_modified.is_none())
        })
    {
        return Err(format!(
            "pinned {resource_id} chromosome stream metadata is invalid"
        ));
    }
    match resource_id {
        "gnomad"
            if source.release == "4.1.1-exomes"
                && source.source_type == "gnomad"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 199_241_266_182
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                    "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/",
                )
                }) => {}
        "gnomad-genomes"
            if source.release == "4.1.1-genomes"
                && source.source_type == "gnomad"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 565_643_483_329
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                    "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/genomes/",
                )
                }) => {}
        "phylop"
            if source.release == "hg38-100way-2015-05-08"
                && source.source_type == "phylop"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 5_452_453_066
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                        "https://hgdownload.soe.ucsc.edu/goldenPath/hg38/phyloP100way/",
                    )
                }) => {}
        _ => return Err(format!("pinned {resource_id} stream identity is invalid")),
    }
    Ok(source)
}

#[derive(Debug, Clone, Copy)]
struct CaddArtifact {
    id: &'static str,
    data_url: &'static str,
    data_bytes: u64,
    data_etag: &'static str,
    data_last_modified: &'static str,
    data_md5: &'static str,
    index_url: &'static str,
    index_bytes: u64,
    index_etag: &'static str,
    index_last_modified: &'static str,
    index_md5: &'static str,
}

#[derive(Debug, Clone)]
pub struct DbsnpArtifact {
    pub release: String,
    pub data_url: String,
    pub data_bytes: u64,
    pub data_md5: String,
    pub data_last_modified: Option<String>,
    pub index_url: String,
    pub index_bytes: u64,
    pub index_md5: String,
    pub index_last_modified: Option<String>,
}

#[derive(Debug, Clone)]
struct DbsnpArtifactPlan {
    artifact: DbsnpArtifact,
    ranges: Vec<CaddByteRange>,
}

const DBSNP_PRIMARY_CONTIGS: [(&str, &str); 25] = [
    ("1", "NC_000001.11"),
    ("2", "NC_000002.12"),
    ("3", "NC_000003.12"),
    ("4", "NC_000004.12"),
    ("5", "NC_000005.10"),
    ("6", "NC_000006.12"),
    ("7", "NC_000007.14"),
    ("8", "NC_000008.11"),
    ("9", "NC_000009.12"),
    ("10", "NC_000010.11"),
    ("11", "NC_000011.10"),
    ("12", "NC_000012.12"),
    ("13", "NC_000013.11"),
    ("14", "NC_000014.9"),
    ("15", "NC_000015.10"),
    ("16", "NC_000016.10"),
    ("17", "NC_000017.11"),
    ("18", "NC_000018.10"),
    ("19", "NC_000019.10"),
    ("20", "NC_000020.11"),
    ("21", "NC_000021.9"),
    ("22", "NC_000022.11"),
    ("X", "NC_000023.11"),
    ("Y", "NC_000024.10"),
    ("M", "NC_012920.1"),
];

const CADD_ARTIFACTS: [CaddArtifact; 2] = [
    CaddArtifact {
        id: "snv",
        data_url: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/whole_genome_SNVs.tsv.gz",
        data_bytes: 87_473_403_655,
        data_etag: "\"145dd23707-61014c1cb9940\"",
        data_last_modified: "Mon, 29 Jan 2024 12:26:37 GMT",
        data_md5: "88577a55f1cd519d44e0f415ba248eb9",
        index_url: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/whole_genome_SNVs.tsv.gz.tbi",
        index_bytes: 2_761_840,
        index_etag: "\"2a2470-610152972a200\"",
        index_last_modified: "Mon, 29 Jan 2024 12:55:36 GMT",
        index_md5: "347df8fac17ea374c4598f4f44c7ce8b",
    },
    CaddArtifact {
        id: "indel",
        data_url: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/gnomad.genomes.r4.0.indel.tsv.gz",
        data_bytes: 1_257_151_321,
        data_etag: "\"4aee9b59-60eab276e4b00\"",
        data_last_modified: "Thu, 11 Jan 2024 13:02:04 GMT",
        data_md5: "4b9c685c96d396af4d001c2f7dd9d8f9",
        index_url: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/gnomad.genomes.r4.0.indel.tsv.gz.tbi",
        index_bytes: 1_899_705,
        index_etag: "\"1cfcb9-60eab289f7800\"",
        index_last_modified: "Thu, 11 Jan 2024 13:02:24 GMT",
        index_md5: "85f3d2daa9202c5915c0ce0f1c749a66",
    },
];

#[derive(Debug, Clone, Copy)]
struct SpliceAiArtifact {
    data_url: &'static str,
    data_bytes: u64,
    data_etag: &'static str,
    data_last_modified: &'static str,
    index_url: &'static str,
    index_bytes: u64,
    index_etag: &'static str,
    index_last_modified: &'static str,
    index_md5: &'static str,
}

const SPLICEAI_ARTIFACT: SpliceAiArtifact = SpliceAiArtifact {
    data_url: "https://ftp.ensembl.org/pub/data_files/homo_sapiens/GRCh38/variation_plugins/spliceai_scores.masked.snv.ensembl_mane_v1.4.grch38.vcf.gz",
    data_bytes: 28_643_031_420,
    data_etag: "\"6ab41f97c-64146427056f6\"",
    data_last_modified: "Thu, 16 Oct 2025 13:04:38 GMT",
    index_url: "https://ftp.ensembl.org/pub/data_files/homo_sapiens/GRCh38/variation_plugins/spliceai_scores.masked.snv.ensembl_mane_v1.4.grch38.vcf.gz.tbi",
    index_bytes: 1_266_506,
    index_etag: "\"13534a-64146427cfab1\"",
    index_last_modified: "Thu, 16 Oct 2025 13:04:39 GMT",
    index_md5: "1501717babb5224fda0b63a977fe1fe6",
};

#[derive(Debug, Clone)]
struct CaddByteRange {
    chromosome: String,
    start: u64,
    end: u64,
    uncompressed_skip: u16,
}

impl CaddByteRange {
    fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone)]
struct CaddArtifactPlan {
    artifact: CaddArtifact,
    ranges: Vec<CaddByteRange>,
}

#[derive(Debug, Clone)]
struct SpliceAiArtifactPlan {
    artifact: SpliceAiArtifact,
    ranges: Vec<CaddByteRange>,
}

fn canonical_chromosomes(include_mitochondrial: bool) -> Vec<&'static str> {
    let mut chromosomes = vec![
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
        "17", "18", "19", "20", "21", "22", "X", "Y",
    ];
    if include_mitochondrial {
        chromosomes.push("M");
    }
    chromosomes
}

#[derive(Debug)]
struct CountedReader<R> {
    inner: R,
    count: Arc<std::sync::atomic::AtomicU64>,
}

impl<R: Read> Read for CountedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(output)?;
        self.count.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaddRecord {
    position: u64,
    reference: String,
    alternate: String,
    line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpliceAiRecord {
    line: String,
}

struct DbsnpReader<R: Read> {
    input: BufReader<flate2::read::MultiGzDecoder<R>>,
    source_contig: String,
    chromosome: String,
}

impl<R: Read> DbsnpReader<R> {
    fn next_record(&mut self) -> Result<Option<String>, String> {
        loop {
            let mut line = String::new();
            let read = self
                .input
                .read_line(&mut line)
                .map_err(|error| format!("cannot decode dbSNP BGZF range: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            // Indexed BGZF ranges end on compressed block boundaries, not
            // necessarily VCF record boundaries. The final decoded bytes may
            // therefore be only the beginning of the next record.
            if !line.ends_with('\n') {
                return Ok(None);
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let trimmed = line.trim_end();
            let (contig, remainder) = trimmed
                .split_once('\t')
                .ok_or("dbSNP VCF row has no tab-delimited fields")?;
            if contig != self.source_contig {
                continue;
            }
            if remainder.split('\t').count() < 7 {
                return Err("dbSNP VCF row has fewer than eight columns".into());
            }
            return Ok(Some(format!("{}\t{remainder}\n", self.chromosome)));
        }
    }
}

struct SpliceAiReader<R: Read> {
    input: BufReader<flate2::read::MultiGzDecoder<R>>,
    chromosome: Option<String>,
}

impl<R: Read> SpliceAiReader<R> {
    fn next_record(&mut self) -> Result<Option<SpliceAiRecord>, String> {
        loop {
            let mut line = String::new();
            let read = self
                .input
                .read_line(&mut line)
                .map_err(|error| format!("cannot decode SpliceAI VCF: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            if !line.ends_with('\n') {
                return Ok(None);
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields = line.trim_end().split('\t').collect::<Vec<_>>();
            if fields.len() < 8 {
                return Err("SpliceAI VCF row has fewer than eight columns".into());
            }
            if !fields[7]
                .split(';')
                .any(|field| field.starts_with("SpliceAI="))
            {
                return Err("SpliceAI VCF row is missing its SpliceAI INFO value".into());
            }
            let chromosome = fields[0].strip_prefix("chr").unwrap_or(fields[0]);
            if self
                .chromosome
                .as_deref()
                .is_some_and(|wanted| wanted != chromosome)
            {
                continue;
            }
            match chromosome {
                "X" => 23,
                "Y" => 24,
                "M" | "MT" => 25,
                value => value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| (1..=22).contains(value))
                    .ok_or_else(|| format!("unsupported SpliceAI chromosome '{chromosome}'"))?,
            };
            fields[1]
                .parse::<u64>()
                .map_err(|_| "SpliceAI VCF row has an invalid position".to_string())?;
            return Ok(Some(SpliceAiRecord {
                line: format!("{}\n", line.trim_end()),
            }));
        }
    }
}

struct CaddChromosomeReader<R: Read> {
    input: BufReader<flate2::read::MultiGzDecoder<R>>,
    chromosome: String,
}

impl<R: Read> CaddChromosomeReader<R> {
    fn new(
        mut input: flate2::read::MultiGzDecoder<R>,
        skip: u16,
        chromosome: &str,
    ) -> Result<Self, String> {
        if skip > 0 {
            std::io::copy(&mut input.by_ref().take(skip as u64), &mut std::io::sink())
                .map_err(|error| format!("cannot seek to CADD tabix virtual offset: {error}"))?;
        }
        Ok(Self {
            input: BufReader::new(input),
            chromosome: chromosome.to_string(),
        })
    }

    fn next_record(&mut self) -> Result<Option<CaddRecord>, String> {
        loop {
            let mut line = String::new();
            let read = self
                .input
                .read_line(&mut line)
                .map_err(|error| format!("cannot decode CADD BGZF range: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            if !line.ends_with('\n') {
                return Ok(None);
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields = line.trim_end().split('\t').collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(format!(
                    "CADD row has {} columns instead of 6",
                    fields.len()
                ));
            }
            let row_chromosome = fields[0].strip_prefix("chr").unwrap_or(fields[0]);
            if row_chromosome != self.chromosome {
                continue;
            }
            let position = fields[1]
                .parse::<u64>()
                .map_err(|_| "CADD row has an invalid position".to_string())?;
            return Ok(Some(CaddRecord {
                position,
                reference: fields[2].to_string(),
                alternate: fields[3].to_string(),
                line: format!("{}\n", line.trim_end()),
            }));
        }
    }
}

fn cadd_http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| format!("cannot create CADD HTTP client: {error}"))
}

fn validate_pinned_headers(
    response: &reqwest::blocking::Response,
    bytes: u64,
    etag: &str,
    last_modified: &str,
    label: &str,
) -> Result<(), String> {
    let actual_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok());
    let actual_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok());
    if !response.status().is_success()
        || response.content_length() != Some(bytes)
        || actual_etag != Some(etag)
        || actual_modified != Some(last_modified)
    {
        return Err(format!(
            "{label} no longer matches its pinned HTTP metadata"
        ));
    }
    Ok(())
}

fn fetch_cadd_index(
    client: &reqwest::blocking::Client,
    artifact: CaddArtifact,
) -> Result<Vec<TabixReferenceOffset>, String> {
    let mut response = client
        .get(artifact.index_url)
        .send()
        .map_err(|error| format!("CADD {} index request failed: {error}", artifact.id))?;
    validate_pinned_headers(
        &response,
        artifact.index_bytes,
        artifact.index_etag,
        artifact.index_last_modified,
        &format!("CADD {} index", artifact.id),
    )?;
    let mut compressed = Vec::with_capacity(artifact.index_bytes as usize);
    response
        .read_to_end(&mut compressed)
        .map_err(|error| format!("cannot read CADD {} index: {error}", artifact.id))?;
    if compressed.len() as u64 != artifact.index_bytes
        || format!("{:x}", md5::compute(&compressed)) != artifact.index_md5
    {
        return Err(format!(
            "CADD {} tabix index checksum mismatch",
            artifact.id
        ));
    }
    parse_tabix_reference_offsets(&compressed)
}

fn cadd_bgzf_block_size(
    client: &reqwest::blocking::Client,
    artifact: CaddArtifact,
    offset: u64,
) -> Result<u64, String> {
    let end = offset
        .checked_add(17)
        .ok_or("CADD BGZF probe offset overflow")?;
    let mut response = client
        .get(artifact.data_url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .map_err(|error| format!("CADD {} BGZF probe failed: {error}", artifact.id))?;
    let expected_range = format!("bytes {offset}-{end}/{}", artifact.data_bytes);
    let actual_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || response.content_length() != Some(18)
        || actual_range != Some(expected_range.as_str())
        || response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            != Some(artifact.data_etag)
        || response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            != Some(artifact.data_last_modified)
    {
        return Err(format!("CADD {} BGZF probe metadata mismatch", artifact.id));
    }
    let mut header = [0_u8; 18];
    response
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read CADD {} BGZF probe: {error}", artifact.id))?;
    if header[0..4] != [0x1f, 0x8b, 0x08, 0x04] || &header[12..14] != b"BC" {
        return Err(format!("CADD {} range is not BGZF", artifact.id));
    }
    Ok(u16::from_le_bytes([header[16], header[17]]) as u64 + 1)
}

fn plan_cadd_artifact(
    client: &reqwest::blocking::Client,
    artifact: CaddArtifact,
) -> Result<CaddArtifactPlan, String> {
    let indexed = fetch_cadd_index(client, artifact)?;
    let chromosomes = canonical_chromosomes(false);
    let mut ranges = Vec::with_capacity(chromosomes.len());
    for chromosome in chromosomes {
        let index = indexed
            .iter()
            .position(|item| item.name.strip_prefix("chr").unwrap_or(&item.name) == chromosome)
            .ok_or_else(|| {
                format!(
                    "CADD {} index is missing chromosome {chromosome}",
                    artifact.id
                )
            })?;
        let current = indexed[index].virtual_offset;
        let start = current >> 16;
        let uncompressed_skip = current as u16;
        let end = if let Some(next) = indexed.get(index + 1) {
            let next_block = next.virtual_offset >> 16;
            next_block
                .checked_add(cadd_bgzf_block_size(client, artifact, next_block)?)
                .and_then(|exclusive| exclusive.checked_sub(1))
                .ok_or("CADD chromosome range overflow")?
        } else {
            artifact.data_bytes - 1
        };
        if end < start || end >= artifact.data_bytes {
            return Err(format!(
                "CADD {} chromosome {chromosome} has an invalid range",
                artifact.id
            ));
        }
        ranges.push(CaddByteRange {
            chromosome: chromosome.to_string(),
            start,
            end,
            uncompressed_skip,
        });
    }
    Ok(CaddArtifactPlan { artifact, ranges })
}

fn fetch_spliceai_index(
    client: &reqwest::blocking::Client,
    artifact: SpliceAiArtifact,
) -> Result<Vec<TabixReferenceOffset>, String> {
    let mut response = client
        .get(artifact.index_url)
        .send()
        .map_err(|error| format!("SpliceAI tabix index request failed: {error}"))?;
    validate_pinned_headers(
        &response,
        artifact.index_bytes,
        artifact.index_etag,
        artifact.index_last_modified,
        "SpliceAI tabix index",
    )?;
    let mut compressed = Vec::with_capacity(artifact.index_bytes as usize);
    response
        .read_to_end(&mut compressed)
        .map_err(|error| format!("cannot read SpliceAI tabix index: {error}"))?;
    if compressed.len() as u64 != artifact.index_bytes
        || format!("{:x}", md5::compute(&compressed)) != artifact.index_md5
    {
        return Err("SpliceAI tabix index checksum mismatch".into());
    }
    parse_tabix_reference_offsets(&compressed)
        .map_err(|error| format!("invalid SpliceAI tabix index: {error}"))
}

fn spliceai_bgzf_block_size(
    client: &reqwest::blocking::Client,
    artifact: SpliceAiArtifact,
    offset: u64,
) -> Result<u64, String> {
    let end = offset
        .checked_add(17)
        .ok_or("SpliceAI BGZF probe offset overflow")?;
    let mut response = client
        .get(artifact.data_url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .map_err(|error| format!("SpliceAI BGZF probe failed: {error}"))?;
    let expected_range = format!("bytes {offset}-{end}/{}", artifact.data_bytes);
    let actual_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || response.content_length() != Some(18)
        || actual_range != Some(expected_range.as_str())
        || response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            != Some(artifact.data_etag)
        || response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            != Some(artifact.data_last_modified)
    {
        return Err("SpliceAI BGZF probe metadata mismatch".into());
    }
    let mut header = [0_u8; 18];
    response
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read SpliceAI BGZF probe: {error}"))?;
    if header[0..4] != [0x1f, 0x8b, 0x08, 0x04] || &header[12..14] != b"BC" {
        return Err("SpliceAI range is not BGZF".into());
    }
    Ok(u16::from_le_bytes([header[16], header[17]]) as u64 + 1)
}

fn plan_spliceai_artifact(
    client: &reqwest::blocking::Client,
) -> Result<SpliceAiArtifactPlan, String> {
    let artifact = SPLICEAI_ARTIFACT;
    let indexed = fetch_spliceai_index(client, artifact)?;
    let chromosomes = canonical_chromosomes(false);
    let mut ranges = Vec::with_capacity(chromosomes.len());
    for chromosome in chromosomes {
        let index = indexed
            .iter()
            .position(|item| item.name.strip_prefix("chr").unwrap_or(&item.name) == chromosome)
            .ok_or_else(|| format!("SpliceAI index is missing chromosome {chromosome}"))?;
        let current = indexed[index].virtual_offset;
        let start = current >> 16;
        let uncompressed_skip = current as u16;
        let end = if let Some(next) = indexed.get(index + 1) {
            let next_block = next.virtual_offset >> 16;
            next_block
                .checked_add(spliceai_bgzf_block_size(client, artifact, next_block)?)
                .and_then(|exclusive| exclusive.checked_sub(1))
                .ok_or("SpliceAI chromosome range overflow")?
        } else {
            artifact.data_bytes - 1
        };
        if end < start || end >= artifact.data_bytes {
            return Err(format!(
                "SpliceAI chromosome {chromosome} has an invalid range"
            ));
        }
        ranges.push(CaddByteRange {
            chromosome: chromosome.to_string(),
            start,
            end,
            uncompressed_skip,
        });
    }
    Ok(SpliceAiArtifactPlan { artifact, ranges })
}

fn fetch_dbsnp_index(
    client: &reqwest::blocking::Client,
    artifact: &DbsnpArtifact,
) -> Result<Vec<TabixReferenceOffset>, String> {
    let response = client
        .get(&artifact.index_url)
        .send()
        .map_err(|error| format!("dbSNP tabix index request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "dbSNP tabix index request returned HTTP {}",
            response.status()
        ));
    }
    // Some local proxies strip Content-Length even though NCBI advertises it.
    // The published size and MD5 remain authoritative; cap the read at one
    // byte beyond that size so accepting a chunked response cannot become an
    // unbounded allocation.
    if response
        .content_length()
        .is_some_and(|bytes| bytes != artifact.index_bytes)
    {
        return Err(format!(
            "dbSNP tabix index advertised {} bytes instead of {}",
            response.content_length().unwrap_or_default(),
            artifact.index_bytes
        ));
    }
    validate_optional_header(
        response.headers(),
        reqwest::header::LAST_MODIFIED,
        artifact.index_last_modified.as_deref(),
        "Last-Modified",
    )?;
    let mut compressed = Vec::with_capacity(artifact.index_bytes as usize);
    response
        .take(artifact.index_bytes.saturating_add(1))
        .read_to_end(&mut compressed)
        .map_err(|error| format!("cannot read dbSNP tabix index: {error}"))?;
    if compressed.len() as u64 != artifact.index_bytes
        || format!("{:x}", md5::compute(&compressed)) != artifact.index_md5
    {
        return Err("dbSNP tabix index checksum mismatch".into());
    }
    parse_tabix_reference_offsets(&compressed)
        .map_err(|error| format!("invalid dbSNP tabix index: {error}"))
}

fn dbsnp_bgzf_block_size(
    client: &reqwest::blocking::Client,
    artifact: &DbsnpArtifact,
    offset: u64,
) -> Result<u64, String> {
    let end = offset.checked_add(17).ok_or("dbSNP BGZF probe overflow")?;
    let mut response = client
        .get(&artifact.data_url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .map_err(|error| format!("dbSNP BGZF probe failed: {error}"))?;
    let expected_range = format!("bytes {offset}-{end}/{}", artifact.data_bytes);
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || response.content_length() != Some(18)
        || response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            != Some(expected_range.as_str())
    {
        return Err("dbSNP BGZF probe returned unexpected range metadata".into());
    }
    validate_optional_header(
        response.headers(),
        reqwest::header::LAST_MODIFIED,
        artifact.data_last_modified.as_deref(),
        "Last-Modified",
    )?;
    let mut header = [0_u8; 18];
    response
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read dbSNP BGZF probe: {error}"))?;
    if header[0..4] != [0x1f, 0x8b, 0x08, 0x04] || &header[12..14] != b"BC" {
        return Err("dbSNP indexed data is not BGZF".into());
    }
    Ok(u16::from_le_bytes([header[16], header[17]]) as u64 + 1)
}

fn plan_dbsnp_artifact(
    client: &reqwest::blocking::Client,
    artifact: DbsnpArtifact,
) -> Result<DbsnpArtifactPlan, String> {
    let indexed = fetch_dbsnp_index(client, &artifact)?;
    let mut ranges = Vec::with_capacity(DBSNP_PRIMARY_CONTIGS.len());
    for (chromosome, source_contig) in DBSNP_PRIMARY_CONTIGS {
        let index = indexed
            .iter()
            .position(|entry| entry.name == source_contig)
            .ok_or_else(|| format!("dbSNP index is missing primary contig {source_contig}"))?;
        let current = indexed[index].virtual_offset;
        let start = current >> 16;
        let uncompressed_skip = current as u16;
        let next = indexed
            .get(index + 1)
            .ok_or_else(|| format!("dbSNP contig {source_contig} has no following index range"))?;
        let next_block = next.virtual_offset >> 16;
        let end = next_block
            .checked_add(dbsnp_bgzf_block_size(client, &artifact, next_block)?)
            .and_then(|exclusive| exclusive.checked_sub(1))
            .ok_or("dbSNP chromosome range overflow")?;
        if end < start || end >= artifact.data_bytes {
            return Err(format!(
                "dbSNP chromosome {chromosome} has an invalid range"
            ));
        }
        ranges.push(CaddByteRange {
            chromosome: chromosome.into(),
            start,
            end,
            uncompressed_skip,
        });
    }
    Ok(DbsnpArtifactPlan { artifact, ranges })
}

pub fn pinned_dbnsfp_manifest() -> Result<DbnsfpPinnedManifest, String> {
    let manifest: DbnsfpPinnedManifest =
        serde_json::from_str(include_str!("../../../config/dbnsfp-4.9a-members.json"))
            .map_err(|error| format!("invalid pinned dbNSFP member manifest: {error}"))?;
    if manifest.schema_version != 1
        || manifest.resource_id != "dbnsfp"
        || manifest.release != "4.9a"
        || manifest.archive_url
            != "https://usf.box.com/shared/static/0tq7q3b8ucaxxkmfyvnb0ss7g58ptgcl"
        || manifest.archive_bytes != 38_969_753_349
        || !manifest
            .archive_md5
            .eq_ignore_ascii_case("be89346ab3dc5c14a8a7b602f50c66fb")
        || manifest.members.len() != 25
    {
        return Err("pinned dbNSFP member manifest identity is invalid".into());
    }
    let expected = (1..=22)
        .map(|number| number.to_string())
        .chain(["X", "Y", "M"].into_iter().map(str::to_string));
    for (member, chromosome) in manifest.members.iter().zip(expected) {
        if member.chromosome != chromosome
            || member.compression_method != 0
            || member.source_bytes != member.compressed_bytes
            || member.source_bytes == 0
            || member.data_offset.saturating_add(member.compressed_bytes) > manifest.archive_bytes
        {
            return Err(format!(
                "pinned dbNSFP member metadata is invalid for chromosome {chromosome}"
            ));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
pub fn dbnsfp_archive_shards(archive_path: &Path) -> Result<Vec<DbnsfpArchiveShard>, String> {
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("cannot open dbNSFP archive: {error}"))?;
    dbnsfp_archive_shards_from_reader(file)
}

#[cfg(test)]
fn dbnsfp_archive_shards_from_reader<R: Read + std::io::Seek>(
    reader: R,
) -> Result<Vec<DbnsfpArchiveShard>, String> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| format!("cannot read dbNSFP ZIP directory: {error}"))?;
    let expected = (1..=22)
        .map(|number| number.to_string())
        .chain(["X", "Y", "M"].into_iter().map(str::to_string))
        .collect::<Vec<_>>();
    let mut selected = std::collections::HashMap::new();
    for index in 0..archive.len() {
        let member = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect dbNSFP ZIP member {index}: {error}"))?;
        let Some(chromosome) = member
            .name()
            .strip_prefix("dbNSFP4.9a_variant.chr")
            .and_then(|name| name.strip_suffix(".gz"))
        else {
            continue;
        };
        if !expected.iter().any(|expected| expected == chromosome) {
            return Err(format!(
                "dbNSFP 4.9a archive contains an unexpected variant chromosome member: {}",
                member.name()
            ));
        }
        let shard = DbnsfpArchiveShard {
            chromosome: chromosome.to_string(),
            member_name: member.name().to_string(),
            source_bytes: member.size(),
            compressed_bytes: member.compressed_size(),
            data_offset: member.data_start(),
            compression_method: match member.compression() {
                zip::CompressionMethod::Stored => 0,
                zip::CompressionMethod::Deflated => 8,
                method => {
                    return Err(format!(
                        "dbNSFP member {} uses unsupported ZIP compression {method:?}",
                        member.name()
                    ));
                }
            },
            crc32: member.crc32(),
        };
        if selected.insert(chromosome.to_string(), shard).is_some() {
            return Err(format!(
                "dbNSFP 4.9a archive repeats chromosome {chromosome}"
            ));
        }
    }
    expected
        .into_iter()
        .map(|chromosome| {
            selected
                .remove(&chromosome)
                .ok_or_else(|| format!("dbNSFP 4.9a archive is missing chromosome {chromosome}"))
        })
        .collect()
}

pub fn status_with_storage(
    resource_id: &str,
    resource_root: &Path,
    chromosomes: &[String],
) -> LivePreparationState {
    let live = live_status(resource_id);
    if live.state != "idle" || chromosomes.is_empty() {
        return live;
    }
    let expected_dbnsfp_schema = (resource_id == "dbnsfp")
        .then(|| {
            load_dbnsfp_field_selection(resource_root)
                .map(|selection| dbnsfp_schema_identity(&selection))
        })
        .transpose()
        .ok()
        .flatten();
    let mut network_bytes = 0_u64;
    let mut prepared_bytes = 0_u64;
    let mut parsed_records = 0_u64;
    let mut completed = 0_u16;
    for chromosome in chromosomes {
        let Ok(paths) = ShardPaths::new(resource_root, chromosome) else {
            return live;
        };
        let Ok(checkpoint) = read_checkpoint(&paths.verification()) else {
            continue;
        };
        if checkpoint.state != CheckpointState::Verified
            || checkpoint.identity.resource_id != resource_id
            || (resource_id == "dbnsfp"
                && expected_dbnsfp_schema.as_deref()
                    != Some(checkpoint.identity.selected_schema.as_str()))
            || required_nonempty_file(&paths.final_osa()).is_err()
            || required_nonempty_file(&paths.final_index()).is_err()
        {
            continue;
        }
        network_bytes = network_bytes.saturating_add(checkpoint.compressed_bytes_read);
        prepared_bytes = prepared_bytes
            .saturating_add(checkpoint.prepared_bytes)
            .saturating_add(checkpoint.prepared_index_bytes);
        parsed_records = parsed_records.saturating_add(checkpoint.parsed_records);
        completed += 1;
    }
    let total = chromosomes.len() as u16;
    let ready = completed == total;
    LivePreparationState {
        resource_id: Some(resource_id.into()),
        state: if ready { "ready" } else { "idle" }.into(),
        phase: if ready { "ready" } else { "idle" }.into(),
        chromosome: None,
        network_bytes,
        expected_network_bytes: network_bytes,
        percent: if ready {
            100.0
        } else {
            completed as f64 * 100.0 / total as f64
        },
        parsed_records,
        prepared_bytes,
        completed_chromosomes: completed,
        remaining_chromosomes: total.saturating_sub(completed),
        detail: if ready {
            format!("All {resource_id} chromosome shards are installed and verified")
        } else if completed > 0 {
            format!("{completed} verified {resource_id} chromosome shards are retained")
        } else {
            "No preparation job is active".into()
        },
        ..LivePreparationState::default()
    }
}

pub struct LivePreparationRequest {
    pub fastvep_executable: PathBuf,
    pub source_type: String,
    pub resource_root: PathBuf,
    pub identity: PreparationIdentity,
}

pub fn start_live(mut request: LivePreparationRequest) -> Result<(), String> {
    let selection = load_supplementary_field_selection(
        &request.identity.resource_id,
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    request.identity.selected_schema = supplementary_schema_identity(
        &request.identity.selected_schema,
        &request.identity.resource_id,
        &selection,
    )?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some(request.identity.resource_id.clone()),
        state: "running".into(),
        phase: "starting".into(),
        chromosome: Some(request.identity.chromosome.clone()),
        expected_network_bytes: request.identity.expected_compressed_bytes,
        remaining_chromosomes: 1,
        detail: "Starting direct HTTP-to-fastVEP stream".into(),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || run_with_live_job(job, || run_live(request, selection)));
    Ok(())
}

pub struct DbnsfpLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
    pub local_archive: Option<PathBuf>,
}

pub struct DbsnpLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
    pub artifact: DbsnpArtifact,
}

pub fn start_dbsnp_live(request: DbsnpLiveRequest) -> Result<(), String> {
    let selection = load_supplementary_field_selection(
        "dbsnp",
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("dbsnp".into()),
        state: "running".into(),
        phase: "reading-index".into(),
        expected_network_bytes: request.artifact.data_bytes,
        remaining_chromosomes: DBSNP_PRIMARY_CONTIGS.len() as u16,
        detail: "Reading the official dbSNP tabix index".into(),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || run_with_live_job(job, || run_dbsnp_live(request, selection)));
    Ok(())
}

pub fn start_dbnsfp_live(request: DbnsfpLiveRequest) -> Result<(), String> {
    let manifest = pinned_dbnsfp_manifest()?;
    let selection = load_dbnsfp_field_selection(&request.resource_root)?;
    remove_legacy_dbnsfp_shards(&request.resource_root, &manifest)?;
    let expected = manifest
        .members
        .iter()
        .map(|member| member.source_bytes)
        .sum();
    let job = register_live_job(LivePreparationState {
        resource_id: Some("dbnsfp".into()),
        state: "running".into(),
        phase: "starting".into(),
        expected_network_bytes: expected,
        remaining_chromosomes: manifest.members.len() as u16,
        detail: if request
            .local_archive
            .as_ref()
            .is_some_and(|path| path.is_file())
        {
            "Reusing the verified local dbNSFP 4.9a archive".into()
        } else {
            "Starting direct chromosome range streams from dbNSFP 4.9a".into()
        },
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || {
        run_with_live_job(job, || run_dbnsfp_live(request, manifest, selection))
    });
    Ok(())
}

pub struct ShardedLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
    pub source: PinnedShardedSource,
}

pub struct CaddLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
}

pub struct SpliceAiLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
}

pub struct RevelLiveRequest {
    pub fastvep_executable: PathBuf,
    pub resource_root: PathBuf,
}

pub fn start_revel_live(request: RevelLiveRequest) -> Result<(), String> {
    let manifest = pinned_revel_manifest()?;
    let selection = load_supplementary_field_selection(
        "revel",
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    let expected = manifest.archives.iter().map(|item| item.bytes).sum();
    let job = register_live_job(LivePreparationState {
        resource_id: Some("revel".into()),
        state: "running".into(),
        phase: "starting".into(),
        expected_network_bytes: expected,
        remaining_chromosomes: manifest.archives.len() as u16,
        detail: "Starting official REVEL v1.3 chromosome ZIP streams".into(),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || {
        run_with_live_job(job, || run_revel_live(request, manifest, selection))
    });
    Ok(())
}

pub fn start_spliceai_live(request: SpliceAiLiveRequest) -> Result<(), String> {
    let selection = load_supplementary_field_selection(
        "spliceai",
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("spliceai".into()),
        state: "running".into(),
        phase: "reading-index".into(),
        expected_network_bytes: SPLICEAI_ARTIFACT.data_bytes,
        remaining_chromosomes: 24,
        detail: "Reading the public Ensembl SpliceAI tabix index".into(),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || run_with_live_job(job, || run_spliceai_live(request, selection)));
    Ok(())
}

pub fn start_cadd_live(request: CaddLiveRequest) -> Result<(), String> {
    let selection = load_supplementary_field_selection(
        "cadd",
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("cadd".into()),
        state: "running".into(),
        phase: "reading-indexes".into(),
        expected_network_bytes: CADD_ARTIFACTS.iter().map(|item| item.data_bytes).sum(),
        remaining_chromosomes: 24,
        detail: "Reading the two small CADD tabix indexes".into(),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || run_with_live_job(job, || run_cadd_live(request, selection)));
    Ok(())
}

type CaddHttpReader = CaddChromosomeReader<CountedReader<Box<dyn Read>>>;

fn open_cadd_range(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plan: &CaddArtifactPlan,
    range: &CaddByteRange,
    part_tag: &str,
    count: Arc<std::sync::atomic::AtomicU64>,
) -> Result<(CaddHttpReader, Option<Arc<AtomicU64>>, u64), String> {
    let (source, hybrid_network, resumed): (Box<dyn Read>, Option<Arc<AtomicU64>>, u64) =
        if source_input_mode() == SourceInputMode::HybridResumable {
            let paths = request.paths.source_part_variant(part_tag);
            let mut identity = request.identity.clone();
            identity.source_url = format!("{}#{part_tag}", plan.artifact.data_url);
            identity.expected_compressed_bytes = range.len();
            identity.source_etag = Some(plan.artifact.data_etag.into());
            identity.source_last_modified = Some(plan.artifact.data_last_modified.into());
            let range_request = StreamingBuildRequest {
                fastvep_executable: request.fastvep_executable,
                source_type: request.source_type,
                paths: &paths,
                identity: &identity,
                log_path: request.log_path,
                dbnsfp_fields: request.dbnsfp_fields,
                source_fields: request.source_fields,
            };
            let (reader, network, resumed) = hybrid_range_reader(
                &range_request,
                plan.artifact.data_url,
                range.start,
                plan.artifact.data_bytes,
                Some(plan.artifact.data_etag),
                Some(plan.artifact.data_last_modified),
            )?;
            (reader, Some(network), resumed)
        } else {
            let response = client
                .get(plan.artifact.data_url)
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", range.start, range.end),
                )
                .send()
                .map_err(|error| {
                    format!(
                        "CADD {} chromosome {} range request failed: {error}",
                        plan.artifact.id, range.chromosome
                    )
                })?;
            let expected_range = format!(
                "bytes {}-{}/{}",
                range.start, range.end, plan.artifact.data_bytes
            );
            let actual_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok());
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                || response.content_length() != Some(range.len())
                || actual_range != Some(expected_range.as_str())
                || response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    != Some(plan.artifact.data_etag)
                || response
                    .headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    != Some(plan.artifact.data_last_modified)
            {
                return Err(format!(
                    "CADD {} chromosome {} no longer matches its pinned byte range",
                    plan.artifact.id, range.chromosome
                ));
            }
            (Box::new(response), None, 0)
        };
    let reader = CaddChromosomeReader::new(
        flate2::read::MultiGzDecoder::new(CountedReader {
            inner: source,
            count,
        }),
        range.uncompressed_skip,
        &range.chromosome,
    )?;
    Ok((reader, hybrid_network, resumed))
}

fn cadd_record_before_or_equal(left: &CaddRecord, right: &CaddRecord) -> bool {
    (
        left.position,
        left.reference.as_str(),
        left.alternate.as_str(),
    ) <= (
        right.position,
        right.reference.as_str(),
        right.alternate.as_str(),
    )
}

#[allow(clippy::too_many_arguments)]
fn stream_cadd_ranges_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plans: &[CaddArtifactPlan; 2],
    chromosome_index: usize,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (mut snv, snv_network, snv_resumed) = open_cadd_range(
        request,
        client,
        &plans[0],
        &plans[0].ranges[chromosome_index],
        "snv",
        count.clone(),
    )?;
    let (mut indel, indel_network, indel_resumed) = open_cadd_range(
        request,
        client,
        &plans[1],
        &plans[1].ranges[chromosome_index],
        "indel",
        count.clone(),
    )?;
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create CADD preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut child = Command::new(request.fastvep_executable)
        .arg("sa-build")
        .arg("--source")
        .arg("cadd")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("cannot start fastVEP CADD preparation: {error}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
        stdin
            .write_all(b"#Chrom\tPos\tRef\tAlt\tRawScore\tPHRED\n")
            .map_err(|error| format!("cannot write CADD header to fastVEP: {error}"))?;
        let mut left = snv.next_record()?;
        let mut right = indel.next_record()?;
        let started = Instant::now();
        let mut last_report = 0_u64;
        while left.is_some() || right.is_some() {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let take_left = match (&left, &right) {
                (Some(left), Some(right)) => cadd_record_before_or_equal(left, right),
                (Some(_), None) => true,
                _ => false,
            };
            let record = if take_left {
                let record = left.take().unwrap();
                left = snv.next_record()?;
                record
            } else {
                let record = right.take().unwrap();
                right = indel.next_record()?;
                record
            };
            stdin
                .write_all(record.line.as_bytes())
                .map_err(|error| format!("cannot stream CADD to fastVEP: {error}"))?;
            let current = count.load(Ordering::Relaxed);
            if current.saturating_sub(last_report) >= 4 * 1024 * 1024 {
                let elapsed = started.elapsed().as_secs_f64();
                let resumed = snv_resumed.saturating_add(indel_resumed);
                let downloaded = match (&snv_network, &indel_network) {
                    (Some(snv), Some(indel)) => snv
                        .load(Ordering::Relaxed)
                        .saturating_add(indel.load(Ordering::Relaxed)),
                    _ => current,
                };
                update_cadd_progress(
                    &request.identity.chromosome,
                    completed,
                    base_network
                        .saturating_add(resumed)
                        .saturating_add(downloaded),
                    total_network,
                    prepared_bytes,
                    if elapsed == 0.0 {
                        0.0
                    } else {
                        downloaded as f64 / elapsed
                    },
                );
                update_replay_detail("CADD", &request.identity.chromosome, current, resumed);
                last_report = current;
            }
        }
        drop(stdin);
        let received = count.load(Ordering::Relaxed);
        if received != request.identity.expected_compressed_bytes {
            return Err(format!(
                "truncated CADD chromosome stream: received {received}, expected {}",
                request.identity.expected_compressed_bytes
            ));
        }
        Ok(received)
    })();
    let received = match result {
        Ok(received) => received,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP CADD preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "fastVEP CADD preparation failed with status {status}"
        ));
    }
    Ok(StreamingBuildResult {
        compressed_bytes_read: received,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_osa())?,
        prepared_index_bytes: required_nonempty_file(&request.paths.partial_index())?,
    })
}

fn update_cadd_progress(
    chromosome: &str,
    completed: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(chromosome.to_string());
        state.phase = "streaming-ranges-to-fastvep".into();
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = 24_u16.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            (network_bytes as f64 * 100.0 / expected_network_bytes as f64).min(99.9)
        };
        state.detail =
            format!("CADD chromosome {chromosome}: streaming indexed SNV and indel ranges");
    }
}

type SpliceAiHttpReader = SpliceAiReader<CountedReader<Box<dyn Read>>>;

fn open_spliceai_range(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plan: &SpliceAiArtifactPlan,
    range: &CaddByteRange,
    count: Arc<std::sync::atomic::AtomicU64>,
) -> Result<(SpliceAiHttpReader, Option<Arc<AtomicU64>>, u64), String> {
    let (source, hybrid_network, resumed): (Box<dyn Read>, Option<Arc<AtomicU64>>, u64) =
        if source_input_mode() == SourceInputMode::HybridResumable {
            let (reader, network, resumed) = hybrid_range_reader(
                request,
                plan.artifact.data_url,
                range.start,
                plan.artifact.data_bytes,
                Some(plan.artifact.data_etag),
                Some(plan.artifact.data_last_modified),
            )?;
            (reader, Some(network), resumed)
        } else {
            let response = client
                .get(plan.artifact.data_url)
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", range.start, range.end),
                )
                .send()
                .map_err(|error| {
                    format!(
                        "SpliceAI chromosome {} range request failed: {error}",
                        range.chromosome
                    )
                })?;
            let expected_range = format!(
                "bytes {}-{}/{}",
                range.start, range.end, plan.artifact.data_bytes
            );
            let actual_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok());
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                || response.content_length() != Some(range.len())
                || actual_range != Some(expected_range.as_str())
                || response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    != Some(plan.artifact.data_etag)
                || response
                    .headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|value| value.to_str().ok())
                    != Some(plan.artifact.data_last_modified)
            {
                return Err(format!(
                    "SpliceAI chromosome {} no longer matches its pinned byte range",
                    range.chromosome
                ));
            }
            (Box::new(response), None, 0)
        };
    let mut decoder = flate2::read::MultiGzDecoder::new(CountedReader {
        inner: source,
        count,
    });
    if range.uncompressed_skip > 0 {
        std::io::copy(
            &mut decoder.by_ref().take(range.uncompressed_skip as u64),
            &mut std::io::sink(),
        )
        .map_err(|error| format!("cannot seek to SpliceAI tabix virtual offset: {error}"))?;
    }
    Ok((
        SpliceAiReader {
            input: BufReader::new(decoder),
            chromosome: Some(range.chromosome.clone()),
        },
        hybrid_network,
        resumed,
    ))
}

#[allow(clippy::too_many_arguments)]
fn stream_spliceai_range_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plan: &SpliceAiArtifactPlan,
    range: &CaddByteRange,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (mut reader, hybrid_network, resumed) =
        open_spliceai_range(request, client, plan, range, count.clone())?;
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create SpliceAI preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut child = Command::new(request.fastvep_executable)
        .arg("sa-build")
        .arg("--source")
        .arg("spliceai")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("cannot start fastVEP SpliceAI preparation: {error}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
        stdin
            .write_all(b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n")
            .map_err(|error| format!("cannot write SpliceAI header to fastVEP: {error}"))?;
        let started = Instant::now();
        let mut last_report = 0_u64;
        while let Some(record) = reader.next_record()? {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            stdin
                .write_all(record.line.as_bytes())
                .map_err(|error| format!("cannot stream SpliceAI to fastVEP: {error}"))?;
            let current = count.load(Ordering::Relaxed);
            if current.saturating_sub(last_report) >= 4 * 1024 * 1024 {
                let elapsed = started.elapsed().as_secs_f64();
                let downloaded = hybrid_network
                    .as_ref()
                    .map(|network| network.load(Ordering::Relaxed))
                    .unwrap_or(current);
                update_spliceai_progress(
                    &range.chromosome,
                    completed,
                    base_network
                        .saturating_add(resumed)
                        .saturating_add(downloaded),
                    total_network,
                    prepared_bytes,
                    if elapsed == 0.0 {
                        0.0
                    } else {
                        downloaded as f64 / elapsed
                    },
                );
                update_replay_detail("SpliceAI", &range.chromosome, current, resumed);
                last_report = current;
            }
        }
        drop(stdin);
        let received = count.load(Ordering::Relaxed);
        if received != request.identity.expected_compressed_bytes {
            return Err(format!(
                "truncated SpliceAI chromosome stream: received {received}, expected {}",
                request.identity.expected_compressed_bytes
            ));
        }
        Ok(received)
    })();
    let received = match result {
        Ok(received) => received,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP SpliceAI preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "fastVEP SpliceAI preparation failed with status {status}"
        ));
    }
    Ok(StreamingBuildResult {
        compressed_bytes_read: received,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_osa())?,
        prepared_index_bytes: required_nonempty_file(&request.paths.partial_index())?,
    })
}

fn update_spliceai_progress(
    chromosome: &str,
    completed: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(chromosome.to_string());
        state.phase = "streaming-ranges-to-fastvep".into();
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = 24_u16.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            (network_bytes as f64 * 100.0 / expected_network_bytes as f64).min(99.9)
        };
        state.detail = format!(
            "SpliceAI chromosome {chromosome}: streaming the indexed public MANE SNV range"
        );
    }
}

fn update_revel_progress(
    chromosome: &str,
    completed: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(chromosome.to_string());
        state.phase = "streaming-zip-to-fastvep".into();
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = 24_u16.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            (network_bytes as f64 * 100.0 / expected_network_bytes as f64).min(99.9)
        };
        state.detail =
            format!("REVEL chromosome {chromosome}: validating and inflating official ZIP members");
    }
}

pub fn start_sharded_live(mut request: ShardedLiveRequest) -> Result<(), String> {
    let selection = load_supplementary_field_selection(
        &request.source.resource_id,
        request
            .resource_root
            .parent()
            .unwrap_or(&request.resource_root),
    )?;
    request.source.selected_schema = supplementary_schema_identity(
        &request.source.selected_schema,
        &request.source.resource_id,
        &selection,
    )?;
    let expected = request
        .source
        .shards
        .iter()
        .map(|shard| shard.compressed_bytes)
        .sum();
    let job = register_live_job(LivePreparationState {
        resource_id: Some(request.source.resource_id.clone()),
        state: "running".into(),
        phase: "starting".into(),
        expected_network_bytes: expected,
        remaining_chromosomes: request.source.shards.len() as u16,
        detail: format!(
            "Starting direct chromosome streams for {} {}",
            request.source.resource_id, request.source.release
        ),
        ..LivePreparationState::default()
    })?;
    std::thread::spawn(move || run_with_live_job(job, || run_sharded_live(request, selection)));
    Ok(())
}

fn run_sharded_live(request: ShardedLiveRequest, selection: SupplementaryFieldSelection) {
    let resource_id = request.source.resource_id.clone();
    let result = (|| {
        let expected_total = request
            .source
            .shards
            .iter()
            .map(|shard| shard.compressed_bytes)
            .sum();
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for shard in &request.source.shards {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let identity = PreparationIdentity {
                resource_id: request.source.resource_id.clone(),
                release: request.source.release.clone(),
                assembly: request.source.assembly.clone(),
                chromosome: shard.chromosome.clone(),
                source_url: shard.url.clone(),
                expected_compressed_bytes: shard.compressed_bytes,
                source_etag: shard.etag.clone(),
                source_last_modified: shard.last_modified.clone(),
                selected_schema: request.source.selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, &shard.chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_sharded_progress(
                        &request.source.resource_id,
                        shard,
                        completed,
                        request.source.shards.len() as u16,
                        network_bytes,
                        expected_total,
                        prepared_bytes,
                        0.0,
                    );
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing {} chromosome {} has a different pinned identity",
                        request.source.resource_id, shard.chromosome
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            if let Ok(mut state) = live_state().lock() {
                state.chromosome = Some(shard.chromosome.clone());
                state.phase = "streaming-to-fastvep".into();
            }
            let base_network = network_bytes;
            let log_path = paths.partial_directory.join("fastvep.log");
            let build = stream_http_to_partial_osa_with_progress(
                &StreamingBuildRequest {
                    fastvep_executable: &request.fastvep_executable,
                    source_type: &request.source.source_type,
                    paths: &paths,
                    identity: &identity,
                    log_path: &log_path,
                    dbnsfp_fields: None,
                    source_fields: Some(&selection.fields),
                },
                live_cancel().as_ref(),
                |progress| {
                    update_sharded_progress(
                        &request.source.resource_id,
                        shard,
                        completed,
                        request.source.shards.len() as u16,
                        base_network.saturating_add(progress.compressed_bytes_read),
                        expected_total,
                        prepared_bytes,
                        progress.bytes_per_second,
                    );
                    update_replay_detail(
                        &request.source.resource_id,
                        &shard.chromosome,
                        progress.consumed_bytes,
                        progress.retained_bytes,
                    )
                },
            )?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!(
                    "Validating {} chromosome {}",
                    request.source.resource_id, shard.chromosome
                );
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_osa_shard_manifest(
            &request.resource_root,
            &request.source.resource_id,
            request
                .source
                .shards
                .iter()
                .map(|shard| shard.chromosome.as_str()),
        )?;
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = format!("All {resource_id} chromosome shards are verified");
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail =
                    format!("Cancellation completed; verified {resource_id} shards were retained");
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail =
                    format!("{resource_id} preparation failed; completed shards were retained");
            }
        }
    }
}

type DbsnpHttpReader = DbsnpReader<CountedReader<Box<dyn Read>>>;

fn open_dbsnp_range(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plan: &DbsnpArtifactPlan,
    range: &CaddByteRange,
    source_contig: &str,
    count: Arc<AtomicU64>,
) -> Result<(DbsnpHttpReader, Arc<AtomicU64>, u64), String> {
    let (source, network, resumed): (Box<dyn Read>, Arc<AtomicU64>, u64) =
        if source_input_mode() == SourceInputMode::HybridResumable {
            let (reader, network, resumed) = hybrid_range_reader(
                request,
                &plan.artifact.data_url,
                range.start,
                plan.artifact.data_bytes,
                None,
                plan.artifact.data_last_modified.as_deref(),
            )?;
            if resumed > 0 {
                if let Ok(mut state) = live_state().lock() {
                    state.detail = format!(
                        "dbSNP chromosome {}: replaying {} retained hybrid part",
                        range.chromosome,
                        format_decimal_bytes(resumed)
                    );
                }
            }
            (reader, network, resumed)
        } else {
            let response = client
                .get(&plan.artifact.data_url)
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", range.start, range.end),
                )
                .send()
                .map_err(|error| {
                    format!(
                        "dbSNP chromosome {} range request failed: {error}",
                        range.chromosome
                    )
                })?;
            let expected_range = format!(
                "bytes {}-{}/{}",
                range.start, range.end, plan.artifact.data_bytes
            );
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                || response.content_length() != Some(range.len())
                || response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    != Some(expected_range.as_str())
            {
                return Err(format!(
                    "dbSNP chromosome {} returned incompatible range metadata",
                    range.chromosome
                ));
            }
            validate_optional_header(
                response.headers(),
                reqwest::header::LAST_MODIFIED,
                plan.artifact.data_last_modified.as_deref(),
                "Last-Modified",
            )?;
            (Box::new(response), count.clone(), 0)
        };
    let mut decoder = flate2::read::MultiGzDecoder::new(CountedReader {
        inner: source,
        count,
    });
    if range.uncompressed_skip > 0 {
        std::io::copy(
            &mut decoder.by_ref().take(range.uncompressed_skip as u64),
            &mut std::io::sink(),
        )
        .map_err(|error| format!("cannot seek to dbSNP tabix virtual offset: {error}"))?;
    }
    Ok((
        DbsnpReader {
            input: BufReader::new(decoder),
            source_contig: source_contig.into(),
            chromosome: range.chromosome.clone(),
        },
        network,
        resumed,
    ))
}

fn stream_dbsnp_range_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    client: &reqwest::blocking::Client,
    plan: &DbsnpArtifactPlan,
    range: &CaddByteRange,
    source_contig: &str,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let count = Arc::new(AtomicU64::new(0));
    let (mut reader, network, resumed) =
        open_dbsnp_range(request, client, plan, range, source_contig, count.clone())?;
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create dbSNP preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut command = Command::new(request.fastvep_executable);
    command
        .arg("sa-build")
        .arg("--source")
        .arg("dbsnp")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    if let Some(fields) = request.source_fields {
        command.env(
            "ANNOCAT_SOURCE_FIELDS",
            serde_json::to_string(fields)
                .map_err(|error| format!("cannot encode dbSNP fields: {error}"))?,
        );
    }
    let mut child = command
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("cannot start fastVEP dbSNP preparation: {error}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
        stdin
            .write_all(b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n")
            .map_err(|error| format!("cannot write dbSNP header to fastVEP: {error}"))?;
        let started = Instant::now();
        let mut last_report = 0_u64;
        while let Some(line) = reader.next_record()? {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            stdin
                .write_all(line.as_bytes())
                .map_err(|error| format!("cannot stream dbSNP to fastVEP: {error}"))?;
            let current = count.load(Ordering::Relaxed);
            if current.saturating_sub(last_report) >= 4 * 1024 * 1024 {
                let elapsed = started.elapsed().as_secs_f64();
                let downloaded = network.load(Ordering::Relaxed);
                update_indexed_progress(
                    "dbSNP",
                    &range.chromosome,
                    completed,
                    DBSNP_PRIMARY_CONTIGS.len() as u16,
                    base_network
                        .saturating_add(resumed)
                        .saturating_add(downloaded),
                    total_network,
                    prepared_bytes,
                    if elapsed == 0.0 {
                        0.0
                    } else {
                        downloaded as f64 / elapsed
                    },
                );
                update_replay_detail("dbSNP", &range.chromosome, current, resumed);
                last_report = current;
            }
        }
        drop(stdin);
        let received = count.load(Ordering::Relaxed);
        if received != range.len() {
            return Err(format!(
                "truncated dbSNP chromosome stream: received {received}, expected {}",
                range.len()
            ));
        }
        Ok(received)
    })();
    let received = match result {
        Ok(received) => received,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP dbSNP preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "fastVEP dbSNP preparation failed with status {status}"
        ));
    }
    Ok(StreamingBuildResult {
        compressed_bytes_read: received,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_osa())?,
        prepared_index_bytes: required_nonempty_file(&request.paths.partial_index())?,
    })
}

fn update_replay_detail(label: &str, chromosome: &str, consumed: u64, resumed: u64) {
    if consumed >= resumed || resumed == 0 {
        return;
    }
    if let Ok(mut state) = live_state().lock() {
        state.detail = format!(
            "{label} chromosome {chromosome}: replaying {} of {} retained hybrid part",
            format_decimal_bytes(consumed),
            format_decimal_bytes(resumed)
        );
    }
}

fn update_indexed_progress(
    label: &str,
    chromosome: &str,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(chromosome.into());
        state.phase = "streaming-ranges-to-fastvep".into();
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = total.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            (network_bytes as f64 * 100.0 / expected_network_bytes as f64).min(99.9)
        };
        state.detail = format!("{label} chromosome {chromosome}: streaming indexed range");
    }
}

fn run_dbsnp_live(request: DbsnpLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema = supplementary_schema_identity(
            &format!("dbsnp-{}", request.artifact.release),
            "dbsnp",
            &selection,
        )?;
        let client = cadd_http_client()?;
        let plan = plan_dbsnp_artifact(&client, request.artifact)?;
        let expected_total = plan.ranges.iter().map(CaddByteRange::len).sum();
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for ((chromosome, source_contig), range) in DBSNP_PRIMARY_CONTIGS.iter().zip(&plan.ranges) {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            if range.chromosome != *chromosome {
                return Err("dbSNP range plan chromosome order is inconsistent".into());
            }
            let identity = PreparationIdentity {
                resource_id: "dbsnp".into(),
                release: plan.artifact.release.clone(),
                assembly: "GRCh38".into(),
                chromosome: chromosome.to_string(),
                source_url: format!(
                    "{}#contig={source_contig}&data-md5={}&tabix-md5={}",
                    plan.artifact.data_url, plan.artifact.data_md5, plan.artifact.index_md5
                ),
                expected_compressed_bytes: range.len(),
                source_etag: None,
                source_last_modified: plan.artifact.data_last_modified.clone(),
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing dbSNP chromosome {chromosome} has a different release identity"
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            let build_request = StreamingBuildRequest {
                fastvep_executable: &request.fastvep_executable,
                source_type: "dbsnp",
                paths: &paths,
                identity: &identity,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: Some(&selection.fields),
            };
            let build = retry_transient_chromosome_stream(
                "dbSNP",
                chromosome,
                live_cancel().as_ref(),
                || {
                    stream_dbsnp_range_to_partial_osa(
                        &build_request,
                        &client,
                        &plan,
                        range,
                        source_contig,
                        network_bytes,
                        expected_total,
                        completed,
                        prepared_bytes,
                    )
                },
            )?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!("Validating dbSNP chromosome {chromosome}");
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_osa_shard_manifest(
            &request.resource_root,
            "dbsnp",
            DBSNP_PRIMARY_CONTIGS
                .iter()
                .map(|(chromosome, _)| *chromosome),
        )?;
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = "All dbSNP chromosome shards are verified".into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail = "dbSNP paused; source prefix and verified shards retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "dbSNP preparation failed; resumable data was retained".into();
            }
        }
    }
}

fn run_cadd_live(request: CaddLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema =
            supplementary_schema_identity("cadd-v1.7-grch38", "cadd", &selection)?;
        let client = cadd_http_client()?;
        let plans = [
            plan_cadd_artifact(&client, CADD_ARTIFACTS[0])?,
            plan_cadd_artifact(&client, CADD_ARTIFACTS[1])?,
        ];
        let expected_total = plans
            .iter()
            .flat_map(|plan| plan.ranges.iter())
            .map(CaddByteRange::len)
            .sum();
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for (index, chromosome) in canonical_chromosomes(false).iter().enumerate() {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let ranges = [&plans[0].ranges[index], &plans[1].ranges[index]];
            if ranges.iter().any(|range| range.chromosome != *chromosome) {
                return Err("CADD range plan chromosome order is inconsistent".into());
            }
            let expected = ranges.iter().map(|range| range.len()).sum();
            let identity = PreparationIdentity {
                resource_id: "cadd".into(),
                release: "1.7".into(),
                assembly: "GRCh38".into(),
                chromosome: chromosome.to_string(),
                source_url: format!(
                    "{}#bytes={}-{}+{}#bytes={}-{}",
                    plans[0].artifact.data_url,
                    ranges[0].start,
                    ranges[0].end,
                    plans[1].artifact.data_url,
                    ranges[1].start,
                    ranges[1].end
                ),
                expected_compressed_bytes: expected,
                source_etag: Some(format!(
                    "snv:{}:{};indel:{}:{}",
                    plans[0].artifact.data_etag,
                    plans[0].artifact.data_md5,
                    plans[1].artifact.data_etag,
                    plans[1].artifact.data_md5
                )),
                source_last_modified: Some(format!(
                    "snv:{};indel:{}",
                    plans[0].artifact.data_last_modified, plans[1].artifact.data_last_modified
                )),
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_cadd_progress(
                        chromosome,
                        completed,
                        network_bytes,
                        expected_total,
                        prepared_bytes,
                        0.0,
                    );
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing CADD chromosome {chromosome} has a different pinned identity"
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            let build = stream_cadd_ranges_to_partial_osa(
                &StreamingBuildRequest {
                    fastvep_executable: &request.fastvep_executable,
                    source_type: "cadd",
                    paths: &paths,
                    identity: &identity,
                    log_path: &paths.partial_directory.join("fastvep.log"),
                    dbnsfp_fields: None,
                    source_fields: Some(&selection.fields),
                },
                &client,
                &plans,
                index,
                network_bytes,
                expected_total,
                completed,
                prepared_bytes,
            )?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!("Validating CADD chromosome {chromosome}");
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_osa_shard_manifest(
            &request.resource_root,
            "cadd",
            canonical_chromosomes(false).into_iter(),
        )?;
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = "All CADD chromosome shards are verified".into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail = "Cancellation completed; verified CADD shards were retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "CADD preparation failed; completed shards were retained".into();
            }
        }
    }
}

fn run_revel_live(
    request: RevelLiveRequest,
    manifest: RevelArchiveManifest,
    selection: SupplementaryFieldSelection,
) {
    let result = (|| {
        let selected_schema =
            supplementary_schema_identity("revel-v1.3-transcript-matched", "revel", &selection)?;
        let expected_total = manifest.archives.iter().map(|item| item.bytes).sum();
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for archive in &manifest.archives {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let identity = PreparationIdentity {
                resource_id: "revel".into(),
                release: manifest.release.clone(),
                assembly: manifest.assembly.clone(),
                chromosome: archive.chromosome.clone(),
                source_url: format!(
                    "https://zenodo.org/api/records/7072866/files/{}/content",
                    archive.filename
                ),
                expected_compressed_bytes: archive.bytes,
                source_etag: Some(format!("md5:{}", archive.md5)),
                source_last_modified: None,
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, &archive.chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_revel_progress(
                        &archive.chromosome,
                        completed,
                        network_bytes,
                        expected_total,
                        prepared_bytes,
                        0.0,
                    );
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing REVEL chromosome {} has a different pinned identity",
                        archive.chromosome
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            let build = stream_revel_archive_to_partial_osa(
                &StreamingBuildRequest {
                    fastvep_executable: &request.fastvep_executable,
                    source_type: "revel",
                    paths: &paths,
                    identity: &identity,
                    log_path: &paths.partial_directory.join("fastvep.log"),
                    dbnsfp_fields: None,
                    source_fields: Some(&selection.fields),
                },
                archive,
                network_bytes,
                expected_total,
                completed,
                prepared_bytes,
            )?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!("Validating REVEL chromosome {}", archive.chromosome);
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_osa_shard_manifest(
            &request.resource_root,
            "revel",
            canonical_chromosomes(false).into_iter(),
        )?;
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = "All REVEL v1.3 chromosome shards are verified".into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail = "Cancellation completed; verified REVEL shards were retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "REVEL preparation failed; completed shards were retained".into();
            }
        }
    }
}

fn run_spliceai_live(request: SpliceAiLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema = supplementary_schema_identity(
            "spliceai-ensembl-mane-v1.4-masked-snv",
            "spliceai",
            &selection,
        )?;
        let client = cadd_http_client()?;
        let plan = plan_spliceai_artifact(&client)?;
        let expected_total = plan.ranges.iter().map(CaddByteRange::len).sum();
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for (index, chromosome) in canonical_chromosomes(false).iter().enumerate() {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let range = &plan.ranges[index];
            if range.chromosome != *chromosome {
                return Err("SpliceAI range plan chromosome order is inconsistent".into());
            }
            let identity = PreparationIdentity {
                resource_id: "spliceai".into(),
                release: "ensembl-mane-v1.4-masked-snv".into(),
                assembly: "GRCh38".into(),
                chromosome: chromosome.to_string(),
                source_url: format!(
                    "{}#bytes={}-{}",
                    plan.artifact.data_url, range.start, range.end
                ),
                expected_compressed_bytes: range.len(),
                source_etag: Some(plan.artifact.data_etag.into()),
                source_last_modified: Some(plan.artifact.data_last_modified.into()),
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_spliceai_progress(
                        chromosome,
                        completed,
                        network_bytes,
                        expected_total,
                        prepared_bytes,
                        0.0,
                    );
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing SpliceAI chromosome {chromosome} has a different pinned identity"
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            let build = stream_spliceai_range_to_partial_osa(
                &StreamingBuildRequest {
                    fastvep_executable: &request.fastvep_executable,
                    source_type: "spliceai",
                    paths: &paths,
                    identity: &identity,
                    log_path: &paths.partial_directory.join("fastvep.log"),
                    dbnsfp_fields: None,
                    source_fields: Some(&selection.fields),
                },
                &client,
                &plan,
                range,
                network_bytes,
                expected_total,
                completed,
                prepared_bytes,
            )?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!("Validating SpliceAI chromosome {chromosome}");
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_osa_shard_manifest(
            &request.resource_root,
            "spliceai",
            canonical_chromosomes(false).into_iter(),
        )?;
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = "All public SpliceAI chromosome shards are verified".into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail =
                    "Cancellation completed; verified SpliceAI shards were retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "SpliceAI preparation failed; completed shards were retained".into();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_sharded_progress(
    resource_id: &str,
    shard: &PinnedStreamShard,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(shard.chromosome.clone());
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = total.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            network_bytes as f64 * 100.0 / expected_network_bytes as f64
        };
        state.detail = format!(
            "{resource_id} chromosome {}: {} of {} source bytes",
            shard.chromosome, network_bytes, expected_network_bytes
        );
    }
}

const TRANSIENT_CHROMOSOME_STREAM_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
];

fn is_transient_chromosome_stream_error(error: &str) -> bool {
    error.contains("chromosome range request failed")
        || error.contains("chromosome stream failed")
        || error.contains("truncated chromosome stream")
        || error.contains("cannot decode dbSNP BGZF range: request or response body error")
}

fn retry_transient_chromosome_stream<F>(
    resource_id: &str,
    chromosome: &str,
    cancelled: &AtomicBool,
    mut operation: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut() -> Result<StreamingBuildResult, String>,
{
    let total_attempts = TRANSIENT_CHROMOSOME_STREAM_RETRY_DELAYS.len() + 1;
    for attempt in 0..total_attempts {
        match operation() {
            Ok(result) => return Ok(result),
            Err(error) if error == "cancelled" || cancelled.load(Ordering::SeqCst) => {
                return Err("cancelled".into());
            }
            Err(error)
                if is_transient_chromosome_stream_error(&error) && attempt + 1 < total_attempts =>
            {
                let delay = TRANSIENT_CHROMOSOME_STREAM_RETRY_DELAYS[attempt];
                crate::terminal_log(
                    "prepare",
                    format!(
                        "{resource_id} chromosome {chromosome} interrupted on attempt {}/{}: {error}; retrying in {}s",
                        attempt + 1,
                        total_attempts,
                        delay.as_secs()
                    ),
                );
                if let Ok(mut state) = live_state().lock() {
                    state.phase = "retrying".into();
                    state.throughput_bytes_per_second = 0.0;
                    state.detail = format!(
                        "{resource_id} chromosome {chromosome} stream was interrupted; retrying from the beginning in {} seconds (attempt {} of {total_attempts})",
                        delay.as_secs(),
                        attempt + 2,
                    );
                }
                std::thread::sleep(delay);
                if cancelled.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                if let Ok(mut state) = live_state().lock() {
                    state.phase = "streaming-to-fastvep".into();
                    state.detail = format!(
                        "{resource_id} chromosome {chromosome}: restarting the source stream"
                    );
                }
            }
            Err(error) if is_transient_chromosome_stream_error(&error) => {
                crate::terminal_log(
                    "prepare",
                    format!(
                        "{resource_id} chromosome {chromosome} failed after {total_attempts} attempts: {error}"
                    ),
                );
                return Err(format!(
                    "{error}; chromosome {chromosome} failed after {total_attempts} attempts"
                ));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("chromosome retry loop always returns")
}

fn run_dbnsfp_live(
    request: DbnsfpLiveRequest,
    manifest: DbnsfpPinnedManifest,
    selection: DbnsfpFieldSelection,
) {
    let result = (|| {
        let mut network_bytes = 0_u64;
        let mut prepared_bytes = 0_u64;
        let mut completed = 0_u16;
        for member in &manifest.members {
            if live_cancel().load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let identity = PreparationIdentity {
                resource_id: "dbnsfp".into(),
                release: "4.9a".into(),
                assembly: "GRCh38".into(),
                chromosome: member.chromosome.clone(),
                source_url: format!("{}#{}", manifest.archive_url, member.member_name),
                expected_compressed_bytes: member.source_bytes,
                source_etag: Some(format!("zip-crc32:{:08x}", member.crc32)),
                source_last_modified: None,
                selected_schema: dbnsfp_schema_identity(&selection),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: 1,
            };
            let paths = ShardPaths::new(&request.resource_root, &member.chromosome)?;
            match restart_decision_with_legacy_upgrade(
                &request.fastvep_executable,
                &paths,
                &identity,
            ) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_dbnsfp_progress(
                        member,
                        completed,
                        manifest.members.len() as u16,
                        network_bytes,
                        manifest.members.iter().map(|item| item.source_bytes).sum(),
                        prepared_bytes,
                        0.0,
                    );
                    continue;
                }
                RestartDecision::StaleIdentity => {
                    return Err(format!(
                        "existing dbNSFP chromosome {} has a different pinned identity",
                        member.chromosome
                    ));
                }
                RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                    initialize_partial(&paths, identity.clone())?;
                }
            }
            if let Ok(mut state) = live_state().lock() {
                state.chromosome = Some(member.chromosome.clone());
                state.phase = "streaming-to-fastvep".into();
            }
            let log_path = paths.partial_directory.join("fastvep.log");
            let build_request = StreamingBuildRequest {
                fastvep_executable: &request.fastvep_executable,
                source_type: "dbnsfp",
                paths: &paths,
                identity: &identity,
                log_path: &log_path,
                dbnsfp_fields: Some(&selection.fields),
                source_fields: None,
            };
            let base_network = network_bytes;
            let expected_total: u64 = manifest.members.iter().map(|item| item.source_bytes).sum();
            let build = if request
                .local_archive
                .as_ref()
                .is_some_and(|path| path.is_file())
            {
                stream_local_dbnsfp_member(
                    &build_request,
                    request.local_archive.as_ref().unwrap(),
                    member,
                    live_cancel().as_ref(),
                    |progress| {
                        update_dbnsfp_progress(
                            member,
                            completed,
                            manifest.members.len() as u16,
                            base_network.saturating_add(progress.compressed_bytes_read),
                            expected_total,
                            prepared_bytes,
                            progress.bytes_per_second,
                        )
                    },
                )
            } else {
                retry_transient_chromosome_stream(
                    "dbNSFP",
                    &member.chromosome,
                    live_cancel().as_ref(),
                    || {
                        stream_pinned_dbnsfp_member(
                            &build_request,
                            &manifest.archive_url,
                            manifest.archive_bytes,
                            member,
                            live_cancel().as_ref(),
                            |progress| {
                                update_dbnsfp_progress(
                                    member,
                                    completed,
                                    manifest.members.len() as u16,
                                    base_network.saturating_add(progress.compressed_bytes_read),
                                    expected_total,
                                    prepared_bytes,
                                    progress.bytes_per_second,
                                );
                                update_replay_detail(
                                    "dbNSFP",
                                    &member.chromosome,
                                    progress.consumed_bytes,
                                    progress.retained_bytes,
                                )
                            },
                        )
                    },
                )
            }?;
            checkpoint_stream_complete(&paths, identity.clone(), &build)?;
            if let Ok(mut state) = live_state().lock() {
                state.phase = "validating".into();
                state.detail = format!("Validating dbNSFP chromosome {}", member.chromosome);
            }
            let verification = verify_partial_osa(&request.fastvep_executable, &paths, &identity)?;
            promote_verified(
                &paths,
                identity,
                build.compressed_bytes_read,
                verification.record_count,
            )?;
            network_bytes = network_bytes.saturating_add(build.compressed_bytes_read);
            prepared_bytes = prepared_bytes
                .saturating_add(build.prepared_osa_bytes)
                .saturating_add(build.prepared_index_bytes);
            completed += 1;
        }
        write_shard_manifest(&request.resource_root, &manifest.members)?;
        if let Some(archive) = request.local_archive.as_deref() {
            remove_consumed_dbnsfp_archive(archive)?;
        }
        Ok((network_bytes, prepared_bytes, completed))
    })();
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((network, prepared, completed)) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = network;
                state.prepared_bytes = prepared;
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.detail = "All dbNSFP 4.9a chromosome shards are verified".into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail =
                    "Cancellation completed; verified dbNSFP shards were retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "dbNSFP preparation failed; completed shards were retained".into();
            }
        }
    }
}

fn remove_legacy_dbnsfp_shards(
    resource_root: &Path,
    manifest: &DbnsfpPinnedManifest,
) -> Result<(), String> {
    for member in &manifest.members {
        let paths = ShardPaths::new(resource_root, &member.chromosome)?;
        if paths.final_directory.is_dir()
            && read_checkpoint(&paths.verification()).map_or(true, |checkpoint| {
                checkpoint.identity.resource_id != "dbnsfp"
                    || checkpoint.identity.release != "4.9a"
                    || !checkpoint
                        .identity
                        .selected_schema
                        .starts_with(DBNSFP_CURATED_SCHEMA)
            })
        {
            fs::remove_dir_all(&paths.final_directory).map_err(|error| {
                format!(
                    "cannot replace legacy dbNSFP chromosome {} cache: {error}",
                    member.chromosome
                )
            })?;
        }
        if paths.partial_directory.is_dir()
            && read_checkpoint(&paths.checkpoint()).map_or(true, |checkpoint| {
                checkpoint.identity.resource_id != "dbnsfp"
                    || checkpoint.identity.release != "4.9a"
                    || !checkpoint
                        .identity
                        .selected_schema
                        .starts_with(DBNSFP_CURATED_SCHEMA)
            })
        {
            fs::remove_dir_all(&paths.partial_directory).map_err(|error| {
                format!(
                    "cannot clear legacy dbNSFP chromosome {} staging cache: {error}",
                    member.chromosome
                )
            })?;
        }
    }
    Ok(())
}

fn remove_consumed_dbnsfp_archive(archive: &Path) -> Result<(), String> {
    let filename = archive
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("local dbNSFP archive has no filename")?;
    for path in [
        archive.to_path_buf(),
        archive.with_file_name(format!("{filename}.verified.json")),
        archive.with_file_name(format!("{filename}.partial")),
        archive.with_file_name(format!("{filename}.new-partial")),
    ] {
        if path.is_file() {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "dbNSFP cache is verified but its consumed source archive could not be removed ({}): {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn stream_local_dbnsfp_member<F>(
    request: &StreamingBuildRequest<'_>,
    archive_path: &Path,
    member: &DbnsfpArchiveShard,
    cancelled: &AtomicBool,
    progress: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut(StreamingProgress),
{
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("cannot open local dbNSFP archive: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("cannot read local dbNSFP archive: {error}"))?;
    let source = archive
        .by_name(&member.member_name)
        .map_err(|error| format!("cannot open {}: {error}", member.member_name))?;
    if source.size() != member.source_bytes || source.crc32() != member.crc32 {
        return Err(format!(
            "local dbNSFP chromosome {} differs from the pinned manifest",
            member.chromosome
        ));
    }
    let mut checked = Crc32Reader {
        inner: source,
        hasher: crc32fast::Hasher::new(),
    };
    let result =
        stream_reader_to_partial_osa_with_progress(request, &mut checked, cancelled, progress)?;
    if checked.hasher.finalize() != member.crc32 {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "local dbNSFP chromosome {} CRC mismatch",
            member.chromosome
        ));
    }
    Ok(result)
}

fn update_dbnsfp_progress(
    member: &DbnsfpArchiveShard,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(member.chromosome.clone());
        state.network_bytes = network_bytes;
        state.expected_network_bytes = expected_network_bytes;
        state.prepared_bytes = prepared_bytes;
        state.throughput_bytes_per_second = throughput;
        state.completed_chromosomes = completed;
        state.remaining_chromosomes = total.saturating_sub(completed);
        state.percent = if expected_network_bytes == 0 {
            0.0
        } else {
            network_bytes as f64 * 100.0 / expected_network_bytes as f64
        };
        state.detail = format!(
            "dbNSFP chromosome {}: {} of {} source bytes",
            member.chromosome, network_bytes, expected_network_bytes
        );
    }
}

fn write_shard_manifest(
    resource_root: &Path,
    members: &[DbnsfpArchiveShard],
) -> Result<(), String> {
    write_osa_shard_manifest(
        resource_root,
        "dbnsfp",
        members.iter().map(|member| member.chromosome.as_str()),
    )
}

fn write_osa_shard_manifest<'a>(
    resource_root: &Path,
    resource_id: &str,
    chromosomes: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    if !matches!(
        resource_id,
        "dbnsfp"
            | "gnomad"
            | "gnomad-genomes"
            | "phylop"
            | "cadd"
            | "spliceai"
            | "revel"
            | "clinvar"
    ) {
        return Err("unsupported OSA shard manifest resource".into());
    }
    let shards = chromosomes
        .map(|chromosome| {
            serde_json::json!({
                "chromosome": chromosome,
                "file": format!("shards/chr{chromosome}/source.osa")
            })
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({"schemaVersion": 1, "shards": shards});
    let final_path = resource_root.join(format!("{resource_id}.osa-shards.json"));
    let partial_path = resource_root.join(format!("{resource_id}.osa-shards.json.partial"));
    fs::create_dir_all(resource_root)
        .map_err(|error| format!("cannot create {resource_id} resource directory: {error}"))?;
    fs::write(
        &partial_path,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {resource_id} shard manifest: {error}"))?;
    if final_path.exists() {
        fs::remove_file(&final_path)
            .map_err(|error| format!("cannot replace {resource_id} shard manifest: {error}"))?;
    }
    fs::rename(&partial_path, &final_path)
        .map_err(|error| format!("cannot publish {resource_id} shard manifest: {error}"))
}

fn run_live(request: LivePreparationRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let paths = ShardPaths::new(&request.resource_root, &request.identity.chromosome)?;
        match restart_decision_with_legacy_upgrade(
            &request.fastvep_executable,
            &paths,
            &request.identity,
        ) {
            RestartDecision::AlreadyVerified => {
                if request.identity.resource_id == "clinvar" {
                    write_osa_shard_manifest(
                        &request.resource_root,
                        "clinvar",
                        std::iter::once("all"),
                    )?;
                }
                return Ok(("ready", 1, 0, 0));
            }
            RestartDecision::StaleIdentity => {
                if identity_md5(&request.identity).is_none() {
                    return Err(
                        "existing shard checkpoint has a different preparation identity".into(),
                    );
                }
                // A rolling release resolved to a new dated snapshot. Its old
                // staging directory was never promoted, so discard only that
                // incomplete checkpoint and restart with the new checksum.
                if paths.partial_directory.exists() {
                    fs::remove_dir_all(&paths.partial_directory).map_err(|error| {
                        format!("cannot replace stale rolling-source staging data: {error}")
                    })?;
                }
                initialize_partial(&paths, request.identity.clone())?;
            }
            RestartDecision::Start | RestartDecision::RestartCurrentChromosome => {
                initialize_partial(&paths, request.identity.clone())?;
            }
        }
        let log_path = paths.partial_directory.join("fastvep.log");
        let build = stream_http_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &request.fastvep_executable,
                source_type: &request.source_type,
                paths: &paths,
                identity: &request.identity,
                log_path: &log_path,
                dbnsfp_fields: None,
                source_fields: Some(&selection.fields),
            },
            live_cancel().as_ref(),
            |progress| {
                if let Ok(mut state) = live_state().lock() {
                    state.phase = "streaming-to-fastvep".into();
                    state.network_bytes = progress.compressed_bytes_read;
                    state.throughput_bytes_per_second = progress.bytes_per_second;
                    state.percent = if progress.expected_compressed_bytes == 0 {
                        0.0
                    } else {
                        progress.compressed_bytes_read as f64 * 100.0
                            / progress.expected_compressed_bytes as f64
                    };
                    state.detail = format!(
                        "{} of {} compressed bytes",
                        progress.compressed_bytes_read, progress.expected_compressed_bytes
                    );
                }
            },
        )?;
        let mut verified_identity = request.identity.clone();
        if identity_md5(&verified_identity).is_some() {
            verified_identity.expected_compressed_bytes = build.compressed_bytes_read;
        }
        checkpoint_stream_complete(&paths, verified_identity.clone(), &build)?;
        if let Ok(mut state) = live_state().lock() {
            state.network_bytes = build.compressed_bytes_read;
            state.prepared_bytes = build
                .prepared_osa_bytes
                .saturating_add(build.prepared_index_bytes);
            state.phase = "validating".into();
            state.detail = "Reopening and validating every prepared OSA block".into();
        }
        let verification =
            verify_partial_osa(&request.fastvep_executable, &paths, &verified_identity)?;
        promote_verified(
            &paths,
            verified_identity,
            build.compressed_bytes_read,
            verification.record_count,
        )?;
        if request.identity.resource_id == "clinvar" {
            write_osa_shard_manifest(&request.resource_root, "clinvar", std::iter::once("all"))?;
        }
        Ok((
            "ready",
            1,
            0,
            build
                .prepared_osa_bytes
                .saturating_add(build.prepared_index_bytes),
        ))
    })();

    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok((phase, completed, remaining, prepared)) => {
                state.state = "ready".into();
                state.phase = phase.into();
                state.completed_chromosomes = completed;
                state.remaining_chromosomes = remaining;
                state.prepared_bytes = prepared;
                if phase == "ready" {
                    state.percent = 100.0;
                    state.detail = "All requested chromosome shards are verified".into();
                }
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail = "Cancellation completed; verified shards were retained".into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = "Preparation failed; incomplete outputs were not promoted".into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn live_preparation_jobs_keep_progress_and_cancellation_independent() {
        let first_id = "parallel-state-fixture-a";
        let second_id = "parallel-state-fixture-b";
        let first = register_live_job(LivePreparationState {
            resource_id: Some(first_id.into()),
            state: "running".into(),
            ..LivePreparationState::default()
        })
        .unwrap();
        let second = register_live_job(LivePreparationState {
            resource_id: Some(second_id.into()),
            state: "running".into(),
            ..LivePreparationState::default()
        })
        .unwrap();

        run_with_live_job(first.clone(), || {
            let state = live_state();
            state.lock().unwrap().percent = 42.0;
        });
        assert_eq!(live_status(first_id).percent, 42.0);
        assert_eq!(live_status(second_id).percent, 0.0);
        assert!(cancel_live(first_id));
        assert!(first.cancel.load(Ordering::SeqCst));
        assert!(!second.cancel.load(Ordering::SeqCst));

        first.state.lock().unwrap().state = "cancelled".into();
        second.state.lock().unwrap().state = "cancelled".into();
        forget_live(first_id);
        forget_live(second_id);
    }

    #[test]
    fn startup_failures_remain_visible_until_the_next_attempt() {
        let resource_id = "startup-failure-fixture";
        record_start_failure(resource_id, "publisher is temporarily unavailable", 42);
        let state = live_status(resource_id);
        assert_eq!(state.state, "failed");
        assert_eq!(state.phase, "start-failed");
        assert_eq!(state.expected_network_bytes, 42);
        assert_eq!(
            state.error.as_deref(),
            Some("publisher is temporarily unavailable")
        );
        forget_live(resource_id);
        assert_eq!(live_status(resource_id).state, "idle");
    }

    fn dbnsfp_zip(path: &Path, chromosomes: &[&str]) {
        let file = fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for chromosome in chromosomes {
            archive
                .start_file(format!("dbNSFP4.9a_variant.chr{chromosome}.gz"), options)
                .unwrap();
            archive.write_all(b"\x1f\x8bfixture").unwrap();
        }
        archive.finish().unwrap();
    }

    fn identity(chromosome: &str) -> PreparationIdentity {
        PreparationIdentity {
            resource_id: "gnomad".into(),
            release: "4.1".into(),
            assembly: "GRCh38".into(),
            chromosome: chromosome.into(),
            source_url: format!("https://example.test/{chromosome}.vcf.bgz"),
            expected_compressed_bytes: 10,
            source_etag: Some("etag".into()),
            source_last_modified: None,
            selected_schema: "gnomad-v4.1-wgs".into(),
            fastvep_commit: "7038e7c".into(),
            osa_schema_version: 1,
        }
    }

    fn dbnsfp_identity(chromosome: &str, selected_schema: &str) -> PreparationIdentity {
        PreparationIdentity {
            resource_id: "dbnsfp".into(),
            release: "4.9a".into(),
            assembly: "GRCh38".into(),
            chromosome: chromosome.into(),
            source_url: format!("https://example.test/chr{chromosome}.gz"),
            expected_compressed_bytes: 10,
            source_etag: Some("zip-crc32:fixture".into()),
            source_last_modified: None,
            selected_schema: selected_schema.into(),
            fastvep_commit: "7038e7c".into(),
            osa_schema_version: 1,
        }
    }

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "annocat-preparation-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn only_transient_transport_failures_restart_a_chromosome_stream() {
        assert!(is_transient_chromosome_stream_error(
            "chromosome stream failed after 425510954 bytes: request or response body error"
        ));
        assert!(is_transient_chromosome_stream_error(
            "dbNSFP chromosome range request failed: connection reset"
        ));
        assert!(is_transient_chromosome_stream_error(
            "truncated chromosome stream: received 42, expected 84"
        ));
        assert!(is_transient_chromosome_stream_error(
            "cannot decode dbSNP BGZF range: request or response body error"
        ));
        assert!(!is_transient_chromosome_stream_error(
            "dbNSFP chromosome 2 CRC mismatch"
        ));
        assert!(!is_transient_chromosome_stream_error(
            "fastVEP preparation failed with status 1"
        ));
    }

    fn fixture_response(headers: &str, body: &'static [u8]) -> reqwest::blocking::Response {
        let url = fixture_url(headers, body);
        reqwest::blocking::get(url).unwrap()
    }

    fn fixture_url(headers: &str, body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            write!(stream, "HTTP/1.1 200 OK\r\n{headers}\r\n").unwrap();
            stream.write_all(body).unwrap();
        });
        format!("http://{address}/fixture")
    }

    fn fixture_url_recording_request(
        body: &'static [u8],
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..count]).into_owned())
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: etag\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (format!("http://{address}/fixture"), receiver)
    }

    fn range_fixture(body: &'static [u8], request_count: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                let range = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("range: bytes="))
                    .unwrap();
                let range = range.split_once('=').unwrap().1;
                let (start, end) = range.split_once('-').unwrap();
                let start: usize = start.parse().unwrap();
                let end: usize = end.parse().unwrap();
                let bytes = &body[start..=end];
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nETag: fixture\r\nConnection: close\r\n\r\n",
                    bytes.len(),
                    body.len()
                )
                .unwrap();
                stream.write_all(bytes).unwrap();
            }
        });
        format!("http://{address}/archive.zip")
    }

    fn interrupted_range_fixture(
        body: &'static [u8],
        first_bytes: usize,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]).into_owned();
                sender.send(request.clone()).unwrap();
                let range = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("range: bytes="))
                    .unwrap()
                    .split_once('=')
                    .unwrap()
                    .1;
                let (start, end) = range.split_once('-').unwrap();
                let start: usize = start.parse().unwrap();
                let end: usize = end.parse().unwrap();
                let bytes = &body[start..=end];
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nETag: fixture\r\nConnection: close\r\n\r\n",
                    bytes.len(),
                    body.len()
                )
                .unwrap();
                if request_number == 0 {
                    stream.write_all(&bytes[..first_bytes]).unwrap();
                } else {
                    stream.write_all(bytes).unwrap();
                }
            }
        });
        (format!("http://{address}/interrupted"), receiver)
    }

    struct InstrumentedReader {
        remaining: usize,
        largest_requested_buffer: usize,
    }

    impl Read for InstrumentedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.largest_requested_buffer = self.largest_requested_buffer.max(buffer.len());
            let count = self.remaining.min(buffer.len());
            buffer[..count].fill(7);
            self.remaining -= count;
            Ok(count)
        }
    }

    struct FailingWriter {
        accepted: usize,
        limit: usize,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.accepted >= self.limit {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "fixture disk full",
                ));
            }
            let count = bytes.len().min(self.limit - self.accepted);
            self.accepted += count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn restart_keeps_verified_chromosomes_and_restarts_only_partial_work() {
        let root = root("restart");
        let chr1 = ShardPaths::new(&root, "1").unwrap();
        initialize_partial(&chr1, identity("1")).unwrap();
        fs::write(chr1.partial_osa(), b"osa").unwrap();
        fs::write(chr1.partial_index(), b"idx").unwrap();
        promote_verified(&chr1, identity("1"), 10, 2).unwrap();
        let mut contract = crate::cache_contract::read(&chr1.cache_contract()).unwrap();
        assert_eq!(
            contract.builder_provenance,
            crate::fastvep::pinned_builder_provenance()
        );
        contract.builder_provenance.commit = "future-compatible-fastvep".into();
        contract.builder_provenance.binary_sha256 = "future-compatible-binary".into();
        fs::write(
            chr1.cache_contract(),
            serde_json::to_vec_pretty(&contract).unwrap(),
        )
        .unwrap();
        assert_eq!(
            restart_decision(&chr1, &identity("1")),
            RestartDecision::AlreadyVerified
        );
        let mut equivalent_mirror = identity("1");
        equivalent_mirror.source_url = "https://equivalent-mirror.test/1.vcf.bgz".into();
        assert_eq!(
            restart_decision(&chr1, &equivalent_mirror),
            RestartDecision::AlreadyVerified
        );

        let chr2 = ShardPaths::new(&root, "2").unwrap();
        initialize_partial(&chr2, identity("2")).unwrap();
        fs::write(chr2.partial_osa(), b"incomplete").unwrap();
        assert_eq!(
            restart_decision(&chr2, &identity("2")),
            RestartDecision::RestartCurrentChromosome
        );
        initialize_partial(&chr2, identity("2")).unwrap();
        assert!(
            chr1.final_osa().exists(),
            "completed chr1 must survive restarting chr2"
        );
        assert!(
            !chr2.partial_osa().exists(),
            "current partial output restarts from zero"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_legacy_cache_without_v2_sidecar_is_not_discarded() {
        let root = root("legacy-cache-contract");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_osa(), b"osa").unwrap();
        fs::write(paths.partial_index(), b"idx").unwrap();
        promote_verified(&paths, expected.clone(), 10, 2).unwrap();
        fs::remove_file(paths.cache_contract()).unwrap();

        assert_eq!(
            restart_decision(&paths, &expected),
            RestartDecision::AlreadyVerified
        );
        assert!(paths.final_osa().is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hybrid_reader_replays_a_saved_prefix_and_appends_the_missing_range() {
        let body: &'static [u8] = b"abcdefghij";
        let root = root("hybrid-prefix-resume");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = range_fixture(body, 1);
        expected.expected_compressed_bytes = body.len() as u64;
        expected.source_etag = Some("fixture".into());
        prepare_source_part(&paths, &expected).unwrap();
        fs::write(paths.source_part(), &body[..5]).unwrap();

        let request = StreamingBuildRequest {
            fastvep_executable: Path::new("unused"),
            source_type: "gnomad",
            paths: &paths,
            identity: &expected,
            log_path: Path::new("unused"),
            dbnsfp_fields: None,
            source_fields: None,
        };
        let (mut reader, network, resumed) = hybrid_http_reader(&request).unwrap();
        let mut replayed = Vec::new();
        reader.read_to_end(&mut replayed).unwrap();
        assert_eq!(resumed, 5);
        assert_eq!(network.load(Ordering::Relaxed), 5);
        assert_eq!(replayed, body);
        assert_eq!(fs::read(paths.source_part()).unwrap(), body);

        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_osa(), b"osa").unwrap();
        fs::write(paths.partial_index(), b"idx").unwrap();
        promote_verified(&paths, expected, body.len() as u64, 1).unwrap();
        assert!(!paths.source_part().exists());
        assert!(!paths.source_part_identity().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hybrid_reader_reconnects_without_replaying_after_a_body_error() {
        let body: &'static [u8] = b"abcdefghij";
        let (url, requests) = interrupted_range_fixture(body, 4);
        let root = root("hybrid-inline-reconnect");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = url;
        expected.expected_compressed_bytes = body.len() as u64;
        expected.source_etag = Some("fixture".into());
        let request = StreamingBuildRequest {
            fastvep_executable: Path::new("unused"),
            source_type: "gnomad",
            paths: &paths,
            identity: &expected,
            log_path: Path::new("unused"),
            dbnsfp_fields: None,
            source_fields: None,
        };
        let (mut reader, network, resumed) = hybrid_http_reader(&request).unwrap();
        let mut received = Vec::new();
        reader.read_to_end(&mut received).unwrap();
        assert_eq!(resumed, 0);
        assert_eq!(network.load(Ordering::Relaxed), body.len() as u64);
        assert_eq!(received, body);
        assert_eq!(fs::read(paths.source_part()).unwrap(), body);
        let first = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(first.to_ascii_lowercase().contains("range: bytes=0-9"));
        assert!(second.to_ascii_lowercase().contains("range: bytes=4-9"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dbsnp_reader_filters_the_indexed_contig_and_rewrites_its_chromosome() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(
                b"##fileformat=VCFv4.2\n\
NC_000001.11\t101\trs1\tA\tG\t.\tPASS\tRS=1\n\
NC_000002.12\t202\trs2\tC\tT\t.\tPASS\tRS=2\n",
            )
            .unwrap();
        let compressed = encoder.finish().unwrap();
        let decoder = flate2::read::MultiGzDecoder::new(Cursor::new(compressed));
        let mut reader = DbsnpReader {
            input: BufReader::new(decoder),
            source_contig: "NC_000001.11".into(),
            chromosome: "1".into(),
        };

        assert_eq!(
            reader.next_record().unwrap().as_deref(),
            Some("1\t101\trs1\tA\tG\t.\tPASS\tRS=1\n")
        );
        assert_eq!(reader.next_record().unwrap(), None);
    }

    #[test]
    fn indexed_bgzf_readers_ignore_only_unterminated_range_tails() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(
                b"NC_000001.11\t101\trs1\tA\tG\t.\tPASS\tRS=1\n\
partial dbSNP row",
            )
            .unwrap();
        let mut dbsnp = DbsnpReader {
            input: BufReader::new(flate2::read::MultiGzDecoder::new(Cursor::new(
                encoder.finish().unwrap(),
            ))),
            source_contig: "NC_000001.11".into(),
            chromosome: "1".into(),
        };
        assert!(dbsnp.next_record().unwrap().is_some());
        assert!(dbsnp.next_record().unwrap().is_none());

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(
                b"1\t100\t.\tA\tG\t.\t.\tSpliceAI=G|GENE1|0.1|0.0|0.0|0.0|1|0|0|0\n\
1\t101\t.\tA",
            )
            .unwrap();
        let mut spliceai = SpliceAiReader {
            input: BufReader::new(flate2::read::MultiGzDecoder::new(Cursor::new(
                encoder.finish().unwrap(),
            ))),
            chromosome: Some("1".into()),
        };
        assert!(spliceai.next_record().unwrap().is_some());
        assert!(spliceai.next_record().unwrap().is_none());

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(b"1\t2\tA\tG\t0.1\t10\n1\t3\tC\tT\t0.2")
            .unwrap();
        let decoder = flate2::read::MultiGzDecoder::new(Cursor::new(encoder.finish().unwrap()));
        let mut cadd = CaddChromosomeReader::new(decoder, 0, "1").unwrap();
        assert!(cadd.next_record().unwrap().is_some());
        assert!(cadd.next_record().unwrap().is_none());
    }

    #[test]
    fn indexed_bgzf_readers_still_reject_malformed_complete_rows() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"partial dbSNP row\n").unwrap();
        let mut dbsnp = DbsnpReader {
            input: BufReader::new(flate2::read::MultiGzDecoder::new(Cursor::new(
                encoder.finish().unwrap(),
            ))),
            source_contig: "NC_000001.11".into(),
            chromosome: "1".into(),
        };
        assert!(
            dbsnp
                .next_record()
                .unwrap_err()
                .contains("no tab-delimited fields")
        );

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"1\t101\t.\tA\n").unwrap();
        let mut spliceai = SpliceAiReader {
            input: BufReader::new(flate2::read::MultiGzDecoder::new(Cursor::new(
                encoder.finish().unwrap(),
            ))),
            chromosome: Some("1".into()),
        };
        assert!(
            spliceai
                .next_record()
                .unwrap_err()
                .contains("fewer than eight columns")
        );

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"1\t3\tC\tT\t0.2\n").unwrap();
        let decoder = flate2::read::MultiGzDecoder::new(Cursor::new(encoder.finish().unwrap()));
        let mut cadd = CaddChromosomeReader::new(decoder, 0, "1").unwrap();
        assert!(
            cadd.next_record()
                .unwrap_err()
                .contains("5 columns instead of 6")
        );
    }

    #[test]
    fn dbnsfp_inventory_selects_exact_variant_members_in_genomic_order() {
        let root = root("dbnsfp-inventory");
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("dbNSFP4.9a.zip");
        let chromosomes = (1..=22)
            .map(|number| number.to_string())
            .chain(["X", "Y", "M"].into_iter().map(str::to_string))
            .collect::<Vec<_>>();
        let reversed = chromosomes
            .iter()
            .rev()
            .map(String::as_str)
            .collect::<Vec<_>>();
        dbnsfp_zip(&archive, &reversed);
        let shards = dbnsfp_archive_shards(&archive).unwrap();
        assert_eq!(
            shards
                .iter()
                .map(|shard| shard.chromosome.as_str())
                .collect::<Vec<_>>(),
            chromosomes.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert!(shards.iter().all(|shard| shard.source_bytes > 0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pinned_wgs_stream_catalog_is_complete_and_uses_native_sources() {
        let gnomad = pinned_sharded_source("gnomad").unwrap();
        assert_eq!(gnomad.source_type, "gnomad");
        assert_eq!(
            gnomad.selected_schema,
            "gnomad-v4.1.1-exomes-af-ac-an-homozygotes-populations"
        );
        assert_eq!(gnomad.shards.len(), 24);
        assert_eq!(
            gnomad
                .shards
                .iter()
                .map(|shard| shard.compressed_bytes)
                .sum::<u64>(),
            199_241_266_182
        );
        assert_eq!(gnomad.shards.first().unwrap().chromosome, "1");
        assert_eq!(gnomad.shards.last().unwrap().chromosome, "Y");

        let genomes = pinned_sharded_source("gnomad-genomes").unwrap();
        assert_eq!(genomes.source_type, "gnomad");
        assert_eq!(
            genomes.selected_schema,
            "gnomad-v4.1.1-genomes-af-ac-an-homozygotes-populations"
        );
        assert_eq!(genomes.shards.len(), 24);
        assert_eq!(
            genomes
                .shards
                .iter()
                .map(|shard| shard.compressed_bytes)
                .sum::<u64>(),
            565_643_483_329
        );

        let phylop = pinned_sharded_source("phylop").unwrap();
        assert_eq!(phylop.source_type, "phylop");
        assert_eq!(phylop.shards.len(), 25);
        assert_eq!(
            phylop
                .shards
                .iter()
                .map(|shard| shard.compressed_bytes)
                .sum::<u64>(),
            5_452_453_066
        );
        assert_eq!(phylop.shards.last().unwrap().chromosome, "M");
        assert!(pinned_sharded_source("spliceai").is_err());

        let revel = pinned_revel_manifest().unwrap();
        assert_eq!(revel.archives.len(), 24);
        assert_eq!(
            revel
                .archives
                .iter()
                .map(|archive| archive.bytes)
                .sum::<u64>(),
            667_188_638
        );
        assert_eq!(revel.archives.first().unwrap().chromosome, "1");
        assert_eq!(revel.archives.last().unwrap().chromosome, "Y");
        assert!(revel.archives.iter().all(|archive| archive.md5.len() == 32));
    }

    #[test]
    fn pinned_dbnsfp_49a_manifest_matches_the_download_catalog() {
        let manifest = pinned_dbnsfp_manifest().unwrap();
        assert_eq!(manifest.release, "4.9a");
        assert_eq!(manifest.archive_bytes, 38_969_753_349);
        assert!(
            manifest
                .archive_md5
                .eq_ignore_ascii_case("be89346ab3dc5c14a8a7b602f50c66fb")
        );
        assert_eq!(manifest.members.len(), 25);
    }

    #[test]
    fn legacy_dbnsfp_preview_shards_are_not_ready_and_are_replaced_on_install() {
        let root = root("dbnsfp-curated-upgrade");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let legacy = dbnsfp_identity("1", "dbnsfp-4.9a");
        initialize_partial(&paths, legacy.clone()).unwrap();
        fs::write(paths.partial_osa(), b"osa").unwrap();
        fs::write(paths.partial_index(), b"idx").unwrap();
        promote_verified(&paths, legacy, 10, 2).unwrap();

        let status = status_with_storage("dbnsfp", &root, &["1".into()]);
        assert_eq!(status.state, "idle");
        assert_eq!(status.completed_chromosomes, 0);

        remove_legacy_dbnsfp_shards(&root, &pinned_dbnsfp_manifest().unwrap()).unwrap();
        assert!(!paths.final_directory.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn curated_dbnsfp_shards_survive_restart_cleanup() {
        let root = root("dbnsfp-curated-restart");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let current = dbnsfp_identity("1", DBNSFP_CURATED_SCHEMA);
        initialize_partial(&paths, current.clone()).unwrap();
        fs::write(paths.partial_osa(), b"osa").unwrap();
        fs::write(paths.partial_index(), b"idx").unwrap();
        promote_verified(&paths, current, 10, 2).unwrap();

        remove_legacy_dbnsfp_shards(&root, &pinned_dbnsfp_manifest().unwrap()).unwrap();
        assert!(paths.final_osa().is_file());
        assert_eq!(
            restart_decision(&paths, &dbnsfp_identity("1", DBNSFP_CURATED_SCHEMA)),
            RestartDecision::AlreadyVerified
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dbnsfp_status_restores_ready_for_the_saved_field_selection() {
        let root = root("dbnsfp-custom-selection-status");
        let selection = default_dbnsfp_field_selection().unwrap();
        save_dbnsfp_field_selection(&root, selection.clone()).unwrap();
        let schema = dbnsfp_schema_identity(&selection);
        assert_ne!(schema, DBNSFP_CURATED_SCHEMA);

        for chromosome in ["1", "2"] {
            let paths = ShardPaths::new(&root, chromosome).unwrap();
            let identity = dbnsfp_identity(chromosome, &schema);
            initialize_partial(&paths, identity.clone()).unwrap();
            fs::write(paths.partial_osa(), b"osa").unwrap();
            fs::write(paths.partial_index(), b"idx").unwrap();
            promote_verified(&paths, identity, 10, 2).unwrap();
        }

        let status = status_with_storage("dbnsfp", &root, &["1".into(), "2".into()]);
        assert_eq!(status.state, "ready");
        assert_eq!(status.completed_chromosomes, 2);
        assert_eq!(status.remaining_chromosomes, 0);
        assert_eq!(status.percent, 100.0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_dbnsfp_cache_discards_consumed_source_archive_files() {
        let root = root("dbnsfp-consumed-archive");
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("dbNSFP4.9a.zip");
        for path in [
            archive.clone(),
            root.join("dbNSFP4.9a.zip.verified.json"),
            root.join("dbNSFP4.9a.zip.partial"),
            root.join("dbNSFP4.9a.zip.new-partial"),
        ] {
            fs::write(path, b"fixture").unwrap();
        }
        remove_consumed_dbnsfp_archive(&archive).unwrap();
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cadd_tabix_offsets_and_chromosome_filter_are_bounded_and_strict() {
        let mut tbi = Vec::new();
        tbi.extend_from_slice(b"TBI\x01");
        tbi.extend_from_slice(&1_i32.to_le_bytes());
        for _ in 0..6 {
            tbi.extend_from_slice(&0_i32.to_le_bytes());
        }
        tbi.extend_from_slice(&2_i32.to_le_bytes());
        tbi.extend_from_slice(b"1\0");
        tbi.extend_from_slice(&1_i32.to_le_bytes());
        tbi.extend_from_slice(&0_u32.to_le_bytes());
        tbi.extend_from_slice(&1_i32.to_le_bytes());
        let virtual_offset = (123_u64 << 16) | 7;
        tbi.extend_from_slice(&virtual_offset.to_le_bytes());
        tbi.extend_from_slice(&((124_u64 << 16) | 2).to_le_bytes());
        tbi.extend_from_slice(&1_i32.to_le_bytes());
        tbi.extend_from_slice(&virtual_offset.to_le_bytes());
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tbi).unwrap();
        let offsets = parse_tabix_reference_offsets(&encoder.finish().unwrap()).unwrap();
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].name, "1");
        assert_eq!(offsets[0].virtual_offset, virtual_offset);

        let rows =
            b"#Chrom\tPos\tRef\tAlt\tRawScore\tPHRED\n1\t2\tA\tG\t0.1\t10\n2\t3\tC\tT\t0.2\t20\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(rows).unwrap();
        let decoder = flate2::read::MultiGzDecoder::new(Cursor::new(encoder.finish().unwrap()));
        let mut reader = CaddChromosomeReader::new(decoder, 0, "1").unwrap();
        assert_eq!(reader.next_record().unwrap().unwrap().position, 2);
        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn cadd_snv_and_indel_merge_order_is_deterministic() {
        let snv = CaddRecord {
            position: 10,
            reference: "A".into(),
            alternate: "G".into(),
            line: String::new(),
        };
        let indel = CaddRecord {
            position: 11,
            reference: "AT".into(),
            alternate: "A".into(),
            line: String::new(),
        };
        assert!(cadd_record_before_or_equal(&snv, &indel));
        assert!(!cadd_record_before_or_equal(&indel, &snv));
    }

    #[test]
    fn tiny_cadd_stream_builds_and_reopens_with_the_pinned_fastvep() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let input = include_bytes!("../../../fixtures/preparation/tiny-cadd.tsv");
        let root = root("tiny-cadd");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let identity = PreparationIdentity {
            resource_id: "cadd".into(),
            release: "1.7".into(),
            assembly: "GRCh38".into(),
            chromosome: "1".into(),
            source_url: "fixture:tiny-cadd".into(),
            expected_compressed_bytes: input.len() as u64,
            source_etag: None,
            source_last_modified: None,
            selected_schema: "cadd-v1.7-grch38".into(),
            fastvep_commit: "7038e7c".into(),
            osa_schema_version: 1,
        };
        initialize_partial(&paths, identity.clone()).unwrap();
        let mut reader = Cursor::new(input);
        let build = stream_reader_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "cadd",
                paths: &paths,
                identity: &identity,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &mut reader,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(build.compressed_bytes_read, input.len() as u64);
        let verification = verify_partial_osa(&fastvep, &paths, &identity).unwrap();
        assert_eq!(verification.record_count, 2);
        promote_verified(
            &paths,
            identity.clone(),
            build.compressed_bytes_read,
            verification.record_count,
        )
        .unwrap();
        fs::remove_file(paths.cache_contract()).unwrap();
        assert_eq!(
            restart_decision_with_legacy_upgrade(&fastvep, &paths, &identity),
            RestartDecision::AlreadyVerified
        );
        let upgraded = crate::cache_contract::read(&paths.cache_contract()).unwrap();
        assert_eq!(upgraded.builder_provenance.commit, "unknown-legacy");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spliceai_reader_keeps_only_the_requested_chromosome() {
        let rows = b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t.\t.\tSpliceAI=G|GENE1|0.1|0.0|0.0|0.0|1|0|0|0\n2\t101\t.\tA\tC\t.\t.\tSpliceAI=C|GENE2|0.0|0.2|0.0|0.0|0|2|0|0\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(rows).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut reader = SpliceAiReader {
            input: BufReader::new(flate2::read::MultiGzDecoder::new(Cursor::new(compressed))),
            chromosome: Some("1".into()),
        };
        let record = reader.next_record().unwrap().unwrap();
        assert!(record.line.starts_with("1\t100\t"));
        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn dbnsfp_inventory_fails_closed_on_incomplete_or_unexpected_members() {
        let root = root("dbnsfp-inventory-invalid");
        fs::create_dir_all(&root).unwrap();
        let incomplete = root.join("incomplete.zip");
        dbnsfp_zip(&incomplete, &["1"]);
        assert!(
            dbnsfp_archive_shards(&incomplete)
                .unwrap_err()
                .contains("missing chromosome 2")
        );
        let unexpected = root.join("unexpected.zip");
        dbnsfp_zip(&unexpected, &["Un"]);
        assert!(
            dbnsfp_archive_shards(&unexpected)
                .unwrap_err()
                .contains("unexpected variant chromosome")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dbnsfp_field_selection_is_validated_ordered_and_persisted() {
        let root = root("dbnsfp-field-selection");
        let default = default_dbnsfp_field_selection().unwrap();
        let contract = dbnsfp_contract().unwrap();
        let (_, required) = dbnsfp_contract_fields(&contract).unwrap();
        let recommended = contract["recommendedFields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert!(default.fields.len() > required.len());
        assert_eq!(default.fields.len(), required.len() + recommended.len());
        assert!(
            recommended
                .iter()
                .all(|field| default.fields.contains(field))
        );
        assert!(default.fields.contains(&"CADD_phred".into()));
        assert!(!default.fields.contains(&"CADD_raw".into()));
        assert!(!default.fields.contains(&"REVEL_rankscore".into()));
        let optional = default
            .fields
            .iter()
            .find(|field| !required.contains(field))
            .unwrap()
            .clone();
        let mut reversed = required.clone();
        reversed.reverse();
        reversed.push(optional.clone());
        let saved = save_dbnsfp_field_selection(
            &root,
            DbnsfpFieldSelection {
                schema_version: DBNSFP_FIELD_SELECTION_SCHEMA_VERSION,
                contract_id: DBNSFP_CURATED_SCHEMA.into(),
                fields: reversed,
            },
        )
        .unwrap();
        assert_eq!(load_dbnsfp_field_selection(&root).unwrap(), saved);
        assert_eq!(saved.fields.last(), Some(&optional));
        assert_ne!(
            dbnsfp_schema_identity(&default),
            dbnsfp_schema_identity(&saved)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_dbnsfp_shards_lock_the_field_selection() {
        let root = root("dbnsfp-field-selection-locked");
        fs::create_dir_all(root.join("shards").join("chr1")).unwrap();
        let inferred = load_dbnsfp_field_selection(&root).unwrap();
        assert_eq!(inferred, full_dbnsfp_field_selection().unwrap());
        assert_eq!(dbnsfp_schema_identity(&inferred), DBNSFP_CURATED_SCHEMA);
        let error = save_dbnsfp_field_selection(&root, default_dbnsfp_field_selection().unwrap())
            .unwrap_err();
        assert!(error.contains("remove the installed dbNSFP cache"));
        let configuration: serde_json::Value =
            serde_json::from_str(&dbnsfp_field_configuration_json(&root).unwrap()).unwrap();
        assert_eq!(configuration["locked"], true);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supplementary_field_contracts_preserve_defaults_and_fingerprint_custom_sets() {
        for resource_id in [
            "clinvar",
            "dbsnp",
            "gnomad",
            "gnomad-genomes",
            "phylop",
            "cadd",
            "spliceai",
            "revel",
        ] {
            let selection = default_supplementary_field_selection(resource_id).unwrap();
            assert!(!selection.fields.is_empty());
            assert_eq!(
                supplementary_schema_identity("base-schema", resource_id, &selection).unwrap(),
                "base-schema"
            );
        }

        let root = root("cadd-field-selection");
        let default = default_supplementary_field_selection("cadd").unwrap();
        assert_eq!(default.fields, ["raw", "phred"]);
        let custom = save_supplementary_field_selection(
            "cadd",
            &root,
            SupplementaryFieldSelection {
                schema_version: 1,
                contract_id: default.contract_id.clone(),
                fields: vec!["phred".into()],
            },
        )
        .unwrap();
        assert_eq!(
            load_supplementary_field_selection("cadd", &root).unwrap(),
            custom
        );
        assert_ne!(
            supplementary_schema_identity("cadd-v1.7", "cadd", &custom).unwrap(),
            "cadd-v1.7"
        );
        let incomplete = root.join("1.7").join("staging").join("chr1.partial");
        fs::create_dir_all(&incomplete).unwrap();
        fs::write(incomplete.join("checkpoint.json"), b"incomplete").unwrap();
        let default = save_supplementary_field_selection("cadd", &root, default).unwrap();
        assert!(!root.join("1.7").join("staging").exists());
        fs::create_dir_all(root.join("1.7").join("shards").join("chr1")).unwrap();
        assert_eq!(
            save_supplementary_field_selection("cadd", &root, default.clone()).unwrap(),
            default
        );
        assert!(
            save_supplementary_field_selection("cadd", &root, custom)
                .unwrap_err()
                .contains("remove the installed cadd cache")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "prints the pinned manifest from an already verified local dbNSFP 4.9a ZIP"]
    fn print_local_dbnsfp_member_manifest() {
        let archive = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("debug")
            .join("downloads")
            .join("dbNSFP4.9a.zip");
        let shards = dbnsfp_archive_shards(&archive).unwrap();
        assert_eq!(pinned_dbnsfp_manifest().unwrap().members, shards);
        println!("{}", serde_json::to_string_pretty(&shards).unwrap());
    }

    #[test]
    fn stale_identity_and_incomplete_validation_fail_closed() {
        let root = root("stale");
        let paths = ShardPaths::new(&root, "chrX").unwrap();
        initialize_partial(&paths, identity("X")).unwrap();
        let mut changed = identity("X");
        changed.source_etag = Some("changed".into());
        assert_eq!(
            restart_decision(&paths, &changed),
            RestartDecision::StaleIdentity
        );
        let mut changed_schema = identity("X");
        changed_schema.selected_schema = "gnomad-v-next".into();
        assert_eq!(
            restart_decision(&paths, &changed_schema),
            RestartDecision::StaleIdentity
        );
        fs::write(paths.partial_osa(), b"osa").unwrap();
        fs::write(paths.partial_index(), b"idx").unwrap();
        assert!(
            promote_verified(&paths, identity("X"), 9, 1)
                .unwrap_err()
                .contains("length mismatch")
        );
        assert!(paths.partial_directory.exists());
        assert!(!paths.final_directory.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_copy_observes_cancellation_without_persisting_source_data() {
        let cancelled = AtomicBool::new(false);
        let input = vec![7_u8; 2 * 1024 * 1024 + 3];
        let mut output = Vec::new();
        let mut reported = Vec::new();
        assert_eq!(
            copy_bounded_with_progress(&mut input.as_slice(), &mut output, &cancelled, |bytes| {
                reported.push(bytes)
            })
            .unwrap(),
            input.len() as u64
        );
        assert_eq!(output, input);
        assert_eq!(reported.last(), Some(&(input.len() as u64)));

        cancelled.store(true, Ordering::SeqCst);
        let mut output = Vec::new();
        assert_eq!(
            copy_bounded_with_progress(&mut input.as_slice(), &mut output, &cancelled, |_| {})
                .unwrap_err(),
            "cancelled"
        );
        assert!(output.is_empty());
    }

    #[test]
    fn disk_plan_uses_observed_prepared_growth_and_no_source_staging_copy() {
        let unknown = preparation_disk_plan(0, 1_000, 0);
        assert_eq!(unknown.source_disk_bytes, 0);
        assert_eq!(unknown.writer_buffer_bytes, 1024 * 1024);
        assert_eq!(unknown.projected_prepared_bytes, None);
        assert_eq!(unknown.required_free_bytes, None);

        let measured = preparation_disk_plan(100, 1_000, 250);
        assert_eq!(measured.projected_prepared_bytes, Some(2_500));
        assert_eq!(measured.remaining_prepared_bytes, Some(2_250));
        assert_eq!(
            measured.required_free_bytes,
            Some(
                2_250
                    + measured.writer_buffer_bytes
                    + measured.metadata_allowance_bytes
                    + measured.safety_reserve_bytes
            )
        );
    }

    #[test]
    fn pinned_http_identity_rejects_changed_size_and_etag() {
        let changed_size = fixture_response("Content-Length: 4\r\nETag: etag\r\n", b"data");
        let mut expected = identity("1");
        expected.expected_compressed_bytes = 5;
        assert!(
            validate_source_response(&changed_size, &expected)
                .unwrap_err()
                .contains("Content-Length")
        );

        let changed_etag = fixture_response("Content-Length: 4\r\nETag: changed\r\n", b"data");
        expected.expected_compressed_bytes = 4;
        assert_eq!(
            validate_source_response(&changed_etag, &expected).unwrap_err(),
            "source ETag changed"
        );

        let rolling = fixture_response("Content-Length: 4\r\n", b"data");
        expected.expected_compressed_bytes = 5;
        expected.source_etag = Some(format!("md5:{:x}", md5::compute(b"data")));
        validate_source_response(&rolling, &expected).unwrap();
        assert_eq!(
            identity_md5(&expected),
            Some("8d777f385d3dfec8815d20f7496026dc")
        );
    }

    #[test]
    fn truncated_http_body_fails_before_it_can_be_promoted() {
        let cancelled = AtomicBool::new(false);
        let mut response = fixture_response("Content-Length: 10\r\nETag: etag\r\n", b"short");
        let mut expected = identity("1");
        expected.expected_compressed_bytes = 10;
        validate_source_response(&response, &expected).unwrap();
        let mut output = Vec::new();
        let error =
            copy_bounded_with_progress(&mut response, &mut output, &cancelled, |_| {}).unwrap_err();
        assert!(error.contains("chromosome stream failed"));
        assert!(output.len() < expected.expected_compressed_bytes as usize);
    }

    #[test]
    fn instrumentation_proves_the_source_stream_has_one_bounded_buffer() {
        let cancelled = AtomicBool::new(false);
        let mut input = InstrumentedReader {
            remaining: 3 * STREAM_WRITER_BUFFER_BYTES as usize + 17,
            largest_requested_buffer: 0,
        };
        let mut output = io::sink();
        let copied =
            copy_bounded_with_progress(&mut input, &mut output, &cancelled, |_| {}).unwrap();
        assert_eq!(copied, 3 * STREAM_WRITER_BUFFER_BYTES + 17);
        assert_eq!(
            input.largest_requested_buffer,
            STREAM_WRITER_BUFFER_BYTES as usize
        );
    }

    #[test]
    fn full_or_locked_destination_fails_without_unbounded_read_ahead() {
        let cancelled = AtomicBool::new(false);
        let mut input = InstrumentedReader {
            remaining: 4 * STREAM_WRITER_BUFFER_BYTES as usize,
            largest_requested_buffer: 0,
        };
        let mut output = FailingWriter {
            accepted: STREAM_WRITER_BUFFER_BYTES as usize + 5,
            limit: STREAM_WRITER_BUFFER_BYTES as usize + 5,
        };
        let error =
            copy_bounded_with_progress(&mut input, &mut output, &cancelled, |_| {}).unwrap_err();
        assert!(error.contains("fastVEP input failed"));
        assert!(input.remaining >= 3 * STREAM_WRITER_BUFFER_BYTES as usize);
    }

    #[test]
    fn tiny_local_http_fixture_streams_through_the_pinned_clinvar_parser() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            // Source-only builds do not bundle the separately licensed tool.
            return;
        }
        let body: &'static [u8] = include_bytes!("../../../fixtures/preparation/tiny-custom.vcf");
        let root = root("local-http");
        let paths = ShardPaths::new(&root, "22").unwrap();
        let mut expected = identity("22");
        expected.source_url = fixture_url(
            &format!("Content-Length: {}\r\nETag: etag\r\n", body.len()),
            body,
        );
        expected.expected_compressed_bytes = body.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        let result = stream_http_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "clinvar",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.compressed_bytes_read, body.len() as u64);
        let report = verify_partial_osa(&fastvep, &paths, &expected).unwrap();
        assert_eq!(report.record_count, 2);
        promote_verified(
            &paths,
            expected,
            result.compressed_bytes_read,
            report.record_count,
        )
        .unwrap();
        assert!(paths.final_osa().is_file());
        assert!(!root.join("staging").join("chr22.partial").exists());
        assert!(
            !root
                .join("shards")
                .join("chr22")
                .join("source.vcf")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pinned_custom_vcf_parser_accepts_gzip_stdin_and_rejects_corruption() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let plain = include_bytes!("../../../fixtures/preparation/tiny-custom.vcf");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(plain).unwrap();
        let gzip: &'static [u8] = Box::leak(encoder.finish().unwrap().into_boxed_slice());
        let root = root("custom-gzip");
        let paths = ShardPaths::new(&root, "22").unwrap();
        let mut expected = identity("22");
        expected.source_url = fixture_url(
            &format!("Content-Length: {}\r\nETag: etag\r\n", gzip.len()),
            gzip,
        );
        expected.expected_compressed_bytes = gzip.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        let result = stream_http_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "custom_vcf",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.compressed_bytes_read, gzip.len() as u64);
        assert_eq!(
            verify_partial_osa(&fastvep, &paths, &expected)
                .unwrap()
                .record_count,
            2
        );
        fs::remove_dir_all(&root).unwrap();

        let corrupt: &'static [u8] = b"\x1f\x8bnot-a-valid-gzip-stream";
        let paths = ShardPaths::new(&root, "22").unwrap();
        expected.source_url = fixture_url(
            &format!("Content-Length: {}\r\nETag: etag\r\n", corrupt.len()),
            corrupt,
        );
        expected.expected_compressed_bytes = corrupt.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        let error = stream_http_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "custom_vcf",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("fastVEP preparation failed"));
        assert!(!paths.partial_osa().exists());
        assert!(!paths.partial_index().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pinned_zip_member_uses_one_range_and_validates_crc_inline() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let plain = include_bytes!("../../../fixtures/preparation/tiny-custom.vcf");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(plain).unwrap();
        let gzip = encoder.finish().unwrap();
        let offset = 17_usize;
        let mut archive = vec![0_u8; offset];
        archive.extend_from_slice(&gzip);
        archive.extend_from_slice(b"ignored trailing bytes");
        let archive: &'static [u8] = Box::leak(archive.into_boxed_slice());
        let member = DbnsfpArchiveShard {
            chromosome: "22".into(),
            member_name: "fixture.gz".into(),
            source_bytes: gzip.len() as u64,
            compressed_bytes: gzip.len() as u64,
            data_offset: offset as u64,
            compression_method: 0,
            crc32: crc32fast::hash(&gzip),
        };
        let root_path = root("pinned-range-member");
        let paths = ShardPaths::new(&root_path, "22").unwrap();
        let mut expected = identity("22");
        expected.expected_compressed_bytes = gzip.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        let result = stream_pinned_dbnsfp_member(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "custom_vcf",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &range_fixture(archive, 1),
            archive.len() as u64,
            &member,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.compressed_bytes_read, gzip.len() as u64);
        assert_eq!(
            verify_partial_osa(&fastvep, &paths, &expected)
                .unwrap()
                .record_count,
            2
        );

        let bad_root = root("pinned-range-member-bad-crc");
        let bad_paths = ShardPaths::new(&bad_root, "22").unwrap();
        initialize_partial(&bad_paths, expected.clone()).unwrap();
        let mut bad_member = member.clone();
        bad_member.crc32 ^= 1;
        let error = stream_pinned_dbnsfp_member(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "custom_vcf",
                paths: &bad_paths,
                identity: &expected,
                log_path: &bad_paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &range_fixture(archive, 1),
            archive.len() as u64,
            &bad_member,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("CRC mismatch"), "{error}");
        assert!(!bad_paths.partial_osa().exists());
        assert!(!bad_paths.partial_index().exists());

        fs::remove_dir_all(root_path).unwrap();
        fs::remove_dir_all(bad_root).unwrap();
    }

    #[test]
    fn malformed_and_out_of_order_streams_never_reach_promotion() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let cases = [
            (
                "out-of-order",
                b"##fileformat=VCFv4.2\n##INFO=<ID=AF,Number=A,Type=Float,Description=\"af\">\n##INFO=<ID=AN,Number=1,Type=Integer,Description=\"an\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t500\t.\tA\tG\t.\tPASS\tAF=0.01;AN=1000\n1\t100\t.\tC\tT\t.\tPASS\tAF=0.02;AN=1000\n".as_slice(),
            ),
            (
                "malformed",
                b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\tnot-a-position\t.\tA\tG\t.\tPASS\t.\n".as_slice(),
            ),
        ];
        for (label, bytes) in cases {
            let body: &'static [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
            let root = root(label);
            let paths = ShardPaths::new(&root, "1").unwrap();
            let mut expected = identity("1");
            expected.source_url = fixture_url(
                &format!("Content-Length: {}\r\nETag: etag\r\n", body.len()),
                body,
            );
            expected.expected_compressed_bytes = body.len() as u64;
            initialize_partial(&paths, expected.clone()).unwrap();
            let build = stream_http_to_partial_osa_with_progress(
                &StreamingBuildRequest {
                    fastvep_executable: &fastvep,
                    source_type: "gnomad",
                    paths: &paths,
                    identity: &expected,
                    log_path: &paths.partial_directory.join("fastvep.log"),
                    dbnsfp_fields: None,
                    source_fields: None,
                },
                &AtomicBool::new(false),
                |_| {},
            );
            match build {
                Err(_) => {}
                Ok(_) => assert!(
                    verify_partial_osa(&fastvep, &paths, &expected).is_err(),
                    "{label} input unexpectedly passed both build and verification"
                ),
            }
            assert!(!paths.final_directory.exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn hybrid_chromosome_stream_requests_an_exact_resumable_range() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let body: &'static [u8] = include_bytes!("../../../fixtures/preparation/tiny-custom.vcf");
        let (url, request_headers) = fixture_url_recording_request(body);
        let root = root("no-range");
        let paths = ShardPaths::new(&root, "22").unwrap();
        let mut expected = identity("22");
        expected.source_url = url;
        expected.expected_compressed_bytes = body.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        stream_http_via_resumable_part(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "clinvar",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let request = request_headers
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("\r\nrange: bytes=0-{}", body.len() - 1))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_kills_fastvep_and_removes_partial_outputs() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let mut vcf = String::from(
            "##fileformat=VCFv4.2\n##INFO=<ID=AF,Number=A,Type=Float,Description=\"af\">\n##INFO=<ID=AN,Number=1,Type=Integer,Description=\"an\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n",
        );
        for position in 1..=60_000 {
            use std::fmt::Write as _;
            writeln!(vcf, "1\t{position}\t.\tA\tG\t.\tPASS\tAF=0.01;AN=1000").unwrap();
        }
        let body: &'static [u8] = Box::leak(vcf.into_bytes().into_boxed_slice());
        let root = root("cancel-child");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = fixture_url(
            &format!("Content-Length: {}\r\nETag: etag\r\n", body.len()),
            body,
        );
        expected.expected_compressed_bytes = body.len() as u64;
        initialize_partial(&paths, expected.clone()).unwrap();
        let cancelled = AtomicBool::new(false);
        let error = stream_http_to_partial_osa_with_progress(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "gnomad",
                paths: &paths,
                identity: &expected,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &cancelled,
            |_| cancelled.store(true, Ordering::SeqCst),
        )
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert!(!paths.partial_osa().exists());
        assert!(!paths.partial_index().exists());
        assert!(!paths.final_directory.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses the 243 KB official REVEL chromosome Y archive"]
    fn official_revel_chromosome_y_zip_streams_and_verifies() {
        let fastvep = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fastvep")
            .join("fastvep.exe");
        if !fastvep.is_file() {
            return;
        }
        let archive = pinned_revel_manifest()
            .unwrap()
            .archives
            .into_iter()
            .find(|archive| archive.chromosome == "Y")
            .unwrap();
        let root = root("official-revel-y");
        let paths = ShardPaths::new(&root, "Y").unwrap();
        let identity = PreparationIdentity {
            resource_id: "revel".into(),
            release: "1.3".into(),
            assembly: "GRCh38".into(),
            chromosome: "Y".into(),
            source_url: format!(
                "https://zenodo.org/api/records/7072866/files/{}/content",
                archive.filename
            ),
            expected_compressed_bytes: archive.bytes,
            source_etag: Some(format!("md5:{}", archive.md5)),
            source_last_modified: None,
            selected_schema: "revel-v1.3-transcript-matched".into(),
            fastvep_commit: "7038e7c".into(),
            osa_schema_version: 1,
        };
        initialize_partial(&paths, identity.clone()).unwrap();
        let build = stream_revel_archive_to_partial_osa(
            &StreamingBuildRequest {
                fastvep_executable: &fastvep,
                source_type: "revel",
                paths: &paths,
                identity: &identity,
                log_path: &paths.partial_directory.join("fastvep.log"),
                dbnsfp_fields: None,
                source_fields: None,
            },
            &archive,
            0,
            archive.bytes,
            0,
            0,
        )
        .unwrap();
        assert_eq!(build.compressed_bytes_read, archive.bytes);
        assert_eq!(
            verify_partial_osa(&fastvep, &paths, &identity)
                .unwrap()
                .record_count,
            31_551
        );
        fs::remove_dir_all(root).unwrap();
    }
}
