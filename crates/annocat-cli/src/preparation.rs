use serde::Serialize;
use std::fs;
#[cfg(test)]
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod builder;
mod cache;
mod catalog;
mod checkpoint;
mod fields;
mod indexed_catalog;
mod progress;
mod range_plan;
mod resumable;
mod state;
mod tabix;
mod transport;

pub(crate) use cache::effective_cache_format as cache_format_for_install;
use cache::{
    VerifiedCacheCompatibility, effective_cache_format, required_nonempty_file, restart_decision,
    verified_cache_compatibility, verify_partial_osa,
};
pub use cache::{initialize_partial, promote_verified};
pub(crate) use cache::{verified_cache_files, verify_source_cache};
use catalog::canonical_chromosomes;
pub use catalog::{
    DbnsfpArchiveShard, DbnsfpPinnedManifest, PinnedShardedSource, RevelArchive,
    RevelArchiveManifest, pinned_dbnsfp_manifest, pinned_revel_manifest, pinned_sharded_source,
};
use checkpoint::{CHECKPOINT_SCHEMA_VERSION, read_checkpoint, write_checkpoint};
pub use checkpoint::{
    CacheFormat, CheckpointState, PreparationCheckpoint, PreparationIdentity, RestartDecision,
    ShardPaths,
};
use fields::dbnsfp_schema_identity;
#[cfg(test)]
use fields::{
    DBNSFP_CURATED_SCHEMA, DBNSFP_FIELD_SELECTION_SCHEMA_VERSION, DBNSFP_LEGACY_CURATED_SCHEMA,
    dbnsfp_contract, dbnsfp_contract_fields, full_dbnsfp_field_selection,
    supplementary_field_contract,
};
pub use fields::{
    DbnsfpFieldSelection, SupplementaryFieldSelection, dbnsfp_field_configuration,
    default_dbnsfp_field_selection, default_supplementary_field_selection,
    load_dbnsfp_field_selection, load_supplementary_field_selection, save_dbnsfp_field_selection,
    save_supplementary_field_selection, supplementary_field_configuration,
    supplementary_schema_identity,
};
use indexed_catalog::{CaddArtifact, SpliceAiArtifact};
use progress::{
    update_indexed_progress, update_local_build_detail, update_local_build_progress_detail,
    update_resumable_download_detail, update_resumable_progress_detail, update_revel_progress,
    update_sharded_progress,
};
use range_plan::IndexedByteRange;
pub use state::{
    LivePreparationState, cancel_live, forget_live, live_status, record_start_failure,
    running_count,
};
use state::{live_cancel, live_state, register_live_job, spawn_live_job};
use tabix::{TabixReferenceOffset, parse_reference_offsets as parse_tabix_reference_offsets};

pub const LEGACY_PREPARATION_IDENTITY_COMMIT: &str = "7038e7c17708e7d2226149e78e0bb297bcc6d1d6";
pub const STREAM_WRITER_BUFFER_BYTES: u64 = 1024 * 1024;
const PREPARATION_METADATA_ALLOWANCE_BYTES: u64 = 4 * 1024 * 1024;
const PREPARATION_SAFETY_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
const FASTVEP_PARSER_WORKER_BUDGET: usize = 4;

fn fastvep_parser_workers_for_concurrency(concurrency: usize) -> usize {
    (FASTVEP_PARSER_WORKER_BUDGET / concurrency.clamp(1, 4)).max(1)
}

fn configure_fastvep_parser_workers(command: &mut Command) {
    command.env(
        "FASTVEP_SA_PARSE_THREADS",
        fastvep_parser_workers_for_concurrency(crate::install_queue::concurrency()).to_string(),
    );
}

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

static RESUMABLE_SOURCE_PARTS: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceInputMode {
    Resumable,
    PureStreaming,
}

pub fn set_source_input_mode(value: &str) -> Result<SourceInputMode, String> {
    let mode = match value {
        "resumable" | "hybrid-resumable" => SourceInputMode::Resumable,
        "pure-streaming" | "" => SourceInputMode::PureStreaming,
        _ => return Err("source input mode must be resumable or pure-streaming".into()),
    };
    RESUMABLE_SOURCE_PARTS.store(mode == SourceInputMode::Resumable, Ordering::SeqCst);
    Ok(mode)
}

