use super::{
    SourceInputMode, StreamingProgress, format_decimal_bytes, live_state, source_input_mode,
};

struct SourceProgress<'a> {
    chromosome: &'a str,
    phase: &'a str,
    detail: String,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
}

fn update(progress: SourceProgress<'_>) {
    if let Ok(mut state) = live_state().lock() {
        state.chromosome = Some(progress.chromosome.into());
        state.phase = progress.phase.into();
        state.network_bytes = progress.network_bytes;
        state.expected_network_bytes = progress.expected_network_bytes;
        state.prepared_bytes = progress.prepared_bytes;
        state.throughput_bytes_per_second = progress.throughput;
        state.completed_chromosomes = progress.completed;
        state.remaining_chromosomes = progress.total.saturating_sub(progress.completed);
        state.percent = if progress.expected_network_bytes == 0 {
            0.0
        } else {
            (progress.network_bytes as f64 * 100.0 / progress.expected_network_bytes as f64)
                .min(99.9)
        };
        state.detail = progress.detail;
    }
}

pub(super) fn update_revel_progress(
    chromosome: &str,
    completed: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    update(SourceProgress {
        chromosome,
        phase: "streaming-zip-to-fastvep",
        detail: format!(
            "REVEL chromosome {chromosome}: validating and inflating official ZIP members"
        ),
        completed,
        total: 24,
        network_bytes,
        expected_network_bytes,
        prepared_bytes,
        throughput,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_indexed_progress(
    label: &str,
    chromosome: &str,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    update(SourceProgress {
        chromosome,
        phase: "streaming-ranges-to-fastvep",
        detail: format!("{label} chromosome {chromosome}: streaming indexed range"),
        completed,
        total,
        network_bytes,
        expected_network_bytes,
        prepared_bytes,
        throughput,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_sharded_progress(
    resource_id: &str,
    chromosome: &str,
    completed: u16,
    total: u16,
    network_bytes: u64,
    expected_network_bytes: u64,
    prepared_bytes: u64,
    throughput: f64,
) {
    update(SourceProgress {
        chromosome,
        phase: "streaming-to-fastvep",
        detail: format!(
            "{resource_id} chromosome {}: {} of {} source bytes",
            chromosome, network_bytes, expected_network_bytes
        ),
        completed,
        total,
        network_bytes,
        expected_network_bytes,
        prepared_bytes,
        throughput,
    });
}

pub(super) fn update_resumable_progress_detail(
    label: &str,
    chromosome: &str,
    progress: StreamingProgress,
) {
    if source_input_mode() != SourceInputMode::Resumable {
        return;
    }
    if progress.consumed_bytes == 0
        && progress.compressed_bytes_read < progress.expected_compressed_bytes
    {
        update_resumable_download_detail(
            label,
            chromosome,
            progress.compressed_bytes_read,
            progress.expected_compressed_bytes,
        );
    } else {
        update_local_build_progress_detail(
            label,
            chromosome,
            progress.consumed_bytes,
            progress.expected_compressed_bytes,
            progress.bytes_per_second,
        );
    }
}

pub(super) fn update_resumable_download_detail(
    label: &str,
    chromosome: &str,
    persisted: u64,
    expected: u64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.phase = "downloading-source-part".into();
        state.detail = format!(
            "{label} chromosome {chromosome}: saved {} of {} resumable source data",
            format_decimal_bytes(persisted),
            format_decimal_bytes(expected)
        );
    }
}

pub(super) fn update_local_build_detail(label: &str, chromosome: &str) {
    update_local_build_progress_detail(label, chromosome, 0, 0, 0.0);
}

pub(super) fn update_local_build_progress_detail(
    label: &str,
    chromosome: &str,
    consumed: u64,
    expected: u64,
    throughput: f64,
) {
    if let Ok(mut state) = live_state().lock() {
        state.phase = "building-cache".into();
        state.throughput_bytes_per_second = throughput;
        state.detail = if consumed > 0 && expected > 0 {
            format!(
                "{label} chromosome {chromosome}: building cache from {} of {} saved source data",
                format_decimal_bytes(consumed),
                format_decimal_bytes(expected)
            )
        } else {
            format!("{label} chromosome {chromosome}: building cache from completed source part")
        };
    }
}
