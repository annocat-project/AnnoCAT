use annocat_core::ResourceRelease;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_RANGE, RANGE};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SAFETY_RESERVE: u64 = 5 * 1024 * 1024 * 1024;
const MAX_CONCURRENT_DOWNLOADS: usize = 2;
const RANGE_BOOTSTRAP_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
struct JobState {
    state: &'static str,
    phase: &'static str,
    downloaded: u64,
    started_at: Instant,
    started_bytes: u64,
    error: Option<String>,
}

#[derive(Clone)]
struct QueuedJob {
    release: ResourceRelease,
    root: PathBuf,
}

static JOBS: OnceLock<Mutex<HashMap<&'static str, JobState>>> = OnceLock::new();
static QUEUE: OnceLock<Mutex<VecDeque<QueuedJob>>> = OnceLock::new();
static CANCELLED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
static DISCARDED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
static PAUSED_HOLDS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
static SCHEDULER: Mutex<()> = Mutex::new(());

fn jobs() -> &'static Mutex<HashMap<&'static str, JobState>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn queue() -> &'static Mutex<VecDeque<QueuedJob>> {
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn cancelled() -> &'static Mutex<HashSet<&'static str>> {
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn discarded() -> &'static Mutex<HashSet<&'static str>> {
    DISCARDED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn paused_holds() -> &'static Mutex<HashSet<&'static str>> {
    PAUSED_HOLDS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn queue_is_held() -> bool {
    paused_holds().lock().is_ok_and(|holds| !holds.is_empty())
}

fn active_count() -> usize {
    jobs()
        .lock()
        .map(|states| {
            states
                .values()
                .filter(|state| state.state == "running")
                .count()
        })
        .unwrap_or(0)
}

pub fn start_background(release: ResourceRelease, root: PathBuf) -> Result<(), String> {
    release
        .download_bytes
        .ok_or("catalog download size is unknown")?;
    let scheduler = SCHEDULER
        .lock()
        .map_err(|_| "download scheduler lock failed")?;
    let resuming = jobs()
        .lock()
        .map_err(|_| "download state lock failed")?
        .get(release.resource_id)
        .is_some_and(|state| state.state == "paused");
    if resuming && let Ok(mut holds) = paused_holds().lock() {
        holds.remove(release.resource_id);
    }
    let already_running = jobs()
        .lock()
        .map_err(|_| "download state lock failed")?
        .get(release.resource_id)
        .is_some_and(|state| state.state == "running");
    let already_queued = queue()
        .lock()
        .map_err(|_| "download queue lock failed")?
        .iter()
        .any(|item| item.release.resource_id == release.resource_id);
    if already_running || already_queued {
        return Err(format!(
            "{} is already downloading or queued",
            release.resource_id
        ));
    }
    if active_count() >= MAX_CONCURRENT_DOWNLOADS {
        let mut pending = queue().lock().map_err(|_| "download queue lock failed")?;
        let queued = QueuedJob {
            release,
            root: root.clone(),
        };
        if resuming || release.resource_id == "grch38-reference" {
            pending.push_front(queued);
        } else {
            pending.push_back(queued);
        }
        drop(pending);
        persist_queue(&root)?;
        crate::terminal_log(
            "resources",
            format!("{} queued", crate::resource_task_title(release.resource_id)),
        );
        return Ok(());
    }
    begin_job(&release, &root)?;
    persist_queue(&root)?;
    drop(scheduler);
    let fill_root = root.clone();
    spawn_job(release, root);
    if resuming {
        fill_download_slots(&fill_root);
    }
    Ok(())
}

fn begin_job(release: &ResourceRelease, root: &Path) -> Result<(), String> {
    let partial_bytes = partial_path(root, release)
        .metadata()
        .map(|meta| meta.len())
        .unwrap_or(0);
    let validating = final_path(root, release)
        .metadata()
        .is_ok_and(|metadata| metadata.len() == release.download_bytes.unwrap_or(0));
    jobs()
        .lock()
        .map_err(|_| "download state lock failed")?
        .insert(
            release.resource_id,
            JobState {
                state: "running",
                phase: if validating {
                    "validating"
                } else {
                    "downloading"
                },
                downloaded: if validating { 0 } else { partial_bytes },
                started_at: Instant::now(),
                started_bytes: if validating { 0 } else { partial_bytes },
                error: None,
            },
        );
    if let Ok(mut requests) = cancelled().lock() {
        requests.remove(release.resource_id);
    }
    if let Ok(mut requests) = discarded().lock() {
        requests.remove(release.resource_id);
    }
    Ok(())
}

fn spawn_job(release: ResourceRelease, root: PathBuf) {
    std::thread::spawn(move || {
        let title = crate::resource_task_title(release.resource_id);
        let result = download_release_controlled(&release, &root, true);
        if let Ok(mut states) = jobs().lock()
            && let Some(state) = states.get_mut(release.resource_id)
        {
            match result {
                Ok(()) => {
                    crate::terminal_log(
                        "resources",
                        format!("{title} download completed and verified"),
                    );
                    state.state = "complete";
                    state.phase = "complete";
                }
                Err(error) if error == "cancelled" => {
                    let should_discard = discarded()
                        .lock()
                        .is_ok_and(|mut requests| requests.remove(release.resource_id));
                    if should_discard {
                        if let Ok(mut holds) = paused_holds().lock() {
                            holds.remove(release.resource_id);
                        }
                        remove_download_files(&root, &release);
                        crate::terminal_log(
                            "resources",
                            format!("{title} download cancelled and local parts removed"),
                        );
                        state.state = "cancelled";
                        state.phase = "cancelled";
                        state.downloaded = 0;
                    } else {
                        crate::terminal_log(
                            "resources",
                            format!("{title} download paused; partial data retained"),
                        );
                        state.state = "paused";
                        state.phase = "paused";
                    }
                }
                Err(error) => {
                    crate::terminal_log("resources", format!("{title} download failed: {error}"));
                    state.state = "failed";
                    state.phase = "failed";
                    state.error = Some(error);
                }
            }
        }
        if let Ok(mut requests) = cancelled().lock() {
            requests.remove(release.resource_id);
        }
        fill_download_slots(&root);
    });
}

pub fn cancel_resource(resource_id: &str, root: &Path) -> bool {
    let active = jobs()
        .lock()
        .map(|states| {
            states
                .get(resource_id)
                .is_some_and(|state| state.state == "running")
        })
        .unwrap_or(false);
    if active {
        if let Some(id) = annocat_core::source_catalog::download_release(resource_id)
            .map(|release| release.resource_id)
            && let Ok(mut requests) = cancelled().lock()
        {
            if let Ok(mut holds) = paused_holds().lock() {
                holds.insert(id);
            }
            requests.insert(id);
        }
        return true;
    }
    let Ok(mut pending) = queue().lock() else {
        return false;
    };
    if let Some(index) = pending
        .iter()
        .position(|item| item.release.resource_id == resource_id)
    {
        pending.remove(index);
        drop(pending);
        let _ = persist_queue(root);
        return true;
    }
    false
}

pub fn discard_resource(resource_id: &str, root: &Path) -> bool {
    let active = jobs()
        .lock()
        .map(|states| {
            states
                .get(resource_id)
                .is_some_and(|state| state.state == "running")
        })
        .unwrap_or(false);
    if active {
        if let Some(id) = annocat_core::source_catalog::download_release(resource_id)
            .map(|release| release.resource_id)
        {
            if let Ok(mut holds) = paused_holds().lock() {
                holds.remove(id);
            }
            if let Ok(mut requests) = discarded().lock() {
                requests.insert(id);
            }
            if let Ok(mut requests) = cancelled().lock() {
                requests.insert(id);
            }
        }
        return true;
    }
    let paused_release = jobs().lock().ok().and_then(|states| {
        states
            .get(resource_id)
            .is_some_and(|state| state.state == "paused")
            .then(|| annocat_core::source_catalog::download_release(resource_id))
            .flatten()
    });
    if let Some(release) = paused_release {
        remove_download_files(root, &release);
        if let Ok(mut states) = jobs().lock() {
            states.insert(
                release.resource_id,
                JobState {
                    state: "cancelled",
                    phase: "cancelled",
                    downloaded: 0,
                    started_at: Instant::now(),
                    started_bytes: 0,
                    error: None,
                },
            );
        }
        if let Ok(mut holds) = paused_holds().lock() {
            holds.remove(release.resource_id);
        }
        fill_download_slots(root);
        return true;
    }
    let removed = queue().lock().ok().and_then(|mut pending| {
        let index = pending
            .iter()
            .position(|item| item.release.resource_id == resource_id)?;
        let item = pending.remove(index)?;
        drop(pending);
        let _ = persist_queue(root);
        Some(item.release)
    });
    if let Some(release) = removed {
        remove_download_files(root, &release);
        if let Ok(mut states) = jobs().lock() {
            states.insert(
                release.resource_id,
                JobState {
                    state: "cancelled",
                    phase: "cancelled",
                    downloaded: 0,
                    started_at: Instant::now(),
                    started_bytes: 0,
                    error: None,
                },
            );
        }
        return true;
    }
    false
}

pub fn is_resource_active(resource_id: &str) -> bool {
    jobs().lock().is_ok_and(|states| {
        states
            .get(resource_id)
            .is_some_and(|state| state.state == "running")
    }) || queue().lock().is_ok_and(|pending| {
        pending
            .iter()
            .any(|item| item.release.resource_id == resource_id)
    })
}

pub fn is_downloaded(release: &ResourceRelease, root: &Path) -> bool {
    final_path(root, release)
        .metadata()
        .is_ok_and(|metadata| metadata.len() == release.download_bytes.unwrap_or(0))
        && is_verified(root, release)
}

pub fn is_running() -> bool {
    jobs()
        .lock()
        .map(|states| states.values().any(|state| state.state == "running"))
        .unwrap_or(false)
        || queue()
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    pub state: String,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub expected_bytes: u64,
    pub percent: f64,
    pub throughput_bytes_per_second: f64,
    pub queue_position: Option<usize>,
    pub error: Option<String>,
}

pub fn status(release: &ResourceRelease, root: &Path) -> DownloadStatus {
    let expected = release.download_bytes.unwrap_or(0);
    let final_bytes = final_path(root, release)
        .metadata()
        .map(|meta| meta.len())
        .unwrap_or(0);
    let final_exists = final_bytes == expected && is_verified(root, release);
    let partial_bytes = partial_path(root, release)
        .metadata()
        .map(|meta| meta.len())
        .unwrap_or(0);
    let state = jobs()
        .lock()
        .ok()
        .and_then(|states| states.get(release.resource_id).cloned());
    let belongs_to_resource = state.is_some();
    let state = state.unwrap_or(JobState {
        state: "idle",
        phase: "idle",
        downloaded: partial_bytes,
        started_at: Instant::now(),
        started_bytes: partial_bytes,
        error: None,
    });
    let active_downloads = active_count();
    let queue_position = queue()
        .lock()
        .ok()
        .and_then(|pending| {
            pending
                .iter()
                .position(|item| item.release.resource_id == release.resource_id)
        })
        .map(|index| index + active_downloads + 1);
    let effective_state = if final_exists {
        "downloaded"
    } else if queue_position.is_some() {
        "queued"
    } else if belongs_to_resource
        && state.state == "running"
        && discarded()
            .lock()
            .is_ok_and(|requests| requests.contains(release.resource_id))
    {
        "cancelling"
    } else if belongs_to_resource && state.state == "running" && state.phase == "validating" {
        "validating"
    } else if belongs_to_resource && state.state == "cancelled" && partial_bytes == 0 {
        "idle"
    } else if (!belongs_to_resource || state.state == "idle") && partial_bytes > 0 {
        "paused"
    } else if !belongs_to_resource {
        "idle"
    } else {
        state.state
    };
    let downloaded = if final_exists {
        final_bytes
    } else if belongs_to_resource && state.state == "running" {
        state.downloaded
    } else if final_bytes == expected {
        final_bytes
    } else {
        partial_bytes
    };
    let percent = if expected > 0 {
        downloaded as f64 * 100.0 / expected as f64
    } else {
        0.0
    };
    let throughput_bytes_per_second = if state.state == "running" {
        let elapsed = state.started_at.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            downloaded.saturating_sub(state.started_bytes) as f64 / elapsed
        } else {
            0.0
        }
    } else {
        0.0
    };
    DownloadStatus {
        state: effective_state.into(),
        phase: state.phase.into(),
        downloaded_bytes: downloaded,
        expected_bytes: expected,
        percent,
        throughput_bytes_per_second,
        queue_position,
        error: belongs_to_resource.then_some(state.error).flatten(),
    }
}

fn fill_download_slots(root: &Path) {
    let Ok(scheduler) = SCHEDULER.lock() else {
        return;
    };
    if queue_is_held() {
        return;
    }
    let mut launches = Vec::new();
    while active_count() < MAX_CONCURRENT_DOWNLOADS {
        let next = queue()
            .lock()
            .ok()
            .and_then(|mut pending| pending.pop_front());
        let Some(next) = next else {
            break;
        };
        if begin_job(&next.release, &next.root).is_ok() {
            launches.push(next);
        }
    }
    let _ = persist_queue(root);
    drop(scheduler);
    for next in launches {
        spawn_job(next.release, next.root);
    }
}

fn persist_queue(root: &Path) -> Result<(), String> {
    let states = jobs().lock().map_err(|_| "download state lock failed")?;
    let pending = queue().lock().map_err(|_| "download queue lock failed")?;
    let ids: Vec<&str> = states
        .iter()
        .filter_map(|(id, state)| (state.state == "running").then_some(*id))
        .chain(pending.iter().map(|item| item.release.resource_id))
        .collect();
    let bytes = serde_json::to_vec_pretty(&ids).map_err(|error| error.to_string())?;
    fs::write(root.join("download-queue.json"), bytes).map_err(|error| error.to_string())
}

pub fn restore_queue(root: &Path) {
    let Ok(bytes) = fs::read(root.join("download-queue.json")) else {
        return;
    };
    let Ok(ids) = serde_json::from_slice::<Vec<String>>(&bytes) else {
        return;
    };
    let _ = fs::write(root.join("download-queue.json"), b"[]");
    for id in ids {
        if let Some(release) = annocat_core::source_catalog::download_release(&id)
            && !final_path(root, &release).exists()
        {
            let _ = start_background(release, root.to_path_buf());
        }
    }
}

pub fn print_download_plan(release: &ResourceRelease, root: &Path) -> Result<(), String> {
    let expected = release
        .download_bytes
        .ok_or("catalog download size is unknown")?;
    let partial = partial_path(root, release);
    let existing = partial.metadata().map(|meta| meta.len()).unwrap_or(0);
    if existing > expected {
        return Err(format!(
            "partial file is larger than the catalog object: {} > {} bytes",
            existing, expected
        ));
    }
    fs::create_dir_all(root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
    let free = fs2::available_space(root)
        .map_err(|error| format!("cannot read free space for {}: {error}", root.display()))?;
    let remaining = expected - existing;
    println!(
        "Download plan for {} {}",
        release.resource_id, release.version
    );
    println!("  Archive          : {}", release.filename);
    println!(
        "  Destination      : {}",
        final_path(root, release).display()
    );
    println!("  Expected size    : {}", format_decimal_size(expected));
    println!("  Existing partial : {}", format_decimal_size(existing));
    println!("  Remaining        : {}", format_decimal_size(remaining));
    println!("  Free space       : {}", format_decimal_size(free));
    println!(
        "  Safety reserve   : {}",
        format_decimal_size(SAFETY_RESERVE)
    );
    println!("  Range resume     : {}", release.range_resume);
    println!("  Installed size   : unknown until archive inventory is implemented");
    if free < remaining.saturating_add(SAFETY_RESERVE) {
        return Err("insufficient free space for the remaining archive plus safety reserve".into());
    }
    Ok(())
}

pub fn download_release(release: &ResourceRelease, root: &Path) -> Result<(), String> {
    download_release_controlled(release, root, false)
}

fn download_release_controlled(
    release: &ResourceRelease,
    root: &Path,
    controlled: bool,
) -> Result<(), String> {
    print_download_plan(release, root)?;
    let expected = release
        .download_bytes
        .ok_or("catalog download size is unknown")?;
    let partial = partial_path(root, release);
    let final_file = final_path(root, release);
    if final_file.exists() {
        if final_file.metadata().map(|value| value.len()).unwrap_or(0) != expected {
            return Err(format!(
                "existing archive has the wrong size; move or delete {} before retrying",
                final_file.display()
            ));
        }
        validate_archive_structure(&final_file, release.archive_format)?;
        let local_sha256 = validate_existing_archive_digests(&final_file, release, controlled)?;
        write_verification(&final_file, release, &local_sha256)?;
        return Ok(());
    }
    let mut existing = partial.metadata().map(|meta| meta.len()).unwrap_or(0);
    let client = Client::builder()
        .cookie_store(true)
        .connect_timeout(Duration::from_secs(30))
        .http1_only()
        .user_agent("AnnoCAT/0.1 local resource downloader")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| format!("cannot create HTTPS client: {error}"))?;
    if existing > 0 && !release.range_resume {
        return Err(
            "this source does not support resume; remove its partial file and retry".into(),
        );
    }
    if existing == 0 && release.range_resume {
        existing = download_range_bootstrap(&client, release, &partial, expected, controlled)?;
    }
    let mut request = client.get(release.url);
    if existing > 0 || release.range_resume {
        request = request.header(RANGE, format!("bytes={existing}-"));
    }
    let mut response =
        send_cancellable(request, release.resource_id, controlled).map_err(|error| {
            if error == "cancelled" {
                error
            } else {
                format!("download request failed: {error}")
            }
        })?;
    validate_response(&response, existing, expected, release.range_resume)?;
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial)
        .map_err(|error| format!("cannot open {}: {error}", partial.display()))?;
    let mut downloaded = existing;
    let mut digest = if release.publisher_md5.is_some() {
        let mut context = md5::Context::new();
        if existing > 0 {
            let mut input = File::open(&partial)
                .map_err(|error| format!("cannot hash {}: {error}", partial.display()))?;
            std::io::copy(&mut input, &mut DigestWriter(&mut context))
                .map_err(|error| format!("cannot hash partial archive: {error}"))?;
        }
        Some(context)
    } else {
        None
    };
    let mut sha256 = Sha256::new();
    if existing > 0 {
        let mut input = File::open(&partial)
            .map_err(|error| format!("cannot hash {}: {error}", partial.display()))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(|error| format!("cannot hash partial archive: {error}"))?;
            if count == 0 {
                break;
            }
            sha256.update(&buffer[..count]);
        }
    }
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if controlled
            && cancelled()
                .lock()
                .is_ok_and(|requests| requests.contains(release.resource_id))
        {
            output
                .sync_all()
                .map_err(|error| format!("cannot preserve partial archive: {error}"))?;
            return Err("cancelled".into());
        }
        let count = response
            .read(&mut buffer)
            .map_err(|error| format!("download interrupted at {downloaded} bytes: {error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write {}: {error}", partial.display()))?;
        downloaded += count as u64;
        if let Some(context) = digest.as_mut() {
            context.consume(&buffer[..count]);
        }
        sha256.update(&buffer[..count]);
        if controlled
            && let Ok(mut states) = jobs().lock()
            && let Some(state) = states.get_mut(release.resource_id)
        {
            state.downloaded = downloaded;
        }
        if downloaded > expected {
            return Err("server sent more bytes than the catalog object length".into());
        }
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot flush partial archive: {error}"))?;
    if downloaded != expected {
        return Err(format!(
            "truncated download: received {downloaded}, expected {expected} bytes"
        ));
    }
    validate_archive_structure(&partial, release.archive_format)?;
    if let (Some(expected_md5), Some(context)) = (release.publisher_md5, digest) {
        let actual = format!("{:x}", context.finalize());
        if !actual.eq_ignore_ascii_case(expected_md5) {
            return Err(format!(
                "publisher MD5 mismatch: expected {expected_md5}, found {actual}"
            ));
        }
    }
    let local_sha256 = format!("{:x}", sha256.finalize());
    if let Some(expected_sha256) = release.publisher_sha256 {
        let actual = &local_sha256;
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(format!(
                "publisher SHA-256 mismatch: expected {expected_sha256}, found {actual}"
            ));
        }
    }
    fs::rename(&partial, &final_file)
        .map_err(|error| format!("cannot atomically promote archive: {error}"))?;
    write_verification(&final_file, release, &local_sha256)?;
    Ok(())
}

