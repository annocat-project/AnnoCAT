use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static CANCEL: AtomicBool = AtomicBool::new(false);
static PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);
static BATCH_ACTIVE: AtomicBool = AtomicBool::new(false);
const PERFORMANCE_FILE: &str = "annotation-performance.json";
const PERFORMANCE_PROFILE_ENV: &str = "ANNOCAT_PROFILE_ANNOTATION";
const CHECKPOINT_SCHEMA_VERSION: u16 = 2;
const ANNOTATION_EXECUTION_CONTRACT: &str = "fastvep-projected-grch38-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    #[default]
    Annotation,
    VcfReview,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationRequest {
    pub input: PathBuf,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub include_annotated_vcf: bool,
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default)]
    pub add_local_consequences: bool,
    #[serde(default)]
    pub confirm_grch38: bool,
}

impl AnnotationRequest {
    fn uses_fastvep(&self) -> bool {
        self.run_mode == RunMode::Annotation || self.add_local_consequences
    }

    fn report_kind(&self) -> &'static str {
        match (self.run_mode, self.add_local_consequences) {
            (RunMode::VcfReview, false) => "vcf-only",
            (RunMode::VcfReview, true) => "core-consequences",
            (RunMode::Annotation, _) => "annotation",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAnnotationRequest {
    pub inputs: Vec<PathBuf>,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub include_annotated_vcf: bool,
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default)]
    pub add_local_consequences: bool,
    #[serde(default)]
    pub confirm_grch38: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRequest {
    pub input: PathBuf,
    pub partial_vcf: PathBuf,
    #[serde(default)]
    pub structured_output: Option<PathBuf>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub include_annotated_vcf: bool,
    #[serde(default)]
    pub confirm_grch38: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub state: &'static str,
    pub phase: &'static str,
    pub detail: String,
    pub run_id: Option<String>,
    pub name: Option<String>,
    pub input: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub records: Option<u64>,
    pub completed_records: u64,
    pub total_records: u64,
    pub output_bytes: u64,
    pub valid_output_bytes: u64,
    pub total_bytes: u64,
    pub chromosome: Option<String>,
    pub percent: f64,
    pub throughput_bytes_per_second: f64,
    pub throughput_records_per_second: f64,
    pub eta_seconds: Option<u64>,
    pub resumable: bool,
    pub cancel_requested: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceBinding {
    resource_id: String,
    release: String,
    assembly: String,
    selected_schema: String,
    cache_format: String,
    osa_schema_version: u16,
    cache_builder_contract: String,
    chromosomes: Vec<String>,
}

#[derive(Clone, Default)]
struct RecoveryIdentity {
    input_content_sha256: Option<String>,
    execution_contract_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagePerformance {
    stage: String,
    wall_time_ms: f64,
    records: u64,
    input_bytes: u64,
    output_bytes: u64,
    records_per_second: f64,
    input_mib_per_second: f64,
    output_mib_per_second: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_time_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_cpu_cores: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_read_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_write_bytes: Option<u64>,
}

#[derive(Debug)]
struct PipelinePerformance {
    run_id: String,
    diagnostic_profiling: bool,
    stages: Vec<StagePerformance>,
}

impl PipelinePerformance {
    fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.into(),
            diagnostic_profiling: diagnostic_profiling_enabled(),
            stages: Vec::new(),
        }
    }

    fn record(&mut self, stage: StagePerformance) {
        let process_usage = stage.cpu_time_ms.map_or_else(String::new, |cpu_time_ms| {
            format!(
                ", CPU {cpu_time_ms:.1} ms ({:.2} average cores), process I/O {} read / {} written",
                stage.average_cpu_cores.unwrap_or(0.0),
                stage.process_read_bytes.unwrap_or(0),
                stage.process_write_bytes.unwrap_or(0)
            )
        });
        crate::terminal_log(
            "performance",
            format!(
                "{} {}: {:.1} ms, {:.1} records/s, {:.1} MiB/s output{}",
                self.run_id,
                stage.stage,
                stage.wall_time_ms,
                stage.records_per_second,
                stage.output_mib_per_second,
                process_usage
            ),
        );
        self.stages.push(stage);
    }

    fn persist(&self, directory: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "runId": self.run_id,
            "generatedAt": current_timestamp(),
            "diagnosticProfiling": self.diagnostic_profiling,
            "metricSemantics": {
                "wallTime": "Elapsed wall-clock time measured by AnnoCAT",
                "processIo": "Process I/O transfer counters; bytes may be served by the operating-system cache",
                "averageCpuCores": "CPU time divided by wall time; values can exceed 1 for parallel work"
            },
            "stages": self.stages
        }))
        .map_err(|error| format!("cannot serialize annotation performance data: {error}"))?;
        super::library_metadata::atomic_write(&directory.join(PERFORMANCE_FILE), &bytes)
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessUsageSnapshot {
    cpu_100ns: u64,
    read_bytes: u64,
    write_bytes: u64,
}

struct StageMeasurement {
    started_at: Instant,
    usage: Option<ProcessUsageSnapshot>,
}

impl StageMeasurement {
    fn current(diagnostic_profiling: bool) -> Self {
        Self {
            started_at: Instant::now(),
            usage: diagnostic_profiling.then(current_process_usage).flatten(),
        }
    }

    fn child(started_at: Instant, diagnostic_profiling: bool, child: &std::process::Child) -> Self {
        Self {
            started_at,
            usage: diagnostic_profiling
                .then(|| child_process_usage(child))
                .flatten(),
        }
    }

    fn finish_current(
        self,
        stage: &str,
        records: u64,
        input_bytes: u64,
        output_bytes: u64,
    ) -> StagePerformance {
        let end_usage = self.usage.and_then(|_| current_process_usage());
        self.finish(stage, records, input_bytes, output_bytes, end_usage)
    }

    fn finish_child(
        self,
        stage: &str,
        records: u64,
        input_bytes: u64,
        output_bytes: u64,
        child: &std::process::Child,
    ) -> StagePerformance {
        let end_usage = self.usage.and_then(|_| child_process_usage(child));
        self.finish(stage, records, input_bytes, output_bytes, end_usage)
    }

    fn finish(
        self,
        stage: &str,
        records: u64,
        input_bytes: u64,
        output_bytes: u64,
        end_usage: Option<ProcessUsageSnapshot>,
    ) -> StagePerformance {
        let elapsed = self.started_at.elapsed();
        let seconds = elapsed.as_secs_f64();
        let usage_delta = self.usage.zip(end_usage).map(|(start, end)| {
            (
                end.cpu_100ns.saturating_sub(start.cpu_100ns),
                end.read_bytes.saturating_sub(start.read_bytes),
                end.write_bytes.saturating_sub(start.write_bytes),
            )
        });
        let cpu_time_ms = usage_delta.map(|(cpu_100ns, _, _)| cpu_100ns as f64 / 10_000.0);
        StagePerformance {
            stage: stage.into(),
            wall_time_ms: seconds * 1_000.0,
            records,
            input_bytes,
            output_bytes,
            records_per_second: rate(records, seconds),
            input_mib_per_second: mib_rate(input_bytes, seconds),
            output_mib_per_second: mib_rate(output_bytes, seconds),
            cpu_time_ms,
            average_cpu_cores: cpu_time_ms
                .filter(|_| seconds > 0.0)
                .map(|milliseconds| milliseconds / 1_000.0 / seconds),
            process_read_bytes: usage_delta.map(|(_, read_bytes, _)| read_bytes),
            process_write_bytes: usage_delta.map(|(_, _, write_bytes)| write_bytes),
        }
    }
}

fn rate(value: u64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        value as f64 / seconds
    } else {
        0.0
    }
}

fn mib_rate(bytes: u64, seconds: f64) -> f64 {
    rate(bytes, seconds) / (1024.0 * 1024.0)
}

fn diagnostic_profiling_enabled() -> bool {
    std::env::var(PERFORMANCE_PROFILE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(windows)]
fn current_process_usage() -> Option<ProcessUsageSnapshot> {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    process_usage(unsafe { GetCurrentProcess() })
}

#[cfg(not(windows))]
fn current_process_usage() -> Option<ProcessUsageSnapshot> {
    None
}

#[cfg(windows)]
fn child_process_usage(child: &std::process::Child) -> Option<ProcessUsageSnapshot> {
    use std::os::windows::io::AsRawHandle;

    process_usage(child.as_raw_handle())
}

#[cfg(not(windows))]
fn child_process_usage(_child: &std::process::Child) -> Option<ProcessUsageSnapshot> {
    None
}

#[cfg(windows)]
fn process_usage(handle: std::os::windows::io::RawHandle) -> Option<ProcessUsageSnapshot> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{
        GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
    };

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let mut io = IO_COUNTERS::default();
    let has_times =
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0 };
    let has_io = unsafe { GetProcessIoCounters(handle, &mut io) != 0 };
    if !has_times || !has_io {
        return None;
    }
    Some(ProcessUsageSnapshot {
        cpu_100ns: filetime_100ns(kernel).saturating_add(filetime_100ns(user)),
        read_bytes: io.ReadTransferCount,
        write_bytes: io.WriteTransferCount,
    })
}

#[cfg(windows)]
fn filetime_100ns(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShardManifest {
    schema_version: u16,
    shards: Vec<ShardEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShardEntry {
    chromosome: String,
    file: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationCheckpoint {
    schema_version: u16,
    run_id: String,
    name: String,
    input: PathBuf,
    #[serde(default)]
    requested_profile: Option<String>,
    source_ids: Vec<String>,
    include_annotated_vcf: bool,
    #[serde(default)]
    run_mode: RunMode,
    #[serde(default)]
    add_local_consequences: bool,
    #[serde(default)]
    confirm_grch38: bool,
    #[serde(default)]
    input_content_sha256: Option<String>,
    #[serde(default)]
    execution_contract_sha256: Option<String>,
    state: String,
    phase: String,
    detail: String,
    completed_records: u64,
    total_records: u64,
    output_bytes: u64,
    valid_output_bytes: u64,
    total_bytes: u64,
    chromosome: Option<String>,
    percent: f64,
    throughput_bytes_per_second: f64,
    throughput_records_per_second: f64,
    eta_seconds: Option<u64>,
    updated_at: String,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(idle_state()))
}

fn recovery_identity() -> &'static Mutex<RecoveryIdentity> {
    static IDENTITY: OnceLock<Mutex<RecoveryIdentity>> = OnceLock::new();
    IDENTITY.get_or_init(|| Mutex::new(RecoveryIdentity::default()))
}

fn idle_state() -> State {
    State {
        state: "idle",
        phase: "waiting",
        detail: "No annotation run is active".into(),
        run_id: None,
        name: None,
        input: None,
        output: None,
        records: None,
        completed_records: 0,
        total_records: 0,
        output_bytes: 0,
        valid_output_bytes: 0,
        total_bytes: 0,
        chromosome: None,
        percent: 0.0,
        throughput_bytes_per_second: 0.0,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        resumable: false,
        cancel_requested: false,
        error: None,
    }
}

pub fn status() -> State {
    state()
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| idle_state())
}

pub fn interrupted_runs(runs: &Path) -> Vec<State> {
    let Ok(entries) = fs::read_dir(runs) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            read_checkpoint(&entry.path()).map(|checkpoint| (entry.path(), checkpoint))
        })
        .filter(|(_, checkpoint)| !matches!(checkpoint.state.as_str(), "completed" | "cancelled"))
        .map(|(directory, checkpoint)| {
            let resumable = recovery_outputs_exist(&directory);
            State {
                state: if resumable { "interrupted" } else { "failed" },
                phase: if resumable { "interrupted" } else { "failed" },
                detail: if resumable {
                    format!("Interrupted during {} · Resume available", checkpoint.phase)
                } else {
                    format!(
                        "Interrupted during {} before recoverable output was written",
                        checkpoint.phase
                    )
                },
                run_id: Some(checkpoint.run_id),
                name: Some(checkpoint.name),
                input: None,
                output: Some(directory),
                records: Some(checkpoint.total_records),
                completed_records: checkpoint.completed_records,
                total_records: checkpoint.total_records,
                output_bytes: checkpoint.output_bytes,
                valid_output_bytes: checkpoint.valid_output_bytes,
                total_bytes: checkpoint.total_bytes,
                chromosome: checkpoint.chromosome,
                percent: checkpoint.percent,
                throughput_bytes_per_second: 0.0,
                throughput_records_per_second: 0.0,
                eta_seconds: None,
                resumable,
                cancel_requested: false,
                error: None,
            }
        })
        .collect()
}

pub fn is_running() -> bool {
    state().lock().is_ok_and(|value| value.state == "running")
}

pub fn cancel() -> bool {
    if !is_running() && !BATCH_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    BATCH_ACTIVE.store(false, Ordering::SeqCst);
    PAUSE_REQUESTED.store(false, Ordering::SeqCst);
    CANCEL.store(true, Ordering::SeqCst);
    if let Ok(mut current) = state().lock() {
        current.cancel_requested = true;
        current.detail = "Stopping fastVEP and discarding partial output".into();
    }
    true
}

pub fn pause() -> bool {
    if !is_running() && !BATCH_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    BATCH_ACTIVE.store(false, Ordering::SeqCst);
    PAUSE_REQUESTED.store(true, Ordering::SeqCst);
    CANCEL.store(true, Ordering::SeqCst);
    if let Ok(mut current) = state().lock() {
        current.cancel_requested = true;
        current.detail = "Pausing fastVEP and retaining partial output".into();
    }
    true
}

