use serde::Serialize;

use crate::{annotation, downloader, preparation, reference, transcript};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub state: String,
    pub phase: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chromosome: Option<String>,
    pub completed_chromosomes: u16,
    pub total_chromosomes: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_records: u64,
    pub total_records: u64,
    pub percent: f64,
    pub throughput_bytes_per_second: f64,
    pub throughput_records_per_second: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub available_actions: Vec<&'static str>,
}

impl TaskSnapshot {
    pub fn is_meaningful(&self) -> bool {
        !(matches!(self.state.as_str(), "idle" | "missing")
            || self.kind == "annotation" && self.state == "cancelled")
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "queued" | "running" | "validating" | "cancelling"
        )
    }
}

pub fn from_download(
    resource_id: &str,
    title: &str,
    status: downloader::DownloadStatus,
) -> TaskSnapshot {
    let detail = match status.state.as_str() {
        "queued" => status
            .queue_position
            .map(|position| format!("Waiting in the download queue (position {position})"))
            .unwrap_or_else(|| "Waiting in the download queue".into()),
        "downloaded" => "Download complete".into(),
        "paused" | "cancelled" if status.downloaded_bytes > 0 => "Downloaded data retained".into(),
        _ => status.error.clone().unwrap_or_else(|| status.phase.clone()),
    };
    TaskSnapshot {
        id: format!("download:{resource_id}"),
        kind: "download",
        title: title.into(),
        state: status.state.clone(),
        phase: status.phase,
        detail,
        resource_id: Some(resource_id.into()),
        chromosome: None,
        completed_chromosomes: 0,
        total_chromosomes: 0,
        run_id: None,
        updated_at: None,
        completed_bytes: status.downloaded_bytes,
        total_bytes: status.expected_bytes,
        completed_records: 0,
        total_records: 0,
        percent: status.percent,
        throughput_bytes_per_second: status.throughput_bytes_per_second,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        error: status.error,
        available_actions: resource_actions("download", &status.state),
    }
}

pub fn from_preparation(
    resource_id: &str,
    title: &str,
    status: preparation::LivePreparationState,
) -> TaskSnapshot {
    let total_chromosomes = status
        .completed_chromosomes
        .saturating_add(status.remaining_chromosomes);
    TaskSnapshot {
        id: format!("install:{resource_id}"),
        kind: "installation",
        title: title.into(),
        state: status.state.clone(),
        phase: status.phase,
        detail: status.detail,
        resource_id: Some(resource_id.into()),
        chromosome: status.chromosome,
        completed_chromosomes: status.completed_chromosomes,
        total_chromosomes,
        run_id: None,
        updated_at: None,
        completed_bytes: status.network_bytes,
        total_bytes: status.expected_network_bytes,
        completed_records: 0,
        total_records: 0,
        percent: status.percent,
        throughput_bytes_per_second: status.throughput_bytes_per_second,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        error: status.error,
        available_actions: resource_actions("installation", &status.state),
    }
}

pub fn from_reference(
    resource_id: &str,
    title: &str,
    status: reference::ReferenceStatus,
) -> TaskSnapshot {
    TaskSnapshot {
        id: format!("install:{resource_id}"),
        kind: "installation",
        title: title.into(),
        state: status.state.clone(),
        phase: status.phase,
        detail: status.detail,
        resource_id: Some(resource_id.into()),
        chromosome: None,
        completed_chromosomes: 0,
        total_chromosomes: 0,
        run_id: None,
        updated_at: None,
        completed_bytes: status.completed_bytes,
        total_bytes: status.total_bytes,
        completed_records: 0,
        total_records: 0,
        percent: status.percent,
        throughput_bytes_per_second: 0.0,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        error: status.error,
        available_actions: resource_actions("installation", &status.state),
    }
}