fn validate_existing_archive_digests(
    path: &Path,
    release: &ResourceRelease,
    controlled: bool,
) -> Result<String, String> {
    let mut input = File::open(path).map_err(|error| format!("cannot hash archive: {error}"))?;
    let mut md5 = release.publisher_md5.map(|_| md5::Context::new());
    let mut sha256 = Sha256::new();
    let mut processed = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if controlled
            && cancelled()
                .lock()
                .is_ok_and(|requests| requests.contains(release.resource_id))
        {
            return Err("cancelled".into());
        }
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash archive: {error}"))?;
        if count == 0 {
            break;
        }
        if let Some(context) = md5.as_mut() {
            context.consume(&buffer[..count]);
        }
        sha256.update(&buffer[..count]);
        processed += count as u64;
        if controlled
            && let Ok(mut states) = jobs().lock()
            && let Some(state) = states.get_mut(release.resource_id)
        {
            state.downloaded = processed;
        }
    }
    if let (Some(expected), Some(context)) = (release.publisher_md5, md5) {
        let actual = format!("{:x}", context.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "publisher MD5 mismatch: expected {expected}, found {actual}"
            ));
        }
    }
    let local_sha256 = format!("{:x}", sha256.finalize());
    if let Some(expected) = release.publisher_sha256
        && !local_sha256.eq_ignore_ascii_case(expected)
    {
        return Err(format!(
            "publisher SHA-256 mismatch: expected {expected}, found {local_sha256}"
        ));
    }
    Ok(local_sha256)
}