pub fn start_background(
    request: AnnotationRequest,
    default_runs: PathBuf,
    resources: PathBuf,
) -> Result<String, String> {
    start_background_inner(request, default_runs, resources, false)
}

pub fn start_recovery_background(
    request: RecoveryRequest,
    default_runs: PathBuf,
    resources: PathBuf,
) -> Result<String, String> {
    if is_running() || BATCH_ACTIVE.load(Ordering::SeqCst) {
        return Err("an annotation run or batch is already active".into());
    }
    let structured_output = recovery_structured_path(&request);
    let recovery_name = request
        .name
        .clone()
        .or_else(|| input_display_name(&request.partial_vcf));
    let mut annotation = AnnotationRequest {
        input: request.input,
        requested_profile: None,
        name: recovery_name,
        output_directory: request.output_directory,
        source_ids: request.source_ids,
        include_annotated_vcf: request.include_annotated_vcf,
        run_mode: RunMode::Annotation,
        add_local_consequences: false,
        confirm_grch38: request.confirm_grch38,
    };
    validate_request(&annotation, &resources)?;
    validate_recovery_files(&request.partial_vcf, &structured_output)?;
    annotation
        .source_ids
        .sort_by_key(|source_id| source_order(source_id));
    let name = display_name(&annotation);
    let run_id = new_run_id(&annotation.input);
    let output_root = annotation.output_directory.clone().unwrap_or(default_runs);
    fs::create_dir_all(&output_root).map_err(|error| {
        format!(
            "cannot create output directory {}: {error}",
            output_root.display()
        )
    })?;
    let directory_name = format!("{}--{}", safe_name(&name), &run_id[4..]);
    let final_directory = output_root.join(&directory_name);
    let staging_directory = output_root.join(format!("{directory_name}.partial"));
    if final_directory.exists() || staging_directory.exists() {
        return Err("a run with the generated identifier already exists".into());
    }
    begin_run_state(
        &annotation,
        &name,
        &run_id,
        &final_directory,
        "recovery-scan",
        "Scanning the interrupted fastVEP output",
    )?;
    CANCEL.store(false, Ordering::SeqCst);
    PAUSE_REQUESTED.store(false, Ordering::SeqCst);
    let returned_id = run_id.clone();
    std::thread::spawn(move || {
        let result = execute_recovery(
            &annotation,
            &resources,
            &name,
            &run_id,
            &staging_directory,
            &final_directory,
            &request.partial_vcf,
            &structured_output,
            None,
        );
        finish_background(result, &run_id, &staging_directory, &final_directory, true);
    });
    Ok(returned_id)
}

pub fn resume_background(
    run_id: &str,
    runs: PathBuf,
    resources: PathBuf,
) -> Result<String, String> {
    if is_running() || BATCH_ACTIVE.load(Ordering::SeqCst) {
        return Err("an annotation run or batch is already active".into());
    }
    let (directory, checkpoint) = find_checkpoint(&runs, run_id)
        .ok_or("the interrupted annotation checkpoint is unavailable")?;
    let staging = if directory.extension().and_then(|value| value.to_str()) == Some("failed") {
        let partial = directory.with_extension("partial");
        if partial.exists() {
            return Err("the interrupted annotation staging directory already exists".into());
        }
        fs::rename(&directory, &partial)
            .map_err(|error| format!("cannot reopen interrupted annotation: {error}"))?;
        partial
    } else {
        directory
    };
    let final_directory = staging.with_extension("");
    if final_directory.exists() {
        return Err("the completed output directory already exists".into());
    }
    let expected_recovery_identity = checkpoint
        .input_content_sha256
        .clone()
        .zip(checkpoint.execution_contract_sha256.clone());
    let annotation = AnnotationRequest {
        input: checkpoint.input,
        requested_profile: checkpoint.requested_profile,
        name: Some(checkpoint.name.clone()),
        output_directory: final_directory.parent().map(Path::to_path_buf),
        source_ids: checkpoint.source_ids,
        include_annotated_vcf: checkpoint.include_annotated_vcf,
        run_mode: checkpoint.run_mode,
        add_local_consequences: checkpoint.add_local_consequences,
        confirm_grch38: checkpoint.confirm_grch38,
    };
    validate_request(&annotation, &resources)?;
    let partial_vcf = staging.join("annotated.vcf");
    let structured_output = staging.join("fastvep.ndjson");
    let recover_partial_output =
        recoverable_outputs_exist(checkpoint.schema_version, &partial_vcf, &structured_output);
    begin_run_state(
        &annotation,
        &checkpoint.name,
        run_id,
        &final_directory,
        "recovery-scan",
        "Scanning the interrupted fastVEP output",
    )?;
    if let Some((input_sha256, execution_sha256)) = expected_recovery_identity.as_ref() {
        store_recovery_identity(input_sha256.clone(), execution_sha256.clone())?;
    }
    CANCEL.store(false, Ordering::SeqCst);
    PAUSE_REQUESTED.store(false, Ordering::SeqCst);
    let returned_id = run_id.to_owned();
    let thread_run_id = returned_id.clone();
    std::thread::spawn(move || {
        let result = if recover_partial_output {
            execute_recovery(
                &annotation,
                &resources,
                &checkpoint.name,
                &thread_run_id,
                &staging,
                &final_directory,
                &partial_vcf,
                &structured_output,
                expected_recovery_identity,
            )
        } else {
            let _ = fs::remove_dir_all(&staging);
            execute(
                &annotation,
                &resources,
                &checkpoint.name,
                &thread_run_id,
                &staging,
                &final_directory,
            )
        };
        finish_background(result, &thread_run_id, &staging, &final_directory, true);
    });
    Ok(returned_id)
}

pub fn discard_interrupted_run(runs: &Path, run_id: &str) -> Result<(), String> {
    if run_id.trim().is_empty() {
        return Err("annotation run ID is required".into());
    }
    let current = status();
    if (is_running() || BATCH_ACTIVE.load(Ordering::SeqCst))
        && current.run_id.as_deref() == Some(run_id)
    {
        return Err("the annotation is still active; cancel it before deleting its output".into());
    }
    let (directory, _) = find_checkpoint(runs, run_id)
        .ok_or("the interrupted annotation checkpoint is unavailable")?;
    fs::remove_dir_all(&directory).map_err(|error| {
        format!(
            "cannot delete interrupted annotation {}: {error}",
            directory.display()
        )
    })
}

pub fn start_batch_background(
    request: BatchAnnotationRequest,
    default_runs: PathBuf,
    resources: PathBuf,
) -> Result<String, String> {
    if request.inputs.is_empty() {
        return Err("choose at least one input VCF".into());
    }
    if request.inputs.len() > 100 {
        return Err("a sequential batch is limited to 100 VCF files".into());
    }
    if is_running()
        || BATCH_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return Err("an annotation run or batch is already active".into());
    }
    let requests = request
        .inputs
        .into_iter()
        .map(|input| AnnotationRequest {
            input,
            requested_profile: request.requested_profile.clone(),
            name: None,
            output_directory: request.output_directory.clone(),
            source_ids: request.source_ids.clone(),
            include_annotated_vcf: request.include_annotated_vcf,
            run_mode: request.run_mode,
            add_local_consequences: request.add_local_consequences,
            confirm_grch38: request.confirm_grch38,
        })
        .collect::<Vec<_>>();
    if let Err(error) = requests
        .iter()
        .try_for_each(|item| validate_request(item, &resources))
    {
        BATCH_ACTIVE.store(false, Ordering::SeqCst);
        return Err(error);
    }
    let batch_id = format!("batch-{}", &new_run_id(&requests[0].input)[4..]);
    let returned_id = batch_id.clone();
    std::thread::spawn(move || {
        let total = requests.len();
        for (index, request) in requests.into_iter().enumerate() {
            if !BATCH_ACTIVE.load(Ordering::SeqCst) {
                break;
            }
            let run_id = match start_background_inner(
                request,
                default_runs.clone(),
                resources.clone(),
                true,
            ) {
                Ok(run_id) => run_id,
                Err(error) => {
                    if let Ok(mut current) = state().lock() {
                        current.state = "failed";
                        current.phase = "failed";
                        current.detail = format!(
                            "Sequential batch stopped before file {} of {total}",
                            index + 1
                        );
                        current.error = Some(error);
                    }
                    break;
                }
            };
            loop {
                std::thread::sleep(Duration::from_millis(150));
                let current = state()
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_else(|_| idle_state());
                if current.run_id.as_deref() != Some(&run_id) {
                    break;
                }
                match current.state {
                    "completed" => break,
                    "failed" | "cancelled" => {
                        BATCH_ACTIVE.store(false, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
        }
        BATCH_ACTIVE.store(false, Ordering::SeqCst);
    });
    Ok(returned_id)
}

fn start_background_inner(
    mut request: AnnotationRequest,
    default_runs: PathBuf,
    resources: PathBuf,
    from_batch: bool,
) -> Result<String, String> {
    if !from_batch && BATCH_ACTIVE.load(Ordering::SeqCst) {
        return Err("a sequential annotation batch is already active".into());
    }
    validate_request(&request, &resources)?;
    request
        .source_ids
        .sort_by_key(|source_id| source_order(source_id));
    let name = display_name(&request);
    let run_id = new_run_id(&request.input);
    let output_root = request.output_directory.clone().unwrap_or(default_runs);
    fs::create_dir_all(&output_root).map_err(|error| {
        format!(
            "cannot create output directory {}: {error}",
            output_root.display()
        )
    })?;
    let directory_name = format!("{}--{}", safe_name(&name), &run_id[4..]);
    let final_directory = output_root.join(&directory_name);
    let staging_directory = output_root.join(format!("{directory_name}.partial"));
    if final_directory.exists() || staging_directory.exists() {
        return Err("a run with the generated identifier already exists".into());
    }
    begin_run_state(
        &request,
        &name,
        &run_id,
        &final_directory,
        "queued",
        "Validating the input VCF",
    )?;
    CANCEL.store(false, Ordering::SeqCst);
    PAUSE_REQUESTED.store(false, Ordering::SeqCst);
    crate::terminal_log(
        "annotation",
        format!(
            "{run_id} queued: input={} mode={} sources={}",
            request.input.display(),
            request.report_kind(),
            if request.source_ids.is_empty() {
                if request.uses_fastvep() {
                    "core".into()
                } else {
                    "none".into()
                }
            } else {
                request.source_ids.join(",")
            }
        ),
    );
    let allow_resume = request.uses_fastvep();
    let returned_id = run_id.clone();
    std::thread::spawn(move || {
        let result = execute(
            &request,
            &resources,
            &name,
            &run_id,
            &staging_directory,
            &final_directory,
        );
        finish_background(
            result,
            &run_id,
            &staging_directory,
            &final_directory,
            allow_resume,
        );
    });
    Ok(returned_id)
}

pub fn run_blocking(
    request: AnnotationRequest,
    default_runs: PathBuf,
    resources: PathBuf,
) -> Result<PathBuf, String> {
    let run_id = start_background(request, default_runs, resources)?;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let current = state()
            .lock()
            .map_err(|_| "annotation state lock failed")?
            .clone();
        if current.run_id.as_deref() != Some(&run_id) {
            return Err("annotation state was replaced unexpectedly".into());
        }
        match current.state {
            "completed" => {
                return current
                    .output
                    .ok_or("AnnoCAT result has no output path".into());
            }
            "failed" => return Err(current.error.unwrap_or(current.detail)),
            "cancelled" => return Err("annotation was cancelled".into()),
            _ => {}
        }
    }
}

pub(crate) fn validate_request(
    request: &AnnotationRequest,
    resources: &Path,
) -> Result<(), String> {
    if !request.input.is_file() {
        return Err(format!("input VCF is missing: {}", request.input.display()));
    }
    validate_source_ids(&request.source_ids)?;
    let input = annocat_core::vcf::inspect_header(&request.input)?;
    if !input.has_records {
        return Err("the input VCF contains no variant records".into());
    }
    super::annotation_input::validate_declared_assembly(input.assembly.as_deref())?;
    if input.assembly.is_none() && !request.confirm_grch38 {
        return Err(
            "the VCF header does not identify its genome build; confirm that it uses GRCh38".into(),
        );
    }
    if request.run_mode == RunMode::VcfReview {
        if !request.source_ids.is_empty() {
            return Err("VCF review cannot select supplemental annotation sources".into());
        }
        if request.include_annotated_vcf && !request.add_local_consequences {
            return Err(
                "an annotated VCF is unavailable when local consequences are disabled".into(),
            );
        }
        if !request.add_local_consequences {
            return Ok(());
        }
    }
    let engine = super::fastvep::readiness();
    if !engine.ready {
        return Err(format!("fastVEP is not ready: {}", engine.next_action));
    }
    if !super::reference::is_ready(resources) {
        return Err("GRCh38 reference is not ready".into());
    }
    if !super::transcript::is_ready(resources) {
        return Err("matching Ensembl transcript cache is not ready".into());
    }
    for source_id in &request.source_ids {
        resolve_source_root(resources, source_id)?;
    }
    Ok(())
}

fn begin_run_state(
    request: &AnnotationRequest,
    name: &str,
    run_id: &str,
    final_directory: &Path,
    phase: &'static str,
    detail: &str,
) -> Result<(), String> {
    *recovery_identity()
        .lock()
        .map_err(|_| "annotation recovery identity lock failed")? = RecoveryIdentity::default();
    let mut current = state().lock().map_err(|_| "annotation state lock failed")?;
    if current.state == "running" {
        return Err("an annotation run is already active".into());
    }
    *current = State {
        state: "running",
        phase,
        detail: detail.into(),
        run_id: Some(run_id.into()),
        name: Some(name.into()),
        input: Some(request.input.clone()),
        output: Some(final_directory.to_path_buf()),
        records: None,
        completed_records: 0,
        total_records: 0,
        output_bytes: 0,
        valid_output_bytes: 0,
        total_bytes: 0,
        chromosome: None,
        percent: 0.0,
        throughput_bytes_per_second: 0.0,
        throughput_records_per_second: 0.0,
        eta_seconds: None,
        resumable: request.uses_fastvep(),
        cancel_requested: false,
        error: None,
    };
    Ok(())
}

fn finish_background(
    result: Result<(u64, u64), String>,
    run_id: &str,
    staging: &Path,
    final_directory: &Path,
    allow_resume: bool,
) {
    if result.as_ref().is_err_and(|error| error != "cancelled") && staging.exists() {
        let failed = staging.with_extension("failed");
        if failed.exists() {
            let _ = fs::remove_dir_all(&failed);
        }
        let _ = fs::rename(staging, failed);
    }
    let Ok(mut current) = state().lock() else {
        return;
    };
    match result {
        Ok((records, bytes)) => {
            crate::terminal_log(
                "annotation",
                format!("{run_id} completed: {records} variants, {bytes} bytes"),
            );
            current.state = "completed";
            current.phase = "completed";
            current.detail = format!("Published {records} canonical variants");
            current.records = Some(records);
            current.completed_records = records;
            current.total_records = records;
            current.output_bytes = bytes;
            current.percent = 100.0;
            current.throughput_bytes_per_second = 0.0;
            current.throughput_records_per_second = 0.0;
            current.eta_seconds = Some(0);
            current.resumable = false;
            current.output = Some(final_directory.to_path_buf());
            current.cancel_requested = false;
        }
        Err(error) if error == "cancelled" => {
            let paused = PAUSE_REQUESTED.swap(false, Ordering::SeqCst) && allow_resume;
            CANCEL.store(false, Ordering::SeqCst);
            if paused && staging.join("annotation-state.json").is_file() {
                crate::terminal_log("annotation", format!("{run_id} paused"));
                current.state = "interrupted";
                current.phase = "interrupted";
                current.detail = if recovery_outputs_exist(staging) {
                    "Annotation paused; partial output retained for recovery".into()
                } else {
                    "Annotation paused; Resume will restart this run".into()
                };
                current.resumable = true;
            } else {
                crate::terminal_log("annotation", format!("{run_id} cancelled and deleted"));
                let _ = fs::remove_dir_all(staging);
                let _ = fs::remove_dir_all(staging.with_extension("failed"));
                current.state = "cancelled";
                current.phase = "cancelled";
                current.detail = "Annotation cancelled; partial result discarded".into();
                current.resumable = false;
                current.output = None;
                current.percent = 0.0;
            }
            current.cancel_requested = false;
            current.error = None;
        }
        Err(error) => {
            crate::terminal_log("annotation", format!("{run_id} failed: {error}"));
            current.state = "failed";
            current.phase = "failed";
            current.detail = "Annotation did not produce a publishable result".into();
            current.resumable = allow_resume
                && [staging.to_path_buf(), staging.with_extension("failed")]
                    .iter()
                    .any(|directory| recovery_outputs_exist(directory));
            current.cancel_requested = false;
            current.error = Some(error);
        }
    }
}

fn execute(
    request: &AnnotationRequest,
    resources: &Path,
    name: &str,
    run_id: &str,
    staging: &Path,
    final_directory: &Path,
) -> Result<(u64, u64), String> {
    if !request.uses_fastvep() {
        return execute_vcf_review(request, name, run_id, staging, final_directory);
    }
    fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    persist_current_checkpoint(staging, request, name, run_id)?;
    check_cancel(staging)?;
    if let Ok(mut current) = state().lock() {
        current.total_bytes = fs::metadata(&request.input)
            .map(|value| value.len())
            .unwrap_or(0);
    }
    persist_current_checkpoint(staging, request, name, run_id)?;
    let output = staging.join("annotated.vcf");
    let structured_output = staging.join("fastvep.ndjson");
    let mut performance = PipelinePerformance::new(run_id);
    let (source_bindings, input_summary, fastvep_performance) = run_fastvep(
        request,
        resources,
        name,
        run_id,
        staging,
        &request.input,
        &output,
        &structured_output,
        0,
        0,
        0,
        None,
        performance.diagnostic_profiling,
    )?;
    performance.record(fastvep_performance);
    if input_summary.records == 0 {
        return fail_staging(staging, "input VCF contains no variant records".into());
    }
    if input_summary.skipped_non_variant_records > 0 {
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} excluded {} reference-only records before fastVEP",
                input_summary.skipped_non_variant_records
            ),
        );
    }
    if let Ok(mut current) = state().lock() {
        current.records = Some(input_summary.records);
        current.total_records = input_summary.records;
    }
    finalize_outputs(
        FinalizeContext {
            request,
            name,
            run_id,
            staging,
            resources,
            final_directory,
            input_summary: &input_summary,
            output: &output,
            structured_output: &structured_output,
        },
        source_bindings,
        performance,
    )
}

