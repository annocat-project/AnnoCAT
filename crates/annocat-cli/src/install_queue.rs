use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, PartialEq, Eq)]
pub enum NextWork {
    Start(String),
    Wait,
    Idle,
}

#[derive(Debug, PartialEq, Eq)]
pub struct EnqueueResult {
    pub inserted: bool,
    pub start_worker: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallQueueState {
    waiting: VecDeque<String>,
    paused: HashSet<String>,
    #[serde(default)]
    in_flight: HashSet<String>,
    scheduling_paused: bool,
    concurrency: usize,
    #[serde(default = "default_resumable_source_parts")]
    resumable_source_parts: bool,
    #[serde(skip)]
    worker_active: bool,
}

const fn default_resumable_source_parts() -> bool {
    true
}

impl Default for InstallQueueState {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            paused: HashSet::new(),
            in_flight: HashSet::new(),
            scheduling_paused: false,
            concurrency: 1,
            resumable_source_parts: true,
            worker_active: false,
        }
    }
}

impl InstallQueueState {
    fn enqueue(&mut self, resource_id: &str, prioritize: bool) -> EnqueueResult {
        // Enqueue is an explicit user action. It resumes scheduling, while any
        // other paused sources remain available for a later explicit resume.
        self.scheduling_paused = false;
        let inserted = if self.waiting.iter().any(|queued| queued == resource_id)
            || self.in_flight.contains(resource_id)
        {
            false
        } else {
            if prioritize {
                self.waiting.push_front(resource_id.into());
            } else {
                self.waiting.push_back(resource_id.into());
            }
            true
        };
        let start_worker = !self.waiting.is_empty() && !self.worker_active;
        if start_worker {
            self.worker_active = true;
        }
        EnqueueResult {
            inserted,
            start_worker,
        }
    }

    fn next(&mut self, running: usize) -> NextWork {
        if self.scheduling_paused || running >= self.concurrency {
            return NextWork::Wait;
        }
        match self.waiting.pop_front() {
            Some(resource_id) => {
                self.in_flight.insert(resource_id.clone());
                NextWork::Start(resource_id)
            }
            None => {
                self.worker_active = false;
                NextWork::Idle
            }
        }
    }

    fn remove_waiting(&mut self, resource_id: &str) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|queued| queued != resource_id);
        self.waiting.len() != before
    }

    fn recover_interrupted_work(&mut self) {
        let interrupted = std::mem::take(&mut self.in_flight);
        for resource_id in interrupted {
            if !self.paused.contains(&resource_id)
                && !self.waiting.iter().any(|queued| queued == &resource_id)
            {
                self.waiting.push_front(resource_id);
            }
        }
        self.worker_active = false;
    }

    fn position(&self, resource_id: &str, running: usize) -> Option<usize> {
        self.waiting
            .iter()
            .position(|queued| queued == resource_id)
            .map(|position| running + position + 1)
    }
}

fn state() -> &'static Mutex<InstallQueueState> {
    static STATE: OnceLock<Mutex<InstallQueueState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(InstallQueueState::default()))
}

fn persistence_path() -> &'static OnceLock<PathBuf> {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    &PATH
}

fn persist(state: &InstallQueueState) -> Result<(), String> {
    let Some(path) = persistence_path().get() else {
        return Ok(());
    };
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("cannot serialize installation queue: {error}"))?;
    fs::write(path, bytes).map_err(|error| format!("cannot save installation queue: {error}"))
}

pub fn restore(root: &Path) -> Result<bool, String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("cannot create download directory: {error}"))?;
    let path = root.join("installation-queue.json");
    let _ = persistence_path().set(path.clone());
    let mut restored = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<InstallQueueState>(&bytes)
            .map_err(|error| format!("cannot read installation queue: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => InstallQueueState::default(),
        Err(error) => return Err(format!("cannot read installation queue: {error}")),
    };
    if !(1..=4).contains(&restored.concurrency) {
        restored.concurrency = 1;
    }
    restored.recover_interrupted_work();
    let start_worker = !restored.scheduling_paused && !restored.waiting.is_empty();
    if start_worker {
        restored.worker_active = true;
    }
    persist(&restored)?;
    *state()
        .lock()
        .map_err(|_| "installation queue lock failed".to_string())? = restored;
    Ok(start_worker)
}

pub fn enqueue(resource_id: &str, prioritize: bool) -> Result<EnqueueResult, String> {
    let mut state = state()
        .lock()
        .map_err(|_| "installation queue lock failed".to_string())?;
    let outcome = state.enqueue(resource_id, prioritize);
    persist(&state)?;
    Ok(outcome)
}

pub fn next(running: usize) -> NextWork {
    let Ok(mut state) = state().lock() else {
        return NextWork::Wait;
    };
    let work = state.next(running);
    if matches!(work, NextWork::Start(_) | NextWork::Idle) {
        let _ = persist(&state);
    }
    work
}

pub fn hold(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        state.scheduling_paused = true;
        let changed = state.paused.insert(resource_id.into());
        let _ = persist(&state);
        changed
    })
}

pub fn release_hold(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        let changed = state.paused.remove(resource_id);
        let _ = persist(&state);
        changed
    })
}

