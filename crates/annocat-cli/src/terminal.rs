use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};

use crate::tasks::TaskSnapshot;

const PROGRESS_SCALE: u64 = 1_000;
static TERMINAL: OnceLock<TerminalUi> = OnceLock::new();

struct TerminalUi {
    progress: MultiProgress,
    bars: Mutex<HashMap<String, ProgressBar>>,
    bounded_style: ProgressStyle,
    spinner_style: ProgressStyle,
}

impl TerminalUi {
    fn new(target: ProgressDrawTarget) -> Self {
        Self {
            progress: MultiProgress::with_draw_target(target),
            bars: Mutex::new(HashMap::new()),
            bounded_style: ProgressStyle::with_template(
                "  {prefix:.bold} [{bar:24.62/245}] {percent_one:>5}%  {wide_msg}",
            )
            .expect("valid terminal progress template")
            .with_key(
                "percent_one",
                |state: &ProgressState, writer: &mut dyn std::fmt::Write| {
                    let _ = write!(writer, "{:.1}", state.fraction() * 100.0);
                },
            )
            .progress_chars("━╸─"),
            spinner_style: ProgressStyle::with_template(
                "  {spinner:.62} {prefix:.bold}  {wide_msg}",
            )
            .expect("valid terminal spinner template"),
        }
    }

    fn log(&self, component: &str, message: &str) {
        let line = format!(
            "{} [{}] {}",
            timestamp(),
            one_line(component),
            one_line(message)
        );
        self.progress.suspend(|| eprintln!("{line}"));
    }

    fn sync_tasks(&self, tasks: &[TaskSnapshot]) {
        let current_ids = tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<HashSet<_>>();
        let mut bars = self.bars.lock().unwrap_or_else(|error| error.into_inner());
        let removed = bars
            .keys()
            .filter(|id| !current_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(bar) = bars.remove(&id) {
                bar.finish_and_clear();
                let _ = self.progress.remove(&bar);
            }
        }

        for task in tasks {
            let active = task.is_active();
            let bar = if let Some(bar) = bars.get(&task.id) {
                bar.clone()
            } else if active {
                let bar = self.progress.add(ProgressBar::new(PROGRESS_SCALE));
                bars.insert(task.id.clone(), bar.clone());
                bar
            } else {
                continue;
            };
            if !active && matches!(task.state.as_str(), "completed" | "ready") {
                bar.finish_and_clear();
                let _ = self.progress.remove(&bar);
                bars.remove(&task.id);
                continue;
            }
            bar.set_prefix(task_title(&task.title));
            bar.set_message(task_message(task));
            if has_known_progress(task) {
                bar.set_style(self.bounded_style.clone());
                bar.set_position(progress_position(task.percent));
            } else {
                bar.set_style(self.spinner_style.clone());
                bar.tick();
            }
            if !active {
                bar.finish();
                bars.remove(&task.id);
            }
        }
    }
}

fn ui() -> &'static TerminalUi {
    TERMINAL.get_or_init(|| TerminalUi::new(ProgressDrawTarget::stderr_with_hz(4)))
}

pub(crate) fn log(component: &str, message: impl AsRef<str>) {
    ui().log(component, message.as_ref());
}

pub(crate) fn sync_tasks(tasks: &[TaskSnapshot]) {
    ui().sync_tasks(tasks);
}

fn has_known_progress(task: &TaskSnapshot) -> bool {
    task.percent.is_finite()
        && (task.percent > 0.0
            || task.total_bytes > 0
            || task.total_records > 0
            || task.total_chromosomes > 0)
}

fn progress_position(percent: f64) -> u64 {
    if !percent.is_finite() {
        return 0;
    }
    (percent.clamp(0.0, 100.0) * 10.0).round() as u64
}

fn task_message(task: &TaskSnapshot) -> String {
    if task.state == "queued" {
        return if task.detail.is_empty() || task.detail == task.phase {
            "Queued".into()
        } else {
            format!("Queued  |  {}", one_line(&task.detail))
        };
    }
    let mut parts = vec![task_activity(task).to_string()];
    if matches!(
        task.state.as_str(),
        "paused" | "cancelled" | "failed" | "interrupted"
    ) && !task.detail.is_empty()
        && task.detail != task.phase
    {
        parts.push(one_line(&task.detail));
    }
    if let Some(chromosome) = task_chromosome(task) {
        parts.push(chromosome);
    }
    if let Some(progress) = task_progress(task) {
        parts.push(progress);
    }
    if let Some(speed) = task_speed(task) {
        parts.push(speed);
    }
    if let Some(seconds) = task.eta_seconds.filter(|seconds| *seconds > 0) {
        parts.push(format!("ETA: {}", format_duration(seconds)));
    }
    parts.join("  |  ")
}