fn execute_vcf_review(
    request: &AnnotationRequest,
    name: &str,
    run_id: &str,
    staging: &Path,
    final_directory: &Path,
) -> Result<(u64, u64), String> {
    fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    persist_current_checkpoint(staging, request, name, run_id)?;
    if CANCEL.load(Ordering::SeqCst) {
        let _ = fs::remove_dir_all(staging);
        return Err("cancelled".into());
    }
    if let Ok(mut current) = state().lock() {
        current.phase = "indexing-variants";
        current.detail = "Reading VCF records".into();
        current.total_bytes = fs::metadata(&request.input)
            .map(|value| value.len())
            .unwrap_or(0);
        current.resumable = false;
    }

    let parquet = staging.join("variants.parquet");
    let canonical = match super::results::convert_input_vcf(
        &request.input,
        &parquet,
        || CANCEL.load(Ordering::SeqCst),
        |records, _writing, output_bytes, bytes_per_second, records_per_second| {
            if let Ok(mut current) = state().lock() {
                current.phase = "indexing-variants";
                current.detail = "Building the local VCF review table".into();
                current.completed_records = records;
                current.output_bytes = output_bytes;
                current.throughput_bytes_per_second = bytes_per_second;
                current.throughput_records_per_second = records_per_second;
            }
        },
    ) {
        Ok(summary) => summary,
        Err(error) if error == "cancelled" => {
            let _ = fs::remove_dir_all(staging);
            return Err(error);
        }
        Err(error) => {
            return fail_staging(staging, format!("VCF review conversion failed: {error}"));
        }
    };

    if let Ok(mut current) = state().lock() {
        current.records = Some(canonical.rows);
        current.completed_records = canonical.records;
        current.total_records = canonical.records;
        current.percent = 85.0;
    }
    set_phase("indexing-evidence", "Preparing result fields");
    let consequences = staging.join("consequences.parquet");
    let evidence = staging.join("evidence.parquet");
    let field_catalog = staging.join("field-catalog.json");
    if let Err(error) =
        super::results::write_empty_detail_tables(&consequences, &evidence, &field_catalog)
    {
        return fail_staging(staging, error);
    }

    set_phase("publishing", "Verifying the AnnoCAT result");
    if let Err(error) = super::results::validate_report_tables_allow_empty_consequences(
        &parquet,
        &consequences,
        &evidence,
        &field_catalog,
        canonical.rows,
    ) {
        return fail_staging(staging, format!("VCF review validation failed: {error}"));
    }

    prepare_report_indexes(run_id, &parquet, &consequences, &evidence)?;
    check_cancel(staging)?;
    set_phase("publishing", "Saving the AnnoCAT result");

    let result_bytes = file_bytes(&parquet)?;
    let consequences_bytes = file_bytes(&consequences)?;
    let evidence_bytes = file_bytes(&evidence)?;
    let field_catalog_bytes = file_bytes(&field_catalog)?;
    let canonical_result_bytes = result_bytes
        .checked_add(consequences_bytes)
        .and_then(|bytes| bytes.checked_add(evidence_bytes))
        .and_then(|bytes| bytes.checked_add(field_catalog_bytes))
        .ok_or("AnnoCAT result size overflow")?;
    let assembly_source = if request.confirm_grch38 {
        "user-confirmed"
    } else {
        "vcf-header"
    };
    let input_name = request
        .input
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("input.vcf");
    let input_content_sha256 = canonical
        .input_content_sha256
        .as_deref()
        .ok_or("VCF review input identity was not recorded")?;
    let mut manifest = serde_json::json!({
        "schemaVersion": 1,
        "canonicalSchemaVersion": super::results::SCHEMA_VERSION,
        "state": "completed",
        "reportKind": request.report_kind(),
        "runId": run_id,
        "name": name,
        "completedAt": current_timestamp(),
        "assembly": "GRCh38",
        "assemblySource": assembly_source,
        "variantCount": canonical.rows,
        "vcfRecordCount": canonical.records,
        "excludedAuxiliaryRecordCount": canonical.excluded_auxiliary_records,
        "alleleCount": canonical.rows,
        "sampleNames": canonical.samples,
        "consequenceCount": 0,
        "evidenceValueCount": 0,
        "dynamicFieldCount": 0,
        "resultBytes": result_bytes,
        "consequencesBytes": consequences_bytes,
        "evidenceBytes": evidence_bytes,
        "fieldCatalogBytes": field_catalog_bytes,
        "canonicalResultBytes": canonical_result_bytes,
        "resultFile": "variants.parquet",
        "consequencesFile": "consequences.parquet",
        "evidenceFile": "evidence.parquet",
        "fieldCatalogFile": "field-catalog.json",
        "input": input_name,
        "inputName": input_name,
        "inputBytes": file_bytes(&request.input)?,
        "requestedProfile": request.requested_profile.as_deref(),
        "annotationSelection": "vcf-review",
        "includeAnnotatedVcf": false,
        "resultSha256": super::fastvep::sha256_file(&parquet)?,
        "consequencesSha256": super::fastvep::sha256_file(&consequences)?,
        "evidenceSha256": super::fastvep::sha256_file(&evidence)?,
        "fieldCatalogSha256": super::fastvep::sha256_file(&field_catalog)?,
        "sourceIds": [],
        "sources": [],
        "observedSourceIds": [],
        "sourcesWithoutObservedEvidence": [],
        "inputIdentityPreserved": true,
    });
    let object = manifest
        .as_object_mut()
        .ok_or("completed manifest is not an object")?;
    object.insert(
        "representativeSelectionContract".into(),
        super::results::REPRESENTATIVE_SELECTION_CONTRACT.into(),
    );
    object.insert("inputContentSha256".into(), input_content_sha256.into());
    let _ = fs::remove_file(staging.join("annotation-state.json"));
    let _ = fs::remove_file(staging.join("annotation-state.json.tmp"));
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(staging, final_directory)
        .map_err(|error| format!("cannot publish completed VCF review: {error}"))?;
    Ok((canonical.rows, result_bytes))
}

