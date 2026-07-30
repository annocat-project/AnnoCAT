use super::{
    StreamingBuildRequest, StreamingBuildResult, copy_bounded_with_progress, format_decimal_bytes,
    live_state, remove_incomplete_outputs,
};
use crate::preparation::cache::required_nonempty_file;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const FASTVEP_LOG_TAIL_BYTES: u64 = 64 * 1024;

fn fastvep_log_detail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(
        length.saturating_sub(FASTVEP_LOG_TAIL_BYTES),
    ))
    .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let tail = String::from_utf8_lossy(&bytes);
    tail.lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.eq_ignore_ascii_case("caused by:")
                && !line.starts_with("Building ")
        })
        .map(|line| line.trim_start_matches("Error: ").to_string())
}

fn source_is_corrupt(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "corrupt gzip",
        "corrupt bgzf",
        "gzip checksum",
        "matching checksum",
        "checksum mismatch",
        "crc mismatch",
        "crc validation",
        "invalid gzip",
        "invalid bgzf",
        "unexpected end of gzip",
        "unexpected end of bgzf",
        "truncated gzip",
        "truncated bgzf",
        "cannot inflate",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn part_identity_path(part: &Path) -> Option<PathBuf> {
    let parent = part.parent()?;
    if parent.file_name()?.to_str()? != "source-parts" {
        return None;
    }
    let name = part.file_name()?.to_str()?.strip_suffix(".part")?;
    Some(parent.join(format!("{name}.identity.json")))
}

/// Prefer fastVEP's parser/decompressor error to a secondary broken-pipe or
/// exit-status message. If that error proves a retained source part is corrupt,
/// discard only those current inputs so Resume downloads the chromosome again.
pub(super) fn recover_build_failure(
    log_path: &Path,
    fallback: String,
    retained_inputs: &[PathBuf],
) -> String {
    let read_fastvep_log = fallback.contains("fastVEP input failed")
        || fallback.contains("cannot stream REVEL to fastVEP")
        || (fallback.contains("fastVEP") && fallback.contains("status"));
    let detail = if read_fastvep_log {
        fastvep_log_detail(log_path).unwrap_or(fallback)
    } else {
        fallback
    };
    if !source_is_corrupt(&detail) {
        return detail;
    }
    let mut discarded = false;
    for part in retained_inputs {
        let Some(identity) = part_identity_path(part) else {
            continue;
        };
        discarded |= fs::remove_file(part).is_ok();
        let _ = fs::remove_file(identity);
    }
    if discarded {
        format!(
            "{detail}; the corrupt retained source part was discarded, so Resume will redownload this chromosome"
        )
    } else {
        detail
    }
}

/// A failed process may have been recorded before AnnoCAT learned how to clean
/// corrupt parts automatically. On the next Resume, remove that same older
/// part before the range downloader decides it is already complete.
pub(super) fn discard_parts_from_previous_corruption(
    log_path: &Path,
    base_part: &Path,
) -> Option<String> {
    let detail = fastvep_log_detail(log_path)?;
    if !source_is_corrupt(&detail) {
        return None;
    }
    let log_modified = fs::metadata(log_path).ok()?.modified().ok()?;
    let parent = base_part.parent()?;
    if parent.file_name()?.to_str()? != "source-parts" {
        return None;
    }
    let base_name = base_part.file_name()?.to_str()?.strip_suffix(".part")?;
    let variant_prefix = format!("{base_name}.");
    let mut discarded = false;
    for entry in fs::read_dir(parent).ok()?.filter_map(Result::ok) {
        let part = entry.path();
        let Some(name) = part.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name != format!("{base_name}.part")
            && !(name.starts_with(&variant_prefix) && name.ends_with(".part"))
        {
            continue;
        }
        let part_modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(_) => continue,
        };
        if part_modified > log_modified {
            continue;
        }
        let Some(identity) = part_identity_path(&part) else {
            continue;
        };
        discarded |= fs::remove_file(&part).is_ok();
        let _ = fs::remove_file(identity);
    }
    discarded.then_some(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "annocat-builder-{label}-{}-{nonce}",
            std::process::id(),
        ))
    }

    #[test]
    fn corruption_diagnostic_replaces_pipe_error_and_discards_only_source_parts() {
        let root = root("corrupt-part");
        let parts = root.join("source-parts");
        fs::create_dir_all(&parts).unwrap();
        let part = parts.join("2.part");
        let identity = parts.join("2.identity.json");
        let unrelated = root.join("input.vcf.gz");
        let log = root.join("fastvep.log");
        fs::write(&part, b"corrupt").unwrap();
        fs::write(&identity, b"identity").unwrap();
        fs::write(&unrelated, b"keep").unwrap();
        fs::write(
            &log,
            b"Error: Reading gnomAD VCF line\n\nCaused by:\n    corrupt gzip stream does not have a matching checksum\n",
        )
        .unwrap();

        let error = recover_build_failure(
            &log,
            "fastVEP input failed: pipe ended".into(),
            &[part.clone(), unrelated.clone()],
        );
        assert!(error.contains("corrupt gzip stream"));
        assert!(error.contains("Resume will redownload"));
        assert!(!part.exists());
        assert!(!identity.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parser_errors_do_not_discard_resumable_parts() {
        let root = root("parser-error");
        let parts = root.join("source-parts");
        fs::create_dir_all(&parts).unwrap();
        let part = parts.join("2.part");
        let identity = parts.join("2.identity.json");
        let log = root.join("fastvep.log");
        fs::write(&part, b"valid source").unwrap();
        fs::write(&identity, b"identity").unwrap();
        fs::write(&log, b"Error: VCF row has fewer than eight columns\n").unwrap();

        let error = recover_build_failure(
            &log,
            "fastVEP preparation failed with status exit code: 1".into(),
            std::slice::from_ref(&part),
        );
        assert_eq!(error, "VCF row has fewer than eight columns");
        assert!(part.exists());
        assert!(identity.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_discards_all_current_range_parts_proven_corrupt_by_older_log() {
        let root = root("previous-corruption");
        let parts = root.join("source-parts");
        let staging = root.join("staging").join("7.partial");
        fs::create_dir_all(&parts).unwrap();
        fs::create_dir_all(&staging).unwrap();
        for name in ["7.part", "7.snv.part", "7.indel.part"] {
            fs::write(parts.join(name), b"corrupt").unwrap();
            fs::write(
                parts.join(name.replace(".part", ".identity.json")),
                b"identity",
            )
            .unwrap();
        }
        let log = staging.join("fastvep.log");
        fs::write(
            &log,
            b"corrupt gzip stream does not have a matching checksum\n",
        )
        .unwrap();

        let detail = discard_parts_from_previous_corruption(&log, &parts.join("7.part"));
        assert!(detail.is_some());
        assert!(!parts.join("7.part").exists());
        assert!(!parts.join("7.snv.part").exists());
        assert!(!parts.join("7.indel.part").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

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
        .arg("--format")
        .arg(request.identity.cache_format()?.builder_argument())
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
    let format = request.identity.cache_format()?;
    let result =
        required_nonempty_file(&request.paths.partial_data(format)).and_then(|osa_bytes| {
            request
                .paths
                .partial_index(format)
                .map_or(Ok(0), |index| required_nonempty_file(&index))
                .map(|index_bytes| StreamingBuildResult {
                    compressed_bytes_read,
                    prepared_osa_bytes: osa_bytes,
                    prepared_index_bytes: index_bytes,
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
            let size = request
                .identity
                .cache_format()
                .ok()
                .and_then(|format| fs::metadata(request.paths.partial_data(format)).ok())
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
        return Err(recover_build_failure(
            request.log_path,
            error,
            &inputs
                .iter()
                .map(|input| input.path.clone())
                .collect::<Vec<_>>(),
        ));
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
            return Err(recover_build_failure(
                request.log_path,
                error,
                &[request.paths.source_part().to_path_buf()],
            ));
        }
    };
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for fastVEP preparation: {error}"))?;
    if !status.success() {
        remove_incomplete_outputs(request.paths);
        return Err(recover_build_failure(
            request.log_path,
            format!("fastVEP preparation failed with status {status}"),
            &[request.paths.source_part().to_path_buf()],
        ));
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