fn download_range_bootstrap(
    client: &Client,
    release: &ResourceRelease,
    partial: &Path,
    expected: u64,
    controlled: bool,
) -> Result<u64, String> {
    let bytes = RANGE_BOOTSTRAP_BYTES.min(expected);
    let request = client
        .get(release.url)
        .header(RANGE, format!("bytes=0-{}", bytes - 1))
        .timeout(Duration::from_secs(30));
    let mut response =
        send_cancellable(request, release.resource_id, controlled).map_err(|error| {
            if error == "cancelled" {
                error
            } else {
                format!("download bootstrap request failed: {error}")
            }
        })?;
    validate_response(&response, 0, expected, true)?;
    if response
        .content_length()
        .is_some_and(|length| length != bytes)
    {
        return Err("download bootstrap returned an unexpected byte count".into());
    }
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(partial)
        .map_err(|error| format!("cannot open {}: {error}", partial.display()))?;
    let mut downloaded = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    while downloaded < bytes {
        if controlled
            && cancelled()
                .lock()
                .is_ok_and(|requests| requests.contains(release.resource_id))
        {
            return Err("cancelled".into());
        }
        let count = response
            .read(&mut buffer)
            .map_err(|error| format!("download bootstrap interrupted: {error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write {}: {error}", partial.display()))?;
        downloaded += count as u64;
        if controlled
            && let Ok(mut states) = jobs().lock()
            && let Some(state) = states.get_mut(release.resource_id)
        {
            state.downloaded = downloaded;
        }
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot flush bootstrap partial: {error}"))?;
    if downloaded != bytes {
        return Err(format!(
            "truncated download bootstrap: received {downloaded}, expected {bytes} bytes"
        ));
    }
    Ok(downloaded)
}

fn send_cancellable(
    request: reqwest::blocking::RequestBuilder,
    resource_id: &'static str,
    controlled: bool,
) -> Result<reqwest::blocking::Response, String> {
    if !controlled {
        return request.send().map_err(|error| error.to_string());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(request.send());
    });
    loop {
        if cancelled()
            .lock()
            .is_ok_and(|requests| requests.contains(resource_id))
        {
            return Err("cancelled".into());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => return result.map_err(|error| error.to_string()),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("download request worker stopped unexpectedly".into());
            }
        }
    }
}