#[allow(clippy::too_many_arguments)]
fn execute_recovery(
    request: &AnnotationRequest,
    resources: &Path,
    name: &str,
    run_id: &str,
    staging: &Path,
    final_directory: &Path,
    partial_vcf: &Path,
    partial_structured: &Path,
    expected_recovery_identity: Option<(String, String)>,
) -> Result<(u64, u64), String> {
    fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    persist_current_checkpoint(staging, request, name, run_id)?;
    let mut performance = PipelinePerformance::new(run_id);

    set_phase("recovery-scan", "Checking the recovered annotated VCF");
    let mut recovered_vcf = crate::annotation_recovery::scan_vcf(partial_vcf, |progress| {
        update_recovery_progress(
            "recovery-scan",
            "Checking annotated VCF",
            &progress,
            0.0,
            5.0,
        )
    })?;
    let discarded_tail_bytes = recovered_vcf
        .total_bytes
        .saturating_sub(recovered_vcf.valid_bytes);
    if discarded_tail_bytes > 0 {
        crate::terminal_log(
            "annotation",
            format!("{run_id} recovery ignored {discarded_tail_bytes} incomplete trailing bytes"),
        );
    }
    check_cancel(staging)?;

    set_phase("recovery-scan", "Checking the recovered annotation data");
    let mut recovered_structured =
        crate::annotation_recovery::scan_ndjson(partial_structured, |progress| {
            update_recovery_progress(
                "recovery-scan",
                "Checking annotation data",
                &progress,
                5.0,
                5.0,
            )
        })?;
    if recovered_vcf.records != recovered_structured.records {
        let common_records = recovered_vcf.records.min(recovered_structured.records);
        recovered_vcf.records = common_records;
        recovered_vcf.valid_bytes =
            crate::annotation_recovery::vcf_prefix_bytes(partial_vcf, common_records)?;
        recovered_vcf.identity_sha256 =
            crate::annotation_recovery::vcf_prefix_identity_sha256(partial_vcf, common_records)?;
        recovered_structured.records = common_records;
        recovered_structured.valid_bytes =
            crate::annotation_recovery::ndjson_prefix_bytes(partial_structured, common_records)?;
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} recovery aligned VCF and structured output at {common_records} complete records"
            ),
        );
    }
    check_cancel(staging)?;

    set_phase(
        "recovery-input",
        "Checking the original VCF and preparing the remaining variants",
    );
    let remaining_input = staging.join("remaining-input.vcf");
    let projected = crate::annotation_input::stream_variants_after(
        &request.input,
        File::create(&remaining_input)
            .map_err(|error| format!("cannot create remaining recovery input: {error}"))?,
        recovered_vcf.records,
        || CANCEL.load(Ordering::SeqCst),
        update_recovery_input_progress,
    )?;
    let input_content_sha256 = projected.content_sha256.clone();
    let input_summary = projected.summary;
    if input_summary.records == 0 {
        return Err("original input VCF contains no variant records".into());
    }
    if projected.skipped_identity_sha256 != recovered_vcf.identity_sha256 {
        set_phase("validating", "Locating the first recovery mismatch");
        let mismatch = crate::annotation_recovery::first_vcf_identity_mismatch(
            &request.input,
            partial_vcf,
            recovered_vcf.records,
            |records, chromosome, records_per_second| {
                update_recovery_comparison_progress(
                    records,
                    input_summary.records,
                    chromosome,
                    records_per_second,
                )
            },
        )?;
        if let Some(mismatch) = mismatch {
            let detail = {
                if mismatch.input_chromosome == mismatch.output_chromosome {
                    format!(
                        "the first difference is at record {} on chromosome {}",
                        mismatch.record, mismatch.input_chromosome
                    )
                } else {
                    format!(
                        "the first difference is at record {} (input chromosome {}, annotated chromosome {})",
                        mismatch.record, mismatch.input_chromosome, mismatch.output_chromosome
                    )
                }
            };
            return Err(format!(
                "interrupted output does not match the selected original VCF: {detail}; no files were changed"
            ));
        }
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} direct ordered comparison confirmed that the interrupted annotated VCF matches the selected original"
            ),
        );
    }
    let prepared_provider = compose_provider_set(resources, run_id, &request.source_ids)?;
    let current_execution_contract = execution_contract_sha256(
        request,
        resources,
        &prepared_provider.1,
        &input_content_sha256,
    )?;
    if let Some((expected_input, expected_execution)) = expected_recovery_identity
        && (expected_input != input_content_sha256
            || expected_execution != current_execution_contract)
    {
        if let Some(directory) = prepared_provider.0.as_ref() {
            let _ = fs::remove_dir_all(directory);
        }
        return Err(
            "the input or annotation contract changed after this run was interrupted; restart the annotation from the original VCF"
                .into(),
        );
    }
    store_recovery_identity(input_content_sha256, current_execution_contract)?;
    if let Ok(mut current) = state().lock() {
        current.records = Some(input_summary.records);
        current.completed_records = recovered_vcf.records;
        current.total_records = input_summary.records;
        current.output_bytes = recovered_vcf.valid_bytes;
        current.valid_output_bytes = recovered_vcf.valid_bytes;
        current.total_bytes = fs::metadata(&request.input)
            .map(|value| value.len())
            .unwrap_or(0);
        current.chromosome = recovered_vcf.chromosome.clone();
        current.percent = recovered_vcf.records as f64 * 100.0 / input_summary.records as f64;
        current.detail = annotation_progress_detail(&current);
    }
    persist_current_checkpoint(staging, request, name, run_id)?;

    let remaining_records = projected.written_records;
    let continuation_vcf = staging.join("continuation.vcf");
    let continuation_structured = staging.join("continuation.ndjson");
    let source_bindings = if remaining_records > 0 {
        check_cancel(staging)?;
        let (bindings, continuation_summary, fastvep_performance) = run_fastvep(
            request,
            resources,
            name,
            run_id,
            staging,
            &remaining_input,
            &continuation_vcf,
            &continuation_structured,
            recovered_vcf.records,
            recovered_vcf.valid_bytes,
            input_summary.records,
            Some(prepared_provider),
            performance.diagnostic_profiling,
        )?;
        performance.record(fastvep_performance);
        if continuation_summary.records != remaining_records {
            return Err(format!(
                "recovery streamed {} remaining variants but expected {remaining_records}",
                continuation_summary.records
            ));
        }
        bindings
    } else {
        let (provider_directory, bindings) = prepared_provider;
        if let Some(directory) = provider_directory {
            let _ = fs::remove_dir_all(directory);
        }
        bindings
    };

    let output = staging.join("annotated.vcf");
    let structured_output = staging.join("fastvep.ndjson");
    let complete_outputs = remaining_records == 0
        && recovered_vcf.valid_bytes == recovered_vcf.total_bytes
        && recovered_structured.valid_bytes == recovered_structured.total_bytes;
    if !complete_outputs {
        set_phase(
            "recovery-merge",
            "Combining recovered and new annotation data",
        );
        let merged_vcf = staging.join("annotated.recovered.vcf");
        let merged_structured = staging.join("fastvep.recovered.ndjson");
        crate::annotation_recovery::copy_prefix(
            partial_vcf,
            recovered_vcf.valid_bytes,
            &merged_vcf,
            |progress| {
                update_recovery_progress(
                    "recovery-merge",
                    "Combining annotated VCF data",
                    &progress,
                    0.0,
                    50.0,
                )
            },
        )?;
        if remaining_records > 0 {
            crate::annotation_recovery::append_vcf_records(&continuation_vcf, &merged_vcf)?;
        }
        crate::annotation_recovery::copy_prefix(
            partial_structured,
            recovered_structured.valid_bytes,
            &merged_structured,
            |progress| {
                update_recovery_progress(
                    "recovery-merge",
                    "Joining structured output",
                    &progress,
                    50.0,
                    50.0,
                )
            },
        )?;
        if remaining_records > 0 {
            crate::annotation_recovery::append_file(&continuation_structured, &merged_structured)?;
        }
        replace_file(&merged_vcf, &output)?;
        replace_file(&merged_structured, &structured_output)?;
    }
    let _ = fs::remove_file(&remaining_input);
    let _ = fs::remove_file(&continuation_vcf);
    let _ = fs::remove_file(&continuation_structured);
    check_cancel(staging)?;
    finalize_outputs(
        FinalizeContext {
            request,
            name,
            run_id,
            staging,
            resources,
            final_directory,
            input_summary: &input_summary,
            output: &output,
            structured_output: &structured_output,
        },
        source_bindings,
        performance,
    )
}

fn execution_contract_sha256(
    request: &AnnotationRequest,
    resources: &Path,
    source_bindings: &[SourceBinding],
    input_content_sha256: &str,
) -> Result<String, String> {
    let readiness = super::fastvep::readiness();
    let reference_manifest = super::reference::fasta_path(resources)
        .parent()
        .ok_or("GRCh38 reference path has no parent directory")?
        .join("resource-manifest.json");
    let transcript_manifest = resources.join("transcript-cache").join("manifest.json");
    let payload = serde_json::json!({
        "contract": ANNOTATION_EXECUTION_CONTRACT,
        "inputContentSha256": input_content_sha256,
        "requestedProfile": request.requested_profile.as_deref(),
        "runMode": request.run_mode,
        "addLocalConsequences": request.add_local_consequences,
        "confirmGrch38": request.confirm_grch38,
        "sources": source_bindings,
        "fastvep": {
            "version": readiness.version,
            "sha256": readiness.sha256.unwrap_or_else(|| readiness.expected_sha256.into()),
        },
        "referenceManifestSha256": super::fastvep::sha256_file(&reference_manifest)?,
        "transcriptManifestSha256": super::fastvep::sha256_file(&transcript_manifest)?,
        "representativeSelectionContract": super::results::REPRESENTATIVE_SELECTION_CONTRACT,
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&payload).map_err(|error| error.to_string())?)
    ))
}

fn store_recovery_identity(
    input_content_sha256: String,
    execution_contract_sha256: String,
) -> Result<(), String> {
    *recovery_identity()
        .lock()
        .map_err(|_| "annotation recovery identity lock failed")? = RecoveryIdentity {
        input_content_sha256: Some(input_content_sha256),
        execution_contract_sha256: Some(execution_contract_sha256),
    };
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_fastvep(
    request: &AnnotationRequest,
    resources: &Path,
    name: &str,
    run_id: &str,
    staging: &Path,
    input: &Path,
    output: &Path,
    structured_output: &Path,
    baseline_records: u64,
    baseline_bytes: u64,
    total_records: u64,
    prepared_provider: Option<(Option<PathBuf>, Vec<SourceBinding>)>,
    diagnostic_profiling: bool,
) -> Result<
    (
        Vec<SourceBinding>,
        annocat_core::vcf::VcfSummary,
        StagePerformance,
    ),
    String,
> {
    let executable = super::fastvep::readiness()
        .executable
        .ok_or("fastVEP executable disappeared")?;
    let stdout =
        File::create(staging.join("fastvep.stdout.log")).map_err(|error| error.to_string())?;
    let stderr =
        File::create(staging.join("fastvep.stderr.log")).map_err(|error| error.to_string())?;
    let (provider_directory, source_bindings) = match prepared_provider {
        Some(provider) => provider,
        None => compose_provider_set(resources, run_id, &request.source_ids)?,
    };
    set_phase("annotating", "fastVEP is annotating variants");
    let fastvep_started_at = Instant::now();
    let mut command = Command::new(executable);
    command
        .arg("annotate")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(output)
        .arg("--output-format")
        .arg("vcf")
        .arg("--structured-output")
        .arg(structured_output)
        .arg("--fasta")
        .arg(super::reference::fasta_path(resources))
        .arg("--transcript-cache")
        .arg(super::transcript::cache_path(resources))
        .args([
            "--symbol",
            "--hgvs",
            "--canonical",
            "--no-progress",
            "--buffer-size",
            "4096",
        ]);
    if let Some(directory) = provider_directory.as_ref() {
        command.arg("--sa-dir").arg(directory);
    }
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn();
    drop(command);
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            if let Some(directory) = provider_directory.as_ref() {
                let _ = fs::remove_dir_all(directory);
            }
            return Err(format!("cannot start fastVEP annotation: {error}"));
        }
    };
    let fastvep_measurement =
        StageMeasurement::child(fastvep_started_at, diagnostic_profiling, &child);
    let stdin = child.stdin.take().ok_or("fastVEP stdin was unavailable")?;
    let input_path = input.to_path_buf();
    let feeder_request = request.clone();
    let feeder_resources = resources.to_path_buf();
    let feeder_bindings = source_bindings.clone();
    let feeder_staging = staging.to_path_buf();
    let feeder_name = name.to_owned();
    let feeder_run_id = run_id.to_owned();
    let feeder = std::thread::spawn(move || {
        let projected = crate::annotation_input::stream_variants(
            &input_path,
            stdin,
            || CANCEL.load(Ordering::SeqCst),
            update_annotation_input_progress,
        )?;
        let execution_contract_sha256 = execution_contract_sha256(
            &feeder_request,
            &feeder_resources,
            &feeder_bindings,
            &projected.content_sha256,
        )?;
        store_recovery_identity(projected.content_sha256.clone(), execution_contract_sha256)?;
        persist_current_checkpoint(
            &feeder_staging,
            &feeder_request,
            &feeder_name,
            &feeder_run_id,
        )?;
        Ok::<_, String>(projected)
    });
    let mut progress = crate::annotation_progress::VcfTail::default();
    let mut last_checkpoint = Instant::now();
    let status = loop {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(directory) = provider_directory.as_ref() {
                let _ = fs::remove_dir_all(directory);
            }
            discard_cancelled_staging(staging);
            let _ = feeder.join();
            return Err("cancelled".into());
        }
        if output.is_file()
            && let Ok(snapshot) = progress.update(output)
            && let Ok(mut current) = state().lock()
        {
            current.completed_records = baseline_records.saturating_add(snapshot.records);
            current.valid_output_bytes = baseline_bytes.saturating_add(snapshot.valid_output_bytes);
            current.chromosome = snapshot.chromosome;
            if total_records > 0 {
                current.output_bytes = baseline_bytes.saturating_add(snapshot.output_bytes);
                current.percent = (current.completed_records as f64 * 100.0 / total_records as f64)
                    .clamp(0.0, 100.0);
                current.throughput_bytes_per_second = snapshot.bytes_per_second;
            }
            current.throughput_records_per_second = snapshot.records_per_second;
            current.eta_seconds = (total_records > current.completed_records
                && snapshot.records_per_second > 0.0)
                .then(|| {
                    ((total_records - current.completed_records) as f64
                        / snapshot.records_per_second)
                        .ceil() as u64
                });
            if total_records > 0 && current.completed_records >= total_records {
                current.phase = "finalizing-annotation";
                current.chromosome = None;
                current.throughput_bytes_per_second = 0.0;
                current.throughput_records_per_second = 0.0;
                current.eta_seconds = None;
                current.detail = "fastVEP is preparing the result".into();
            } else {
                current.detail = annotation_progress_detail(&current);
            }
        }
        if last_checkpoint.elapsed() >= Duration::from_secs(1) {
            let _ = persist_current_checkpoint(staging, request, name, run_id);
            last_checkpoint = Instant::now();
        }
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for fastVEP: {error}"))?
        {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(150)),
        }
    };
    if status.success() {
        set_phase("verifying", "Preparing fastVEP output checks");
    }
    if let Some(directory) = provider_directory.as_ref() {
        let _ = fs::remove_dir_all(directory);
    }
    let input_summary = feeder
        .join()
        .map_err(|_| "fastVEP input streamer panicked".to_string())?;
    if !status.success() {
        return fail_staging(
            staging,
            format!("fastVEP annotation exited with {status}; see fastvep.stderr.log"),
        );
    }
    let input_summary = input_summary?.summary;
    let input_bytes = fs::metadata(input).map(|value| value.len()).unwrap_or(0);
    let output_bytes = fs::metadata(output)
        .map(|value| value.len())
        .unwrap_or(0)
        .saturating_add(
            fs::metadata(structured_output)
                .map(|value| value.len())
                .unwrap_or(0),
        );
    let performance = fastvep_measurement.finish_child(
        "fastvep",
        input_summary.records,
        input_bytes,
        output_bytes,
        &child,
    );
    Ok((source_bindings, input_summary, performance))
}

