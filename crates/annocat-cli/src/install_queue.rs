use std::collections::{HashSet, VecDeque};
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

#[derive(Debug)]
struct InstallQueueState {
    waiting: VecDeque<String>,
    paused: HashSet<String>,
    scheduling_paused: bool,
    concurrency: usize,
    worker_active: bool,
}

impl Default for InstallQueueState {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            paused: HashSet::new(),
            scheduling_paused: false,
            concurrency: 1,
            worker_active: false,
        }
    }
}

impl InstallQueueState {
    fn enqueue(&mut self, resource_id: &str, prioritize: bool) -> EnqueueResult {
        // Enqueue is an explicit user action. It resumes scheduling, while any
        // other paused sources remain available for a later explicit resume.
        self.scheduling_paused = false;
        let inserted = if self.waiting.iter().any(|queued| queued == resource_id) {
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
            Some(resource_id) => NextWork::Start(resource_id),
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

pub fn enqueue(resource_id: &str, prioritize: bool) -> Result<EnqueueResult, String> {
    state()
        .lock()
        .map(|mut state| state.enqueue(resource_id, prioritize))
        .map_err(|_| "installation queue lock failed".into())
}

pub fn next(running: usize) -> NextWork {
    state()
        .lock()
        .map(|mut state| state.next(running))
        .unwrap_or(NextWork::Wait)
}

pub fn hold(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        state.scheduling_paused = true;
        state.paused.insert(resource_id.into())
    })
}

pub fn release_hold(resource_id: &str) -> bool {
    state()
        .lock()
        .is_ok_and(|mut state| state.paused.remove(resource_id))
}

pub fn remove_waiting(resource_id: &str) -> bool {
    state()
        .lock()
        .is_ok_and(|mut state| state.remove_waiting(resource_id))
}

pub fn remove(resource_id: &str) -> bool {
    state().lock().is_ok_and(|mut state| {
        let waiting = state.remove_waiting(resource_id);
        let paused = state.paused.remove(resource_id);
        if paused {
            state.scheduling_paused = false;
        }
        waiting || paused
    })
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
    Ok(concurrency)
}

pub fn concurrency() -> usize {
    state().lock().map(|state| state.concurrency).unwrap_or(1)
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
        let mut state = InstallQueueState::default();
        state.concurrency = 2;
        state.enqueue("cadd", false);
        state.paused.insert("dbsnp".into());
        state.scheduling_paused = true;
        assert_eq!(state.next(1), NextWork::Wait);
    }

    #[test]
    fn explicit_start_resumes_queue_without_counting_other_paused_sources() {
        let mut state = InstallQueueState::default();
        state.concurrency = 1;
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
}