fn validate_response(
    response: &reqwest::blocking::Response,
    start: u64,
    expected: u64,
    range_requested: bool,
) -> Result<(), String> {
    if start == 0 && response.status() == StatusCode::OK {
        if response
            .content_length()
            .is_some_and(|length| length != expected)
        {
            return Err("server Content-Length differs from the pinned catalog size".into());
        }
        return Ok(());
    }
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(format!(
            "server did not honor resume; expected 206, received {}",
            response.status()
        ));
    }
    if !range_requested {
        return Err("server unexpectedly returned partial content".into());
    }
    let value = response
        .headers()
        .get(CONTENT_RANGE)
        .ok_or("206 response omitted Content-Range")?
        .to_str()
        .map_err(|_| "invalid Content-Range header")?;
    let (range, total) = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .ok_or("invalid Content-Range syntax")?;
    let (actual_start, _) = range
        .split_once('-')
        .ok_or("invalid Content-Range byte range")?;
    let actual_start: u64 = actual_start
        .parse()
        .map_err(|_| "invalid Content-Range start")?;
    let total: u64 = total.parse().map_err(|_| "invalid Content-Range total")?;
    if actual_start != start || total != expected {
        return Err(format!(
            "remote object changed or resume mismatch: start {actual_start}/{start}, total {total}/{expected}"
        ));
    }
    Ok(())
}