struct FinalizeContext<'a> {
    request: &'a AnnotationRequest,
    name: &'a str,
    run_id: &'a str,
    staging: &'a Path,
    resources: &'a Path,
    final_directory: &'a Path,
    input_summary: &'a annocat_core::vcf::VcfSummary,
    output: &'a Path,
    structured_output: &'a Path,
}

fn finalize_outputs(
    context: FinalizeContext<'_>,
    source_bindings: Vec<SourceBinding>,
    mut performance: PipelinePerformance,
) -> Result<(u64, u64), String> {
    let FinalizeContext {
        request,
        name,
        run_id,
        staging,
        resources,
        final_directory,
        input_summary,
        output,
        structured_output,
    } = context;
    let annotated_vcf_bytes = file_bytes(output)?;
    let verification_measurement = StageMeasurement::current(performance.diagnostic_profiling);
    set_phase(
        "verifying",
        "Checking record counts and the dynamic CSQ schema",
    );
    let output_summary = super::csq::inspect(output)
        .map_err(|error| format!("fastVEP output validation failed: {error}"))?;
    if output_summary.records != input_summary.records {
        return fail_staging(
            staging,
            format!(
                "fastVEP output record count changed from {} to {}",
                input_summary.records, output_summary.records
            ),
        );
    }
    if output_summary.identity_sha256 != input_summary.identity_sha256 {
        set_phase("validating", "Comparing original and annotated VCF records");
        let mismatch = crate::annotation_recovery::first_vcf_identity_mismatch(
            &request.input,
            output,
            input_summary.records,
            |records, chromosome, records_per_second| {
                update_recovery_comparison_progress(
                    records,
                    input_summary.records,
                    chromosome,
                    records_per_second,
                )
            },
        )?;
        if mismatch.is_some() {
            return fail_staging(
                staging,
                "fastVEP output does not preserve the complete ordered input variant set".into(),
            );
        }
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} direct ordered comparison confirmed complete VCF identity despite a parser fingerprint difference"
            ),
        );
    }
    if output_summary.records_without_csq > 0 {
        return fail_staging(
            staging,
            format!(
                "{} output records have no CSQ annotation",
                output_summary.records_without_csq
            ),
        );
    }
    performance.record(verification_measurement.finish_current(
        "verification",
        input_summary.records,
        annotated_vcf_bytes,
        0,
    ));
    update_indexing_progress(
        "indexing-variants",
        "Indexing variants",
        0,
        input_summary.records,
        0.0,
        45.0,
        0.0,
    );
    let parquet = staging.join("variants.parquet");
    let variant_conversion_measurement =
        StageMeasurement::current(performance.diagnostic_profiling);
    let canonical = match super::results::convert_vcf_with_reference(
        output,
        &parquet,
        &super::reference::fasta_path(resources),
        || CANCEL.load(Ordering::SeqCst),
        |records, _writing, output_bytes, bytes_per_second, records_per_second| {
            update_indexing_progress(
                "indexing-variants",
                "Preparing result",
                records,
                input_summary.records,
                0.0,
                45.0,
                records_per_second,
            );
            if output_bytes > 0 {
                update_parquet_output(output_bytes, bytes_per_second);
            }
        },
    ) {
        Ok(summary) => summary,
        Err(error) if error == "cancelled" => {
            discard_cancelled_staging(staging);
            return Err(error);
        }
        Err(error) => return fail_staging(staging, format!("result conversion failed: {error}")),
    };
    let output_bytes = file_bytes(&parquet)?;
    performance.record(variant_conversion_measurement.finish_current(
        "variant-parquet",
        canonical.rows,
        annotated_vcf_bytes,
        output_bytes,
    ));
    let consequences = staging.join("consequences.parquet");
    let evidence = staging.join("evidence.parquet");
    let field_catalog = staging.join("field-catalog.json");
    update_indexing_progress(
        "indexing-evidence",
        "Indexing transcript and source evidence",
        0,
        input_summary.records,
        45.0,
        45.0,
        0.0,
    );
    let structured_input_bytes = file_bytes(structured_output)?;
    let structured_conversion_measurement =
        StageMeasurement::current(performance.diagnostic_profiling);
    let structured = match super::results::convert_structured_with_canonical_vcf_and_sources(
        structured_output,
        output,
        &consequences,
        &evidence,
        &field_catalog,
        &super::reference::fasta_path(resources),
        &request.source_ids,
        || CANCEL.load(Ordering::SeqCst),
        |records, writing, output_bytes, bytes_per_second, records_per_second| {
            let detail = if writing {
                "Writing transcript and evidence Parquet"
            } else {
                "Indexing transcript and source evidence"
            };
            update_indexing_progress(
                "indexing-evidence",
                detail,
                records,
                input_summary.records,
                45.0,
                45.0,
                records_per_second,
            );
            if writing {
                update_parquet_output(output_bytes, bytes_per_second);
            }
        },
    ) {
        Ok(summary) => summary,
        Err(error) if error == "cancelled" => {
            discard_cancelled_staging(staging);
            return Err(error);
        }
        Err(error) => {
            return fail_staging(
                staging,
                format!("structured result conversion failed: {error}"),
            );
        }
    };
    let consequences_bytes = file_bytes(&consequences)?;
    let evidence_bytes = file_bytes(&evidence)?;
    let field_catalog_bytes = file_bytes(&field_catalog)?;
    let structured_result_bytes = consequences_bytes
        .saturating_add(evidence_bytes)
        .saturating_add(field_catalog_bytes);
    performance.record(structured_conversion_measurement.finish_current(
        "structured-parquet",
        structured.records,
        structured_input_bytes,
        structured_result_bytes,
    ));
    if structured.records != canonical.records
        || structured.excluded_auxiliary_records != canonical.excluded_auxiliary_records
        || canonical
            .records
            .saturating_add(canonical.excluded_auxiliary_records)
            != input_summary.records
    {
        return fail_staging(
            staging,
            format!(
                "result record counts do not align: {} input, {} result, {} excluded auxiliary; structured has {} result and {} excluded auxiliary",
                input_summary.records,
                canonical.records,
                canonical.excluded_auxiliary_records,
                structured.records,
                structured.excluded_auxiliary_records
            ),
        );
    }
    if canonical.excluded_auxiliary_records > 0 {
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} excluded {} decoy or viral records from result tables",
                canonical.excluded_auxiliary_records
            ),
        );
    }
    check_cancel(staging)?;
    update_indexing_progress(
        "publishing",
        "Preparing result indexes",
        input_summary.records,
        input_summary.records,
        90.0,
        10.0,
        0.0,
    );
    let indexing_measurement = StageMeasurement::current(performance.diagnostic_profiling);
    prepare_report_indexes(run_id, &parquet, &consequences, &evidence)?;
    let detail_index_bytes = fs::metadata(staging.join("detail-row-groups.json"))
        .map(|value| value.len())
        .unwrap_or(0);
    performance.record(indexing_measurement.finish_current(
        "report-indexing",
        input_summary.records,
        output_bytes.saturating_add(structured_result_bytes),
        detail_index_bytes,
    ));
    check_cancel(staging)?;
    set_phase("publishing", "Saving result files");
    let publishing_measurement = StageMeasurement::current(performance.diagnostic_profiling);
    let annotated_vcf = if request.include_annotated_vcf {
        Some((file_bytes(output)?, super::fastvep::sha256_file(output)?))
    } else {
        fs::remove_file(output)
            .map_err(|error| format!("cannot remove temporary annotated VCF: {error}"))?;
        None
    };
    let canonical_result_bytes = output_bytes
        .checked_add(consequences_bytes)
        .and_then(|bytes| bytes.checked_add(evidence_bytes))
        .and_then(|bytes| bytes.checked_add(field_catalog_bytes))
        .ok_or("AnnoCAT result size overflow")?;
    let readiness = super::fastvep::readiness();
    let input_name = request
        .input
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("input.vcf");
    let input_content_sha256 = recovery_identity()
        .lock()
        .map_err(|_| "annotation recovery identity lock failed")?
        .input_content_sha256
        .clone()
        .ok_or("annotation input identity was not recorded")?;
    let annotation_selection = if request.requested_profile.is_some() {
        "profile"
    } else if request.source_ids.is_empty() {
        "core-only"
    } else {
        "sources"
    };
    let reference_manifest = super::reference::fasta_path(resources)
        .parent()
        .ok_or("GRCh38 reference path has no parent directory")?
        .join("resource-manifest.json");
    let transcript_manifest = resources.join("transcript-cache").join("manifest.json");
    let sources_without_observed_evidence = request
        .source_ids
        .iter()
        .filter(|source| {
            !structured
                .sources
                .iter()
                .any(|observed| observed.eq_ignore_ascii_case(source))
        })
        .cloned()
        .collect::<Vec<_>>();
    if !sources_without_observed_evidence.is_empty() {
        crate::terminal_log(
            "annotation",
            format!(
                "{run_id} completed without observed evidence from: {}",
                sources_without_observed_evidence.join(", ")
            ),
        );
    }
    let mut manifest = serde_json::json!({
        "schemaVersion": 1,
        "canonicalSchemaVersion": super::results::SCHEMA_VERSION,
        "fastvepStructuredFormat": "ndjson-v2",
        "state": "completed",
        "reportKind": request.report_kind(),
        "runId": run_id,
        "name": name,
        "completedAt": current_timestamp(),
        "assembly": "GRCh38",
        "variantCount": canonical.rows,
        "vcfRecordCount": output_summary.records,
        "excludedAuxiliaryRecordCount": canonical.excluded_auxiliary_records,
        "alleleCount": output_summary.alternate_alleles,
        "csqEntryCount": output_summary.csq_entries,
        "structuredRecordCount": structured.records,
        "consequenceCount": structured.consequences,
        "evidenceValueCount": structured.evidence,
        "dynamicFieldCount": structured.fields,
        "resultBytes": output_bytes,
        "consequencesBytes": consequences_bytes,
        "evidenceBytes": evidence_bytes,
        "fieldCatalogBytes": field_catalog_bytes,
        "canonicalResultBytes": canonical_result_bytes,
        "csqFields": output_summary.fields,
        "resultFile": "variants.parquet",
        "consequencesFile": "consequences.parquet",
        "evidenceFile": "evidence.parquet",
        "fieldCatalogFile": "field-catalog.json",
        "input": input_name,
        "resultSha256": super::fastvep::sha256_file(&parquet)?,
        "consequencesSha256": super::fastvep::sha256_file(&consequences)?,
        "evidenceSha256": super::fastvep::sha256_file(&evidence)?,
        "fieldCatalogSha256": super::fastvep::sha256_file(&field_catalog)?,
        "fastvepVersion": readiness.version,
        "fastvepSha256": readiness.sha256,
        "sourceIds": request.source_ids,
        "sources": source_bindings,
        "observedSourceIds": structured.sources,
        "sourcesWithoutObservedEvidence": sources_without_observed_evidence,
        "inputIdentityVerified": true,
        "observedSourceValueCounts": structured.source_value_counts,
    });
    let object = manifest
        .as_object_mut()
        .ok_or("completed manifest is not an object")?;
    object.insert(
        "representativeSelectionContract".into(),
        super::results::REPRESENTATIVE_SELECTION_CONTRACT.into(),
    );
    object.insert("inputName".into(), input_name.into());
    object.insert("inputBytes".into(), file_bytes(&request.input)?.into());
    object.insert("inputContentSha256".into(), input_content_sha256.into());
    if let Some(profile) = request.requested_profile.as_deref() {
        object.insert("requestedProfile".into(), profile.into());
    }
    object.insert("annotationSelection".into(), annotation_selection.into());
    object.insert(
        "includeAnnotatedVcf".into(),
        request.include_annotated_vcf.into(),
    );
    object.insert(
        "annotationExecutionContract".into(),
        ANNOTATION_EXECUTION_CONTRACT.into(),
    );
    object.insert(
        "referenceManifestSha256".into(),
        super::fastvep::sha256_file(&reference_manifest)?.into(),
    );
    object.insert(
        "transcriptManifestSha256".into(),
        super::fastvep::sha256_file(&transcript_manifest)?.into(),
    );
    if let Some((bytes, sha256)) = annotated_vcf {
        let object = manifest
            .as_object_mut()
            .ok_or("completed manifest is not an object")?;
        object.insert("annotatedVcfFile".into(), "annotated.vcf".into());
        object.insert("annotatedVcfBytes".into(), bytes.into());
        object.insert("annotatedVcfSha256".into(), sha256.into());
    }
    if let Err(error) = fs::remove_file(structured_output) {
        return fail_staging(
            staging,
            format!("cannot remove temporary structured output: {error}"),
        );
    }
    let _ = fs::remove_file(staging.join("annotation-state.json"));
    let _ = fs::remove_file(staging.join("annotation-state.json.tmp"));
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(staging, final_directory)
        .map_err(|error| format!("cannot save the AnnoCAT result: {error}"))?;
    performance.record(publishing_measurement.finish_current(
        "publishing",
        input_summary.records,
        canonical_result_bytes,
        canonical_result_bytes.saturating_add(detail_index_bytes),
    ));
    if let Err(error) = performance.persist(final_directory) {
        crate::terminal_log(
            "performance",
            format!("{run_id} could not write {PERFORMANCE_FILE}: {error}"),
        );
    }
    Ok((canonical.rows, output_bytes))
}