fn task_activity(task: &TaskSnapshot) -> &'static str {
    match task.state.as_str() {
        "queued" => "Queued",
        "validating" => "Verifying",
        "pausing" => "Pausing",
        "cancelling" => "Canceling",
        "paused" | "cancelled" => "Paused",
        "failed" => "Needs attention",
        "downloaded" => "Ready to install",
        "completed" | "ready" => "Completed",
        "running" => match task.phase.as_str() {
            "recovery-scan" => "Checking recovery data",
            "recovery-input" => "Preparing recovery data",
            "recovery-merge" => "Combining recovered data",
            "indexing-variants" | "indexing-evidence" => "Preparing result",
            "report-indexing" => "Preparing viewer columns",
            "reconnecting" | "retrying" => "Reconnecting",
            "replaying" => "Restoring data",
            "building-cache" => "Preparing cache",
            "downloading-source-part" | "downloading" => "Downloading",
            "streaming-to-fastvep" if task.kind == "annotation" => "Annotating",
            "streaming-to-fastvep" => "Preparing cache",
            "finalizing-annotation" => "Preparing result",
            "validating" => "Verifying",
            "reading-index" | "reading-indexes" => "Reading index",
            "publishing" => "Saving result",
            _ if task.kind == "download" => "Downloading",
            _ if task.kind == "installation" => "Installing",
            _ => "Annotating",
        },
        _ => "Working",
    }
}

fn task_chromosome(task: &TaskSnapshot) -> Option<String> {
    match (&task.chromosome, task.total_chromosomes) {
        (Some(chromosome), total) if total > 0 => Some(format!(
            "chr{} of {total}",
            chromosome.strip_prefix("chr").unwrap_or(chromosome)
        )),
        (Some(chromosome), _) => Some(format!(
            "chr{}",
            chromosome.strip_prefix("chr").unwrap_or(chromosome)
        )),
        (None, total) if total > 0 => Some(format!(
            "{} of {total} chr complete",
            task.completed_chromosomes
        )),
        _ => None,
    }
}

fn task_progress(task: &TaskSnapshot) -> Option<String> {
    if task.kind == "annotation" && task.phase.starts_with("indexing-") && task.completed_bytes > 0
    {
        Some(format!("Written: {}", format_size(task.completed_bytes)))
    } else if task.kind == "annotation" && task.total_records > 0 {
        Some(format!(
            "Variants: {} of {}",
            format_count(task.completed_records),
            format_count(task.total_records)
        ))
    } else if task.total_bytes > 0 {
        Some(format_size_pair(task.completed_bytes, task.total_bytes))
    } else if task.completed_bytes > 0 {
        Some(format_size(task.completed_bytes))
    } else {
        None
    }
}

fn task_speed(task: &TaskSnapshot) -> Option<String> {
    if task.throughput_records_per_second > 0.0 {
        Some(format!(
            "{:.0} variants/s",
            task.throughput_records_per_second
        ))
    } else {
        format_rate(task.throughput_bytes_per_second)
    }
}

fn format_rate(bytes_per_second: f64) -> Option<String> {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return None;
    }
    let (value, unit) = if bytes_per_second >= 1_000_000_000.0 {
        (bytes_per_second / 1_000_000_000.0, "GB/s")
    } else if bytes_per_second >= 1_000_000.0 {
        (bytes_per_second / 1_000_000.0, "MB/s")
    } else if bytes_per_second >= 1_000.0 {
        (bytes_per_second / 1_000.0, "KB/s")
    } else {
        (bytes_per_second, "B/s")
    };
    Some(format!("{value:.1} {unit}"))
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_000.0 && unit < UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_size_pair(completed: u64, total: u64) -> String {
    let (divisor, unit) = if total >= 1_000_000_000_000 {
        (1_000_000_000_000.0, "TB")
    } else if total >= 1_000_000_000 {
        (1_000_000_000.0, "GB")
    } else if total >= 1_000_000 {
        (1_000_000.0, "MB")
    } else if total >= 1_000 {
        (1_000.0, "KB")
    } else {
        return format!("{completed} of {total} B");
    };
    format!(
        "{:.1} of {:.1} {unit}",
        completed as f64 / divisor,
        total as f64 / divisor
    )
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let first = digits.len() % 3;
    let mut groups = Vec::new();
    if first > 0 {
        groups.push(&digits[..first]);
    }
    groups.extend(
        (first..digits.len())
            .step_by(3)
            .map(|index| &digits[index..index + 3]),
    );
    groups.join(",")
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
    }
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn task_title(value: &str) -> String {
    let value = one_line(value);
    if value.chars().count() <= 20 {
        return value;
    }
    let mut shortened = value.chars().take(17).collect::<String>();
    shortened.push_str("...");
    shortened
}

fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;

        let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut time) };
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month = months
            .get(time.wMonth.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("???");
        let hour = match time.wHour % 12 {
            0 => 12,
            value => value,
        };
        let period = if time.wHour < 12 { "AM" } else { "PM" };
        format!(
            "{month} {}, {} {hour}:{:02}:{:02} {period}",
            time.wDay, time.wYear, time.wMinute, time.wSecond
        )
    }
    #[cfg(not(windows))]
    {
        crate::annotation::current_timestamp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task() -> TaskSnapshot {
        let mut task = crate::tasks::from_completed_run("cadd", "CADD", "", "GRCh38", 1, 1);
        task.kind = "installation";
        task.state = "running".into();
        task.phase = "replaying".into();
        task.chromosome = Some("1".into());
        task.completed_chromosomes = 0;
        task.total_chromosomes = 24;
        task.completed_bytes = 500_000_000;
        task.total_bytes = 1_000_000_000;
        task.percent = 50.0;
        task.throughput_bytes_per_second = 12_300_000.0;
        task
    }

    #[test]
    fn task_text_preserves_existing_progress_semantics() {
        let task = sample_task();
        let message = task_message(&task);
        assert!(message.contains("Restoring data"));
        assert!(message.contains("chr1 of 24"));
        assert!(message.contains("0.5 of 1.0 GB"));
        assert!(message.contains("12.3 MB/s"));
        assert!(!message.contains("Data:"));
        assert!(!message.contains("Speed:"));
        assert_eq!(progress_position(task.percent), 500);
        assert_eq!(format_size(872_900_000), "872.9 MB");
    }

    #[test]
    fn progress_is_clamped_and_nonfinite_values_are_indeterminate() {
        assert_eq!(progress_position(-1.0), 0);
        assert_eq!(progress_position(50.05), 501);
        assert_eq!(progress_position(101.0), PROGRESS_SCALE);
        assert_eq!(progress_position(f64::NAN), 0);
        let mut task = sample_task();
        task.percent = 0.0;
        assert!(has_known_progress(&task));
        task.total_bytes = 0;
        task.total_records = 0;
        task.total_chromosomes = 0;
        assert!(!has_known_progress(&task));
        task.percent = f64::NAN;
        assert!(!has_known_progress(&task));
    }

    #[test]
    fn activity_labels_preserve_task_phase_meanings() {
        let mut task = sample_task();
        for (phase, expected) in [
            ("recovery-scan", "Checking recovery data"),
            ("recovery-input", "Preparing recovery data"),
            ("recovery-merge", "Combining recovered data"),
            ("indexing-variants", "Preparing result"),
            ("report-indexing", "Preparing viewer columns"),
            ("reconnecting", "Reconnecting"),
            ("replaying", "Restoring data"),
            ("building-cache", "Preparing cache"),
            ("downloading", "Downloading"),
            ("streaming-to-fastvep", "Preparing cache"),
            ("validating", "Verifying"),
            ("reading-index", "Reading index"),
            ("publishing", "Saving result"),
        ] {
            task.phase = phase.into();
            assert_eq!(task_activity(&task), expected);
            assert_eq!(progress_position(task.percent), 500);
        }
        task.kind = "annotation";
        task.phase = "streaming-to-fastvep".into();
        assert_eq!(task_activity(&task), "Annotating");
        task.state = "completed".into();
        assert_eq!(task_activity(&task), "Completed");
    }

    #[test]
    fn queued_tasks_show_position_without_empty_progress() {
        let mut task = sample_task();
        task.state = "queued".into();
        task.phase = "queued".into();
        task.detail = "Queue position 3".into();
        task.completed_bytes = 0;
        task.completed_chromosomes = 0;
        assert_eq!(task_message(&task), "Queued  |  Queue position 3");
    }

    #[test]
    fn hidden_renderer_reuses_and_removes_task_rows() {
        let terminal = TerminalUi::new(ProgressDrawTarget::hidden());
        let mut task = sample_task();
        terminal.sync_tasks(std::slice::from_ref(&task));
        assert_eq!(terminal.bars.lock().unwrap().len(), 1);
        terminal.sync_tasks(std::slice::from_ref(&task));
        assert_eq!(terminal.bars.lock().unwrap().len(), 1);
        task.state = "paused".into();
        task.phase = "paused".into();
        task.detail = "Downloaded data retained".into();
        terminal.sync_tasks(std::slice::from_ref(&task));
        assert!(terminal.bars.lock().unwrap().is_empty());
        assert!(task_message(&task).contains("Paused"));
        assert!(task_message(&task).contains("Downloaded data retained"));
        terminal.sync_tasks(&[]);
        assert!(terminal.bars.lock().unwrap().is_empty());
    }

    #[test]
    fn successful_tasks_clear_their_progress_rows() {
        let terminal = TerminalUi::new(ProgressDrawTarget::hidden());
        let mut task = sample_task();
        terminal.sync_tasks(std::slice::from_ref(&task));
        assert_eq!(terminal.bars.lock().unwrap().len(), 1);

        task.state = "completed".into();
        task.phase = "completed".into();
        terminal.sync_tasks(std::slice::from_ref(&task));
        assert!(terminal.bars.lock().unwrap().is_empty());
    }

    #[test]
    fn terminal_text_cannot_add_rows_or_control_sequences() {
        assert_eq!(one_line("one\r\ntwo\t\u{1b}[31m"), "one  two  [31m");
        assert_eq!(task_title("CADD"), "CADD");
        assert_eq!(
            task_title("A source name that is too long"),
            "A source name tha..."
        );
        assert_eq!(format_count(4_675_648), "4,675,648");
        assert_eq!(format_duration(3_661), "1h 1m");
    }
}
