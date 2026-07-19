use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePreparationState {
    pub resource_id: Option<String>,
    pub state: String,
    pub phase: String,
    pub chromosome: Option<String>,
    pub network_bytes: u64,
    pub expected_network_bytes: u64,
    pub percent: f64,
    pub parsed_records: u64,
    pub prepared_bytes: u64,
    pub throughput_bytes_per_second: f64,
    pub completed_chromosomes: u16,
    pub remaining_chromosomes: u16,
    pub cancel_requested: bool,
    pub error: Option<String>,
    pub detail: String,
}

impl Default for LivePreparationState {
    fn default() -> Self {
        Self {
            resource_id: None,
            state: "idle".into(),
            phase: "idle".into(),
            chromosome: None,
            network_bytes: 0,
            expected_network_bytes: 0,
            percent: 0.0,
            parsed_records: 0,
            prepared_bytes: 0,
            throughput_bytes_per_second: 0.0,
            completed_chromosomes: 0,
            remaining_chromosomes: 0,
            cancel_requested: false,
            error: None,
            detail: "No preparation job is active".into(),
        }
    }
}

pub(super) struct LivePreparationJob {
    pub(super) state: Arc<Mutex<LivePreparationState>>,
    pub(super) cancel: Arc<AtomicBool>,
}

static LIVE_JOBS: OnceLock<Mutex<HashMap<String, Arc<LivePreparationJob>>>> = OnceLock::new();
static FALLBACK_LIVE_JOB: OnceLock<Arc<LivePreparationJob>> = OnceLock::new();
thread_local! {
    static CURRENT_LIVE_JOB: RefCell<Option<Arc<LivePreparationJob>>> = const { RefCell::new(None) };
}

fn live_jobs() -> &'static Mutex<HashMap<String, Arc<LivePreparationJob>>> {
    LIVE_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fallback_live_job() -> Arc<LivePreparationJob> {
    FALLBACK_LIVE_JOB
        .get_or_init(|| {
            Arc::new(LivePreparationJob {
                state: Arc::new(Mutex::new(LivePreparationState::default())),
                cancel: Arc::new(AtomicBool::new(false)),
            })
        })
        .clone()
}

fn live_job() -> Arc<LivePreparationJob> {
    CURRENT_LIVE_JOB
        .with(|current| current.borrow().clone())
        .unwrap_or_else(fallback_live_job)
}

pub(super) fn live_state() -> Arc<Mutex<LivePreparationState>> {
    live_job().state.clone()
}

pub(super) fn live_cancel() -> Arc<AtomicBool> {
    live_job().cancel.clone()
}

pub(super) fn register_live_job(
    state: LivePreparationState,
) -> Result<Arc<LivePreparationJob>, String> {
    let resource_id = state
        .resource_id
        .clone()
        .ok_or("preparation job is missing its resource identity")?;
    let mut jobs = live_jobs()
        .lock()
        .map_err(|_| "preparation state lock failed")?;
    if jobs
        .get(&resource_id)
        .is_some_and(|job| job.state.lock().is_ok_and(|state| state.state == "running"))
    {
        return Err(format!("{resource_id} preparation is already running"));
    }
    let job = Arc::new(LivePreparationJob {
        state: Arc::new(Mutex::new(state)),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    jobs.insert(resource_id, job.clone());
    Ok(job)
}

pub(super) fn run_with_live_job(job: Arc<LivePreparationJob>, run: impl FnOnce()) {
    let resource_id = job
        .state
        .lock()
        .ok()
        .and_then(|state| state.resource_id.clone())
        .unwrap_or_else(|| "unknown".into());
    crate::terminal_log("prepare", format!("{resource_id} started"));
    CURRENT_LIVE_JOB.with(|current| *current.borrow_mut() = Some(job));
    run();
    let outcome = CURRENT_LIVE_JOB.with(|current| {
        current
            .borrow()
            .as_ref()
            .and_then(|job| job.state.lock().ok())
            .map(|state| {
                let mut message = format!(
                    "{resource_id} {} (phase={}, chromosomes={}/{})",
                    state.state,
                    state.phase,
                    state.completed_chromosomes,
                    state
                        .completed_chromosomes
                        .saturating_add(state.remaining_chromosomes)
                );
                if let Some(error) = &state.error {
                    message.push_str(&format!(": {error}"));
                }
                message
            })
    });
    if let Some(outcome) = outcome {
        crate::terminal_log("prepare", outcome);
    }
    CURRENT_LIVE_JOB.with(|current| *current.borrow_mut() = None);
}

pub fn running_count() -> usize {
    live_jobs()
        .lock()
        .map(|jobs| {
            jobs.values()
                .filter(|job| job.state.lock().is_ok_and(|state| state.state == "running"))
                .count()
        })
        .unwrap_or(0)
}

pub fn live_status(resource_id: &str) -> LivePreparationState {
    let job = live_jobs()
        .lock()
        .ok()
        .and_then(|jobs| jobs.get(resource_id).cloned());
    job.and_then(|job| job.state.lock().ok().map(|state| state.clone()))
        .unwrap_or_else(|| LivePreparationState {
            resource_id: Some(resource_id.into()),
            ..LivePreparationState::default()
        })
}

pub fn record_start_failure(
    resource_id: &str,
    error: impl Into<String>,
    expected_network_bytes: u64,
) {
    let error = error.into();
    let job = Arc::new(LivePreparationJob {
        state: Arc::new(Mutex::new(LivePreparationState {
            resource_id: Some(resource_id.into()),
            state: "failed".into(),
            phase: "start-failed".into(),
            expected_network_bytes,
            error: Some(error.clone()),
            detail: error,
            ..LivePreparationState::default()
        })),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    if let Ok(mut jobs) = live_jobs().lock()
        && !jobs.get(resource_id).is_some_and(|existing| {
            existing
                .state
                .lock()
                .is_ok_and(|state| state.state == "running")
        })
    {
        jobs.insert(resource_id.into(), job);
    }
}

pub fn cancel_live(resource_id: &str) -> bool {
    let job = live_jobs()
        .lock()
        .ok()
        .and_then(|jobs| jobs.get(resource_id).cloned());
    let active = job.as_ref().is_some_and(|job| {
        job.state.lock().is_ok_and(|mut state| {
            if state.state == "running" {
                state.cancel_requested = true;
                true
            } else {
                false
            }
        })
    });
    if active && let Some(job) = job {
        job.cancel.store(true, Ordering::SeqCst);
    }
    active
}

pub fn forget_live(resource_id: &str) {
    if let Ok(mut jobs) = live_jobs().lock()
        && jobs
            .get(resource_id)
            .is_some_and(|job| job.state.lock().is_ok_and(|state| state.state != "running"))
    {
        jobs.remove(resource_id);
    }
}
