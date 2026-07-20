use super::{
    StreamingBuildRequest, StreamingBuildResult, copy_bounded_with_progress, format_decimal_bytes,
    live_state, remove_incomplete_outputs,
};
use crate::preparation::cache::required_nonempty_file;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(super) struct RawFileInput {
    pub path: PathBuf,
    pub uncompressed_skip: u64,
}

fn command(
    request: &StreamingBuildRequest<'_>,
    inputs: &[&Path],
    skips: &[u64],
    chromosome: Option<&str>,
    stdin: Stdio,
) -> Result<Command, String> {
    if inputs.is_empty() || inputs.len() != skips.len() {
        return Err("fastVEP raw input paths and framing metadata are inconsistent".into());
    }
    let log = fs::File::create(request.log_path)
        .map_err(|error| format!("cannot create fastVEP preparation log: {error}"))?;
    let output_base = request.paths.partial_directory.join("source");
    let mut command = Command::new(request.fastvep_executable);
    command
        .arg("sa-build")
        .arg("--source")
        .arg(request.source_type)
        .arg("--input")
        .args(inputs)
        .arg("--output")
        .arg(output_base)
        .arg("--assembly")
        .arg(&request.identity.assembly)
        .arg("--no-progress")
        .stdin(stdin)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    super::configure_fastvep_parser_workers(&mut command);
    if skips.iter().any(|skip| *skip != 0) {
        command.arg("--input-skip").arg(
            skips
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    if let Some(chromosome) = chromosome {
        command.arg("--chromosome").arg(chromosome);
    }
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
    Ok(command)
}

fn completed_result(
    request: &StreamingBuildRequest<'_>,
    compressed_bytes_read: u64,
) -> Result<StreamingBuildResult, String> {
    let result = required_nonempty_file(&request.paths.partial_osa()).and_then(|osa_bytes| {
        required_nonempty_file(&request.paths.partial_index()).map(|index_bytes| {
            StreamingBuildResult {
                compressed_bytes_read,
                prepared_osa_bytes: osa_bytes,
                prepared_index_bytes: index_bytes,
            }
        })
    });
    if result.is_err() {
        remove_incomplete_outputs(request.paths);
    }
    result
}

fn wait_for_file_builder(
    request: &StreamingBuildRequest<'_>,
    child: &mut Child,
    label: &str,
    chromosome: &str,
    source_bytes: u64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut last_size = 0_u64;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for fastVEP preparation: {error}"))?
        {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(format!(
                    "fastVEP {label} preparation failed with status {status}"
                ));
            }
            None => {}
        }
        if last_report.elapsed() >= Duration::from_secs(1) {
            let size = fs::metadata(request.paths.partial_osa())
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let elapsed = last_report.elapsed().as_secs_f64();
            let throughput = if elapsed == 0.0 {
                0.0
            } else {
                size.saturating_sub(last_size) as f64 / elapsed
            };
            if let Ok(mut state) = live_state().lock() {
                state.phase = "building-cache".into();
                state.prepared_bytes = size;
                state.throughput_bytes_per_second = throughput;
                state.detail = format!(
                    "{label} chromosome {chromosome}: fastVEP is building the cache from {} of raw source data ({} written)",
                    format_decimal_bytes(source_bytes),
                    format_decimal_bytes(size),
                );
            }
            last_size = size;
            last_report = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(200));
        if started.elapsed() > Duration::from_secs(60 * 60 * 24) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("fastVEP cache build exceeded 24 hours".into());
        }
    }
}

/// Let fastVEP open, decode, filter, merge, and parse completed raw source
/// artifacts. AnnoCAT validates only byte counts and process/output lifecycle.
pub(super) fn build_from_files(
    request: &StreamingBuildRequest<'_>,
    inputs: &[RawFileInput],
    chromosome: Option<&str>,
    label: &str,
    cancelled: &AtomicBool,
) -> Result<StreamingBuildResult, String> {
    let mut total = 0_u64;
    for input in inputs {
        total = total.saturating_add(
            fs::metadata(&input.path)
                .map_err(|error| {
                    format!(
                        "cannot inspect raw source part {}: {error}",
                        input.path.display()
                    )
                })?
                .len(),
        );
    }
    if total != request.identity.expected_compressed_bytes {
        return Err(format!(
            "raw source parts contain {total} bytes instead of {}",
            request.identity.expected_compressed_bytes
        ));
    }
    let paths = inputs
        .iter()
        .map(|input| input.path.as_path())
        .collect::<Vec<_>>();
    let skips = inputs
        .iter()
        .map(|input| input.uncompressed_skip)
        .collect::<Vec<_>>();
    let mut child = command(request, &paths, &skips, chromosome, Stdio::null())?
        .spawn()
        .map_err(|error| format!("cannot start fastVEP {label} preparation: {error}"))?;
    let chromosome = chromosome.unwrap_or(&request.identity.chromosome);
    let outcome = wait_for_file_builder(request, &mut child, label, chromosome, total, cancelled);
    if let Err(error) = outcome {
        remove_incomplete_outputs(request.paths);
        return Err(error);
    }
    completed_result(request, total)
}

/// Forward one raw source stream unchanged. Decompression, tabix-prefix skip,
/// chromosome filtering, row validation, and cache construction happen only in
/// fastVEP.
pub(super) fn build_from_reader<R, F>(
    request: &StreamingBuildRequest<'_>,
    input: &mut R,
    uncompressed_skip: u64,
    chromosome: Option<&str>,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<StreamingBuildResult, String>
where
    R: Read,
    F: FnMut(u64, f64),
{
    let mut command = command(
        request,
        &[Path::new("-")],
        &[uncompressed_skip],
        chromosome,
        Stdio::piped(),
    )?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start fastVEP preparation: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
    let started = Instant::now();
    let copied = copy_bounded_with_progress(input, &mut stdin, cancelled, |bytes| {
        let elapsed = started.elapsed().as_secs_f64();
        progress(
            bytes,
            if elapsed == 0.0 {
                0.0
            } else {
                bytes as f64 / elapsed
            },
        );
    });
    drop(stdin);
    let copied = match copied {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            remove_incomplete_outputs(request.paths);
            return Err(error);
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(format!("fastVEP preparation failed with status {status}"));
    }
    if copied != request.identity.expected_compressed_bytes {
        remove_incomplete_outputs(request.paths);
        return Err(format!(
            "raw source stream ended after {copied} of {} expected bytes",
            request.identity.expected_compressed_bytes
        ));
    }
    completed_result(request, copied)
}