fn validate_archive_structure(path: &Path, format: &str) -> Result<(), String> {
    if format == "gzip" || format == "bgzip" {
        let mut file =
            File::open(path).map_err(|error| format!("cannot validate gzip: {error}"))?;
        let mut signature = [0_u8; 2];
        file.read_exact(&mut signature)
            .map_err(|error| error.to_string())?;
        return if signature == [0x1f, 0x8b] {
            Ok(())
        } else {
            Err("gzip signature is missing".into())
        };
    }
    if format == "plain" {
        return path
            .metadata()
            .map_err(|error| error.to_string())
            .and_then(|metadata| {
                if metadata.len() == 0 {
                    Err("plain file is empty".into())
                } else {
                    Ok(())
                }
            });
    }
    if format != "zip" {
        return Err(format!("unsupported archive format: {format}"));
    }
    let mut file = File::open(path).map_err(|error| format!("cannot validate ZIP: {error}"))?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length < 22 {
        return Err("ZIP is too short".into());
    }
    let mut signature = [0_u8; 4];
    file.read_exact(&mut signature)
        .map_err(|error| error.to_string())?;
    if &signature != b"PK\x03\x04" {
        return Err("ZIP local-file signature is missing".into());
    }
    let tail_size = length.min(128 * 1024) as usize;
    file.seek(SeekFrom::End(-(tail_size as i64)))
        .map_err(|error| error.to_string())?;
    let mut tail = vec![0_u8; tail_size];
    file.read_exact(&mut tail)
        .map_err(|error| error.to_string())?;
    if !tail.windows(4).any(|window| window == b"PK\x05\x06") {
        return Err("ZIP end-of-central-directory record is missing".into());
    }
    Ok(())
}