pub fn source_input_mode() -> SourceInputMode {
    if RESUMABLE_SOURCE_PARTS.load(Ordering::SeqCst) {
        SourceInputMode::Resumable
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
            prepared_sha256: None,
            prepared_index_sha256: None,
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

/// Feed one remote chromosome into fastVEP. Resumable mode completes a durable
/// source part first; pure mode forwards the response directly.
pub fn stream_http_to_partial_osa_with_progress<F>(
    request: &StreamingBuildRequest<'_>,
    cancelled: &AtomicBool,
    progress: F,
) -> Result<StreamingBuildResult, String>
where
    F: FnMut(StreamingProgress),
{
    if source_input_mode() == SourceInputMode::Resumable {
        return stream_http_via_resumable_part(request, cancelled, progress);
    }
    if !request.paths.partial_directory.is_dir() {
        return Err("partial shard directory has not been initialized".into());
    }
    let client = crate::http_client::source()?;
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
    let part = resumable::acquire_range(
        request.paths,
        request.identity,
        &request.identity.source_url,
        0,
        request.identity.expected_compressed_bytes,
        request.identity.source_etag.as_deref(),
        request.identity.source_last_modified.as_deref(),
        cancelled,
        |state| {
            progress(StreamingProgress {
                compressed_bytes_read: state.persisted_bytes,
                consumed_bytes: 0,
                retained_bytes: state.resumed_bytes,
                expected_compressed_bytes: state.expected_bytes,
                elapsed: state.elapsed,
                bytes_per_second: state.bytes_per_second,
            })
        },
    )?;
    let resumed = part.resumed_bytes;
    let downloaded = part.downloaded_bytes;
    let mut reader = part.reader;
    let build_started = Instant::now();
    let report = |progress: &mut F, consumed: u64| {
        let elapsed = build_started.elapsed();
        progress(StreamingProgress {
            compressed_bytes_read: request.identity.expected_compressed_bytes,
            consumed_bytes: consumed,
            retained_bytes: request.identity.expected_compressed_bytes,
            expected_compressed_bytes: request.identity.expected_compressed_bytes,
            elapsed,
            bytes_per_second: if elapsed.is_zero() {
                0.0
            } else {
                consumed as f64 / elapsed.as_secs_f64()
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
    Ok(StreamingBuildResult {
        compressed_bytes_read: resumed.saturating_add(downloaded),
        ..result
    })
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
        .arg("--format")
        .arg(request.identity.cache_format()?.builder_argument())
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    configure_fastvep_parser_workers(&mut command);
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
            return Err(builder::recover_build_failure(
                request.log_path,
                error,
                &[request.paths.source_part().to_path_buf()],
            ));
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
        return Err(builder::recover_build_failure(
            request.log_path,
            format!("fastVEP preparation failed with status {status}"),
            &[request.paths.source_part().to_path_buf()],
        ));
    }

    let format = request.identity.cache_format()?;
    Ok(StreamingBuildResult {
        compressed_bytes_read,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_data(format))?,
        prepared_index_bytes: request
            .paths
            .partial_index(format)
            .map_or(Ok(0), |index| required_nonempty_file(&index))?,
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
    let url = &request.identity.source_url;
    let resumable = source_input_mode() == SourceInputMode::Resumable;
    let source: Box<dyn Read> = if resumable {
        let part = resumable::acquire_range(
            request.paths,
            request.identity,
            url,
            0,
            archive.bytes,
            None,
            None,
            live_cancel().as_ref(),
            |state| {
                update_revel_progress(
                    &archive.chromosome,
                    completed,
                    base_network.saturating_add(state.persisted_bytes),
                    total_network,
                    prepared_bytes,
                    state.bytes_per_second,
                );
                update_resumable_download_detail(
                    "REVEL",
                    &archive.chromosome,
                    state.persisted_bytes,
                    state.expected_bytes,
                );
            },
        )?;
        Box::new(part.reader)
    } else {
        let response = crate::http_client::source()?
            .get(url)
            .header(
                reqwest::header::USER_AGENT,
                "AnnoCAT/0.1 (local variant annotation)",
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
        Box::new(response)
    };
    if resumable {
        update_local_build_detail("REVEL", &archive.chromosome);
    }
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create REVEL preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut command = Command::new(request.fastvep_executable);
    command
        .arg("sa-build")
        .arg("--source")
        .arg("revel")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(&output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--format")
        .arg(request.identity.cache_format()?.builder_argument())
        .arg("--no-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    configure_fastvep_parser_workers(&mut command);
    let mut child = command
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
                    let mut buffer = vec![0_u8; 1024 * 1024];
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
                        let downloaded = if resumable { archive.bytes } else { consumed };
                        if resumable {
                            update_local_build_progress_detail(
                                "REVEL",
                                &archive.chromosome,
                                consumed,
                                archive.bytes,
                                if elapsed == 0.0 {
                                    0.0
                                } else {
                                    consumed as f64 / elapsed
                                },
                            );
                        } else {
                            update_revel_progress(
                                &archive.chromosome,
                                completed,
                                base_network.saturating_add(downloaded),
                                total_network,
                                prepared_bytes,
                                if elapsed == 0.0 {
                                    0.0
                                } else {
                                    downloaded as f64 / elapsed
                                },
                            );
                        }
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
            return Err(builder::recover_build_failure(
                request.log_path,
                error,
                &[request.paths.source_part().to_path_buf()],
            ));
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP REVEL preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(builder::recover_build_failure(
            request.log_path,
            format!("fastVEP REVEL preparation failed with status {status}"),
            &[request.paths.source_part().to_path_buf()],
        ));
    }
    let format = request.identity.cache_format()?;
    Ok(StreamingBuildResult {
        compressed_bytes_read: received,
        prepared_osa_bytes: required_nonempty_file(&request.paths.partial_data(format))?,
        prepared_index_bytes: request
            .paths
            .partial_index(format)
            .map_or(Ok(0), |index| required_nonempty_file(&index))?,
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
    if source_input_mode() == SourceInputMode::Resumable {
        let part = resumable::acquire_range(
            request.paths,
            request.identity,
            archive_url,
            member.data_offset,
            archive_bytes,
            None,
            None,
            cancelled,
            |state| {
                progress(StreamingProgress {
                    compressed_bytes_read: state.persisted_bytes,
                    consumed_bytes: 0,
                    retained_bytes: state.resumed_bytes,
                    expected_compressed_bytes: state.expected_bytes,
                    elapsed: state.elapsed,
                    bytes_per_second: state.bytes_per_second,
                })
            },
        )?;
        let reader = part.reader;
        let mut checked = Crc32Reader {
            inner: reader,
            hasher: crc32fast::Hasher::new(),
        };
        let result = stream_reader_to_partial_osa_with_progress(
            request,
            &mut checked,
            cancelled,
            |state| {
                progress(StreamingProgress {
                    compressed_bytes_read: member.compressed_bytes,
                    consumed_bytes: state.consumed_bytes,
                    retained_bytes: member.compressed_bytes,
                    expected_compressed_bytes: member.compressed_bytes,
                    elapsed: state.elapsed,
                    bytes_per_second: state.bytes_per_second,
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
    let client = crate::http_client::source()?;
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
    let mut buffer = vec![0_u8; STREAM_WRITER_BUFFER_BYTES as usize];
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
    for format in [CacheFormat::OsaV1, CacheFormat::OsaV2] {
        let _ = fs::remove_file(paths.partial_data(format));
        if let Some(index) = paths.partial_index(format) {
            let _ = fs::remove_file(index);
        }
    }
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
    ranges: Vec<IndexedByteRange>,
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

#[derive(Debug, Clone)]
struct CaddArtifactPlan {
    artifact: CaddArtifact,
    ranges: Vec<IndexedByteRange>,
}

#[derive(Debug, Clone)]
struct SpliceAiArtifactPlan {
    artifact: SpliceAiArtifact,
    ranges: Vec<IndexedByteRange>,
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

#[allow(clippy::too_many_arguments)]
fn fetch_index(
    client: &reqwest::blocking::Client,
    label: &str,
    url: &str,
    bytes: u64,
    md5: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    strict_headers: bool,
) -> Result<Vec<TabixReferenceOffset>, String> {
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| format!("{label} request failed: {error}"))?;
    if strict_headers {
        validate_pinned_headers(
            &response,
            bytes,
            etag.ok_or_else(|| format!("{label} has no pinned ETag"))?,
            last_modified.ok_or_else(|| format!("{label} has no pinned Last-Modified"))?,
            label,
        )?;
    } else {
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|actual| actual != bytes)
        {
            return Err(format!("{label} returned unexpected HTTP metadata"));
        }
        validate_optional_header(
            response.headers(),
            reqwest::header::LAST_MODIFIED,
            last_modified,
            "Last-Modified",
        )?;
    }
    let mut compressed = Vec::with_capacity(bytes as usize);
    (&mut response)
        .take(bytes.saturating_add(1))
        .read_to_end(&mut compressed)
        .map_err(|error| format!("cannot read {label}: {error}"))?;
    if compressed.len() as u64 != bytes || format!("{:x}", md5::compute(&compressed)) != md5 {
        return Err(format!("{label} checksum mismatch"));
    }
    parse_tabix_reference_offsets(&compressed).map_err(|error| format!("invalid {label}: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn bgzf_block_size(
    client: &reqwest::blocking::Client,
    label: &str,
    url: &str,
    data_bytes: u64,
    etag: Option<&str>,
    last_modified: Option<&str>,
    strict_headers: bool,
    offset: u64,
) -> Result<u64, String> {
    let end = offset.checked_add(17).ok_or("BGZF probe offset overflow")?;
    let mut response = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .map_err(|error| format!("{label} BGZF probe failed: {error}"))?;
    let expected_range = format!("bytes {offset}-{end}/{data_bytes}");
    let actual_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || response.content_length() != Some(18)
        || actual_range != Some(expected_range.as_str())
    {
        return Err(format!(
            "{label} BGZF probe returned unexpected range metadata"
        ));
    }
    if strict_headers {
        validate_pinned_headers(
            &response,
            18,
            etag.ok_or_else(|| format!("{label} has no pinned ETag"))?,
            last_modified.ok_or_else(|| format!("{label} has no pinned Last-Modified"))?,
            label,
        )?;
    } else {
        validate_optional_header(
            response.headers(),
            reqwest::header::LAST_MODIFIED,
            last_modified,
            "Last-Modified",
        )?;
    }
    let mut header = [0_u8; 18];
    response
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read {label} BGZF probe: {error}"))?;
    if header[0..4] != [0x1f, 0x8b, 0x08, 0x04] || &header[12..14] != b"BC" {
        return Err(format!("{label} range is not BGZF"));
    }
    Ok(u16::from_le_bytes([header[16], header[17]]) as u64 + 1)
}

#[derive(Clone, Copy)]
struct IndexedSource<'a> {
    label: &'a str,
    data_url: &'a str,
    data_bytes: u64,
    data_etag: Option<&'a str>,
    data_last_modified: Option<&'a str>,
    index_url: &'a str,
    index_bytes: u64,
    index_md5: &'a str,
    index_etag: Option<&'a str>,
    index_last_modified: Option<&'a str>,
    strict_headers: bool,
}

fn indexed_ranges(
    client: &reqwest::blocking::Client,
    source: IndexedSource<'_>,
    contigs: Vec<(String, String)>,
    require_following: bool,
) -> Result<Vec<IndexedByteRange>, String> {
    let indexed = fetch_index(
        client,
        &format!("{} tabix index", source.label),
        source.index_url,
        source.index_bytes,
        source.index_md5,
        source.index_etag,
        source.index_last_modified,
        source.strict_headers,
    )?;
    contigs
        .into_iter()
        .map(|(chromosome, source_contig)| {
            let index = indexed
                .iter()
                .position(|item| {
                    item.name.strip_prefix("chr").unwrap_or(&item.name) == source_contig
                })
                .ok_or_else(|| format!("{} index is missing {source_contig}", source.label))?;
            let current = indexed[index].virtual_offset;
            let start = current >> 16;
            let uncompressed_skip = current as u16;
            let end = match indexed.get(index + 1) {
                Some(next) => {
                    let next_block = next.virtual_offset >> 16;
                    next_block
                        .checked_add(bgzf_block_size(
                            client,
                            source.label,
                            source.data_url,
                            source.data_bytes,
                            source.data_etag,
                            source.data_last_modified,
                            source.strict_headers,
                            next_block,
                        )?)
                        .and_then(|exclusive| exclusive.checked_sub(1))
                        .ok_or_else(|| format!("{} chromosome range overflow", source.label))?
                }
                None if !require_following => source.data_bytes - 1,
                None => return Err(format!("{source_contig} has no following index range")),
            };
            if end < start || end >= source.data_bytes {
                return Err(format!(
                    "{} chromosome {chromosome} has an invalid range",
                    source.label
                ));
            }
            Ok(IndexedByteRange {
                chromosome,
                start,
                end,
                uncompressed_skip,
            })
        })
        .collect()
}

fn plan_cadd_artifact(
    client: &reqwest::blocking::Client,
    artifact: CaddArtifact,
    resource_root: &Path,
) -> Result<CaddArtifactPlan, String> {
    let identity = format!(
        "{}|{}|{}|{}|{}|{}",
        artifact.data_url,
        artifact.data_bytes,
        artifact.data_md5,
        artifact.index_url,
        artifact.index_bytes,
        artifact.index_md5,
    );
    let ranges = range_plan::load_or_build(
        resource_root,
        &format!("cadd-{}", artifact.id),
        &identity,
        || {
            let label = format!("CADD {}", artifact.id);
            indexed_ranges(
                client,
                IndexedSource {
                    label: &label,
                    data_url: &artifact.data_url,
                    data_bytes: artifact.data_bytes,
                    data_etag: Some(&artifact.data_etag),
                    data_last_modified: Some(&artifact.data_last_modified),
                    index_url: &artifact.index_url,
                    index_bytes: artifact.index_bytes,
                    index_md5: &artifact.index_md5,
                    index_etag: Some(&artifact.index_etag),
                    index_last_modified: Some(&artifact.index_last_modified),
                    strict_headers: true,
                },
                canonical_chromosomes(false)
                    .into_iter()
                    .map(|chromosome| (chromosome.into(), chromosome.into()))
                    .collect(),
                false,
            )
        },
    )?;
    Ok(CaddArtifactPlan { artifact, ranges })
}

fn plan_spliceai_artifact(
    client: &reqwest::blocking::Client,
    resource_root: &Path,
) -> Result<SpliceAiArtifactPlan, String> {
    let artifact = indexed_catalog::spliceai_artifact()?;
    let identity = format!(
        "{}|{}|{}|{}|{}",
        artifact.data_url,
        artifact.data_bytes,
        artifact.index_url,
        artifact.index_bytes,
        artifact.index_md5,
    );
    let ranges = range_plan::load_or_build(resource_root, "spliceai", &identity, || {
        indexed_ranges(
            client,
            IndexedSource {
                label: "SpliceAI",
                data_url: &artifact.data_url,
                data_bytes: artifact.data_bytes,
                data_etag: Some(&artifact.data_etag),
                data_last_modified: Some(&artifact.data_last_modified),
                index_url: &artifact.index_url,
                index_bytes: artifact.index_bytes,
                index_md5: &artifact.index_md5,
                index_etag: Some(&artifact.index_etag),
                index_last_modified: Some(&artifact.index_last_modified),
                strict_headers: true,
            },
            canonical_chromosomes(false)
                .into_iter()
                .map(|chromosome| (chromosome.into(), chromosome.into()))
                .collect(),
            false,
        )
    })?;
    Ok(SpliceAiArtifactPlan { artifact, ranges })
}

fn plan_dbsnp_artifact(
    client: &reqwest::blocking::Client,
    artifact: DbsnpArtifact,
    resource_root: &Path,
) -> Result<DbsnpArtifactPlan, String> {
    let identity = format!(
        "{}|{}|{}|{}|{}|{}",
        artifact.data_url,
        artifact.data_bytes,
        artifact.data_md5,
        artifact.index_url,
        artifact.index_bytes,
        artifact.index_md5,
    );
    let ranges = range_plan::load_or_build(resource_root, "dbsnp", &identity, || {
        indexed_ranges(
            client,
            IndexedSource {
                label: "dbSNP",
                data_url: &artifact.data_url,
                data_bytes: artifact.data_bytes,
                data_etag: None,
                data_last_modified: artifact.data_last_modified.as_deref(),
                index_url: &artifact.index_url,
                index_bytes: artifact.index_bytes,
                index_md5: &artifact.index_md5,
                index_etag: None,
                index_last_modified: artifact.index_last_modified.as_deref(),
                strict_headers: false,
            },
            DBSNP_PRIMARY_CONTIGS
                .into_iter()
                .map(|(chromosome, source)| (chromosome.into(), source.into()))
                .collect(),
            true,
        )
    })?;
    Ok(DbsnpArtifactPlan { artifact, ranges })
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
    storage_status(resource_id, resource_root, chromosomes, live)
}

pub fn verified_storage_status(
    resource_id: &str,
    resource_root: &Path,
    chromosomes: &[String],
) -> LivePreparationState {
    if chromosomes.is_empty() {
        return LivePreparationState::default();
    }
    storage_status(
        resource_id,
        resource_root,
        chromosomes,
        LivePreparationState::default(),
    )
}

fn storage_status(
    resource_id: &str,
    resource_root: &Path,
    chromosomes: &[String],
    live: LivePreparationState,
) -> LivePreparationState {
    let expected_selected_schema = if resource_id == "dbnsfp" {
        Some(
            load_dbnsfp_field_selection(resource_root)
                .map(|selection| dbnsfp_schema_identity(&selection))
                .ok(),
        )
        .flatten()
    } else {
        let configuration_root = resource_root.parent().unwrap_or(resource_root);
        load_supplementary_field_selection(resource_id, configuration_root)
            .and_then(|selection| {
                let base = chromosomes
                    .iter()
                    .find_map(|chromosome| {
                        ShardPaths::new(resource_root, chromosome)
                            .ok()
                            .and_then(|paths| read_checkpoint(&paths.verification()).ok())
                            .map(|checkpoint| {
                                checkpoint
                                    .identity
                                    .selected_schema
                                    .split(':')
                                    .next()
                                    .unwrap_or_default()
                                    .to_owned()
                            })
                    })
                    .unwrap_or_default();
                supplementary_schema_identity(&base, resource_id, &selection)
            })
            .ok()
    };
    let mut network_bytes = 0_u64;
    let mut prepared_bytes = 0_u64;
    let mut parsed_records = 0_u64;
    let mut ready_shards = 0_u16;
    let mut rebuild_shards = 0_u16;
    for chromosome in chromosomes {
        let Ok(paths) = ShardPaths::new(resource_root, chromosome) else {
            return live;
        };
        let Ok(checkpoint) = read_checkpoint(&paths.verification()) else {
            continue;
        };
        let compatibility = if checkpoint.identity.resource_id != resource_id {
            VerifiedCacheCompatibility::RebuildRequired
        } else {
            let mut expected = checkpoint.identity.clone();
            if let Some(schema) = expected_selected_schema.as_deref() {
                expected.selected_schema = schema.into();
            }
            verified_cache_compatibility(&paths, &expected)
        };
        match compatibility {
            VerifiedCacheCompatibility::Missing => continue,
            VerifiedCacheCompatibility::Ready => ready_shards += 1,
            VerifiedCacheCompatibility::RebuildRequired => rebuild_shards += 1,
        }
        network_bytes = network_bytes.saturating_add(checkpoint.compressed_bytes_read);
        prepared_bytes = prepared_bytes
            .saturating_add(checkpoint.prepared_bytes)
            .saturating_add(checkpoint.prepared_index_bytes);
        parsed_records = parsed_records.saturating_add(checkpoint.parsed_records);
    }
    let total = chromosomes.len() as u16;
    let installed = ready_shards.saturating_add(rebuild_shards);
    let shards_ready = ready_shards == total;
    let manifest_path = resource_root.join(format!("{resource_id}.osa-shards.json"));
    let manifest_missing = shards_ready && !manifest_path.is_file();
    let ready = shards_ready && !manifest_missing;
    let state = if rebuild_shards > 0 {
        "rebuild-required"
    } else if manifest_missing {
        "rebuild-required"
    } else if ready {
        "ready"
    } else {
        "idle"
    };
    LivePreparationState {
        resource_id: Some(resource_id.into()),
        state: state.into(),
        phase: state.into(),
        chromosome: None,
        network_bytes,
        expected_network_bytes: network_bytes,
        percent: if installed == total {
            100.0
        } else {
            installed as f64 * 100.0 / total as f64
        },
        parsed_records,
        prepared_bytes,
        completed_chromosomes: installed,
        remaining_chromosomes: total.saturating_sub(installed),
        detail: if manifest_missing && rebuild_shards == 0 {
            format!("{resource_id} cache manifest is missing; rebuild is required")
        } else if state == "rebuild-required" {
            format!(
                "{rebuild_shards} {resource_id} cache shard(s) use an incompatible or unverified cache contract"
            )
        } else if ready {
            format!("All {resource_id} chromosome shards are installed and verified")
        } else if installed > 0 {
            format!("{installed} installed {resource_id} chromosome shards are retained")
        } else {
            "No preparation job is active".into()
        },
        ..LivePreparationState::default()
    }
}

pub fn discard_generated_cache(resource_id: &str, resource_root: &Path) -> Result<(), String> {
    if resource_root
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        != Some(resource_id)
    {
        return Err("refusing to rebuild a cache outside its managed resource directory".into());
    }
    for directory in ["shards", "staging"] {
        let path = resource_root.join(directory);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "cannot remove generated cache directory {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    let manifest = resource_root.join(format!("{resource_id}.osa-shards.json"));
    if manifest.exists() {
        fs::remove_file(&manifest).map_err(|error| {
            format!(
                "cannot remove generated cache manifest {}: {error}",
                manifest.display()
            )
        })?;
    }
    Ok(())
}

pub struct LivePreparationRequest {
    pub fastvep_executable: PathBuf,
    pub source_type: String,
    pub resource_root: PathBuf,
    pub identity: PreparationIdentity,
}

fn load_selected_fields(
    resource_id: &str,
    resource_root: &Path,
) -> Result<SupplementaryFieldSelection, String> {
    let configuration_root = resource_root.parent().unwrap_or(resource_root);
    let selection = load_supplementary_field_selection(resource_id, configuration_root)?;
    save_supplementary_field_selection(resource_id, configuration_root, selection)
}

pub fn start_live(mut request: LivePreparationRequest) -> Result<(), String> {
    let selection = load_selected_fields(&request.identity.resource_id, &request.resource_root)?;
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
    spawn_live_job(job, move || run_live(request, selection))
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
    let selection = load_selected_fields("dbsnp", &request.resource_root)?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("dbsnp".into()),
        state: "running".into(),
        phase: "reading-index".into(),
        expected_network_bytes: request.artifact.data_bytes,
        remaining_chromosomes: DBSNP_PRIMARY_CONTIGS.len() as u16,
        detail: "Reading the official dbSNP tabix index".into(),
        ..LivePreparationState::default()
    })?;
    spawn_live_job(job, move || run_dbsnp_live(request, selection))
}

pub fn start_dbnsfp_live(request: DbnsfpLiveRequest) -> Result<(), String> {
    let manifest = pinned_dbnsfp_manifest()?;
    let selection = save_dbnsfp_field_selection(
        &request.resource_root,
        load_dbnsfp_field_selection(&request.resource_root)?,
    )?;
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
    spawn_live_job(job, move || run_dbnsfp_live(request, manifest, selection))
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

pub struct HpoLiveRequest {
    pub resource_root: PathBuf,
    pub manifest: crate::phenotype::HpoAssetManifest,
}

pub fn start_hpo_live(request: HpoLiveRequest) -> Result<(), String> {
    let expected_network_bytes = request.manifest.expected_bytes();
    let job = register_live_job(LivePreparationState {
        resource_id: Some("hpo".into()),
        state: "running".into(),
        phase: "starting".into(),
        expected_network_bytes,
        remaining_chromosomes: 1,
        detail: "Starting the phenotype and condition knowledge installation".into(),
        ..LivePreparationState::default()
    })?;
    spawn_live_job(job, move || run_hpo_live(request))
}

pub fn start_revel_live(request: RevelLiveRequest) -> Result<(), String> {
    let manifest = pinned_revel_manifest()?;
    let selection = load_selected_fields("revel", &request.resource_root)?;
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
    spawn_live_job(job, move || run_revel_live(request, manifest, selection))
}

pub fn start_spliceai_live(request: SpliceAiLiveRequest) -> Result<(), String> {
    let artifact = indexed_catalog::spliceai_artifact()?;
    let selection = load_selected_fields("spliceai", &request.resource_root)?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("spliceai".into()),
        state: "running".into(),
        phase: "reading-index".into(),
        expected_network_bytes: artifact.data_bytes,
        remaining_chromosomes: 24,
        detail: "Reading the public Ensembl SpliceAI tabix index".into(),
        ..LivePreparationState::default()
    })?;
    spawn_live_job(job, move || run_spliceai_live(request, selection))
}

pub fn start_cadd_live(request: CaddLiveRequest) -> Result<(), String> {
    let artifacts = indexed_catalog::cadd_artifacts()?;
    let selection = load_selected_fields("cadd", &request.resource_root)?;
    let job = register_live_job(LivePreparationState {
        resource_id: Some("cadd".into()),
        state: "running".into(),
        phase: "reading-indexes".into(),
        expected_network_bytes: artifacts.iter().map(|item| item.data_bytes).sum(),
        remaining_chromosomes: 24,
        detail: "Reading the two small CADD tabix indexes".into(),
        ..LivePreparationState::default()
    })?;
    spawn_live_job(job, move || run_cadd_live(request, selection))
}

#[allow(clippy::too_many_arguments)]
fn stream_cadd_ranges_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    plans: &[CaddArtifactPlan; 2],
    chromosome_index: usize,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let snv_range = &plans[0].ranges[chromosome_index];
    let indel_range = &plans[1].ranges[chromosome_index];
    let mut inputs = Vec::with_capacity(2);
    let mut persisted_before = 0_u64;
    for (plan, range, tag) in [
        (&plans[0], snv_range, "snv"),
        (&plans[1], indel_range, "indel"),
    ] {
        let paths = request.paths.source_part_variant(tag);
        let mut identity = request.identity.clone();
        identity.source_url = format!("{}#{tag}", plan.artifact.data_url);
        identity.expected_compressed_bytes = range.len();
        identity.source_etag = Some(plan.artifact.data_etag.clone());
        identity.source_last_modified = Some(plan.artifact.data_last_modified.clone());
        let base = persisted_before;
        let part = resumable::acquire_range(
            &paths,
            &identity,
            &plan.artifact.data_url,
            range.start,
            plan.artifact.data_bytes,
            Some(&plan.artifact.data_etag),
            Some(&plan.artifact.data_last_modified),
            live_cancel().as_ref(),
            |state| {
                let persisted = base.saturating_add(state.persisted_bytes);
                update_indexed_progress(
                    "CADD",
                    &request.identity.chromosome,
                    completed,
                    24,
                    base_network.saturating_add(persisted),
                    total_network,
                    prepared_bytes,
                    state.bytes_per_second,
                );
                update_resumable_download_detail(
                    "CADD",
                    &request.identity.chromosome,
                    persisted,
                    snv_range.len().saturating_add(indel_range.len()),
                );
            },
        )?;
        drop(part.reader);
        inputs.push(builder::RawFileInput {
            path: paths.source_part().to_path_buf(),
            uncompressed_skip: range.uncompressed_skip as u64,
        });
        persisted_before = persisted_before.saturating_add(range.len());
    }
    builder::build_from_files(
        request,
        &inputs,
        Some(&request.identity.chromosome),
        "CADD",
        live_cancel().as_ref(),
    )
}

#[allow(clippy::too_many_arguments)]
fn stream_spliceai_range_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    plan: &SpliceAiArtifactPlan,
    range: &IndexedByteRange,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let resumable = source_input_mode() == SourceInputMode::Resumable;
    if resumable {
        let part = resumable::acquire_range(
            request.paths,
            request.identity,
            &plan.artifact.data_url,
            range.start,
            plan.artifact.data_bytes,
            Some(&plan.artifact.data_etag),
            Some(&plan.artifact.data_last_modified),
            live_cancel().as_ref(),
            |state| {
                update_indexed_progress(
                    "SpliceAI",
                    &range.chromosome,
                    completed,
                    24,
                    base_network.saturating_add(state.persisted_bytes),
                    total_network,
                    prepared_bytes,
                    state.bytes_per_second,
                );
                update_resumable_download_detail(
                    "SpliceAI",
                    &range.chromosome,
                    state.persisted_bytes,
                    state.expected_bytes,
                );
            },
        )?;
        drop(part.reader);
        return builder::build_from_files(
            request,
            &[builder::RawFileInput {
                path: request.paths.source_part().to_path_buf(),
                uncompressed_skip: range.uncompressed_skip as u64,
            }],
            Some(&range.chromosome),
            "SpliceAI",
            live_cancel().as_ref(),
        );
    }
    let mut reader = transport::ReconnectingRangeReader::new(
        &plan.artifact.data_url,
        "spliceai",
        &range.chromosome,
        range.start,
        range.end,
        plan.artifact.data_bytes,
        Some(&plan.artifact.data_etag),
        Some(&plan.artifact.data_last_modified),
    )?;
    builder::build_from_reader(
        request,
        &mut reader,
        range.uncompressed_skip as u64,
        Some(&range.chromosome),
        live_cancel().as_ref(),
        |bytes, throughput| {
            update_indexed_progress(
                "SpliceAI",
                &range.chromosome,
                completed,
                24,
                base_network.saturating_add(bytes),
                total_network,
                prepared_bytes,
                throughput,
            );
        },
    )
}

pub fn start_sharded_live(mut request: ShardedLiveRequest) -> Result<(), String> {
    let selection = load_selected_fields(&request.source.resource_id, &request.resource_root)?;
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
    spawn_live_job(job, move || run_sharded_live(request, selection))
}

fn finish_sharded_preparation(
    result: Result<(u64, u64, u16), String>,
    ready_detail: &str,
    cancelled_detail: &str,
    failed_detail: &str,
) {
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
                state.detail = ready_detail.into();
            }
            Err(error) if error == "cancelled" => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail = cancelled_detail.into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail = failed_detail.into();
            }
        }
    }
}

fn run_hpo_live(request: HpoLiveRequest) {
    let cancelled = live_cancel();
    let started = Instant::now();
    let result = crate::phenotype::install_hpo(
        &request.resource_root,
        &request.manifest,
        cancelled.as_ref(),
        |progress| {
            if let Ok(mut state) = live_state().lock() {
                state.phase = progress.phase;
                state.detail = progress.detail;
                state.network_bytes = progress.network_bytes;
                state.expected_network_bytes = progress.expected_network_bytes;
                state.parsed_records = progress.parsed_records;
                state.prepared_bytes = progress.prepared_bytes;
                state.percent = if progress.expected_network_bytes == 0 {
                    0.0
                } else {
                    progress.network_bytes as f64 * 100.0 / progress.expected_network_bytes as f64
                };
                state.throughput_bytes_per_second =
                    progress.network_bytes as f64 / started.elapsed().as_secs_f64().max(0.001);
            }
        },
    );
    if let Ok(mut state) = live_state().lock() {
        match result {
            Ok(ready) => {
                state.state = "ready".into();
                state.phase = "ready".into();
                state.network_bytes = ready.asset_bytes;
                state.expected_network_bytes = ready.asset_bytes;
                state.parsed_records = ready.disease_count as u64;
                state.completed_chromosomes = 1;
                state.remaining_chromosomes = 0;
                state.percent = 100.0;
                state.throughput_bytes_per_second = 0.0;
                state.detail = format!(
                    "Indexed {} phenotype terms, {} condition terms, and {} disease profiles",
                    ready.term_count, ready.mondo_term_count, ready.disease_count
                );
            }
            Err(_) if cancelled.load(Ordering::SeqCst) => {
                state.state = "cancelled".into();
                state.phase = "cancelled".into();
                state.detail =
                    "Knowledge installation paused; verified assets and partial downloads were retained"
                        .into();
            }
            Err(error) => {
                state.state = "failed".into();
                state.phase = "failed".into();
                state.error = Some(error);
                state.detail =
                    "Knowledge installation failed; incomplete assets were not promoted".into();
            }
        }
    }
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
            let paths = ShardPaths::new(&request.resource_root, &shard.chromosome)?;
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
                osa_schema_version: effective_cache_format(&paths, &request.source.resource_id)?
                    .schema_version(),
            };
            match restart_decision(&paths, &identity) {
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
                        &shard.chromosome,
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
                        &shard.chromosome,
                        completed,
                        request.source.shards.len() as u16,
                        base_network.saturating_add(progress.compressed_bytes_read),
                        expected_total,
                        prepared_bytes,
                        progress.bytes_per_second,
                    );
                    update_resumable_progress_detail(
                        &request.source.resource_id,
                        &shard.chromosome,
                        progress,
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
    finish_sharded_preparation(
        result,
        &format!("All {resource_id} chromosome shards are verified"),
        &format!("Cancellation completed; verified {resource_id} shards were retained"),
        &format!("{resource_id} preparation failed; completed shards were retained"),
    );
}

fn stream_dbsnp_range_to_partial_osa(
    request: &StreamingBuildRequest<'_>,
    plan: &DbsnpArtifactPlan,
    range: &IndexedByteRange,
    base_network: u64,
    total_network: u64,
    completed: u16,
    prepared_bytes: u64,
) -> Result<StreamingBuildResult, String> {
    let resumable = source_input_mode() == SourceInputMode::Resumable;
    if resumable {
        let part = resumable::acquire_range(
            request.paths,
            request.identity,
            &plan.artifact.data_url,
            range.start,
            plan.artifact.data_bytes,
            None,
            plan.artifact.data_last_modified.as_deref(),
            live_cancel().as_ref(),
            |state| {
                update_indexed_progress(
                    "dbSNP",
                    &range.chromosome,
                    completed,
                    DBSNP_PRIMARY_CONTIGS.len() as u16,
                    base_network.saturating_add(state.persisted_bytes),
                    total_network,
                    prepared_bytes,
                    state.bytes_per_second,
                );
                update_resumable_download_detail(
                    "dbSNP",
                    &range.chromosome,
                    state.persisted_bytes,
                    state.expected_bytes,
                );
            },
        )?;
        drop(part.reader);
        return builder::build_from_files(
            request,
            &[builder::RawFileInput {
                path: request.paths.source_part().to_path_buf(),
                uncompressed_skip: range.uncompressed_skip as u64,
            }],
            Some(&range.chromosome),
            "dbSNP",
            live_cancel().as_ref(),
        );
    }
    let mut reader = transport::ReconnectingRangeReader::new(
        &plan.artifact.data_url,
        "dbsnp",
        &range.chromosome,
        range.start,
        range.end,
        plan.artifact.data_bytes,
        None,
        plan.artifact.data_last_modified.as_deref(),
    )?;
    builder::build_from_reader(
        request,
        &mut reader,
        range.uncompressed_skip as u64,
        Some(&range.chromosome),
        live_cancel().as_ref(),
        |bytes, throughput| {
            update_indexed_progress(
                "dbSNP",
                &range.chromosome,
                completed,
                DBSNP_PRIMARY_CONTIGS.len() as u16,
                base_network.saturating_add(bytes),
                total_network,
                prepared_bytes,
                throughput,
            );
        },
    )
}

fn run_dbsnp_live(request: DbsnpLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema = supplementary_schema_identity(
            &format!("dbsnp-{}", request.artifact.release),
            "dbsnp",
            &selection,
        )?;
        let client = crate::http_client::source()?;
        let plan = plan_dbsnp_artifact(&client, request.artifact, &request.resource_root)?;
        let expected_total = plan.ranges.iter().map(IndexedByteRange::len).sum();
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
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
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
                osa_schema_version: effective_cache_format(&paths, "dbsnp")?.schema_version(),
            };
            match restart_decision(&paths, &identity) {
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
                        &plan,
                        range,
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
    finish_sharded_preparation(
        result,
        "All dbSNP chromosome shards are verified",
        "dbSNP paused; source prefix and verified shards retained",
        "dbSNP preparation failed; resumable data was retained",
    );
}

fn run_cadd_live(request: CaddLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema =
            supplementary_schema_identity("cadd-v1.7-grch38", "cadd", &selection)?;
        let client = crate::http_client::source()?;
        let artifacts = indexed_catalog::cadd_artifacts()?;
        let plans = [
            plan_cadd_artifact(&client, artifacts[0].clone(), &request.resource_root)?,
            plan_cadd_artifact(&client, artifacts[1].clone(), &request.resource_root)?,
        ];
        let expected_total = plans
            .iter()
            .flat_map(|plan| plan.ranges.iter())
            .map(IndexedByteRange::len)
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
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
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
                osa_schema_version: effective_cache_format(&paths, "cadd")?.schema_version(),
            };
            match restart_decision(&paths, &identity) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_indexed_progress(
                        "CADD",
                        chromosome,
                        completed,
                        24,
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
    finish_sharded_preparation(
        result,
        "All CADD chromosome shards are verified",
        "Cancellation completed; verified CADD shards were retained",
        "CADD preparation failed; completed shards were retained",
    );
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
            let paths = ShardPaths::new(&request.resource_root, &archive.chromosome)?;
            let identity = PreparationIdentity {
                resource_id: "revel".into(),
                release: manifest.release.clone(),
                assembly: manifest.assembly.clone(),
                chromosome: archive.chromosome.clone(),
                source_url: manifest.archive_url(&archive.filename),
                expected_compressed_bytes: archive.bytes,
                source_etag: Some(format!("md5:{}", archive.md5)),
                source_last_modified: None,
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: effective_cache_format(&paths, "revel")?.schema_version(),
            };
            match restart_decision(&paths, &identity) {
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
    finish_sharded_preparation(
        result,
        "All REVEL v1.3 chromosome shards are verified",
        "Cancellation completed; verified REVEL shards were retained",
        "REVEL preparation failed; completed shards were retained",
    );
}

fn run_spliceai_live(request: SpliceAiLiveRequest, selection: SupplementaryFieldSelection) {
    let result = (|| {
        let selected_schema = supplementary_schema_identity(
            "spliceai-ensembl-mane-v1.4-masked-snv",
            "spliceai",
            &selection,
        )?;
        let client = crate::http_client::source()?;
        let plan = plan_spliceai_artifact(&client, &request.resource_root)?;
        let expected_total = plan.ranges.iter().map(IndexedByteRange::len).sum();
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
            let paths = ShardPaths::new(&request.resource_root, chromosome)?;
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
                source_etag: Some(plan.artifact.data_etag.clone()),
                source_last_modified: Some(plan.artifact.data_last_modified.clone()),
                selected_schema: selected_schema.clone(),
                fastvep_commit: LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
                osa_schema_version: effective_cache_format(&paths, "spliceai")?.schema_version(),
            };
            match restart_decision(&paths, &identity) {
                RestartDecision::AlreadyVerified => {
                    completed += 1;
                    if let Ok(checkpoint) = read_checkpoint(&paths.verification()) {
                        network_bytes =
                            network_bytes.saturating_add(checkpoint.compressed_bytes_read);
                        prepared_bytes = prepared_bytes
                            .saturating_add(checkpoint.prepared_bytes)
                            .saturating_add(checkpoint.prepared_index_bytes);
                    }
                    update_indexed_progress(
                        "SpliceAI",
                        chromosome,
                        completed,
                        24,
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
    finish_sharded_preparation(
        result,
        "All public SpliceAI chromosome shards are verified",
        "Cancellation completed; verified SpliceAI shards were retained",
        "SpliceAI preparation failed; completed shards were retained",
    );
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
    for (attempt, retry_delay) in TRANSIENT_CHROMOSOME_STREAM_RETRY_DELAYS
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
    {
        match operation() {
            Ok(result) => return Ok(result),
            Err(error) if error == "cancelled" || cancelled.load(Ordering::SeqCst) => {
                return Err("cancelled".into());
            }
            Err(error) if is_transient_chromosome_stream_error(&error) && retry_delay.is_some() => {
                let delay = retry_delay.expect("guarded retry delay");
                crate::terminal_log(
                    "resources",
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
                    "resources",
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
            let paths = ShardPaths::new(&request.resource_root, &member.chromosome)?;
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
                osa_schema_version: effective_cache_format(&paths, "dbnsfp")?.schema_version(),
            };
            match restart_decision(&paths, &identity) {
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
                        "dbNSFP",
                        &member.chromosome,
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
                        update_sharded_progress(
                            "dbNSFP",
                            &member.chromosome,
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
                                update_sharded_progress(
                                    "dbNSFP",
                                    &member.chromosome,
                                    completed,
                                    manifest.members.len() as u16,
                                    base_network.saturating_add(progress.compressed_bytes_read),
                                    expected_total,
                                    prepared_bytes,
                                    progress.bytes_per_second,
                                );
                                update_resumable_progress_detail(
                                    "dbNSFP",
                                    &member.chromosome,
                                    progress,
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
    finish_sharded_preparation(
        result,
        "All dbNSFP 4.9a chromosome shards are verified",
        "Cancellation completed; verified dbNSFP shards were retained",
        "dbNSFP preparation failed; completed shards were retained",
    );
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
            | "dbsnp"
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
    let mut selected_format = None;
    let mut shards = Vec::new();
    for chromosome in chromosomes {
        let paths = ShardPaths::new(resource_root, chromosome)?;
        let checkpoint = read_checkpoint(&paths.verification()).map_err(|error| {
            format!("{resource_id} chromosome {chromosome} is not verified: {error}")
        })?;
        if checkpoint.state != CheckpointState::Verified
            || checkpoint.identity.resource_id != resource_id
        {
            return Err(format!(
                "{resource_id} chromosome {chromosome} has an invalid verification identity"
            ));
        }
        let format = checkpoint.identity.cache_format()?;
        if selected_format.is_some_and(|selected| selected != format) {
            return Err(format!(
                "{resource_id} verified shards use mixed OSA cache formats"
            ));
        }
        selected_format = Some(format);
        required_nonempty_file(&paths.final_data(format))?;
        if let Some(index) = paths.final_index(format) {
            required_nonempty_file(&index)?;
        }
        shards.push(serde_json::json!({
            "chromosome": chromosome,
            "file": format!("shards/chr{chromosome}/{}", format.data_file_name())
        }));
    }
    if shards.is_empty() {
        return Err(format!("{resource_id} shard manifest cannot be empty"));
    }
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
        match restart_decision(&paths, &request.identity) {
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
    use super::state::run_with_live_job;
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parser_worker_budget_tracks_installation_concurrency() {
        assert_eq!(fastvep_parser_workers_for_concurrency(1), 4);
        assert_eq!(fastvep_parser_workers_for_concurrency(2), 2);
        assert_eq!(fastvep_parser_workers_for_concurrency(3), 1);
        assert_eq!(fastvep_parser_workers_for_concurrency(4), 1);
    }

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
        fs::write(chr1.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(chr1.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
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
        fs::write(chr2.partial_data(CacheFormat::OsaV1), b"incomplete").unwrap();
        assert_eq!(
            restart_decision(&chr2, &identity("2")),
            RestartDecision::RestartCurrentChromosome
        );
        initialize_partial(&chr2, identity("2")).unwrap();
        assert!(
            chr1.final_data(CacheFormat::OsaV1).exists(),
            "completed chr1 must survive restarting chr2"
        );
        assert!(
            !chr2.partial_data(CacheFormat::OsaV1).exists(),
            "current partial output restarts from zero"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_keeps_one_cache_format_across_the_source() {
        let resource_root = root("source-cache-format");
        let chr1 = ShardPaths::new(&resource_root, "1").unwrap();
        let osa1 = dbnsfp_identity("1", DBNSFP_CURATED_SCHEMA);
        initialize_partial(&chr1, osa1.clone()).unwrap();
        fs::write(chr1.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(chr1.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&chr1, osa1, 10, 2).unwrap();

        let chr2 = ShardPaths::new(&resource_root, "2").unwrap();
        assert_eq!(
            effective_cache_format(&chr2, "dbnsfp"),
            Ok(CacheFormat::OsaV1)
        );

        let mut osa2 = dbnsfp_identity("2", DBNSFP_CURATED_SCHEMA);
        osa2.osa_schema_version = CacheFormat::OsaV2.schema_version();
        initialize_partial(&chr2, osa2.clone()).unwrap();
        fs::write(chr2.partial_data(CacheFormat::OsaV2), b"osa2").unwrap();
        promote_verified(&chr2, osa2, 10, 2).unwrap();
        let chr3 = ShardPaths::new(&resource_root, "3").unwrap();
        assert!(
            effective_cache_format(&chr3, "dbnsfp")
                .unwrap_err()
                .contains("mixed OSA1 and OSA2")
        );

        let fresh = root("fresh-cache-format");
        let fresh_paths = ShardPaths::new(&fresh, "1").unwrap();
        assert_eq!(
            effective_cache_format(&fresh_paths, "dbnsfp"),
            Ok(CacheFormat::OsaV2)
        );
        fs::remove_dir_all(resource_root).unwrap();
    }

    #[test]
    fn old_partial_checkpoint_restarts_with_its_saved_format() {
        let root = root("old-partial-cache-format");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::remove_file(paths.partial_cache_contract()).unwrap();

        assert_eq!(
            effective_cache_format(&paths, "gnomad"),
            Ok(CacheFormat::OsaV1)
        );
        assert_eq!(
            restart_decision(&paths, &expected),
            RestartDecision::RestartCurrentChromosome
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_cache_accepts_missing_legacy_hashes_but_rejects_size_changes() {
        let root = root("verified-file-size");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, expected.clone(), 10, 2).unwrap();

        let mut checkpoint: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.verification()).unwrap()).unwrap();
        checkpoint.as_object_mut().unwrap().remove("preparedSha256");
        checkpoint
            .as_object_mut()
            .unwrap()
            .remove("preparedIndexSha256");
        fs::write(
            paths.verification(),
            serde_json::to_vec_pretty(&checkpoint).unwrap(),
        )
        .unwrap();
        assert_eq!(
            verified_cache_compatibility(&paths, &expected),
            VerifiedCacheCompatibility::Ready
        );

        fs::write(paths.final_data(CacheFormat::OsaV1), b"longer").unwrap();
        assert_eq!(
            verified_cache_compatibility(&paths, &expected),
            VerifiedCacheCompatibility::RebuildRequired
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_verification_detects_same_size_hash_changes_without_writing_metadata() {
        let root = root("verified-file-hash");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, expected, 10, 2).unwrap();
        let checkpoint = fs::read(paths.verification()).unwrap();
        let contract = fs::read(paths.cache_contract()).unwrap();

        fs::write(paths.final_data(CacheFormat::OsaV1), b"bad").unwrap();
        let error = verify_source_cache(
            Path::new("unused-fastvep"),
            &root,
            "gnomad",
            &["1".into()],
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("SHA-256 mismatch"));
        assert_eq!(fs::read(paths.verification()).unwrap(), checkpoint);
        assert_eq!(fs::read(paths.cache_contract()).unwrap(), contract);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_legacy_cache_without_v2_sidecar_requires_rebuild_but_is_retained() {
        let root = root("legacy-cache-contract");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, expected.clone(), 10, 2).unwrap();
        fs::remove_file(paths.cache_contract()).unwrap();

        assert_eq!(
            verified_cache_compatibility(&paths, &expected),
            VerifiedCacheCompatibility::RebuildRequired
        );
        assert_eq!(
            restart_decision(&paths, &expected),
            RestartDecision::StaleIdentity
        );
        assert!(paths.final_data(CacheFormat::OsaV1).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn osa2_cache_promotes_without_an_external_index() {
        let root = root("osa2-promotion");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = dbnsfp_identity("1", DBNSFP_CURATED_SCHEMA);
        expected.osa_schema_version = CacheFormat::OsaV2.schema_version();
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV2), b"osa2").unwrap();

        promote_verified(&paths, expected.clone(), 10, 2).unwrap();

        assert!(paths.final_data(CacheFormat::OsaV2).is_file());
        assert!(paths.final_index(CacheFormat::OsaV2).is_none());
        let checkpoint = read_checkpoint(&paths.verification()).unwrap();
        assert_eq!(checkpoint.prepared_index_bytes, 0);
        assert_eq!(checkpoint.identity.cache_format(), Ok(CacheFormat::OsaV2));
        let contract = crate::cache_contract::read(&paths.cache_contract()).unwrap();
        assert_eq!(contract.cache_contract.osa_schema_version, 2);
        assert_eq!(
            contract.cache_contract.reader_compatibility,
            "fastvep-osa-v2"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resumable_part_finishes_saved_prefix_before_opening_local_reader() {
        let body: &'static [u8] = b"abcdefghij";
        let root = root("resumable-prefix-resume");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = range_fixture(body, 1);
        expected.expected_compressed_bytes = body.len() as u64;
        expected.source_etag = Some("fixture".into());
        fs::create_dir_all(paths.source_part().parent().unwrap()).unwrap();
        fs::write(
            paths.source_part_identity(),
            serde_json::to_vec_pretty(&expected).unwrap(),
        )
        .unwrap();
        fs::write(paths.source_part(), &body[..5]).unwrap();
        let part = resumable::acquire_range(
            &paths,
            &expected,
            &expected.source_url,
            0,
            body.len() as u64,
            expected.source_etag.as_deref(),
            None,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(part.resumed_bytes, 5);
        assert_eq!(part.downloaded_bytes, 5);
        let mut received = Vec::new();
        part.reader
            .take(body.len() as u64)
            .read_to_end(&mut received)
            .unwrap();
        assert_eq!(received, body);
        assert_eq!(fs::read(paths.source_part()).unwrap(), body);

        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, expected, body.len() as u64, 1).unwrap();
        assert!(!paths.source_part().exists());
        assert!(!paths.source_part_identity().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resumable_part_reconnects_before_opening_local_reader() {
        let body: &'static [u8] = b"abcdefghij";
        let (url, requests) = interrupted_range_fixture(body, 4);
        let root = root("resumable-inline-reconnect");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = url;
        expected.expected_compressed_bytes = body.len() as u64;
        expected.source_etag = Some("fixture".into());
        let part = resumable::acquire_range(
            &paths,
            &expected,
            &expected.source_url,
            0,
            body.len() as u64,
            expected.source_etag.as_deref(),
            None,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let mut received = Vec::new();
        part.reader
            .take(body.len() as u64)
            .read_to_end(&mut received)
            .unwrap();
        assert_eq!(part.resumed_bytes, 0);
        assert_eq!(part.downloaded_bytes, body.len() as u64);
        assert_eq!(received, body);
        assert_eq!(fs::read(paths.source_part()).unwrap(), body);
        let first = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(first.to_ascii_lowercase().contains("range: bytes=0-9"));
        assert!(second.to_ascii_lowercase().contains("range: bytes=4-9"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_resumable_download_continues_without_exposing_partial_input() {
        let body: &'static [u8] = Box::leak(vec![7_u8; 3 * 1024 * 1024].into_boxed_slice());
        let root = root("resumable-process-restart");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = identity("1");
        expected.source_url = range_fixture(body, 2);
        expected.expected_compressed_bytes = body.len() as u64;
        expected.source_etag = Some("fixture".into());
        let cancelled = AtomicBool::new(false);

        let error = resumable::acquire_range(
            &paths,
            &expected,
            &expected.source_url,
            0,
            body.len() as u64,
            expected.source_etag.as_deref(),
            None,
            &cancelled,
            |state| {
                if state.persisted_bytes > 0 {
                    cancelled.store(true, Ordering::SeqCst);
                }
            },
        )
        .unwrap_err();
        assert_eq!(error, "cancelled");
        let retained = fs::metadata(paths.source_part()).unwrap().len();
        assert!(retained > 0 && retained < body.len() as u64);

        cancelled.store(false, Ordering::SeqCst);
        let mut part = resumable::acquire_range(
            &paths,
            &expected,
            &expected.source_url,
            0,
            body.len() as u64,
            expected.source_etag.as_deref(),
            None,
            &cancelled,
            |_| {},
        )
        .unwrap();
        assert_eq!(part.resumed_bytes, retained);
        assert_eq!(
            part.downloaded_bytes,
            body.len() as u64 - retained,
            "only the missing suffix should be downloaded"
        );
        let mut received = Vec::new();
        part.reader.read_to_end(&mut received).unwrap();
        assert_eq!(received, body);
        fs::remove_dir_all(root).unwrap();
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
    fn incompatible_dbnsfp_shards_require_explicit_rebuild() {
        let parent = root("dbnsfp-curated-upgrade").join("dbnsfp");
        let root = parent.join("4.9a");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let legacy = dbnsfp_identity("1", "dbnsfp-4.9a");
        initialize_partial(&paths, legacy.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, legacy, 10, 2).unwrap();

        let status = status_with_storage("dbnsfp", &root, &["1".into()]);
        assert_eq!(status.state, "rebuild-required");
        assert_eq!(status.completed_chromosomes, 1);
        assert!(paths.final_data(CacheFormat::OsaV1).is_file());

        discard_generated_cache("dbnsfp", &root).unwrap();
        assert!(!paths.final_directory.exists());
        fs::remove_dir_all(parent.parent().unwrap()).unwrap();
    }

    #[test]
    fn osa2_without_a_contract_requires_rebuild() {
        let root = root("osa2-missing-contract");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let mut expected = dbnsfp_identity("1", DBNSFP_CURATED_SCHEMA);
        expected.osa_schema_version = CacheFormat::OsaV2.schema_version();
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV2), b"osa2").unwrap();
        promote_verified(&paths, expected.clone(), 10, 2).unwrap();
        fs::remove_file(paths.cache_contract()).unwrap();

        assert_eq!(
            verified_cache_compatibility(&paths, &expected),
            VerifiedCacheCompatibility::RebuildRequired
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_cache_contract_requires_rebuild() {
        let root = root("malformed-cache-contract");
        let paths = ShardPaths::new(&root, "1").unwrap();
        let expected = identity("1");
        initialize_partial(&paths, expected.clone()).unwrap();
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
        promote_verified(&paths, expected.clone(), 10, 2).unwrap();
        fs::write(paths.cache_contract(), b"{").unwrap();

        assert_eq!(
            verified_cache_compatibility(&paths, &expected),
            VerifiedCacheCompatibility::RebuildRequired
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rebuild_discards_only_generated_cache_outputs() {
        let parent = root("safe-rebuild").join("gnomad");
        let resource_root = parent.join("4.1");
        for directory in ["shards", "staging", "source-parts"] {
            fs::create_dir_all(resource_root.join(directory)).unwrap();
            fs::write(resource_root.join(directory).join("fixture"), b"fixture").unwrap();
        }
        fs::write(resource_root.join("gnomad.osa-shards.json"), b"manifest").unwrap();
        fs::write(parent.join("field-selection.json"), b"settings").unwrap();

        discard_generated_cache("gnomad", &resource_root).unwrap();

        assert!(!resource_root.join("shards").exists());
        assert!(!resource_root.join("staging").exists());
        assert!(!resource_root.join("gnomad.osa-shards.json").exists());
        assert!(resource_root.join("source-parts").join("fixture").is_file());
        assert!(parent.join("field-selection.json").is_file());
        fs::remove_dir_all(parent.parent().unwrap()).unwrap();
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
            fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
            fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
            promote_verified(&paths, identity, 10, 2).unwrap();
        }
        write_osa_shard_manifest(&root, "dbnsfp", ["1", "2"].into_iter()).unwrap();

        let status = status_with_storage("dbnsfp", &root, &["1".into(), "2".into()]);
        assert_eq!(status.state, "ready");
        assert_eq!(status.completed_chromosomes, 2);
        assert_eq!(status.remaining_chromosomes, 0);
        assert_eq!(status.percent, 100.0);
        assert!(root.join("dbnsfp.osa-shards.json").is_file());
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
    fn tabix_virtual_offsets_are_bounded_and_strict() {
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
    }

    #[test]
    fn tiny_cadd_cache_without_a_contract_requires_rebuild() {
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
            restart_decision(&paths, &identity),
            RestartDecision::StaleIdentity
        );
        assert!(!paths.cache_contract().exists());
        fs::remove_dir_all(root).unwrap();
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
        assert!(!default.fields.contains(&"CADD_phred".into()));
        assert!(!default.fields.contains(&"phyloP100way_vertebrate".into()));
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
        let configuration =
            serde_json::to_value(dbnsfp_field_configuration(&root).unwrap()).unwrap();
        assert_eq!(configuration["locked"], true);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_dbnsfp_field_selection_migrates_only_before_cache_creation() {
        let root = root("dbnsfp-legacy-field-selection");
        fs::create_dir_all(&root).unwrap();
        let legacy = DbnsfpFieldSelection {
            schema_version: DBNSFP_FIELD_SELECTION_SCHEMA_VERSION,
            contract_id: DBNSFP_LEGACY_CURATED_SCHEMA.into(),
            fields: vec![
                "aaref".into(),
                "aaalt".into(),
                "aapos".into(),
                "genename".into(),
                "Ensembl_geneid".into(),
                "Ensembl_transcriptid".into(),
                "Ensembl_proteinid".into(),
                "Uniprot_acc".into(),
                "HGVSc_VEP".into(),
                "HGVSp_VEP".into(),
                "APPRIS".into(),
                "GENCODE_basic".into(),
                "TSL".into(),
                "VEP_canonical".into(),
                "REVEL_score".into(),
            ],
        };
        fs::write(
            root.join("field-selection.json"),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let migrated = load_dbnsfp_field_selection(&root).unwrap();
        assert_eq!(migrated.contract_id, DBNSFP_CURATED_SCHEMA);
        assert!(migrated.fields.contains(&"REVEL_score".into()));
        for required in ["Uniprot_entry", "MutPred_protID", "MutPred_AAchange"] {
            assert!(migrated.fields.contains(&required.into()));
        }
        assert_eq!(
            serde_json::from_slice::<DbnsfpFieldSelection>(
                &fs::read(root.join("field-selection.json")).unwrap()
            )
            .unwrap(),
            migrated
        );

        fs::create_dir_all(root.join("shards").join("chr1")).unwrap();
        fs::write(
            root.join("field-selection.json"),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let locked = load_dbnsfp_field_selection(&root).unwrap();
        assert_eq!(locked, legacy);
        assert!(dbnsfp_schema_identity(&locked).starts_with(DBNSFP_LEGACY_CURATED_SCHEMA));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_dbnsfp_staging_does_not_lock_field_selection() {
        let root = root("dbnsfp-field-selection-staging");
        let staging = root.join("staging").join("chr1.partial");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("checkpoint.json"), b"incomplete").unwrap();

        let saved =
            save_dbnsfp_field_selection(&root, full_dbnsfp_field_selection().unwrap()).unwrap();
        assert_eq!(saved, full_dbnsfp_field_selection().unwrap());
        assert!(!root.join("staging").exists());
        assert!(!dbnsfp_field_configuration(&root).unwrap().locked);
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
            let identity =
                supplementary_schema_identity("base-schema", resource_id, &selection).unwrap();
            if matches!(resource_id, "gnomad" | "gnomad-genomes") {
                let contract = supplementary_field_contract(resource_id).unwrap();
                let allowed_count = contract["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|group| group["fields"].as_array().unwrap().len())
                    .sum::<usize>();
                assert_eq!(selection.fields.len(), allowed_count);
                assert_ne!(identity, "base-schema");
            } else {
                assert_eq!(identity, "base-schema");
            }
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
    fn gnomad_full_defaults_are_persisted_without_relabeling_legacy_caches() {
        let legacy_root = root("gnomad-legacy-field-selection");
        fs::create_dir_all(legacy_root.join("4.1").join("shards").join("chr1")).unwrap();
        let legacy = load_supplementary_field_selection("gnomad", &legacy_root).unwrap();
        let full = default_supplementary_field_selection("gnomad").unwrap();
        assert!(legacy.fields.len() < full.fields.len());
        assert_eq!(
            supplementary_schema_identity("gnomad-v4.1", "gnomad", &legacy).unwrap(),
            "gnomad-v4.1"
        );
        fs::remove_dir_all(legacy_root).unwrap();

        let fresh_root = root("gnomad-full-field-selection");
        let saved =
            save_supplementary_field_selection("gnomad", &fresh_root, full.clone()).unwrap();
        assert_eq!(saved, full);
        assert!(fresh_root.join("field-selection.json").is_file());
        fs::create_dir_all(fresh_root.join("4.1").join("shards").join("chr1")).unwrap();
        assert_eq!(
            load_supplementary_field_selection("gnomad", &fresh_root).unwrap(),
            full
        );
        assert_ne!(
            supplementary_schema_identity("gnomad-v4.1", "gnomad", &full).unwrap(),
            "gnomad-v4.1"
        );
        fs::remove_dir_all(fresh_root).unwrap();
    }

    #[test]
    fn gnomad_exome_and_genome_field_selections_are_independent() {
        let base = root("gnomad-independent-field-selection");
        let exomes_root = base.join("gnomad");
        let genomes_root = base.join("gnomad-genomes");
        let mut exomes = default_supplementary_field_selection("gnomad").unwrap();
        let mut genomes = default_supplementary_field_selection("gnomad-genomes").unwrap();
        exomes.fields.pop();
        genomes.fields.remove(0);

        save_supplementary_field_selection("gnomad", &exomes_root, exomes.clone()).unwrap();
        save_supplementary_field_selection("gnomad-genomes", &genomes_root, genomes.clone())
            .unwrap();
        fs::create_dir_all(exomes_root.join("4.1.1-exomes/shards/chr1")).unwrap();

        assert!(
            supplementary_field_configuration("gnomad", &exomes_root)
                .unwrap()
                .locked
        );
        assert!(
            !supplementary_field_configuration("gnomad-genomes", &genomes_root)
                .unwrap()
                .locked
        );
        genomes.fields.pop();
        assert_eq!(
            save_supplementary_field_selection("gnomad-genomes", &genomes_root, genomes.clone())
                .unwrap(),
            genomes
        );
        assert_eq!(
            load_supplementary_field_selection("gnomad", &exomes_root).unwrap(),
            exomes
        );
        fs::remove_dir_all(base).unwrap();
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
        fs::write(paths.partial_data(CacheFormat::OsaV1), b"osa").unwrap();
        fs::write(paths.partial_index(CacheFormat::OsaV1).unwrap(), b"idx").unwrap();
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
        assert!(paths.final_data(CacheFormat::OsaV1).is_file());
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
        assert!(error.contains("invalid gzip header"));
        assert!(error.contains("Resume will redownload"));
        assert!(!paths.source_part().exists());
        assert!(!paths.source_part_identity().exists());
        assert!(!paths.partial_data(CacheFormat::OsaV1).exists());
        assert!(!paths.partial_index(CacheFormat::OsaV1).unwrap().exists());
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
        let mut progress_samples = Vec::new();
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
            |progress| progress_samples.push(progress),
        )
        .unwrap();
        assert_eq!(result.compressed_bytes_read, gzip.len() as u64);
        assert!(
            progress_samples
                .iter()
                .any(|progress| progress.consumed_bytes > 0 && progress.bytes_per_second > 0.0)
        );
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
        assert!(!bad_paths.partial_data(CacheFormat::OsaV1).exists());
        assert!(
            !bad_paths
                .partial_index(CacheFormat::OsaV1)
                .unwrap()
                .exists()
        );

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
    fn resumable_chromosome_download_requests_an_exact_range() {
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
        assert!(!paths.partial_data(CacheFormat::OsaV1).exists());
        assert!(!paths.partial_index(CacheFormat::OsaV1).unwrap().exists());
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
        let manifest = pinned_revel_manifest().unwrap();
        let archive = manifest
            .archives
            .iter()
            .find(|archive| archive.chromosome == "Y")
            .unwrap();
        let root = root("official-revel-y");
        let paths = ShardPaths::new(&root, "Y").unwrap();
        let identity = PreparationIdentity {
            resource_id: "revel".into(),
            release: "1.3".into(),
            assembly: "GRCh38".into(),
            chromosome: "Y".into(),
            source_url: manifest.archive_url(&archive.filename),
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
            archive,
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
    #[test]
    fn every_current_osa_resource_supports_a_shard_manifest() {
        let root = std::env::temp_dir().join(format!("annocat-manifests-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        for resource_id in [
            "dbnsfp",
            "dbsnp",
            "gnomad",
            "gnomad-genomes",
            "phylop",
            "cadd",
            "spliceai",
            "revel",
            "clinvar",
        ] {
            let resource_root = root.join(resource_id);
            let format = CacheFormat::preferred_for_resource(resource_id).unwrap();
            for chromosome in ["1", "2"] {
                let paths = ShardPaths::new(&resource_root, chromosome).unwrap();
                let mut shard_identity = identity(chromosome);
                shard_identity.resource_id = resource_id.into();
                shard_identity.osa_schema_version = format.schema_version();
                initialize_partial(&paths, shard_identity.clone()).unwrap();
                fs::write(paths.partial_data(format), b"data").unwrap();
                if let Some(index) = paths.partial_index(format) {
                    fs::write(index, b"index").unwrap();
                }
                promote_verified(&paths, shard_identity, 10, 1).unwrap();
            }
            write_osa_shard_manifest(&resource_root, resource_id, ["1", "2"].into_iter()).unwrap();
            let manifest =
                fs::read_to_string(resource_root.join(format!("{resource_id}.osa-shards.json")))
                    .unwrap();
            let file_name = format.data_file_name();
            assert!(manifest.contains(&format!("shards/chr1/{file_name}")));
            assert!(manifest.contains(&format!("shards/chr2/{file_name}")));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