pub fn remove_waiting(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        let changed = state.remove_waiting(resource_id);
        let _ = persist(&state);
        changed
    })
}

pub fn remove(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        let waiting = state.remove_waiting(resource_id);
        let paused = state.paused.remove(resource_id);
        let in_flight = state.in_flight.remove(resource_id);
        if paused {
            state.scheduling_paused = false;
        }
        let _ = persist(&state);
        waiting || paused || in_flight
    })
}

pub fn finish(resource_id: &str) {
    if let Ok(mut state) = state().lock() {
        state.in_flight.remove(resource_id);
        let _ = persist(&state);
    }
}

pub fn position(resource_id: &str, running: usize) -> Option<usize> {
    state()
        .lock()
        .ok()
        .and_then(|state| state.position(resource_id, running))
}

pub fn set_concurrency(concurrency: usize) -> Result<usize, String> {
    if !(1..=4).contains(&concurrency) {
        return Err("installation concurrency must be between 1 and 4".into());
    }
    let mut state = state()
        .lock()
        .map_err(|_| "installation queue lock failed".to_string())?;
    state.concurrency = concurrency;
    persist(&state)?;
    Ok(concurrency)
}

pub fn concurrency() -> usize {
    state().lock().map(|state| state.concurrency).unwrap_or(1)
}

pub fn set_resumable_source_parts(enabled: bool) -> Result<(), String> {
    let mut state = state()
        .lock()
        .map_err(|_| "installation queue lock failed".to_string())?;
    state.resumable_source_parts = enabled;
    persist(&state)
}

pub fn resumable_source_parts() -> bool {
    state()
        .lock()
        .map(|state| state.resumable_source_parts)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_sources_are_prioritized_without_duplicates() {
        let mut state = InstallQueueState::default();
        state.enqueue("gnomad", false);
        state.enqueue("clinvar", false);
        state.enqueue("cadd", true);
        assert_eq!(state.next(0), NextWork::Start("cadd".into()));
        assert!(!state.enqueue("gnomad", true).inserted);
        assert_eq!(state.next(0), NextWork::Start("gnomad".into()));
        assert_eq!(state.next(0), NextWork::Start("clinvar".into()));
    }

    #[test]
    fn pause_stops_automatic_queue_advance() {
        let mut state = InstallQueueState {
            concurrency: 2,
            ..InstallQueueState::default()
        };
        state.enqueue("cadd", false);
        state.paused.insert("dbsnp".into());
        state.scheduling_paused = true;
        assert_eq!(state.next(1), NextWork::Wait);
    }

    #[test]
    fn explicit_start_resumes_queue_without_counting_other_paused_sources() {
        let mut state = InstallQueueState {
            concurrency: 1,
            ..InstallQueueState::default()
        };
        state.paused.insert("dbsnp".into());
        state.scheduling_paused = true;
        state.enqueue("cadd", false);
        assert_eq!(state.next(0), NextWork::Start("cadd".into()));
        assert!(state.paused.contains("dbsnp"));
    }

    #[test]
    fn empty_queue_releases_worker_ownership_atomically() {
        let mut state = InstallQueueState::default();
        assert!(state.enqueue("cadd", false).start_worker);
        assert_eq!(state.next(0), NextWork::Start("cadd".into()));
        assert_eq!(state.next(0), NextWork::Idle);
        assert!(state.enqueue("phylop", false).start_worker);
    }

    #[test]
    fn queue_position_includes_running_sources() {
        let mut state = InstallQueueState::default();
        state.enqueue("cadd", false);
        state.enqueue("phylop", false);
        assert_eq!(state.position("cadd", 2), Some(3));
        assert_eq!(state.position("phylop", 2), Some(4));
    }

    #[test]
    fn interrupted_work_returns_to_the_front_of_the_queue() {
        let mut state = InstallQueueState::default();
        state.waiting.push_back("phylop".into());
        state.in_flight.insert("dbsnp".into());
        state.recover_interrupted_work();
        assert_eq!(state.next(0), NextWork::Start("dbsnp".into()));
        assert_eq!(state.next(0), NextWork::Start("phylop".into()));
    }

    #[test]
    fn persisted_queue_keeps_user_settings_but_not_worker_ownership() {
        let mut state = InstallQueueState {
            concurrency: 4,
            resumable_source_parts: false,
            worker_active: true,
            ..InstallQueueState::default()
        };
        state.waiting.push_back("cadd".into());
        state.paused.insert("dbsnp".into());
        state.in_flight.insert("spliceai".into());
        let encoded = serde_json::to_vec(&state).unwrap();
        let restored: InstallQueueState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.concurrency, 4);
        assert!(!restored.resumable_source_parts);
        assert!(!restored.worker_active);
        assert!(restored.waiting.contains(&"cadd".into()));
        assert!(restored.paused.contains("dbsnp"));
        assert!(restored.in_flight.contains("spliceai"));
    }

    #[test]
    fn legacy_queue_defaults_to_resumable_source_parts() {
        let restored: InstallQueueState = serde_json::from_str(
            r#"{"waiting":[],"paused":[],"inFlight":[],"schedulingPaused":false,"concurrency":1}"#,
        )
        .unwrap();
        assert!(restored.resumable_source_parts);
    }
}