fn partial_path(root: &Path, release: &ResourceRelease) -> PathBuf {
    root.join(format!("{}.partial", release.filename))
}

fn remove_download_files(root: &Path, release: &ResourceRelease) {
    for path in [
        partial_path(root, release),
        final_path(root, release),
        verification_path(root, release),
    ] {
        let _ = fs::remove_file(path);
    }
}
pub(crate) fn final_path(root: &Path, release: &ResourceRelease) -> PathBuf {
    root.join(release.filename)
}
fn verification_path(root: &Path, release: &ResourceRelease) -> PathBuf {
    root.join(format!("{}.verified.json", release.filename))
}

fn archive_modified_nanos(path: &Path) -> Option<u128> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn is_verified(root: &Path, release: &ResourceRelease) -> bool {
    let archive = final_path(root, release);
    let expected_modified = archive_modified_nanos(&archive).map(|value| value.to_string());
    fs::read(verification_path(root, release))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            value["resourceId"] == release.resource_id
                && value["version"] == release.version
                && value["archiveBytes"].as_u64() == release.download_bytes
                && value["publisherMd5"].as_str() == release.publisher_md5
                && value["publisherSha256"].as_str() == release.publisher_sha256
                && value["localSha256"]
                    .as_str()
                    .is_some_and(|hash| hash.len() == 64)
                && value["archiveModifiedNanos"].as_str() == expected_modified.as_deref()
        })
}

