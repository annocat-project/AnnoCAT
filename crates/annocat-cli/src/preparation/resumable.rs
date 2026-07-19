use super::{PreparationIdentity, checkpoint::ShardPaths, transport::ReconnectingRangeReader};
use std::{
    fs,
    io::{Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub(super) struct PartProgress {
    pub persisted_bytes: u64,
    pub resumed_bytes: u64,
    pub expected_bytes: u64,
    pub elapsed: Duration,
    pub bytes_per_second: f64,
}

#[derive(Debug)]
pub(super) struct CompletePart {
    pub reader: fs::File,
    pub resumed_bytes: u64,
    pub downloaded_bytes: u64,
}

/// Finish one durable source part before exposing any bytes to fastVEP.
/// Interrupted downloads continue at the exact next byte; cache construction
/// reads the completed local part once and never replays a partial prefix.
#[allow(clippy::too_many_arguments)]
pub(super) fn acquire_range<F>(
    paths: &ShardPaths,
    identity: &PreparationIdentity,
    source_url: &str,
    range_start: u64,
    object_bytes: u64,
    expected_etag: Option<&str>,
    expected_last_modified: Option<&str>,
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<CompletePart, String>
where
    F: FnMut(PartProgress),
{
    let resumed = prepare_part(paths, identity)?;
    let expected = identity.expected_compressed_bytes;
    let started = Instant::now();
    let mut downloaded = 0_u64;

    progress(PartProgress {
        persisted_bytes: resumed,
        resumed_bytes: resumed,
        expected_bytes: expected,
        elapsed: Duration::ZERO,
        bytes_per_second: 0.0,
    });

    if resumed < expected {
        let absolute_start = range_start
            .checked_add(resumed)
            .ok_or("resumable range start overflow")?;
        let absolute_end = range_start
            .checked_add(expected)
            .and_then(|exclusive| exclusive.checked_sub(1))
            .ok_or("resumable range end overflow")?;
        if absolute_end >= object_bytes {
            return Err("resumable source range exceeds its source object".into());
        }

        let mut source = ReconnectingRangeReader::new(
            source_url,
            &identity.resource_id,
            &identity.chromosome,
            absolute_start,
            absolute_end,
            object_bytes,
            expected_etag,
            expected_last_modified,
        )?;
        let mut output = fs::OpenOptions::new()
            .append(true)
            .open(paths.source_part())
            .map_err(|error| format!("cannot append resumable source part: {error}"))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            if cancelled.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let read = source
                .read(&mut buffer)
                .map_err(|error| format!("cannot continue resumable source part: {error}"))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| format!("cannot persist resumable source part: {error}"))?;
            downloaded = downloaded.saturating_add(read as u64);
            let elapsed = started.elapsed();
            progress(PartProgress {
                persisted_bytes: resumed.saturating_add(downloaded),
                resumed_bytes: resumed,
                expected_bytes: expected,
                elapsed,
                bytes_per_second: if elapsed.is_zero() {
                    0.0
                } else {
                    downloaded as f64 / elapsed.as_secs_f64()
                },
            });
        }
        output
            .flush()
            .map_err(|error| format!("cannot flush resumable source part: {error}"))?;
    }

    let actual = fs::metadata(paths.source_part())
        .map(|metadata| metadata.len())
        .map_err(|error| format!("cannot inspect resumable source part: {error}"))?;
    if actual != expected {
        return Err(format!(
            "resumable source part is incomplete: retained {actual}, expected {expected}"
        ));
    }
    let reader = fs::File::open(paths.source_part())
        .map_err(|error| format!("cannot reopen completed source part: {error}"))?;
    Ok(CompletePart {
        reader,
        resumed_bytes: resumed,
        downloaded_bytes: downloaded,
    })
}

fn prepare_part(paths: &ShardPaths, identity: &PreparationIdentity) -> Result<u64, String> {
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
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.source_part())
        .map_err(|error| format!("cannot create resumable source part: {error}"))?;
    Ok(fs::metadata(paths.source_part())
        .map(|metadata| metadata.len())
        .unwrap_or(0))
}