fn file_bytes(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("cannot measure {}: {error}", path.display()))
}

fn prepare_report_indexes(
    run_id: &str,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
) -> Result<(), String> {
    if CANCEL.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    if let Err(error) = crate::detail_lookup::prepare(variants, consequences, evidence) {
        crate::terminal_log(
            "annotation",
            format!("{run_id} completed without the optional fast variant lookup index: {error}"),
        );
    }
    if CANCEL.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    Ok(())
}

fn fail_staging<T>(staging: &Path, error: String) -> Result<T, String> {
    let failed = staging.with_extension("failed");
    if failed.exists() {
        let _ = fs::remove_dir_all(&failed);
    }
    let _ = fs::rename(staging, failed);
    Err(error)
}

fn set_phase(phase: &'static str, detail: &str) {
    if let Ok(mut current) = state().lock() {
        current.phase = phase;
        current.detail = detail.into();
    }
}

fn update_recovery_progress(
    phase: &'static str,
    label: &str,
    progress: &crate::annotation_recovery::Progress,
    percent_start: f64,
    percent_span: f64,
) {
    let ratio = if progress.total_bytes > 0 {
        progress.completed_bytes as f64 / progress.total_bytes as f64
    } else {
        0.0
    };
    if let Ok(mut current) = state().lock() {
        current.phase = phase;
        current.output_bytes = progress.completed_bytes;
        current.total_bytes = progress.total_bytes;
        current.throughput_bytes_per_second = progress.bytes_per_second;
        if phase == "recovery-scan" {
            current.completed_records = progress.completed_records;
        }
        current.chromosome = progress.chromosome.clone();
        current.percent = (percent_start + ratio.clamp(0.0, 1.0) * percent_span).clamp(0.0, 100.0);
        current.detail = if let Some(chromosome) = current.chromosome.as_deref() {
            format!("{label} · Chromosome {chromosome}")
        } else {
            label.into()
        };
    }
}

fn update_indexing_progress(
    phase: &'static str,
    detail: &str,
    completed_records: u64,
    total_records: u64,
    percent_start: f64,
    percent_span: f64,
    records_per_second: f64,
) {
    let ratio = if total_records > 0 {
        completed_records as f64 / total_records as f64
    } else {
        0.0
    };
    if let Ok(mut current) = state().lock() {
        current.phase = phase;
        current.detail = detail.into();
        current.completed_records = completed_records.min(total_records);
        current.total_records = total_records;
        current.chromosome = None;
        current.percent = (percent_start + ratio.clamp(0.0, 1.0) * percent_span).clamp(0.0, 100.0);
        current.output_bytes = 0;
        current.throughput_bytes_per_second = 0.0;
        current.throughput_records_per_second = records_per_second;
        current.eta_seconds = (records_per_second > 0.0).then(|| {
            ((total_records.saturating_sub(completed_records) as f64 / records_per_second).ceil())
                as u64
        });
    }
}

fn update_annotation_input_progress(progress: crate::annotation_input::Progress) {
    update_input_projection_progress("annotating", "Annotating variants", progress, false);
}

fn update_recovery_input_progress(progress: crate::annotation_input::Progress) {
    update_input_projection_progress(
        "validating",
        "Validating original variants for recovery",
        progress,
        true,
    );
}

fn update_input_projection_progress(
    phase: &'static str,
    label: &str,
    progress: crate::annotation_input::Progress,
    show_input_records: bool,
) {
    let ratio = if progress.total_bytes > 0 {
        progress.completed_bytes as f64 / progress.total_bytes as f64
    } else {
        0.0
    };
    if let Ok(mut current) = state().lock() {
        current.phase = phase;
        current.output_bytes = progress.completed_bytes;
        current.total_bytes = progress.total_bytes;
        if show_input_records {
            current.completed_records = progress.completed_records;
        }
        current.chromosome = progress.chromosome;
        current.percent = (ratio * 100.0).clamp(0.0, 100.0);
        current.throughput_bytes_per_second = progress.bytes_per_second;
        current.throughput_records_per_second = progress.records_per_second;
        current.detail = current.chromosome.as_deref().map_or_else(
            || label.into(),
            |chromosome| format!("{label} · Chromosome {chromosome}"),
        );
    }
}

fn update_recovery_comparison_progress(
    completed_records: u64,
    total_records: u64,
    chromosome: Option<String>,
    records_per_second: f64,
) {
    if let Ok(mut current) = state().lock() {
        current.phase = "validating";
        current.detail = chromosome.as_deref().map_or_else(
            || "Comparing original and annotated VCF".into(),
            |chromosome| format!("Comparing original and annotated VCF · Chromosome {chromosome}"),
        );
        current.completed_records = completed_records;
        current.total_records = total_records;
        current.chromosome = chromosome;
        current.percent = if total_records > 0 {
            completed_records as f64 * 100.0 / total_records as f64
        } else {
            0.0
        };
        current.throughput_records_per_second = records_per_second;
        current.throughput_bytes_per_second = 0.0;
    }
}

fn update_parquet_output(output_bytes: u64, bytes_per_second: f64) {
    if let Ok(mut current) = state().lock() {
        current.output_bytes = output_bytes;
        current.throughput_bytes_per_second = bytes_per_second;
    }
}

fn check_cancel(staging: &Path) -> Result<(), String> {
    if !CANCEL.load(Ordering::SeqCst) {
        return Ok(());
    }
    discard_cancelled_staging(staging);
    Err("cancelled".into())
}

fn discard_cancelled_staging(staging: &Path) {
    if !PAUSE_REQUESTED.load(Ordering::SeqCst) {
        let _ = fs::remove_dir_all(staging);
    }
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("cannot replace recovered output: {error}"))?;
    }
    fs::rename(source, destination)
        .map_err(|error| format!("cannot publish recovered output: {error}"))
}

fn recovery_structured_path(request: &RecoveryRequest) -> PathBuf {
    request.structured_output.clone().unwrap_or_else(|| {
        request
            .partial_vcf
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("fastvep.ndjson")
    })
}

fn validate_recovery_files(partial_vcf: &Path, structured_output: &Path) -> Result<(), String> {
    if !partial_vcf.is_file() {
        return Err(format!(
            "interrupted annotated VCF is missing: {}",
            partial_vcf.display()
        ));
    }
    if !structured_output.is_file() {
        return Err(format!(
            "matching fastvep.ndjson is missing: {}",
            structured_output.display()
        ));
    }
    Ok(())
}

fn recoverable_outputs_exist(
    checkpoint_schema_version: u16,
    partial_vcf: &Path,
    structured_output: &Path,
) -> bool {
    checkpoint_schema_version == CHECKPOINT_SCHEMA_VERSION
        && validate_recovery_files(partial_vcf, structured_output).is_ok()
}

fn recovery_outputs_exist(directory: &Path) -> bool {
    directory.join("annotated.vcf").is_file() && directory.join("fastvep.ndjson").is_file()
}

fn find_checkpoint(runs: &Path, run_id: &str) -> Option<(PathBuf, AnnotationCheckpoint)> {
    fs::read_dir(runs)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .find_map(|entry| {
            let directory = entry.path();
            let checkpoint = read_checkpoint(&directory)?;
            (checkpoint.run_id == run_id).then_some((directory, checkpoint))
        })
}

fn persist_current_checkpoint(
    staging: &Path,
    request: &AnnotationRequest,
    name: &str,
    run_id: &str,
) -> Result<(), String> {
    let current = state()
        .lock()
        .map_err(|_| "annotation state lock failed")?
        .clone();
    let identity = recovery_identity()
        .lock()
        .map_err(|_| "annotation recovery identity lock failed")?
        .clone();
    let checkpoint = AnnotationCheckpoint {
        schema_version: CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.into(),
        name: name.into(),
        input: request.input.clone(),
        requested_profile: request.requested_profile.clone(),
        source_ids: request.source_ids.clone(),
        include_annotated_vcf: request.include_annotated_vcf,
        run_mode: request.run_mode,
        add_local_consequences: request.add_local_consequences,
        confirm_grch38: request.confirm_grch38,
        input_content_sha256: identity.input_content_sha256,
        execution_contract_sha256: identity.execution_contract_sha256,
        state: current.state.into(),
        phase: current.phase.into(),
        detail: current.detail,
        completed_records: current.completed_records,
        total_records: current.total_records,
        output_bytes: current.output_bytes,
        valid_output_bytes: current.valid_output_bytes,
        total_bytes: current.total_bytes,
        chromosome: current.chromosome,
        percent: current.percent,
        throughput_bytes_per_second: current.throughput_bytes_per_second,
        throughput_records_per_second: current.throughput_records_per_second,
        eta_seconds: current.eta_seconds,
        updated_at: current_timestamp(),
    };
    let bytes = serde_json::to_vec_pretty(&checkpoint).map_err(|error| error.to_string())?;
    let temporary = staging.join("annotation-state.json.tmp");
    let destination = staging.join("annotation-state.json");
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write annotation recovery state: {error}"))?;
    if destination.exists() {
        fs::remove_file(&destination)
            .map_err(|error| format!("cannot replace annotation recovery state: {error}"))?;
    }
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("cannot publish annotation recovery state: {error}"))
}

fn read_checkpoint(directory: &Path) -> Option<AnnotationCheckpoint> {
    ["annotation-state.json", "annotation-state.json.tmp"]
        .into_iter()
        .map(|name| directory.join(name))
        .find_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            if metadata.len() == 0 || metadata.len() > 1024 * 1024 {
                return None;
            }
            let checkpoint: AnnotationCheckpoint =
                serde_json::from_slice(&fs::read(path).ok()?).ok()?;
            (matches!(checkpoint.schema_version, 1 | CHECKPOINT_SCHEMA_VERSION))
                .then_some(checkpoint)
        })
}

fn annotation_progress_detail(current: &State) -> String {
    let mut parts = Vec::new();
    if let Some(chromosome) = current.chromosome.as_deref() {
        parts.push(format!("Chromosome {chromosome}"));
    }
    if current.total_records > 0 {
        parts.push(format!(
            "{} of {} variants",
            current.completed_records, current.total_records
        ));
    }
    if current.throughput_records_per_second > 0.0 {
        parts.push(format!(
            "{:.0} variants/s",
            current.throughput_records_per_second
        ));
    }
    if parts.is_empty() {
        "fastVEP is annotating variants".into()
    } else {
        parts.join(" · ")
    }
}

fn display_name(request: &AnnotationRequest) -> String {
    request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| input_display_name(&request.input))
        .unwrap_or_else(|| "Annotation".into())
}

fn input_display_name(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let lowercase = file_name.to_ascii_lowercase();
    for suffix in [".vcf.gz", ".vcf.bgz", ".vcf"] {
        if lowercase.ends_with(suffix) && file_name.len() > suffix.len() {
            return Some(file_name[..file_name.len() - suffix.len()].to_owned());
        }
    }
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
}

fn safe_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('-');
    if value.is_empty() {
        "annotation".into()
    } else {
        value.chars().take(80).collect()
    }
}

fn new_run_id(input: &Path) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!("{}:{nanos}:{}", input.display(), std::process::id());
    let digest = format!("{:x}", Sha256::digest(seed.as_bytes()));
    format!("run-{}", &digest[..12])
}

fn validate_source_ids(source_ids: &[String]) -> Result<(), String> {
    let allowed = [
        "dbnsfp",
        "clinvar",
        "dbsnp",
        "gnomad",
        "gnomad-genomes",
        "phylop",
        "cadd",
        "spliceai",
        "revel",
    ];
    let mut seen = HashSet::new();
    for source_id in source_ids {
        if !allowed.contains(&source_id.as_str()) {
            return Err(format!(
                "source '{source_id}' is not connected to a fastSA provider"
            ));
        }
        if !seen.insert(source_id) {
            return Err(format!("source '{source_id}' was selected more than once"));
        }
    }
    if source_ids.iter().any(|id| id == "gnomad")
        && source_ids.iter().any(|id| id == "gnomad-genomes")
    {
        return Err("choose either gnomAD exomes or gnomAD genomes, not both".into());
    }
    Ok(())
}

pub(crate) fn normalize_source_ids(mut source_ids: Vec<String>) -> Result<Vec<String>, String> {
    validate_source_ids(&source_ids)?;
    source_ids.sort_by_key(|source_id| source_order(source_id));
    Ok(source_ids)
}

fn source_order(source_id: &str) -> usize {
    [
        "dbnsfp",
        "clinvar",
        "dbsnp",
        "gnomad",
        "gnomad-genomes",
        "phylop",
        "cadd",
        "spliceai",
        "revel",
    ]
    .iter()
    .position(|candidate| *candidate == source_id)
    .unwrap_or(usize::MAX)
}