fn write_verification(
    path: &Path,
    release: &ResourceRelease,
    local_sha256: &str,
) -> Result<(), String> {
    let value = serde_json::json!({
        "resourceId": release.resource_id,
        "version": release.version,
        "archiveBytes": release.download_bytes,
        "publisherMd5": release.publisher_md5,
        "publisherSha256": release.publisher_sha256,
        "localSha256": local_sha256,
        "archiveModifiedNanos": archive_modified_nanos(path).map(|value| value.to_string()),
        "validation": "catalog size + archive structure + publisher digest when available"
    });
    fs::write(
        verification_path(path.parent().unwrap_or_else(|| Path::new("")), release),
        serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write verification manifest: {error}"))
}

struct DigestWriter<'a>(&'a mut md5::Context);

impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.consume(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn format_decimal_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn paths_never_confuse_partial_with_downloaded() {
        let release = &annocat_core::source_catalog::download_release("dbnsfp").unwrap();
        assert!(
            partial_path(Path::new("data"), release)
                .to_string_lossy()
                .ends_with(".zip.partial")
        );
        assert!(
            final_path(Path::new("data"), release)
                .to_string_lossy()
                .ends_with(".zip")
        );
    }

    #[test]
    fn completed_archive_reports_full_progress() {
        let release = ResourceRelease {
            resource_id: "test",
            version: "1",
            filename: "test.zip",
            url: "https://example.invalid/test.zip",
            download_bytes: Some(4),
            range_resume: true,
            size_checked_at: "test",
            archive_format: "zip",
            publisher_md5: None,
            publisher_sha256: None,
        };
        let root = std::env::temp_dir().join(format!(
            "annocat-status-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(release.filename), [0_u8; 4]).unwrap();
        let archive = final_path(&root, &release);
        let local_sha256 = validate_existing_archive_digests(&archive, &release, false).unwrap();
        write_verification(&archive, &release, &local_sha256).unwrap();

        let status = status(&release, &root);
        assert_eq!(status.state, "downloaded");
        assert_eq!(status.downloaded_bytes, 4);
        assert_eq!(status.percent, 100.0);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(verification_path(&root, &release)).unwrap()).unwrap();
        assert_eq!(manifest["localSha256"], local_sha256);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destructive_cleanup_removes_partial_archive_and_verification() {
        let release = ResourceRelease {
            resource_id: "test-cleanup",
            version: "1",
            filename: "test-cleanup.gz",
            url: "https://example.invalid/test-cleanup.gz",
            download_bytes: Some(4),
            range_resume: true,
            size_checked_at: "test",
            archive_format: "gzip",
            publisher_md5: None,
            publisher_sha256: None,
        };
        let root = std::env::temp_dir().join(format!(
            "annocat-cleanup-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        for path in [
            partial_path(&root, &release),
            final_path(&root, &release),
            verification_path(&root, &release),
        ] {
            fs::write(path, b"test").unwrap();
        }

        remove_download_files(&root, &release);

        assert!(!partial_path(&root, &release).exists());
        assert!(!final_path(&root, &release).exists());
        assert!(!verification_path(&root, &release).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_restart_resumes_an_existing_partial_with_http_range() {
        const BODY: &[u8] = b"\x1f\x8bannocat-resume-fixture";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]).into_owned();
            sender.send(request).unwrap();
            let start = 7;
            let remaining = &BODY[start..];
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                remaining.len(),
                BODY.len() - 1,
                BODY.len()
            )
            .unwrap();
            stream.write_all(remaining).unwrap();
        });
        let url: &'static str = Box::leak(format!("http://{address}/fixture.gz").into_boxed_str());
        let release = ResourceRelease {
            resource_id: "restart-resume-fixture",
            version: "1",
            filename: "restart-resume-fixture.gz",
            url,
            download_bytes: Some(BODY.len() as u64),
            range_resume: true,
            size_checked_at: "test",
            archive_format: "gzip",
            publisher_md5: None,
            publisher_sha256: None,
        };
        let root = std::env::temp_dir().join(format!(
            "annocat-restart-resume-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(partial_path(&root, &release), &BODY[..7]).unwrap();

        download_release(&release, &root).unwrap();

        let request = receiver.recv().unwrap();
        assert!(request.to_ascii_lowercase().contains("range: bytes=7-"));
        assert_eq!(fs::read(final_path(&root, &release)).unwrap(), BODY);
        assert!(!partial_path(&root, &release).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paused_download_holds_scheduler_until_resume_or_delete() {
        let resource_id = "dbnsfp";
        paused_holds().lock().unwrap().insert(resource_id);
        assert!(queue_is_held());
        paused_holds().lock().unwrap().remove(resource_id);
        assert!(!queue_is_held());
    }
}