pub fn from_transcript(
    resource_id: &str,
    title: &str,
    status: transcript::TranscriptStatus,
) -> TaskSnapshot {
    TaskSnapshot {
        id: format!("install:{resource_id}"),
        kind: "installation",
        title: title.into(),
        state: status.state.into(),
        phase: status.phase.into(),
        detail: status.detail,
        resource_id: Some(resource_id.into()),
        chromosome: None,
        completed_chromosomes: 0,
        total_chromosomes: 0,
        run_id: None,
        updated_at: None,
        completed_bytes: 0,
        total_bytes: 0,
        completed_records: 0,
        total_records: 0,
        percent: if status.state == "ready" { 100.0 } else { 0.0 },
        throughput_bytes_per_second: 0.0,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        error: status.error,
        available_actions: resource_actions("installation", status.state),
    }
}

pub fn from_annotation(status: annotation::State) -> TaskSnapshot {
    let state = status.state.to_owned();
    TaskSnapshot {
        id: status
            .run_id
            .as_deref()
            .map(|run_id| format!("annotation:{run_id}"))
            .unwrap_or_else(|| "annotation:current".into()),
        kind: "annotation",
        title: status.name.clone().unwrap_or_else(|| "Annotation".into()),
        state: state.clone(),
        phase: status.phase.into(),
        detail: status.detail,
        resource_id: None,
        chromosome: status.chromosome,
        completed_chromosomes: 0,
        total_chromosomes: 0,
        run_id: status.run_id,
        updated_at: None,
        completed_bytes: status.output_bytes,
        total_bytes: status.total_bytes,
        completed_records: status.completed_records,
        total_records: status.total_records,
        percent: if state == "completed" {
            100.0
        } else {
            status.percent
        },
        throughput_bytes_per_second: status.throughput_bytes_per_second,
        throughput_records_per_second: status.throughput_records_per_second,
        eta_seconds: status.eta_seconds,
        error: status.error,
        available_actions: match state.as_str() {
            "running" if status.resumable => vec!["pause", "cancel"],
            "running" => vec!["cancel"],
            "interrupted" | "failed" if status.resumable => vec!["resume"],
            _ => Vec::new(),
        },
    }
}

pub fn from_completed_run(
    run_id: &str,
    title: &str,
    completed_at: &str,
    assembly: &str,
    variant_count: u64,
    result_bytes: u64,
) -> TaskSnapshot {
    TaskSnapshot {
        id: format!("run:{run_id}"),
        kind: "annotation",
        title: title.into(),
        state: "completed".into(),
        phase: "completed".into(),
        detail: format!("{assembly} · {variant_count} variants"),
        resource_id: None,
        chromosome: None,
        completed_chromosomes: 0,
        total_chromosomes: 0,
        run_id: Some(run_id.into()),
        updated_at: Some(completed_at.into()),
        completed_bytes: result_bytes,
        total_bytes: result_bytes,
        completed_records: variant_count,
        total_records: variant_count,
        percent: 100.0,
        throughput_bytes_per_second: 0.0,
        throughput_records_per_second: 0.0,
        eta_seconds: Some(0),
        error: None,
        available_actions: Vec::new(),
    }
}

pub fn choose_resource_task(
    download: TaskSnapshot,
    installation: TaskSnapshot,
) -> Option<TaskSnapshot> {
    if installation.is_active() {
        return Some(installation);
    }
    if download.is_active() {
        return Some(download);
    }
    if installation.is_meaningful() {
        return Some(installation);
    }
    download.is_meaningful().then_some(download)
}