fn resolve_source_root(resources: &Path, source_id: &str) -> Result<PathBuf, String> {
    let parent = resources.join(source_id);
    let entries = fs::read_dir(&parent)
        .map_err(|_| format!("{source_id} is selected but its prepared data is not installed"))?;
    let mut candidates = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            if source_id == "dbnsfp"
                && path.file_name().and_then(|value| value.to_str()) != Some("4.9a")
            {
                return false;
            }
            path.join(format!("{source_id}.osa-shards.json")).is_file()
                || path
                    .join("shards")
                    .join("chrall")
                    .join("source.osa")
                    .is_file()
                || path
                    .join("shards")
                    .join("chrall")
                    .join("source.osa2")
                    .is_file()
        })
        .collect::<Vec<_>>();
    candidates.sort();
    let chromosomes = crate::resource_chromosomes(source_id);
    let mut issue = None;
    for candidate in candidates.into_iter().rev() {
        let status =
            super::preparation::verified_storage_status(source_id, &candidate, &chromosomes);
        if status.state == "ready" {
            return Ok(candidate);
        }
        if issue.is_none() && status.state == "rebuild-required" {
            issue = Some(status.state);
        }
    }
    Err(match issue.as_deref() {
        Some("rebuild-required") => {
            format!("{source_id} is selected but its cache must be rebuilt in Data Sources")
        }
        _ => format!("{source_id} is selected but has no complete verified fastSA provider"),
    })
}

fn compose_provider_set(
    resources: &Path,
    run_id: &str,
    source_ids: &[String],
) -> Result<(Option<PathBuf>, Vec<SourceBinding>), String> {
    if source_ids.is_empty() {
        return Ok((None, Vec::new()));
    }
    let root = resources.join(".run-providers").join(run_id);
    if root.exists() {
        fs::remove_dir_all(&root)
            .map_err(|error| format!("cannot clear stale provider set: {error}"))?;
    }
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create annotation provider set: {error}"))?;
    let result = source_ids
        .iter()
        .map(|source_id| compose_source_provider(resources, &root, source_id))
        .collect::<Result<Vec<_>, _>>();
    match result {
        Ok(bindings) => {
            fs::write(
                root.join("annocat-provider-set.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "schemaVersion": 1,
                    "runId": run_id,
                    "sources": bindings,
                }))
                .map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("cannot write provider-set manifest: {error}"))?;
            Ok((Some(root), bindings))
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&root);
            Err(error)
        }
    }
}

fn compose_source_provider(
    resources: &Path,
    destination: &Path,
    source_id: &str,
) -> Result<SourceBinding, String> {
    let source_root = resolve_source_root(resources, source_id)?;
    let shard_manifest = source_root.join(format!("{source_id}.osa-shards.json"));
    if !shard_manifest.is_file() {
        return compose_single_provider(&source_root, destination, source_id);
    }
    let manifest: ShardManifest = serde_json::from_slice(
        &fs::read(&shard_manifest)
            .map_err(|error| format!("cannot read {source_id} shard manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid {source_id} shard manifest: {error}"))?;
    if manifest.schema_version != 1 || manifest.shards.is_empty() {
        return Err(format!("{source_id} shard manifest is incomplete"));
    }
    let entries = manifest.shards;
    let mut binding: Option<SourceBinding> = None;
    let mut provider_shards = Vec::with_capacity(entries.len());
    for entry in entries {
        let relative = Path::new(&entry.file);
        if relative.is_absolute()
            || !relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "{source_id} shard manifest contains an unsafe path"
            ));
        }
        let osa = source_root.join(relative);
        let shard_directory = osa
            .parent()
            .ok_or_else(|| format!("{source_id} shard has no directory"))?;
        let verification_path = shard_directory.join("verified.json");
        let checkpoint: super::preparation::PreparationCheckpoint =
            serde_json::from_slice(&fs::read(&verification_path).map_err(|_| {
                format!(
                    "{source_id} chromosome {} is not verified",
                    entry.chromosome
                )
            })?)
            .map_err(|error| format!("invalid {source_id} verification: {error}"))?;
        if checkpoint.state != super::preparation::CheckpointState::Verified
            || checkpoint.identity.resource_id != source_id
            || checkpoint.identity.assembly != "GRCh38"
        {
            return Err(format!(
                "{source_id} chromosome {} verification identity is invalid",
                entry.chromosome
            ));
        }
        let (format, builder_contract) =
            provider_cache_contract(shard_directory, &checkpoint, source_id)?;
        let expected_osa = shard_directory.join(format.data_file_name());
        if osa != expected_osa {
            return Err(format!(
                "{source_id} chromosome {} shard filename does not match its verified cache format",
                entry.chromosome
            ));
        }
        let destination_relative = PathBuf::from(source_id)
            .join("shards")
            .join(format!("chr{}", entry.chromosome))
            .join(format.data_file_name());
        let destination_osa = destination.join(&destination_relative);
        fs::create_dir_all(destination_osa.parent().expect("provider shard has parent"))
            .map_err(|error| format!("cannot create {source_id} provider directory: {error}"))?;
        fs::hard_link(&osa, &destination_osa)
            .map_err(|error| format!("cannot link verified {source_id} shard: {error}"))?;
        if let Some(index_name) = format.index_file_name() {
            let index = shard_directory.join(index_name);
            fs::hard_link(&index, destination_osa.parent().unwrap().join(index_name))
                .map_err(|error| format!("cannot link verified {source_id} index: {error}"))?;
        }
        provider_shards.push(serde_json::json!({
            "chromosome": entry.chromosome,
            "file": destination_relative.to_string_lossy().replace('\\', "/"),
        }));
        match binding.as_mut() {
            Some(binding)
                if binding.release != checkpoint.identity.release
                    || binding.selected_schema != checkpoint.identity.selected_schema
                    || binding.osa_schema_version != format.schema_version()
                    || binding.cache_builder_contract != builder_contract =>
            {
                return Err(format!(
                    "{source_id} verified shards have inconsistent identities"
                ));
            }
            Some(binding) => binding.chromosomes.push(entry.chromosome),
            None => {
                binding = Some(SourceBinding {
                    resource_id: source_id.into(),
                    release: checkpoint.identity.release,
                    assembly: checkpoint.identity.assembly,
                    selected_schema: checkpoint.identity.selected_schema,
                    cache_format: format.builder_argument().into(),
                    osa_schema_version: format.schema_version(),
                    cache_builder_contract: builder_contract,
                    chromosomes: vec![entry.chromosome],
                });
            }
        }
    }
    fs::write(
        destination.join(format!("{source_id}.osa-shards.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "shards": provider_shards,
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {source_id} provider manifest: {error}"))?;
    binding.ok_or_else(|| format!("{source_id} provider has no verified shards"))
}

fn compose_single_provider(
    source_root: &Path,
    destination: &Path,
    source_id: &str,
) -> Result<SourceBinding, String> {
    let shard = source_root.join("shards").join("chrall");
    let checkpoint: super::preparation::PreparationCheckpoint = serde_json::from_slice(
        &fs::read(shard.join("verified.json"))
            .map_err(|_| format!("{source_id} provider is not verified"))?,
    )
    .map_err(|error| format!("invalid {source_id} verification: {error}"))?;
    if checkpoint.state != super::preparation::CheckpointState::Verified
        || checkpoint.identity.resource_id != source_id
        || checkpoint.identity.assembly != "GRCh38"
        || checkpoint.identity.chromosome != "all"
    {
        return Err(format!("{source_id} verification identity is invalid"));
    }
    let (format, builder_contract) = provider_cache_contract(&shard, &checkpoint, source_id)?;
    let osa = shard.join(format.data_file_name());
    let destination_osa = destination.join(format!("{source_id}.{}", format.builder_argument()));
    fs::hard_link(&osa, &destination_osa)
        .map_err(|error| format!("cannot link verified {source_id} provider: {error}"))?;
    if let Some(index_name) = format.index_file_name() {
        let index = shard.join(index_name);
        fs::hard_link(&index, destination.join(format!("{source_id}.osa.idx")))
            .map_err(|error| format!("cannot link verified {source_id} index: {error}"))?;
    }
    Ok(SourceBinding {
        resource_id: source_id.into(),
        release: checkpoint.identity.release,
        assembly: checkpoint.identity.assembly,
        selected_schema: checkpoint.identity.selected_schema,
        cache_format: format.builder_argument().into(),
        osa_schema_version: format.schema_version(),
        cache_builder_contract: builder_contract,
        chromosomes: vec!["all".into()],
    })
}

fn provider_cache_contract(
    shard_directory: &Path,
    checkpoint: &super::preparation::PreparationCheckpoint,
    source_id: &str,
) -> Result<(super::preparation::CacheFormat, String), String> {
    let format = super::preparation::verified_cache_files(shard_directory, checkpoint)
        .map_err(|error| format!("{source_id} cache files are invalid: {error}"))?;
    let contract_path = shard_directory.join("cache-contract-v2.json");
    if !contract_path.is_file() {
        return Err(format!(
            "{source_id} cache needs a one-time compatibility upgrade in Data Sources"
        ));
    }
    let manifest = crate::cache_contract::read(&contract_path)?;
    if manifest.cache_contract.osa_schema_version != format.schema_version()
        || manifest.cache_contract.reader_compatibility != format.reader_compatibility()
        || manifest.cache_contract.builder_contract != format.builder_contract()
        || annocat_core::source_catalog::adapter_contract(source_id)
            != Some(manifest.cache_contract.adapter_contract.as_str())
        || manifest.cache_contract.selected_field_schema != checkpoint.identity.selected_schema
        || manifest.source_artifact.resource_id != source_id
        || manifest.source_artifact.release != checkpoint.identity.release
        || manifest.source_artifact.assembly != checkpoint.identity.assembly
        || manifest.source_artifact.chromosome != checkpoint.identity.chromosome
        || manifest.source_artifact.artifact_id
            != annocat_core::source_catalog::artifact_identity(
                source_id,
                &checkpoint.identity.release,
                &checkpoint.identity.assembly,
                &checkpoint.identity.chromosome,
            )
    {
        return Err(format!(
            "{source_id} cache contract does not match its verified checkpoint"
        ));
    }
    Ok((format, manifest.cache_contract.builder_contract))
}

pub(crate) fn current_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        day_seconds / 3600,
        day_seconds / 60 % 60,
        day_seconds % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_display_names_remove_complete_vcf_suffixes() {
        for (path, expected) in [
            ("sample.vcf", "sample"),
            ("sample.vcf.gz", "sample"),
            ("sample.vcf.bgz", "sample"),
            ("sample.VCF.GZ", "sample"),
        ] {
            assert_eq!(
                input_display_name(Path::new(path)).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            input_display_name(Path::new("sample.txt")).as_deref(),
            Some("sample")
        );
    }

    #[test]
    fn stage_performance_reports_rates_and_optional_process_counters() {
        let measurement = StageMeasurement {
            started_at: Instant::now() - Duration::from_millis(100),
            usage: Some(ProcessUsageSnapshot {
                cpu_100ns: 10_000,
                read_bytes: 100,
                write_bytes: 200,
            }),
        };
        let stage = measurement.finish(
            "fixture",
            50,
            1024 * 1024,
            2 * 1024 * 1024,
            Some(ProcessUsageSnapshot {
                cpu_100ns: 510_000,
                read_bytes: 1_100,
                write_bytes: 2_200,
            }),
        );

        assert_eq!(stage.stage, "fixture");
        assert!((490.0..=510.0).contains(&stage.records_per_second));
        assert_eq!(stage.cpu_time_ms, Some(50.0));
        assert!(stage.average_cpu_cores.is_some_and(|value| value > 0.45));
        assert_eq!(stage.process_read_bytes, Some(1_000));
        assert_eq!(stage.process_write_bytes, Some(2_000));
    }

    #[test]
    fn stage_performance_omits_disabled_diagnostic_fields() {
        let stage = StageMeasurement {
            started_at: Instant::now() - Duration::from_millis(1),
            usage: None,
        }
        .finish("fixture", 1, 1, 1, None);
        let value = serde_json::to_value(stage).unwrap();

        assert!(value.get("cpuTimeMs").is_none());
        assert!(value.get("averageCpuCores").is_none());
        assert!(value.get("processReadBytes").is_none());
        assert!(value.get("processWriteBytes").is_none());
    }

    #[test]
    fn names_are_safe_and_bounded() {
        assert_eq!(safe_name("HG002 / chr 22"), "HG002---chr-22");
        assert_eq!(safe_name("***"), "annotation");
        assert!(safe_name(&"a".repeat(100)).len() <= 80);
    }

    #[test]
    fn unix_epoch_date_conversion_is_stable() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_650), (2026, 7, 16));
    }

    #[test]
    fn annotated_vcf_is_off_by_default() {
        let request: AnnotationRequest =
            serde_json::from_str(r#"{"input":"fixture.vcf"}"#).unwrap();
        assert!(!request.include_annotated_vcf);
        assert_eq!(request.run_mode, RunMode::Annotation);
        assert!(!request.add_local_consequences);
        assert!(!request.confirm_grch38);
    }

    #[test]
    fn vcf_review_runs_without_fastvep_and_round_trips_through_a_shared_report() {
        let root = std::env::temp_dir().join(format!(
            "annocat-vcf-review-run-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let input = root.join("input.vcf");
        let staging = root.join("review.partial");
        let completed = root.join("review");
        let imported_runs = root.join("imported");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n##reference=GRCh38\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tCASE\n22\t100\trs1\tA\tG\t50\tPASS\t.\tGT:DP\t0/1:24\n",
        )
        .unwrap();
        let request = AnnotationRequest {
            input,
            requested_profile: None,
            name: Some("Review fixture".into()),
            output_directory: Some(root.clone()),
            source_ids: Vec::new(),
            include_annotated_vcf: false,
            run_mode: RunMode::VcfReview,
            add_local_consequences: false,
            confirm_grch38: false,
        };
        validate_request(&request, &root.join("missing-resources")).unwrap();
        let (rows, _) = execute_vcf_review(
            &request,
            "Review fixture",
            "run-vcf-review",
            &staging,
            &completed,
        )
        .unwrap();
        assert_eq!(rows, 1);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(completed.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["reportKind"], "vcf-only");
        assert!(manifest.get("fastvepVersion").is_none());
        assert!(completed.join("detail-row-groups.json").is_file());

        let package = root.join("review.zip");
        crate::report_package::create(
            &completed,
            &package,
            &crate::report_import::CandidateOverlay::empty("run-vcf-review"),
        )
        .unwrap();
        let imported = crate::report_library::import(&package, &imported_runs).unwrap();
        let imported_manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(imported.directory.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(imported_manifest["reportKind"], "vcf-only");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vcf_review_requires_confirmed_grch38_identity() {
        let root = std::env::temp_dir().join(format!(
            "annocat-vcf-review-assembly-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("unknown.vcf");
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n22\t100\t.\tA\tG\t.\tPASS\t.\n",
        )
        .unwrap();
        let mut request = AnnotationRequest {
            input,
            requested_profile: None,
            name: None,
            output_directory: None,
            source_ids: Vec::new(),
            include_annotated_vcf: false,
            run_mode: RunMode::VcfReview,
            add_local_consequences: false,
            confirm_grch38: false,
        };
        assert!(
            validate_request(&request, &root)
                .unwrap_err()
                .contains("confirm")
        );
        request.confirm_grch38 = true;
        validate_request(&request, &root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn annotation_rejects_a_declared_non_grch38_assembly_before_engine_checks() {
        let root = std::env::temp_dir().join(format!(
            "annocat-annotation-assembly-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("grch37.vcf");
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n##contig=<ID=1,assembly=b37,length=249250621>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t100\t.\tA\tG\t.\tPASS\t.\n",
        )
        .unwrap();
        let request = AnnotationRequest {
            input,
            requested_profile: None,
            name: None,
            output_directory: None,
            source_ids: Vec::new(),
            include_annotated_vcf: false,
            run_mode: RunMode::Annotation,
            add_local_consequences: false,
            confirm_grch38: false,
        };
        assert!(
            validate_request(&request, &root)
                .unwrap_err()
                .contains("does not support GRCh37, b37, or hg19")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn annotation_rejects_a_header_only_vcf_before_engine_checks() {
        let root = std::env::temp_dir().join(format!(
            "annocat-empty-annotation-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("empty.vcf");
        fs::write(
            &input,
            "##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n",
        )
        .unwrap();
        let request = AnnotationRequest {
            input,
            requested_profile: None,
            name: None,
            output_directory: None,
            source_ids: Vec::new(),
            include_annotated_vcf: false,
            run_mode: RunMode::Annotation,
            add_local_consequences: false,
            confirm_grch38: true,
        };

        assert_eq!(
            validate_request(&request, &root).unwrap_err(),
            "the input VCF contains no variant records"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sequential_batch_request_preserves_input_order() {
        let request: BatchAnnotationRequest = serde_json::from_str(
            r#"{"inputs":["first.vcf","second.vcf"],"sourceIds":["clinvar"]}"#,
        )
        .unwrap();
        assert_eq!(
            request.inputs,
            [PathBuf::from("first.vcf"), PathBuf::from("second.vcf")]
        );
        assert_eq!(request.source_ids, ["clinvar"]);
        assert!(!request.include_annotated_vcf);
    }

    #[test]
    fn provider_set_hard_links_verified_source_without_copying_it() {
        let root = std::env::temp_dir().join(format!(
            "annocat-provider-set-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shard = root
            .join("clinvar")
            .join("20260715")
            .join("shards")
            .join("chrall");
        fs::create_dir_all(&shard).unwrap();
        fs::write(shard.join("source.osa"), b"osa fixture").unwrap();
        fs::write(shard.join("source.osa.idx"), b"index fixture").unwrap();
        let identity = super::super::preparation::PreparationIdentity {
            resource_id: "clinvar".into(),
            release: "20260715".into(),
            assembly: "GRCh38".into(),
            chromosome: "all".into(),
            source_url: "https://example.invalid/clinvar.vcf.gz".into(),
            expected_compressed_bytes: 10,
            source_etag: Some("fixture".into()),
            source_last_modified: None,
            selected_schema: "clinvar-20260715".into(),
            fastvep_commit: "fixture".into(),
            osa_schema_version: 1,
        };
        fs::write(
            shard.join("verified.json"),
            serde_json::to_vec(&super::super::preparation::PreparationCheckpoint {
                schema_version: 1,
                identity: identity.clone(),
                state: super::super::preparation::CheckpointState::Verified,
                compressed_bytes_read: 10,
                parsed_records: 1,
                prepared_bytes: 11,
                prepared_index_bytes: 13,
                prepared_sha256: None,
                prepared_index_sha256: None,
            })
            .unwrap(),
        )
        .unwrap();
        crate::cache_contract::write_atomic(
            &shard.join("cache-contract-v2.json"),
            &crate::cache_contract::CacheContractManifest::current(
                crate::fastvep::pinned_builder_provenance(),
                &identity.resource_id,
                &identity.release,
                &identity.assembly,
                &identity.chromosome,
                identity.expected_compressed_bytes,
                identity.source_etag.as_deref(),
                identity.source_last_modified.as_deref(),
                &identity.selected_schema,
                super::super::preparation::CacheFormat::OsaV1,
                Some(&identity.fastvep_commit),
            ),
        )
        .unwrap();
        fs::write(
            root.join("clinvar")
                .join("20260715")
                .join("clinvar.osa-shards.json"),
            br#"{"schemaVersion":1,"shards":[{"chromosome":"all","file":"shards/chrall/source.osa"}]}"#,
        )
        .unwrap();
        let newer_shard = root
            .join("clinvar")
            .join("20260716")
            .join("shards")
            .join("chrall");
        fs::create_dir_all(&newer_shard).unwrap();
        fs::write(newer_shard.join("source.osa"), b"newer legacy osa").unwrap();
        fs::write(newer_shard.join("source.osa.idx"), b"newer index").unwrap();
        let mut newer_identity = identity.clone();
        newer_identity.release = "20260716".into();
        newer_identity.selected_schema = "clinvar-20260716".into();
        fs::write(
            newer_shard.join("verified.json"),
            serde_json::to_vec(&super::super::preparation::PreparationCheckpoint {
                schema_version: 1,
                identity: newer_identity,
                state: super::super::preparation::CheckpointState::Verified,
                compressed_bytes_read: 10,
                parsed_records: 1,
                prepared_bytes: 16,
                prepared_index_bytes: 11,
                prepared_sha256: None,
                prepared_index_sha256: None,
            })
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            resolve_source_root(&root, "clinvar").unwrap(),
            root.join("clinvar").join("20260715")
        );

        let (directory, bindings) =
            compose_provider_set(&root, "run-fixture", &["clinvar".into()]).unwrap();
        let directory = directory.unwrap();
        assert_eq!(bindings[0].release, "20260715");
        assert_eq!(bindings[0].chromosomes, ["all"]);
        let provider = directory
            .join("clinvar")
            .join("shards")
            .join("chrall")
            .join("source.osa");
        assert!(provider.is_file());
        assert_eq!(fs::read(provider).unwrap(), b"osa fixture");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_set_links_verified_osa2_without_an_index() {
        let root = std::env::temp_dir().join(format!(
            "annocat-provider-set-osa2-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source_root = root.join("phylop").join("hg38");
        let chromosomes = crate::resource_chromosomes("phylop");
        let mut manifest_shards = Vec::new();
        for chromosome in &chromosomes {
            let shard = source_root.join("shards").join(format!("chr{chromosome}"));
            fs::create_dir_all(&shard).unwrap();
            fs::write(shard.join("source.osa2"), b"osa2 fixture").unwrap();
            let identity = super::super::preparation::PreparationIdentity {
                resource_id: "phylop".into(),
                release: "hg38".into(),
                assembly: "GRCh38".into(),
                chromosome: chromosome.clone(),
                source_url: "https://example.invalid/phylop.bw".into(),
                expected_compressed_bytes: 10,
                source_etag: Some("fixture".into()),
                source_last_modified: None,
                selected_schema: "ucsc-hg38-phylop100way-per-base".into(),
                fastvep_commit: "fixture".into(),
                osa_schema_version: 2,
            };
            fs::write(
                shard.join("verified.json"),
                serde_json::to_vec(&super::super::preparation::PreparationCheckpoint {
                    schema_version: 1,
                    identity,
                    state: super::super::preparation::CheckpointState::Verified,
                    compressed_bytes_read: 10,
                    parsed_records: 1,
                    prepared_bytes: 12,
                    prepared_index_bytes: 0,
                    prepared_sha256: None,
                    prepared_index_sha256: None,
                })
                .unwrap(),
            )
            .unwrap();
            crate::cache_contract::write_atomic(
                &shard.join("cache-contract-v2.json"),
                &crate::cache_contract::CacheContractManifest::current(
                    crate::fastvep::pinned_builder_provenance(),
                    "phylop",
                    "hg38",
                    "GRCh38",
                    chromosome,
                    10,
                    Some("fixture"),
                    None,
                    "ucsc-hg38-phylop100way-per-base",
                    super::super::preparation::CacheFormat::OsaV2,
                    Some("fixture"),
                ),
            )
            .unwrap();
            manifest_shards.push(serde_json::json!({
                "chromosome": chromosome,
                "file": format!("shards/chr{chromosome}/source.osa2"),
            }));
        }
        fs::write(
            source_root.join("phylop.osa-shards.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 1,
                "shards": manifest_shards,
            }))
            .unwrap(),
        )
        .unwrap();

        let (directory, bindings) =
            compose_provider_set(&root, "run-osa2-fixture", &["phylop".into()]).unwrap();
        let directory = directory.unwrap();
        assert_eq!(bindings[0].cache_format, "osa2");
        assert_eq!(bindings[0].osa_schema_version, 2);
        assert_eq!(bindings[0].chromosomes.len(), chromosomes.len());
        assert!(
            directory
                .join("phylop")
                .join("shards")
                .join("chr1")
                .join("source.osa2")
                .is_file()
        );
        assert!(
            !directory
                .join("phylop")
                .join("shards")
                .join("chr1")
                .join("source.osa.idx")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exome_and_genome_gnomad_caches_are_mutually_exclusive() {
        assert!(validate_source_ids(&["gnomad".into()]).is_ok());
        assert!(validate_source_ids(&["gnomad-genomes".into()]).is_ok());
        assert_eq!(
            validate_source_ids(&["gnomad".into(), "gnomad-genomes".into()]).unwrap_err(),
            "choose either gnomAD exomes or gnomAD genomes, not both"
        );
    }

    #[test]
    fn dbnsfp_provider_resolution_never_selects_a_version_after_4_9a() {
        let root = std::env::temp_dir().join(format!(
            "annocat-provider-version-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let unsupported = root.join("dbnsfp").join("5.0");
        fs::create_dir_all(&unsupported).unwrap();
        fs::write(unsupported.join("dbnsfp.osa-shards.json"), b"fixture").unwrap();
        assert!(resolve_source_root(&root, "dbnsfp").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_run_discard_removes_only_its_checkpoint_directory() {
        let root = std::env::temp_dir().join(format!(
            "annocat-discard-interrupted-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let interrupted = root.join("fixture.partial");
        let unrelated = root.join("unrelated");
        fs::create_dir_all(&interrupted).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        let checkpoint = AnnotationCheckpoint {
            schema_version: 1,
            run_id: "run-discard-fixture".into(),
            name: "Discard fixture".into(),
            input: root.join("input.vcf"),
            requested_profile: None,
            source_ids: Vec::new(),
            include_annotated_vcf: false,
            run_mode: RunMode::Annotation,
            add_local_consequences: false,
            confirm_grch38: true,
            input_content_sha256: None,
            execution_contract_sha256: None,
            state: "interrupted".into(),
            phase: "indexing-variants".into(),
            detail: "Interrupted during indexing".into(),
            completed_records: 1,
            total_records: 2,
            output_bytes: 100,
            valid_output_bytes: 100,
            total_bytes: 200,
            chromosome: Some("22".into()),
            percent: 50.0,
            throughput_bytes_per_second: 0.0,
            throughput_records_per_second: 0.0,
            eta_seconds: None,
            updated_at: current_timestamp(),
        };
        fs::write(
            interrupted.join("annotation-state.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();

        discard_interrupted_run(&root, "run-discard-fixture").unwrap();

        assert!(!interrupted.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_checkpoint_recovers_outputs_before_input_hash_finishes() {
        let root = std::env::temp_dir().join(format!(
            "annocat-early-resume-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let partial_vcf = root.join("annotated.vcf");
        let structured_output = root.join("fastvep.ndjson");
        fs::write(&partial_vcf, b"partial VCF").unwrap();
        fs::write(&structured_output, b"partial structured output").unwrap();

        assert!(recoverable_outputs_exist(
            CHECKPOINT_SCHEMA_VERSION,
            &partial_vcf,
            &structured_output
        ));
        assert!(!recoverable_outputs_exist(
            CHECKPOINT_SCHEMA_VERSION - 1,
            &partial_vcf,
            &structured_output
        ));

        fs::remove_dir_all(root).unwrap();
    }
}