fn resource_actions(kind: &str, state: &str) -> Vec<&'static str> {
    match state {
        "queued" | "running" | "validating" => vec!["pause", "cancel"],
        "paused" | "cancelled" | "failed" => vec!["resume", "cancel"],
        "downloaded" if kind == "download" => vec!["install", "cancel"],
        "ready" => vec!["remove"],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(state: &str) -> TaskSnapshot {
        from_download(
            "clinvar",
            "ClinVar",
            downloader::DownloadStatus {
                state: state.into(),
                phase: state.into(),
                downloaded_bytes: 20,
                expected_bytes: 100,
                percent: 20.0,
                throughput_bytes_per_second: 0.0,
                queue_position: None,
                error: None,
            },
        )
    }

    fn preparation(state: &str) -> TaskSnapshot {
        from_preparation(
            "clinvar",
            "ClinVar",
            preparation::LivePreparationState {
                resource_id: Some("clinvar".into()),
                state: state.into(),
                phase: state.into(),
                detail: state.into(),
                ..preparation::LivePreparationState::default()
            },
        )
    }

    #[test]
    fn active_download_takes_precedence_over_stale_install_failure() {
        assert_eq!(
            choose_resource_task(download("running"), preparation("failed"))
                .unwrap()
                .kind,
            "download"
        );
    }

    #[test]
    fn active_installation_takes_precedence_over_downloaded_archive() {
        assert_eq!(
            choose_resource_task(download("downloaded"), preparation("running"))
                .unwrap()
                .kind,
            "installation"
        );
    }

    #[test]
    fn idle_resource_is_not_a_task() {
        assert!(choose_resource_task(download("idle"), preparation("idle")).is_none());
    }

    #[test]
    fn failed_download_preserves_error_and_resume_action() {
        let task = from_download(
            "clinvar",
            "ClinVar",
            downloader::DownloadStatus {
                state: "failed".into(),
                phase: "failed".into(),
                downloaded_bytes: 20,
                expected_bytes: 100,
                percent: 20.0,
                throughput_bytes_per_second: 0.0,
                queue_position: None,
                error: Some("connection closed".into()),
            },
        );
        assert_eq!(task.error.as_deref(), Some("connection closed"));
        assert_eq!(task.available_actions, vec!["resume", "cancel"]);
    }

    #[test]
    fn download_throughput_is_forwarded_without_browser_recalculation() {
        let mut task = download("running");
        assert_eq!(task.throughput_bytes_per_second, 0.0);
        task = from_download(
            "clinvar",
            "ClinVar",
            downloader::DownloadStatus {
                state: "running".into(),
                phase: "downloading".into(),
                downloaded_bytes: 50,
                expected_bytes: 100,
                percent: 50.0,
                throughput_bytes_per_second: 12.5,
                queue_position: None,
                error: None,
            },
        );
        assert_eq!(task.throughput_bytes_per_second, 12.5);
        assert_eq!(task.available_actions, vec!["pause", "cancel"]);
    }

    #[test]
    fn annotation_progress_is_projected_into_the_shared_task() {
        let task = from_annotation(annotation::State {
            state: "running",
            phase: "annotating",
            detail: "Chromosome 22".into(),
            run_id: Some("run-test".into()),
            name: Some("Recovery".into()),
            input: None,
            output: None,
            records: Some(1_000),
            completed_records: 400,
            total_records: 1_000,
            output_bytes: 1_100_000_000,
            valid_output_bytes: 1_100_000_000,
            total_bytes: 2_000_000_000,
            chromosome: Some("22".into()),
            percent: 40.0,
            throughput_bytes_per_second: 1_100_000_000.0,
            throughput_records_per_second: 25_000.0,
            eta_seconds: Some(24),
            resumable: true,
            cancel_requested: false,
            error: None,
        });
        assert_eq!(task.chromosome.as_deref(), Some("22"));
        assert_eq!(task.completed_records, 400);
        assert_eq!(task.percent, 40.0);
        assert_eq!(task.throughput_bytes_per_second, 1_100_000_000.0);
        assert_eq!(task.throughput_records_per_second, 25_000.0);
        assert_eq!(task.eta_seconds, Some(24));
        assert_eq!(task.available_actions, vec!["pause", "cancel"]);
    }

    #[test]
    fn cancelled_annotation_is_removed_from_the_task_list() {
        let task = from_annotation(annotation::State {
            state: "cancelled",
            phase: "cancelled",
            detail: "Annotation cancelled; partial result discarded".into(),
            run_id: Some("run-test".into()),
            name: Some("Cancelled run".into()),
            input: None,
            output: None,
            records: None,
            completed_records: 0,
            total_records: 1_000,
            output_bytes: 0,
            valid_output_bytes: 0,
            total_bytes: 2_000_000_000,
            chromosome: None,
            percent: 0.0,
            throughput_bytes_per_second: 0.0,
            throughput_records_per_second: 0.0,
            eta_seconds: None,
            resumable: false,
            cancel_requested: false,
            error: None,
        });
        assert!(!task.is_meaningful());
        assert!(task.available_actions.is_empty());
    }
}
