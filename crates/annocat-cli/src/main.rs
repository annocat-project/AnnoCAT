use annocat_core::{
    demo_variants_json, practical_resource_plan_json, profiles_json,
    resource_catalog_candidates_json, sources_json,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedCatalogRelease {
    version: String,
    url: String,
    download_bytes: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    index_url: Option<String>,
    index_bytes: Option<u64>,
    index_md5: Option<String>,
    index_last_modified: Option<String>,
}

const ROLLING_RESOURCE_IDS: &[&str] = &["clinvar", "dbsnp"];

fn is_rolling_resource(resource_id: &str) -> bool {
    ROLLING_RESOURCE_IDS.contains(&resource_id)
}

fn http_content_length_header(
    headers: &reqwest::header::HeaderMap,
    error: &str,
) -> Result<u64, String> {
    headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| error.to_string())
}

fn latest_clinvar_filename(directory_listing: &str) -> Option<String> {
    directory_listing
        .split('"')
        .filter(|value| {
            value.starts_with("clinvar_")
                && value.ends_with(".vcf.gz")
                && value.len() == "clinvar_20260715.vcf.gz".len()
                && value.as_bytes()[8..16].iter().all(u8::is_ascii_digit)
        })
        .max()
        .map(str::to_owned)
}

fn clinvar_md5(sidecar: &str) -> Result<String, String> {
    let digest = sidecar
        .split_whitespace()
        .next()
        .ok_or("the ClinVar MD5 sidecar is empty")?
        .to_ascii_lowercase();
    if digest.len() != 32 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the ClinVar MD5 sidecar contains an invalid digest".into());
    }
    Ok(digest)
}

fn latest_dbsnp_filename(directory_listing: &str) -> Option<String> {
    directory_listing
        .split('"')
        .filter_map(|value| {
            let version = value
                .strip_prefix("GCF_000001405.")?
                .strip_suffix(".gz")?
                .parse::<u32>()
                .ok()?;
            Some((version, value.to_owned()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, filename)| filename)
}

fn dbsnp_build(release_notes: &str) -> Option<String> {
    let lower = release_notes.to_ascii_lowercase();
    let suffix = lower.split_once("dbsnp build ")?.1;
    let build = suffix
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    (!build.is_empty()).then_some(build)
}

fn resolve_dbsnp_release() -> Result<ResolvedCatalogRelease, String> {
    const DIRECTORY: &str = "https://ftp.ncbi.nlm.nih.gov/snp/latest_release/VCF/";
    const NOTES: &str = "https://ftp.ncbi.nlm.nih.gov/snp/latest_release/release_notes.txt";
    let client = reqwest::blocking::Client::builder()
        .user_agent("AnnoCat/0.1 source resolver")
        .build()
        .map_err(|error| format!("cannot create dbSNP resolver: {error}"))?;
    let listing = client
        .get(DIRECTORY)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot discover the current dbSNP files: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the dbSNP file listing: {error}"))?;
    let filename = latest_dbsnp_filename(&listing)
        .ok_or("the dbSNP directory did not contain a GRCh38 VCF")?;
    let notes = client
        .get(NOTES)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot discover the current dbSNP build: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the dbSNP release notes: {error}"))?;
    let build = dbsnp_build(&notes).ok_or("the dbSNP release notes omitted the build number")?;
    let url = format!("{DIRECTORY}{filename}");
    let response = client
        .head(&url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("the discovered dbSNP object is unavailable: {error}"))?;
    let download_bytes = http_content_length_header(
        response.headers(),
        "the discovered dbSNP object omitted or returned an invalid Content-Length",
    )?;
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let checksum = client
        .get(format!("{url}.md5"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot fetch the dbSNP MD5 sidecar: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the dbSNP MD5 sidecar: {error}"))
        .and_then(|sidecar| clinvar_md5(&sidecar))?;
    let index_url = format!("{url}.tbi");
    let index_response = client
        .head(&index_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("the discovered dbSNP tabix index is unavailable: {error}"))?;
    let index_bytes = http_content_length_header(
        index_response.headers(),
        "the discovered dbSNP tabix index omitted or returned an invalid Content-Length",
    )?;
    let index_last_modified = index_response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let index_md5 = client
        .get(format!("{index_url}.md5"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot fetch the dbSNP tabix MD5 sidecar: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the dbSNP tabix MD5 sidecar: {error}"))
        .and_then(|sidecar| clinvar_md5(&sidecar))?;
    let assembly = filename
        .strip_suffix(".gz")
        .ok_or("the discovered dbSNP filename is malformed")?;
    Ok(ResolvedCatalogRelease {
        version: format!("b{build}-{assembly}"),
        url,
        download_bytes,
        etag: Some(format!("md5:{checksum}")),
        last_modified,
        index_url: Some(index_url),
        index_bytes: Some(index_bytes),
        index_md5: Some(index_md5),
        index_last_modified,
    })
}

fn resolve_clinvar_release() -> Result<ResolvedCatalogRelease, String> {
    const DIRECTORY: &str = "https://ftp.ncbi.nlm.nih.gov/pub/clinvar/vcf_GRCh38/";
    let client = reqwest::blocking::Client::builder()
        .user_agent("AnnoCat/0.1 source resolver")
        .build()
        .map_err(|error| format!("cannot create ClinVar resolver: {error}"))?;
    let listing = client
        .get(DIRECTORY)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot discover the current ClinVar release: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the ClinVar release listing: {error}"))?;
    let filename = latest_clinvar_filename(&listing)
        .ok_or("the ClinVar directory did not contain a dated GRCh38 VCF")?;
    let version = filename
        .strip_prefix("clinvar_")
        .and_then(|value| value.strip_suffix(".vcf.gz"))
        .ok_or("the discovered ClinVar filename is malformed")?
        .to_owned();
    let url = format!("{DIRECTORY}{filename}");
    let response = client
        .head(&url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("the discovered ClinVar object is unavailable: {error}"))?;
    let download_bytes = http_content_length_header(
        response.headers(),
        "the discovered ClinVar object omitted or returned an invalid Content-Length",
    )?;
    let text_header = |name: reqwest::header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let checksum = client
        .get(format!("{url}.md5"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot fetch the ClinVar MD5 sidecar: {error}"))?
        .text()
        .map_err(|error| format!("cannot read the ClinVar MD5 sidecar: {error}"))
        .and_then(|sidecar| clinvar_md5(&sidecar))?;
    Ok(ResolvedCatalogRelease {
        version,
        url,
        download_bytes,
        // NCBI's dated ClinVar snapshots publish an MD5 sidecar but generally
        // omit ETag. Record the checksum in the identity so a weekly size
        // change is accepted only when the streamed object matches NCBI's
        // digest.
        etag: Some(format!("md5:{checksum}")),
        last_modified: text_header(reqwest::header::LAST_MODIFIED),
        index_url: None,
        index_bytes: None,
        index_md5: None,
        index_last_modified: None,
    })
}

fn installed_resource_versions(resource_id: &str, resources: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(resources.join(resource_id)) else {
        return Vec::new();
    };
    let mut versions = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "partial" && !name.ends_with(".partial"))
        .collect::<Vec<_>>();
    versions.sort();
    versions
}

fn resource_update_status_json(resource_id: &str) -> Result<String, String> {
    let paths = portable_paths()?;
    let (current_version, policy) = if resource_id == "clinvar" {
        (resolve_clinvar_release()?.version, "rolling-snapshot")
    } else if resource_id == "dbsnp" {
        (resolve_dbsnp_release()?.version, "rolling-snapshot")
    } else if resource_id == "ensembl-gff3" {
        ("115".to_string(), "compatibility-pinned")
    } else {
        let release = resource_release(resource_id)?;
        (release.version.to_string(), "catalog-pinned")
    };
    let installed_versions = if resource_id == "ensembl-gff3" {
        if transcript::is_ready(&paths.resources) {
            vec!["115".to_string()]
        } else {
            Vec::new()
        }
    } else {
        installed_resource_versions(resource_id, &paths.resources)
    };
    let update_available = !installed_versions.is_empty()
        && !installed_versions
            .iter()
            .any(|version| version == &current_version);
    serde_json::to_string(&serde_json::json!({
        "resourceId": resource_id,
        "policy": policy,
        "currentVersion": current_version,
        "installedVersions": installed_versions,
        "installed": !installed_versions.is_empty(),
        "updateAvailable": update_available
    }))
    .map_err(|error| error.to_string())
}

mod annotation;
mod cache_contract;
mod csq;
mod downloader;
mod fastvep;
mod install_queue;
mod library_metadata;
mod preparation;
mod reference;
mod report_import;
mod report_library;
mod report_package;
mod results;
mod tasks;
mod transcript;
mod worker;

pub(crate) fn terminal_log(component: &str, message: impl AsRef<str>) {
    let mut width = terminal_line_width()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if *width > 0 {
        eprint!("\r{:width$}\r", "", width = *width);
        *width = 0;
    }
    eprintln!(
        "{} [{component}] {}",
        terminal_timestamp(),
        message.as_ref()
    );
}

fn terminal_timestamp() -> String {
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
        return format!(
            "{month} {}, {} {hour}:{:02}:{:02} {period}",
            time.wDay, time.wYear, time.wMinute, time.wSecond
        );
    }
    #[cfg(not(windows))]
    {
        annotation::current_timestamp()
    }
}

static TERMINAL_LINE_WIDTH: std::sync::OnceLock<std::sync::Mutex<usize>> =
    std::sync::OnceLock::new();

fn terminal_line_width() -> &'static std::sync::Mutex<usize> {
    TERMINAL_LINE_WIDTH.get_or_init(|| std::sync::Mutex::new(0))
}

fn terminal_progress(message: &str) {
    let mut width = terminal_line_width()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if message.is_empty() {
        if *width > 0 {
            eprint!("\r{:width$}\r", "", width = *width);
            let _ = io::stderr().flush();
            *width = 0;
        }
        return;
    }
    let available = terminal_viewport_width().saturating_sub(1).max(20);
    let message = if message.chars().count() > available {
        let mut compact = message
            .chars()
            .take(available.saturating_sub(1))
            .collect::<String>();
        compact.push('…');
        compact
    } else {
        message.to_owned()
    };
    let display_width = (*width).max(message.chars().count()).min(available);
    eprint!("\r{message:<display_width$}");
    let _ = io::stderr().flush();
    *width = display_width;
}

fn terminal_viewport_width() -> usize {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_ERROR_HANDLE,
        };
        let handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if !handle.is_null() && unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } != 0 {
            return (i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1).max(20)
                as usize;
        }
    }
    100
}

fn format_terminal_rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return String::new();
    }
    let (value, unit) = if bytes_per_second >= 1_000_000_000.0 {
        (bytes_per_second / 1_000_000_000.0, "GB/s")
    } else if bytes_per_second >= 1_000_000.0 {
        (bytes_per_second / 1_000_000.0, "MB/s")
    } else if bytes_per_second >= 1000.0 {
        (bytes_per_second / 1000.0, "KB/s")
    } else {
        (bytes_per_second, "B/s")
    };
    format!("{value:.1} {unit}")
}

fn format_terminal_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn terminal_task_activity(task: &tasks::TaskSnapshot) -> String {
    let rate = format_terminal_rate(task.throughput_bytes_per_second);
    if !rate.is_empty() {
        return format!("{:.1}% {rate}", task.percent);
    }
    if let Some((_, detail)) = task.detail.split_once(": ") {
        if let Some(replay) = detail.strip_prefix("replaying ") {
            return format!(
                "replay {}",
                replay
                    .strip_suffix(" retained hybrid part")
                    .unwrap_or(replay)
                    .replace(" of ", "/")
            );
        }
        if detail.starts_with("reconnecting") || detail.starts_with("validating") {
            return detail.to_owned();
        }
    }
    if task.kind == "annotation" {
        return task.phase.replace('-', " ");
    }
    format!("{:.1}% {}", task.percent, task.phase.replace('-', " "))
}

fn terminal_active_summary() -> String {
    let active = portable_paths()
        .map(|paths| {
            task_snapshots(&paths)
                .into_iter()
                .filter(tasks::TaskSnapshot::is_active)
                .map(|task| {
                    let label = task.resource_id.as_deref().unwrap_or(task.kind).to_owned();
                    let chromosome = task
                        .chromosome
                        .as_deref()
                        .map(|value| format!(" chr{value}"))
                        .unwrap_or_default();
                    format!("{label}{chromosome} {}", terminal_task_activity(&task))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if active.is_empty() {
        String::new()
    } else {
        format!("[active] {}", active.join(" | "))
    }
}

const INDEX_HTML: &str = include_str!("../../../web/index.html");
const APP_JS: &str = include_str!("../../../web/src/app.js");
const STYLE_CSS: &str = include_str!("../../../web/src/style.css");
const WIZARD_CSS: &str = include_str!("../../../web/src/wizard.css");
const BATCH_CSS: &str = include_str!("../../../web/src/batch.css");
const LIGHT_THEME_CSS: &str = include_str!("../../../web/src/light-theme.css");
const DOWNLOADS_UI_CSS: &str = include_str!("../../../web/src/downloads-ui.css");
const RESOURCE_LOCATION_CSS: &str = include_str!("../../../web/src/resource-location.css");
const REPORT_SHARE_CSS: &str = include_str!("../../../web/src/report-share.css");
const BRAND_THEME_CSS: &str = include_str!("../../../web/src/brand-theme.css");

fn main() {
    if let Err(error) = run() {
        eprintln!("annocat: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => print_help(),
        Some("version") | Some("--version") | Some("-V") => {
            println!("annocat {}", env!("CARGO_PKG_VERSION"))
        }
        Some("doctor") => doctor_command(&args[1..])?,
        Some("annotate") => annotate_command(&args[1..])?,
        Some("fastvep") => fastvep_command(&args[1..])?,
        Some("sources") => list_sources(),
        Some("inspect-vcf") => inspect_vcf_command(&args[1..])?,
        Some("inspect-fastvep") => inspect_fastvep_command(&args[1..])?,
        Some("validate-report") => validate_report_command(&args[1..])?,
        Some("share-report") => share_report_command(&args[1..])?,
        Some("report-worker") => report_worker_command(&args[1..])?,
        Some("check-normalization") => check_normalization_command(&args[1..])?,
        Some("resources") => resource_command(&args[1..])?,
        Some("serve") => {
            ensure_portable_layout()?;
            serve(parse_port(&args[1..])?, false)?
        }
        Some("launch") => {
            ensure_portable_layout()?;
            serve(parse_port(&args[1..])?, true)?
        }
        Some("interactive") => interactive()?,
        Some(command) => return Err(format!("unknown command '{command}'. Run 'annocat help'.")),
    }
    Ok(())
}

fn validate_report_command(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: annocat validate-report REPORT.zip".into());
    };
    println!("{}", worker::validate_report(std::path::Path::new(path))?);
    Ok(())
}

fn share_report_command(args: &[String]) -> Result<(), String> {
    let [run_id, destination] = args else {
        return Err("usage: annocat share-report RUN_ID DESTINATION.zip".into());
    };
    let paths = portable_paths()?;
    let result = completed_run_result(&paths.runs, run_id)?;
    let run_directory = result.parent().ok_or("completed run has no directory")?;
    let package = report_package::create(run_directory, std::path::Path::new(destination))?;
    if let Err(error) = worker::validate_report(&package.path) {
        let _ = std::fs::remove_file(&package.path);
        return Err(format!("created report failed import validation: {error}"));
    }
    println!(
        "Created AnnoCat report: {} ({} bytes)",
        package.path.display(),
        package.bytes
    );
    Ok(())
}

fn report_worker_command(args: &[String]) -> Result<(), String> {
    #[cfg(windows)]
    let report = {
        use std::os::windows::io::FromRawHandle;
        let [action, raw_handle] = args else {
            return Err("invalid report worker request".into());
        };
        if action != "validate-handle" {
            return Err("Windows report worker accepts only inherited archive handles".into());
        }
        worker::require_appcontainer()?;
        let handle = raw_handle
            .parse::<usize>()
            .map_err(|_| "invalid inherited report archive handle")?;
        if handle == 0 {
            return Err("invalid inherited report archive handle".into());
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle as _) };
        report_import::validate_archive_file(file)?
    };
    #[cfg(not(windows))]
    let report = {
        let [action, path] = args else {
            return Err("invalid report worker request".into());
        };
        if action != "validate" {
            return Err("invalid report worker action".into());
        }
        report_import::validate_archive(std::path::Path::new(path))?
    };
    println!(
        "Valid AnnoCat report: {} (schema {}, {} files, {} bytes)",
        report.run_id, report.schema_version, report.file_count, report.uncompressed_bytes
    );
    Ok(())
}

fn print_help() {
    println!(
        "AnnoCat — local-first WGS variant annotation\n\nUSAGE:\n  annocat <COMMAND> [OPTIONS]\n\nCOMMANDS:\n  annotate INPUT [--name NAME] [--output DIRECTORY] [--annotated-vcf]\n                     Run pinned fastVEP and publish a validated result\n  share-report RUN_ID DESTINATION.zip\n                     Create a portable canonical AnnoCat report\n  validate-report REPORT.zip\n                     Validate a shared AnnoCat report without importing it\n  doctor [--json]    Check primary fastVEP backend readiness\n  fastvep status [--json]\n                     Check portable fastVEP readiness\n  sources            Show annotation sources\n  inspect-vcf FILE   Validate and summarize a VCF/VCF.GZ\n  inspect-fastvep FILE\n                     Validate and summarize dynamic CSQ output\n  check-normalization FILE [--chromosome NAME] [--limit N]\n  resources plan comprehensive\n                     Show the current comprehensive GRCh38 source plan\n  launch [--port N]  Start AnnoCat and open the browser (recommended)\n  serve [--port N]   Run the local service without opening a browser\n  interactive        Open a guided terminal menu\n  version            Print the version\n  help               Print this help"
    );
}

fn annotate_command(args: &[String]) -> Result<(), String> {
    let input = args.first().ok_or(
        "usage: annocat annotate INPUT [--name NAME] [--output DIRECTORY] [--annotated-vcf]",
    )?;
    let mut name = None;
    let mut output_directory = None;
    let mut include_annotated_vcf = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--name" if index + 1 < args.len() => {
                name = Some(args[index + 1].clone());
                index += 2;
            }
            "--output" if index + 1 < args.len() => {
                output_directory = Some(std::path::PathBuf::from(&args[index + 1]));
                index += 2;
            }
            "--annotated-vcf" => {
                include_annotated_vcf = true;
                index += 1;
            }
            value => return Err(format!("unknown annotation option: {value}")),
        }
    }
    let paths = portable_paths()?;
    let output = annotation::run_blocking(
        annotation::AnnotationRequest {
            input: std::path::PathBuf::from(input),
            name,
            output_directory,
            source_ids: Vec::new(),
            include_annotated_vcf,
        },
        paths.runs,
        paths.resources,
    )?;
    println!("Completed annotation: {}", output.display());
    Ok(())
}

fn inspect_fastvep_command(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: annocat inspect-fastvep FILE.vcf[.gz]".into());
    };
    let summary = csq::inspect(std::path::Path::new(path))?;
    println!("fastVEP output: {path}");
    println!("  CSQ fields          : {}", summary.fields.len());
    println!("  Records             : {}", summary.records);
    println!("  Alternate alleles   : {}", summary.alternate_alleles);
    println!("  CSQ entries         : {}", summary.csq_entries);
    println!("  Records without CSQ : {}", summary.records_without_csq);
    println!("  CSQ schema          : {}", summary.fields.join(" | "));
    Ok(())
}

fn inspect_vcf_command(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: annocat inspect-vcf FILE.vcf[.gz]".into());
    };
    let summary = annocat_core::vcf::inspect(std::path::Path::new(path))?;
    println!("VCF: {path}");
    println!(
        "  Assembly             : {}",
        summary.assembly.as_deref().unwrap_or("not declared")
    );
    println!(
        "  Samples              : {}",
        if summary.samples.is_empty() {
            "none".into()
        } else {
            summary.samples.join(", ")
        }
    );
    println!("  Records              : {}", summary.records);
    println!("  Alternate alleles    : {}", summary.alleles);
    println!("  SNP alleles          : {}", summary.snps);
    println!("  Indel alleles        : {}", summary.indels);
    println!("  Other alleles        : {}", summary.other_alleles);
    println!("  Multiallelic records : {}", summary.multiallelic_records);
    Ok(())
}

fn check_normalization_command(args: &[String]) -> Result<(), String> {
    let input = args
        .first()
        .ok_or("usage: annocat check-normalization FILE [--chromosome NAME] [--limit N]")?;
    let mut chromosome = None;
    let mut limit = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--chromosome" if index + 1 < args.len() => {
                chromosome = Some(args[index + 1].as_str());
                index += 2;
            }
            "--limit" if index + 1 < args.len() => {
                limit = Some(
                    args[index + 1]
                        .parse::<u64>()
                        .map_err(|_| "--limit must be an integer")?,
                );
                index += 2;
            }
            value => return Err(format!("unknown normalization option: {value}")),
        }
    }
    let paths = portable_paths()?;
    if !reference::is_ready(&paths.resources) {
        return Err("the required GRCh38 reference is not ready".into());
    }
    let summary = annocat_core::vcf::check_normalization(
        std::path::Path::new(input),
        &reference::fasta_path(&paths.resources),
        chromosome,
        limit,
    )?;
    println!("Normalization check");
    println!("  Records scanned      : {}", summary.records_scanned);
    println!("  Alleles scanned      : {}", summary.alleles_scanned);
    println!("  Canonicalized        : {}", summary.canonicalized);
    println!("  Changed              : {}", summary.changed);
    println!("  REF mismatches       : {}", summary.reference_mismatches);
    println!("  Unsupported alleles  : {}", summary.unsupported);
    for example in summary.examples {
        println!("  {example}");
    }
    Ok(())
}

fn command_available(name: &str, version_arg: &str) -> bool {
    Command::new(name)
        .arg(version_arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn doctor_command(args: &[String]) -> Result<(), String> {
    if args == ["--json"] {
        println!("{}", fastvep::readiness_json());
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: annocat doctor [--json]".into());
    }
    doctor();
    Ok(())
}

fn fastvep_command(args: &[String]) -> Result<(), String> {
    match args {
        [command] if command == "status" => {
            print_fastvep_readiness(&fastvep::readiness());
            Ok(())
        }
        [command, json] if command == "status" && json == "--json" => {
            println!("{}", fastvep::readiness_json());
            Ok(())
        }
        _ => Err("usage: annocat fastvep status [--json]".into()),
    }
}

fn print_fastvep_readiness(report: &fastvep::Readiness) {
    println!("AnnoCat fastVEP backend readiness");
    println!("  State       : {}", report.state);
    println!(
        "  Executable  : {}",
        report
            .executable
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "not found".into())
    );
    println!(
        "  Version     : {}",
        report.version.as_deref().unwrap_or("unknown")
    );
    println!(
        "  SHA-256     : {}",
        report.sha256.as_deref().unwrap_or("unavailable")
    );
    println!(
        "  Managed     : {}",
        if report.managed { "yes" } else { "no" }
    );
    println!("  Next action : {}", report.next_action);
}

fn doctor() {
    let fastvep = fastvep::readiness();
    println!("AnnoCat environment check");
    println!("  Rust executable : OK");
    println!(
        "  Git             : {}",
        if command_available("git", "--version") {
            "available"
        } else {
            "not found"
        }
    );
    println!("  Browser UI      : ready (embedded, localhost only)");
    println!("  Annotation data : not installed");
    println!(
        "  fastVEP backend : {}",
        if fastvep.ready {
            "detected"
        } else {
            "not ready"
        }
    );
    println!("  Next action     : {}", fastvep.next_action);
}

fn list_sources() {
    println!("{:<14} {:<8} PURPOSE", "ID", "DEFAULT");
    for source in &annocat_core::source_catalog::catalog().sources {
        println!(
            "{:<14} {:<8} {}",
            source.id,
            if source.default_enabled { "yes" } else { "no" },
            source.purpose
        );
    }
}

fn resource_command(args: &[String]) -> Result<(), String> {
    ensure_portable_layout()?;
    match args {
        [command, profile]
            if command == "plan"
                && matches!(profile.as_str(), "comprehensive" | "practical-wgs") =>
        {
            println!("Comprehensive annotation resource plan (GRCh38)");
            let profile = annocat_core::source_catalog::profile("wgs")
                .ok_or("the comprehensive profile is missing")?;
            for id in ["grch38-reference", "ensembl-gff3"]
                .into_iter()
                .chain(profile.source_ids.iter().map(String::as_str))
            {
                if let Ok(release) = resource_release(id) {
                    let size = release
                        .download_bytes
                        .map(format_terminal_size)
                        .unwrap_or_else(|| "unknown".into());
                    println!("  {:<10} {:<8} {:>10}  missing", id, release.version, size);
                } else {
                    println!("  {id:<10} pending  size unknown  catalog metadata pending");
                }
            }
            Ok(())
        }
        [command, resource, destination, action] if command == "download" => {
            let release = resource_release(resource)?;
            match action.as_str() {
                "--yes" => {
                    downloader::download_release(&release, std::path::Path::new(destination))
                }
                "--dry-run" => {
                    downloader::print_download_plan(&release, std::path::Path::new(destination))
                }
                _ => Err("download requires --dry-run or --yes".into()),
            }
        }
        [command, resource, action] if command == "download" => {
            let release = resource_release(resource)?;
            let destination = portable_paths()?.downloads;
            match action.as_str() {
                "--yes" => downloader::download_release(&release, &destination),
                "--dry-run" => downloader::print_download_plan(&release, &destination),
                _ => Err("download requires --dry-run or --yes".into()),
            }
        }
        _ => Err("usage: annocat resources plan comprehensive | resources download ID [DESTINATION] --dry-run|--yes".into()),
    }
}

fn resource_release(id: &str) -> Result<annocat_core::ResourceRelease, String> {
    annocat_core::source_catalog::download_release(id)
        .ok_or_else(|| format!("resource '{id}' is not present in the download catalog"))
}

fn parse_port(args: &[String]) -> Result<u16, String> {
    if args.is_empty() {
        return Ok(8787);
    }
    if args.len() == 2 && args[0] == "--port" {
        return args[1]
            .parse()
            .map_err(|_| "--port must be an integer from 1 to 65535".into())
            .and_then(|port| {
                if port == 0 {
                    Err("--port must be an integer from 1 to 65535".into())
                } else {
                    Ok(port)
                }
            });
    }
    Err("usage: annocat serve|launch [--port N]".into())
}

fn interactive() -> Result<(), String> {
    loop {
        println!(
            "\nAnnoCat\n  1. Check environment\n  2. List annotation sources\n  3. Start browser UI\n  4. Exit"
        );
        print!("> ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut choice = String::new();
        io::stdin()
            .read_line(&mut choice)
            .map_err(|e| e.to_string())?;
        match choice.trim() {
            "1" => doctor(),
            "2" => list_sources(),
            "3" => return serve(8787, true),
            "4" | "q" | "quit" => return Ok(()),
            _ => println!("Choose 1, 2, 3, or 4."),
        }
    }
}

fn serve(port: u16, open_browser: bool) -> Result<(), String> {
    let address = format!("127.0.0.1:{port}");
    let listener =
        TcpListener::bind(&address).map_err(|e| format!("cannot bind {address}: {e}"))?;
    println!("AnnoCat is running at http://{address}");
    println!("Local annotation and results service. Press Ctrl+C to stop.");
    terminal_log(
        "server",
        format!(
            "version {} process {} started{}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            if open_browser {
                " with browser launch"
            } else {
                ""
            }
        ),
    );
    if let Ok(paths) = portable_paths() {
        terminal_log(
            "server",
            format!(
                "resources={} downloads={} runs={}",
                paths.resources.display(),
                paths.downloads.display(),
                paths.runs.display()
            ),
        );
        downloader::restore_queue(&paths.downloads);
        schedule_preparation(
            "grch38-reference",
            paths.downloads.clone(),
            paths.resources.clone(),
        );
    }
    std::thread::spawn(|| {
        loop {
            terminal_progress(&terminal_active_summary());
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
    if open_browser {
        let url = format!("http://{address}");
        Command::new("explorer.exe")
            .arg(&url)
            .spawn()
            .map_err(|error| format!("could not open the browser at {url}: {error}"))?;
    }
    let (dialog_sender, dialog_receiver) = std::sync::mpsc::channel::<NativeDialogJob>();
    NATIVE_DIALOG_JOBS
        .set(dialog_sender)
        .map_err(|_| "native dialog dispatcher is already initialized".to_string())?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    std::thread::spawn(move || {
                        if let Err(error) = respond(&mut stream) {
                            eprintln!("request failed: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("connection failed: {error}"),
            }
        }
    });
    // Native Windows dialogs are COM STA UI objects. Keep them on the
    // process's original thread while the HTTP server remains concurrent.
    while let Ok(job) = dialog_receiver.recv() {
        job();
    }
    Ok(())
}

type NativeDialogJob = Box<dyn FnOnce() + Send + 'static>;
static NATIVE_DIALOG_JOBS: std::sync::OnceLock<std::sync::mpsc::Sender<NativeDialogJob>> =
    std::sync::OnceLock::new();

fn run_native_dialog<T: Send + 'static>(
    dialog: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let Some(dispatcher) = NATIVE_DIALOG_JOBS.get() else {
        return Ok(dialog());
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    dispatcher
        .send(Box::new(move || {
            let _ = sender.send(dialog());
        }))
        .map_err(|_| "native dialog dispatcher stopped".to_string())?;
    receiver
        .recv()
        .map_err(|_| "native dialog closed without a result".to_string())
}

fn respond(stream: &mut TcpStream) -> io::Result<()> {
    const MAX_BODY: usize = 2 * 1024 * 1024;
    let (mut request_bytes, header_end, content_length) = read_http_headers(stream)?;
    let (method, target, has_csrf_header) = {
        let request = String::from_utf8_lossy(&request_bytes[..header_end]);
        let first_line = request.lines().next().unwrap_or("");
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_owned();
        let target = parts.next().unwrap_or("/").to_owned();
        let has_csrf_header = request.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("X-AnnoCat-CSRF") && value.trim() == "1"
            })
        });
        (method, target, has_csrf_header)
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let mutating = path.ends_with("/start")
        || path.ends_with("/cancel")
        || path.ends_with("/delete")
        || path.ends_with("/name")
        || path.ends_with("/share")
        || path.ends_with("/export")
        || (path.ends_with("/config") && method == "POST")
        || (path.ends_with("/notes") && method == "POST")
        || matches!(
            path,
            "/api/pick-folder"
                | "/api/pick-resource-folder"
                | "/api/pick-results-folder"
                | "/api/pick-vcfs"
                | "/api/pick-results"
        );
    if mutating {
        terminal_log("http", format!("{method} {path}"));
    }
    if mutating && method != "POST" {
        return write_http_response(
            stream,
            "405 Method Not Allowed",
            "application/json",
            "{\"error\":\"POST required\"}",
        );
    }
    if mutating && !has_csrf_header {
        return write_http_response(
            stream,
            "403 Forbidden",
            "application/json",
            "{\"error\":\"CSRF header required\"}",
        );
    }
    let body_start = (header_end + 4).min(request_bytes.len());
    finish_http_request_body(
        stream,
        &mut request_bytes,
        body_start,
        content_length,
        MAX_BODY,
    )?;
    let request_body = &request_bytes[body_start..];
    if path == "/api/annotations/start" {
        let response = portable_paths().and_then(|paths| {
            let value: serde_json::Value = serde_json::from_slice(request_body)
                .map_err(|error| format!("invalid annotation request: {error}"))?;
            if value.get("inputs").is_some() {
                let request =
                    serde_json::from_value::<annotation::BatchAnnotationRequest>(value)
                        .map_err(|error| format!("invalid annotation batch request: {error}"))?;
                annotation::start_batch_background(request, paths.runs, paths.resources)
            } else {
                let request = serde_json::from_value::<annotation::AnnotationRequest>(value)
                    .map_err(|error| format!("invalid annotation request: {error}"))?;
                annotation::start_background(request, paths.runs, paths.resources)
            }
        });
        let (status, body) = match response {
            Ok(run_id) => (
                "202 Accepted",
                format!(
                    "{{\"accepted\":true,\"runId\":\"{}\"}}",
                    json_escape(&run_id)
                ),
            ),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if path == "/api/annotations/cancel" {
        return write_http_response(
            stream,
            "200 OK",
            "application/json",
            &format!("{{\"cancelRequested\":{}}}", annotation::cancel()),
        );
    }
    if let Some(resource_id) = path
        .strip_prefix("/api/resources/")
        .and_then(|value| value.strip_suffix("/updates/check"))
        && !resource_id.is_empty()
        && !resource_id.contains('/')
    {
        let response = match resource_update_status_json(resource_id) {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if path == "/api/resources/dbnsfp/config" {
        let response = portable_paths().and_then(|paths| {
            let root = paths.resources.join("dbnsfp").join("4.9a");
            if method == "GET" {
                preparation::dbnsfp_field_configuration(&root).and_then(|configuration| {
                    serde_json::to_string(&configuration).map_err(|error| error.to_string())
                })
            } else if method == "POST" {
                let selection =
                    serde_json::from_slice::<preparation::DbnsfpFieldSelection>(request_body)
                        .map_err(|error| format!("invalid dbNSFP field selection: {error}"))?;
                preparation::save_dbnsfp_field_selection(&root, selection)?;
                preparation::dbnsfp_field_configuration(&root).and_then(|configuration| {
                    serde_json::to_string(&configuration).map_err(|error| error.to_string())
                })
            } else {
                Err("GET or POST required".into())
            }
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(resource_id) = path
        .strip_prefix("/api/resources/")
        .and_then(|value| value.strip_suffix("/fields"))
        && !resource_id.is_empty()
        && !resource_id.contains('/')
    {
        let response = portable_paths().and_then(|paths| {
            let root = paths.resources.join(resource_id);
            if method == "GET" {
                preparation::supplementary_field_configuration(resource_id, &root).and_then(
                    |configuration| {
                        serde_json::to_string(&configuration).map_err(|error| error.to_string())
                    },
                )
            } else if method == "POST" {
                if preparation::live_status(resource_id).state == "running" {
                    return Err(format!(
                        "pause or cancel the active {resource_id} installation before changing fields"
                    ));
                }
                let selection = serde_json::from_slice::<preparation::SupplementaryFieldSelection>(
                    request_body,
                )
                .map_err(|error| format!("invalid {resource_id} field selection: {error}"))?;
                preparation::save_supplementary_field_selection(resource_id, &root, selection)?;
                preparation::supplementary_field_configuration(resource_id, &root).and_then(
                    |configuration| {
                        serde_json::to_string(&configuration).map_err(|error| error.to_string())
                    },
                )
            } else {
                Err("GET or POST required".into())
            }
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(resource_id) = path
        .strip_prefix("/api/resources/")
        .and_then(|value| value.strip_suffix("/delete"))
        && !resource_id.is_empty()
        && !resource_id.contains('/')
    {
        let response = match delete_managed_resource(resource_id) {
            Ok(()) => ("200 OK", "{\"deleted\":true}".to_string()),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if let Some((resource_id, action)) = download_api_route(path) {
        let response = match resource_release(resource_id) {
            Ok(release) => match action {
                "status" => (
                    "200 OK",
                    downloader::status_json(
                        &release,
                        &portable_paths()
                            .map(|paths| paths.downloads)
                            .unwrap_or_default(),
                    ),
                ),
                "start" if method == "POST" => match portable_paths() {
                    Ok(paths) => {
                        let downloads = paths.downloads.clone();
                        let resources = paths.resources.clone();
                        match downloader::start_background(release, downloads.clone()) {
                            Ok(()) => {
                                if matches!(resource_id, "grch38-reference" | "ensembl-gff3") {
                                    schedule_preparation(release.resource_id, downloads, resources);
                                }
                                ("202 Accepted", "{\"accepted\":true}".into())
                            }
                            Err(error) => (
                                "409 Conflict",
                                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                            ),
                        }
                    }
                    Err(error) => (
                        "500 Internal Server Error",
                        format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                    ),
                },
                "pause" if method == "POST" => {
                    let download_paused = portable_paths()
                        .map(|paths| downloader::cancel_resource(resource_id, &paths.downloads))
                        .unwrap_or(false);
                    let preparation_paused = preparation::cancel_live(resource_id);
                    let queued_preparation_removed = install_queue::remove_waiting(resource_id);
                    if preparation_paused || queued_preparation_removed {
                        install_queue::hold(resource_id);
                    }
                    let paused =
                        download_paused || preparation_paused || queued_preparation_removed;
                    ("200 OK", format!("{{\"pauseRequested\":{paused}}}"))
                }
                "cancel" if method == "POST" => {
                    match cancel_and_delete_managed_resource(resource_id) {
                        Ok(()) => (
                            "202 Accepted",
                            "{\"cancelRequested\":true,\"deleteRequested\":true}".into(),
                        ),
                        Err(error) => (
                            "409 Conflict",
                            format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                        ),
                    }
                }
                "start" | "pause" | "cancel" => (
                    "405 Method Not Allowed",
                    "{\"error\":\"POST required\"}".into(),
                ),
                _ => (
                    "404 Not Found",
                    "{\"error\":\"Unknown download action\"}".into(),
                ),
            },
            Err(error) => (
                "404 Not Found",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if let Some((resource_id, action)) = preparation_api_route(path) {
        if resource_id == "ensembl-gff3" {
            let response = match (action, portable_paths()) {
                ("status", Ok(paths)) => ("200 OK", transcript::status_json(&paths.resources)),
                ("start", Ok(paths)) if method == "POST" => {
                    let release =
                        resource_release("ensembl-gff3").expect("Ensembl release is cataloged");
                    let executable = fastvep::readiness().executable;
                    match executable
                        .ok_or_else(|| "fastVEP executable is unavailable".to_string())
                        .and_then(|fastvep| {
                            transcript::start_background(
                                fastvep,
                                downloader::final_path(&paths.downloads, &release),
                                reference::fasta_path(&paths.resources),
                                paths.resources,
                            )
                        }) {
                        Ok(()) => ("202 Accepted", "{\"accepted\":true}".into()),
                        Err(error) => (
                            "409 Conflict",
                            format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                        ),
                    }
                }
                (_, Err(error)) => (
                    "500 Internal Server Error",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
                _ => (
                    "404 Not Found",
                    "{\"error\":\"Unknown transcript preparation action\"}".into(),
                ),
            };
            return write_http_response(stream, response.0, "application/json", &response.1);
        }
        let response = match action {
            "status" => portable_paths()
                .map(|paths| managed_preparation_status(resource_id, &paths.resources))
                .and_then(|status| {
                    serde_json::to_string(&status).map_err(|error| error.to_string())
                })
                .map(|body| ("200 OK", body))
                .unwrap_or_else(|error| {
                    (
                        "500 Internal Server Error",
                        format!("{{\"error\":\"{}\"}}", json_escape(&error.to_string())),
                    )
                }),
            "start" if method == "POST" => match set_preparation_concurrency(query)
                .and_then(|_| set_source_input_mode(query))
                .and_then(|_| enqueue_preparation(resource_id))
            {
                Ok(()) => ("202 Accepted", "{\"accepted\":true}".into()),
                Err(error) => (
                    "409 Conflict",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            },
            "cancel" if method == "POST" => (
                "200 OK",
                format!(
                    "{{\"cancelRequested\":{}}}",
                    preparation::cancel_live(resource_id)
                ),
            ),
            _ => (
                "404 Not Found",
                "{\"error\":\"Unknown preparation action\"}".into(),
            ),
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if let Some((profile_id, action)) = profile_preparation_api_route(path) {
        let response = match action {
            "status" => match profile_preparation_status(profile_id) {
                Ok(body) => ("200 OK", body),
                Err(error) => (
                    "404 Not Found",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            },
            "start" if method == "POST" => match set_preparation_concurrency(query)
                .and_then(|_| set_source_input_mode(query))
                .and_then(|_| start_profile_preparation(profile_id))
            {
                Ok(()) => ("202 Accepted", "{\"accepted\":true}".into()),
                Err(error) => (
                    "409 Conflict",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            },
            "cancel" if method == "POST" => {
                let cancelled = annocat_core::source_catalog::profile(profile_id)
                    .map(|profile| {
                        profile
                            .source_ids
                            .iter()
                            .filter(|id| preparation::cancel_live(id.as_str()))
                            .count()
                            > 0
                    })
                    .unwrap_or(false);
                ("200 OK", format!("{{\"cancelRequested\":{cancelled}}}"))
            }
            _ => (
                "404 Not Found",
                "{\"error\":\"Unknown profile preparation action\"}".into(),
            ),
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/name"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = portable_paths().and_then(|paths| {
            completed_run_result(&paths.runs, run_id)?;
            let request: serde_json::Value = serde_json::from_slice(request_body)
                .map_err(|error| format!("invalid rename request: {error}"))?;
            let name = request["name"]
                .as_str()
                .ok_or("rename request needs a name")?;
            library_metadata::rename(&paths.runs, run_id, name)
        });
        let (status, body) = match response {
            Ok(name) => ("200 OK", serde_json::json!({"name": name}).to_string()),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/notes"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = portable_paths().and_then(|paths| {
            completed_run_result(&paths.runs, run_id)?;
            if method == "POST" {
                let request: serde_json::Value = serde_json::from_slice(request_body)
                    .map_err(|error| format!("invalid case-notes request: {error}"))?;
                let notes = request["notes"]
                    .as_str()
                    .ok_or("case-notes request needs notes")?;
                library_metadata::save_notes(&paths.runs, run_id, notes)?;
                Ok(serde_json::json!({"notes": notes, "saved": true}).to_string())
            } else if method == "GET" {
                Ok(
                    serde_json::json!({"notes": library_metadata::notes(&paths.runs, run_id)?})
                        .to_string(),
                )
            } else {
                Err("GET or POST required".into())
            }
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/share"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = if method == "POST" {
            match share_completed_run_interactive(run_id) {
                Ok(Some(package)) => (
                    "200 OK",
                    serde_json::json!({
                        "path": package.path,
                        "bytes": package.bytes,
                        "runId": package.run_id
                    })
                    .to_string(),
                ),
                Ok(None) => ("200 OK", "{\"path\":null}".into()),
                Err(error) => (
                    "409 Conflict",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            }
        } else {
            (
                "405 Method Not Allowed",
                "{\"error\":\"POST required\"}".into(),
            )
        };
        return write_http_response(stream, response.0, "application/json", &response.1);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/export"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = serde_json::from_slice::<FilteredExportRequest>(request_body)
            .map_err(|error| format!("invalid filtered export request: {error}"))
            .and_then(|request| export_filtered_results_interactive(run_id, &request));
        let (status, body) = match response {
            Ok(Some(summary)) => ("200 OK", serde_json::to_string(&summary).unwrap()),
            Ok(None) => ("200 OK", "{\"path\":null}".into()),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/candidates"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = portable_paths().and_then(|paths| {
            let variants = completed_run_result(&paths.runs, run_id)?;
            if method == "GET" {
                let candidates = library_metadata::candidates(&paths.runs, run_id)?;
                Ok(serde_json::json!({
                    "schemaVersion": 1,
                    "count": candidates.len(),
                    "candidates": candidates
                })
                .to_string())
            } else if method == "POST" {
                let request: serde_json::Value = serde_json::from_slice(request_body)
                    .map_err(|error| format!("invalid candidate request: {error}"))?;
                let action = request["action"]
                    .as_str()
                    .ok_or("candidate request needs an action")?;
                let allele_ids = request["alleleIds"]
                    .as_array()
                    .ok_or("candidate request needs alleleIds")?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "candidate allele IDs must be strings".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let add = match action {
                    "add" => true,
                    "remove" => false,
                    _ => return Err("candidate action must be add or remove".into()),
                };
                if add {
                    let existing = results::existing_allele_ids(&variants, &allele_ids)?;
                    if existing.len()
                        != allele_ids
                            .iter()
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                    {
                        return Err(
                            "one or more candidate alleles do not belong to this report".into()
                        );
                    }
                }
                let candidates =
                    library_metadata::update_candidates(&paths.runs, run_id, &allele_ids, add)?;
                Ok(serde_json::json!({
                    "schemaVersion": 1,
                    "count": candidates.len(),
                    "candidates": candidates
                })
                .to_string())
            } else {
                Err("GET or POST required".into())
            }
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "409 Conflict",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/candidate-variants"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let offset = query_parameter_u64(query, "offset").unwrap_or(0);
        let limit = query_parameter_u64(query, "limit").unwrap_or(100);
        let page_request = result_page_request(query);
        let response = portable_paths().and_then(|paths| {
            let result = completed_run_result(&paths.runs, run_id)?;
            let consequences = completed_run_file(
                &paths.runs,
                run_id,
                "consequencesFile",
                "consequences.parquet",
                "parquet",
            )
            .ok();
            let evidence = completed_run_file(
                &paths.runs,
                run_id,
                "evidenceFile",
                "evidence.parquet",
                "parquet",
            )
            .ok();
            let catalog = completed_run_file(
                &paths.runs,
                run_id,
                "fieldCatalogFile",
                "field-catalog.json",
                "json",
            )
            .ok();
            let candidate_ids = library_metadata::candidates(&paths.runs, run_id)?
                .into_iter()
                .map(|candidate| candidate.allele_id)
                .collect::<Vec<_>>();
            results::page_json_with_details_for_candidates(
                &result,
                consequences.as_deref(),
                evidence.as_deref(),
                catalog.as_deref(),
                offset,
                limit,
                &page_request?,
                &candidate_ids,
            )
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "404 Not Found",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/fields"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let response = portable_paths()
            .and_then(|paths| {
                completed_run_file(
                    &paths.runs,
                    run_id,
                    "fieldCatalogFile",
                    "field-catalog.json",
                    "json",
                )
            })
            .and_then(|catalog| {
                let metadata = std::fs::metadata(&catalog)
                    .map_err(|error| format!("field catalog is missing: {error}"))?;
                if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
                    return Err("field catalog has an invalid size".into());
                }
                let body = std::fs::read_to_string(catalog)
                    .map_err(|error| format!("cannot read field catalog: {error}"))?;
                serde_json::from_str::<serde_json::Value>(&body)
                    .map_err(|error| format!("invalid field catalog: {error}"))?;
                Ok(body)
            });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "404 Not Found",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some((run_id, allele_id)) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.split_once("/variants/"))
        .filter(|(run_id, allele_id)| {
            !run_id.is_empty()
                && !run_id.contains('/')
                && !allele_id.is_empty()
                && !allele_id.contains('/')
        })
    {
        let response = portable_paths().and_then(|paths| {
            let variants = completed_run_result(&paths.runs, run_id)?;
            let consequences = completed_run_file(
                &paths.runs,
                run_id,
                "consequencesFile",
                "consequences.parquet",
                "parquet",
            )
            .ok();
            let evidence = completed_run_file(
                &paths.runs,
                run_id,
                "evidenceFile",
                "evidence.parquet",
                "parquet",
            )
            .ok();
            results::complete_detail_json(
                &variants,
                consequences.as_deref(),
                evidence.as_deref(),
                allele_id,
            )
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "404 Not Found",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    if let Some(run_id) = path
        .strip_prefix("/api/runs/")
        .and_then(|value| value.strip_suffix("/variants"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        let offset = query_parameter_u64(query, "offset").unwrap_or(0);
        let limit = query_parameter_u64(query, "limit").unwrap_or(100);
        let page_request = result_page_request(query);
        let response = portable_paths().and_then(|paths| {
            let result = completed_run_result(&paths.runs, run_id)?;
            let consequences = completed_run_file(
                &paths.runs,
                run_id,
                "consequencesFile",
                "consequences.parquet",
                "parquet",
            )
            .ok();
            let evidence = completed_run_file(
                &paths.runs,
                run_id,
                "evidenceFile",
                "evidence.parquet",
                "parquet",
            )
            .ok();
            let catalog = completed_run_file(
                &paths.runs,
                run_id,
                "fieldCatalogFile",
                "field-catalog.json",
                "json",
            )
            .ok();
            let page_request = page_request?;
            results::page_json_with_details(
                &result,
                consequences.as_deref(),
                evidence.as_deref(),
                catalog.as_deref(),
                offset,
                limit,
                &page_request,
            )
        });
        let (status, body) = match response {
            Ok(body) => ("200 OK", body),
            Err(error) => (
                "404 Not Found",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        };
        return write_http_response(stream, status, "application/json", &body);
    }
    let (status, content_type, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", INDEX_HTML.to_string()),
        "/app.js" => (
            "200 OK",
            "text/javascript; charset=utf-8",
            APP_JS.to_string(),
        ),
        "/style.css" => ("200 OK", "text/css; charset=utf-8", STYLE_CSS.to_string()),
        "/wizard.css" => ("200 OK", "text/css; charset=utf-8", WIZARD_CSS.to_string()),
        "/batch.css" => ("200 OK", "text/css; charset=utf-8", BATCH_CSS.to_string()),
        "/light-theme.css" => (
            "200 OK",
            "text/css; charset=utf-8",
            LIGHT_THEME_CSS.to_string(),
        ),
        "/downloads-ui.css" => (
            "200 OK",
            "text/css; charset=utf-8",
            DOWNLOADS_UI_CSS.to_string(),
        ),
        "/resource-location.css" => (
            "200 OK",
            "text/css; charset=utf-8",
            RESOURCE_LOCATION_CSS.to_string(),
        ),
        "/report-share.css" => (
            "200 OK",
            "text/css; charset=utf-8",
            REPORT_SHARE_CSS.to_string(),
        ),
        "/brand-theme.css" => (
            "200 OK",
            "text/css; charset=utf-8",
            BRAND_THEME_CSS.to_string(),
        ),
        "/api/sources" => ("200 OK", "application/json", sources_json()),
        "/api/profiles" => ("200 OK", "application/json", profiles_json()),
        "/api/resources/catalog-candidates" => (
            "200 OK",
            "application/json",
            resource_catalog_candidates_json(),
        ),
        "/api/resources/plan" => ("200 OK", "application/json", practical_resource_plan_json()),
        "/api/resources/status" => match resources_status_json() {
            Ok(body) => ("200 OK", "application/json", body),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/tasks" => match tasks_json() {
            Ok(body) => ("200 OK", "application/json", body),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/resources/dbnsfp/download/status" => {
            let release = resource_release("dbnsfp").unwrap();
            let root = portable_paths()
                .map(|paths| paths.downloads)
                .unwrap_or_default();
            (
                "200 OK",
                "application/json",
                downloader::status_json(&release, &root),
            )
        }
        "/api/resources/dbnsfp/download/start" => {
            let release = resource_release("dbnsfp").unwrap();
            let paths = portable_paths();
            let root = paths
                .as_ref()
                .map(|paths| paths.downloads.clone())
                .unwrap_or_default();
            let resources = paths
                .as_ref()
                .map(|paths| paths.resources.clone())
                .unwrap_or_default();
            let result = downloader::start_background(release, root.clone());
            match result {
                Ok(()) => {
                    println!(
                        "[resources] dbNSFP 4.9a download/resume started: {}",
                        root.display()
                    );
                    schedule_preparation("dbnsfp", root, resources);
                    (
                        "202 Accepted",
                        "application/json",
                        "{\"accepted\":true}".into(),
                    )
                }
                Err(error) => (
                    "409 Conflict",
                    "application/json",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            }
        }
        "/api/resources/dbnsfp/download/cancel" => {
            let cancelled = portable_paths()
                .map(|paths| downloader::cancel_resource("dbnsfp", &paths.downloads))
                .unwrap_or(false);
            if cancelled {
                println!("[resources] dbNSFP cancellation requested; preserving .partial file");
            }
            (
                "200 OK",
                "application/json",
                format!("{{\"cancelRequested\":{cancelled}}}"),
            )
        }
        "/api/resources/grch38-reference/download/status" => {
            let release = resource_release("grch38-reference").unwrap();
            let root = portable_paths().map(|p| p.downloads).unwrap_or_default();
            (
                "200 OK",
                "application/json",
                downloader::status_json(&release, &root),
            )
        }
        "/api/resources/grch38-reference/download/start" => {
            let release = resource_release("grch38-reference").unwrap();
            match portable_paths() {
                Ok(paths) => {
                    let downloads = paths.downloads.clone();
                    let resources = paths.resources.clone();
                    match downloader::start_background(release, downloads.clone()) {
                        Ok(()) => {
                            schedule_preparation("grch38-reference", downloads, resources);
                            (
                                "202 Accepted",
                                "application/json",
                                "{\"accepted\":true}".into(),
                            )
                        }
                        Err(error) => (
                            "409 Conflict",
                            "application/json",
                            format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                        ),
                    }
                }
                Err(error) => (
                    "500 Internal Server Error",
                    "application/json",
                    format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                ),
            }
        }
        "/api/resources/grch38-reference/download/cancel" => (
            "200 OK",
            "application/json",
            format!(
                "{{\"cancelRequested\":{}}}",
                portable_paths()
                    .map(|paths| downloader::cancel_resource("grch38-reference", &paths.downloads))
                    .unwrap_or(false)
            ),
        ),
        "/api/resources/grch38-reference/prepare/status" => match portable_paths() {
            Ok(paths) => (
                "200 OK",
                "application/json",
                reference::status_json(&paths.downloads, &paths.resources),
            ),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/resources/grch38-reference/prepare/start" => match portable_paths()
            .and_then(|p| reference::start_background(p.downloads, p.resources))
        {
            Ok(()) => (
                "202 Accepted",
                "application/json",
                "{\"accepted\":true}".into(),
            ),
            Err(error) => (
                "409 Conflict",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/resources/grch38-reference/prepare/cancel" => (
            "200 OK",
            "application/json",
            format!("{{\"cancelRequested\":{}}}", reference::cancel_background()),
        ),
        "/api/paths" => ("200 OK", "application/json", portable_paths_json()),
        "/api/about" => ("200 OK", "application/json", about_json()),
        "/api/fastvep/status" => ("200 OK", "application/json", fastvep::readiness_json()),
        "/api/setup/status" => match portable_paths() {
            Ok(paths) => {
                let reference_ready = reference::is_ready(&paths.resources);
                let engine_ready = fastvep::readiness().ready;
                let transcript_cache_ready = transcript::is_ready(&paths.resources);
                (
                    "200 OK",
                    "application/json",
                    format!(
                        "{{\"ready\":{},\"referenceReady\":{reference_ready},\"engineReady\":{engine_ready},\"transcriptCacheReady\":{transcript_cache_ready}}}",
                        reference_ready && engine_ready && transcript_cache_ready
                    ),
                )
            }
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/demo/variants" => ("200 OK", "application/json", demo_variants_json()),
        "/api/annotations/status" => ("200 OK", "application/json", annotation::status_json()),
        "/api/runs" => match portable_paths().and_then(|paths| completed_runs_json(&paths.runs)) {
            Ok(body) => ("200 OK", "application/json", body),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/health" => (
            "200 OK",
            "application/json",
            "{\"status\":\"ok\",\"mode\":\"local\"}".to_string(),
        ),
        "/api/pick-folder" => match pick_output_folder() {
            Ok(Some(path)) => (
                "200 OK",
                "application/json",
                format!("{{\"path\":\"{}\"}}", json_escape(&path)),
            ),
            Ok(None) => ("200 OK", "application/json", "{\"path\":null}".to_string()),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/pick-resource-folder" => match pick_resource_folder() {
            Ok(Some(path)) => (
                "200 OK",
                "application/json",
                format!("{{\"path\":\"{}\"}}", json_escape(&path)),
            ),
            Ok(None) => ("200 OK", "application/json", "{\"path\":null}".to_string()),
            Err(error) => (
                "409 Conflict",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/pick-results-folder" => match pick_results_folder() {
            Ok(Some(path)) => (
                "200 OK",
                "application/json",
                format!("{{\"path\":\"{}\"}}", json_escape(&path)),
            ),
            Ok(None) => ("200 OK", "application/json", "{\"path\":null}".to_string()),
            Err(error) => (
                "409 Conflict",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/pick-vcfs" => match pick_vcf_files() {
            Ok(paths) => (
                "200 OK",
                "application/json",
                serde_json::json!({"paths": paths}).to_string(),
            ),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        "/api/pick-results" => match pick_result_file() {
            Ok(path) => match path {
                Some(path)
                    if std::path::Path::new(&path)
                        .extension()
                        .and_then(|value| value.to_str())
                        == Some("zip") =>
                {
                    let imported = worker::validate_report(std::path::Path::new(&path))
                        .and_then(|_| portable_paths())
                        .and_then(|paths| {
                            report_library::import(std::path::Path::new(&path), &paths.runs)
                        });
                    match imported {
                        Ok(report) => (
                            "200 OK",
                            "application/json",
                            serde_json::json!({
                                "path": path,
                                "runId": report.run_id,
                                "name": report.name,
                                "directory": report.directory
                            })
                            .to_string(),
                        ),
                        Err(error) => (
                            "409 Conflict",
                            "application/json",
                            format!("{{\"error\":\"{}\"}}", json_escape(&error)),
                        ),
                    }
                }
                Some(_) => (
                    "409 Conflict",
                    "application/json",
                    "{\"error\":\"Import currently accepts AnnoCat report ZIPs\"}".into(),
                ),
                None => ("200 OK", "application/json", "{\"path\":null}".into()),
            },
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                format!("{{\"error\":\"{}\"}}", json_escape(&error)),
            ),
        },
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "Not found".to_string(),
        ),
    };
    write_http_response(stream, status, content_type, &body)
}

fn read_http_headers(stream: &mut TcpStream) -> io::Result<(Vec<u8>, usize, usize)> {
    const MAX_HEADER: usize = 64 * 1024;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut request = Vec::with_capacity(4096);
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete HTTP headers",
            ));
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if request.len() > MAX_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP headers are too large",
            ));
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("Content-Length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    Ok((request, header_end - 4, content_length))
}

fn finish_http_request_body(
    stream: &mut TcpStream,
    request: &mut Vec<u8>,
    body_start: usize,
    content_length: usize,
    max_body: usize,
) -> io::Result<()> {
    if content_length > max_body {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP request body is too large",
        ));
    }
    let expected = body_start + content_length;
    let mut buffer = [0_u8; 4096];
    while request.len() < expected {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete HTTP body",
            ));
        }
        request.extend_from_slice(&buffer[..read.min(expected - request.len())]);
    }
    request.truncate(expected);
    Ok(())
}

fn download_api_route(path: &str) -> Option<(&str, &str)> {
    let remainder = path.strip_prefix("/api/resources/")?;
    let (resource_id, action) = remainder.split_once("/download/")?;
    if resource_id.is_empty() || action.contains('/') {
        None
    } else {
        Some((resource_id, action))
    }
}

fn preparation_api_route(path: &str) -> Option<(&str, &str)> {
    let remainder = path.strip_prefix("/api/resources/")?;
    let (resource_id, action) = remainder.split_once("/prepare/")?;
    if resource_id == "grch38-reference" || resource_id.is_empty() || action.contains('/') {
        None
    } else {
        Some((resource_id, action))
    }
}

fn profile_preparation_api_route(path: &str) -> Option<(&str, &str)> {
    let remainder = path.strip_prefix("/api/profiles/")?;
    let (profile_id, action) = remainder.split_once("/prepare/")?;
    if profile_id.is_empty() || action.contains('/') {
        None
    } else {
        Some((profile_id, action))
    }
}

fn query_parameter_u64(query: &str, name: &str) -> Option<u64> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.parse().ok()).flatten()
    })
}

fn query_parameter(query: &str, name: &str) -> Option<Result<String, String>> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn result_page_request(query: &str) -> Result<results::PageRequest, String> {
    let text = |name| -> Result<String, String> {
        query_parameter(query, name)
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let integer = |name| -> Result<Option<i64>, String> {
        let value = text(name)?;
        if value.trim().is_empty() {
            Ok(None)
        } else {
            value
                .trim()
                .parse::<i64>()
                .map(Some)
                .map_err(|_| format!("{name} must be an integer"))
        }
    };
    let number = |name| -> Result<Option<f64>, String> {
        let value = text(name)?;
        if value.trim().is_empty() {
            Ok(None)
        } else {
            value
                .trim()
                .parse::<f64>()
                .map(Some)
                .map_err(|_| format!("{name} must be a number"))
        }
    };
    let boolean = |name| -> Result<Option<bool>, String> {
        match text(name)?.trim().to_ascii_lowercase().as_str() {
            "" => Ok(None),
            "true" | "yes" | "1" => Ok(Some(true)),
            "false" | "no" | "0" => Ok(Some(false)),
            _ => Err(format!("{name} must be true or false")),
        }
    };
    let evidence_columns = text("evidenceColumns")?;
    let evidence_columns = if evidence_columns.trim().is_empty() {
        Vec::new()
    } else {
        evidence_columns
            .split(',')
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| "evidenceColumns must contain comma-separated indexes".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let filter_rules = text("filterRules")?;
    let filter_rules = if filter_rules.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&filter_rules)
            .map_err(|error| format!("filterRules must be a JSON array: {error}"))?
    };
    let evidence_filters = text("evidenceFilters")?;
    let evidence_filters = if evidence_filters.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&evidence_filters)
            .map_err(|error| format!("evidenceFilters must be a JSON array: {error}"))?
    };
    Ok(results::PageRequest {
        search: text("search")?,
        sort: text("sort")?,
        direction: text("direction")?,
        sort_evidence: integer("sortEvidence")?
            .map(|value| {
                usize::try_from(value).map_err(|_| "sortEvidence must be a non-negative index")
            })
            .transpose()?,
        chromosome: text("chromosome")?,
        position_min: integer("positionMin")?,
        position_max: integer("positionMax")?,
        reference: text("reference")?,
        alternate: text("alternate")?,
        variant_id: text("variantId")?,
        gene: text("gene")?,
        transcript_id: text("transcriptId")?,
        consequence: text("consequence")?,
        impact: text("impact")?,
        quality_min: number("qualityMin")?,
        quality_max: number("qualityMax")?,
        filter: text("filter")?,
        canonical: boolean("canonical")?,
        evidence_columns,
        evidence_filters,
        filter_rules,
    })
}

fn percent_decode(value: &str) -> Result<String, String> {
    if value.len() > 48 * 1024 {
        return Err("query parameter is too long".into());
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1]).ok_or("invalid percent encoding")?;
                let low = hex_value(bytes[index + 2]).ok_or("invalid percent encoding")?;
                decoded.push(high * 16 + low);
                index += 2;
            }
            b'%' => return Err("invalid percent encoding".into()),
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).map_err(|_| "query parameter is not UTF-8".into())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn profile_preparation_status(profile_id: &str) -> Result<String, String> {
    let profile = annocat_core::source_catalog::profile(profile_id)
        .ok_or_else(|| format!("unknown profile '{profile_id}'"))?;
    let resources = portable_paths()?.resources;
    let sources = profile
        .source_ids
        .iter()
        .filter(|id| id.as_str() != "fastvep")
        .map(|id| {
            let state = managed_preparation_status(id, &resources);
            let release = annocat_core::source_catalog::download_release(id);
            let catalog_ready =
                state.state == "ready" || (preparation_available(id) && release.is_some());
            serde_json::json!({
                "resourceId": id,
                "preparation": state,
                "catalogReady": catalog_ready,
                "expectedCompressedBytes": release.and_then(|release| release.download_bytes),
                "release": release.map(|release| release.version)
            })
        })
        .collect::<Vec<_>>();
    let running_source = sources
        .iter()
        .find(|source| source["preparation"]["state"] == "running");
    let failed = sources
        .iter()
        .any(|source| source["preparation"]["state"] == "failed");
    let actionable = sources
        .iter()
        .filter(|source| source["catalogReady"] == true)
        .collect::<Vec<_>>();
    let blocked_resource_ids = sources
        .iter()
        .filter(|source| source["catalogReady"] == false)
        .filter_map(|source| source["resourceId"].as_str())
        .collect::<Vec<_>>();
    let sum = |field: &str| {
        actionable
            .iter()
            .filter_map(|source| source["preparation"][field].as_u64())
            .sum::<u64>()
    };
    let expected_network_bytes = actionable
        .iter()
        .filter_map(|source| source["expectedCompressedBytes"].as_u64())
        .sum::<u64>();
    let network_bytes = sum("networkBytes");
    let prepared_bytes = sum("preparedBytes");
    let disk =
        preparation::preparation_disk_plan(network_bytes, expected_network_bytes, prepared_bytes);
    let completed_chromosomes = sum("completedChromosomes");
    let remaining_chromosomes = sum("remainingChromosomes");
    let completed_resources = actionable
        .iter()
        .filter(|source| source["preparation"]["state"] == "ready")
        .count();
    let percent = if expected_network_bytes == 0 {
        0.0
    } else {
        network_bytes as f64 * 100.0 / expected_network_bytes as f64
    };
    let state = if running_source.is_some() {
        "running"
    } else if failed {
        "failed"
    } else if completed_resources == actionable.len() && !actionable.is_empty() {
        if blocked_resource_ids.is_empty() {
            "ready"
        } else {
            "partial"
        }
    } else if actionable.is_empty() && !blocked_resource_ids.is_empty() {
        "blocked"
    } else {
        "idle"
    };
    serde_json::to_string(&serde_json::json!({
        "profileId": profile_id,
        "state": state,
        "currentResourceId": running_source.and_then(|source| source["resourceId"].as_str()),
        "currentChromosome": running_source.and_then(|source| source["preparation"]["chromosome"].as_str()),
        "networkBytes": network_bytes,
        "expectedNetworkBytes": expected_network_bytes,
        "preparedBytes": prepared_bytes,
        "disk": disk,
        "throughputBytesPerSecond": actionable.iter().filter_map(|source| source["preparation"]["throughputBytesPerSecond"].as_f64()).sum::<f64>(),
        "completedChromosomes": completed_chromosomes,
        "remainingChromosomes": remaining_chromosomes,
        "completedResources": completed_resources,
        "remainingResources": actionable.len().saturating_sub(completed_resources),
        "percent": percent,
        "blockedResourceIds": blocked_resource_ids,
        "sources": sources
    }))
    .map_err(|error| error.to_string())
}

fn managed_preparation_status(
    resource_id: &str,
    resources: &std::path::Path,
) -> preparation::LivePreparationState {
    let Some(release) = annocat_core::source_catalog::download_release(resource_id) else {
        return preparation::live_status(resource_id);
    };
    let chromosomes = if resource_id == "dbnsfp" {
        preparation::pinned_dbnsfp_manifest()
            .map(|manifest| {
                manifest
                    .members
                    .into_iter()
                    .map(|member| member.chromosome)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else if resource_id == "dbsnp" {
        (1..=22)
            .map(|chromosome| chromosome.to_string())
            .chain(["X".to_string(), "Y".to_string(), "M".to_string()])
            .collect()
    } else if matches!(resource_id, "gnomad" | "gnomad-genomes" | "phylop") {
        preparation::pinned_sharded_source(resource_id)
            .map(|source| {
                source
                    .shards
                    .into_iter()
                    .map(|shard| shard.chromosome)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else if matches!(resource_id, "cadd" | "spliceai" | "revel") {
        (1..=22)
            .map(|chromosome| chromosome.to_string())
            .chain(["X".to_string(), "Y".to_string()])
            .collect()
    } else {
        vec!["all".to_string()]
    };
    if is_rolling_resource(resource_id) {
        let live = preparation::live_status(resource_id);
        if live.state != "idle" {
            return live;
        }
        for version in installed_resource_versions(resource_id, resources)
            .into_iter()
            .rev()
        {
            let status = preparation::status_with_storage(
                resource_id,
                &resources.join(resource_id).join(version),
                &chromosomes,
            );
            if status.state == "ready" {
                return status;
            }
        }
    }
    let status = preparation::status_with_storage(
        resource_id,
        &resources.join(resource_id).join(release.version),
        &chromosomes,
    );
    if status.state == "idle"
        && let Some(position) = install_queue::position(resource_id, preparation::running_count())
    {
        return preparation::LivePreparationState {
            resource_id: Some(resource_id.into()),
            state: "queued".into(),
            phase: "queued".into(),
            expected_network_bytes: release.download_bytes.unwrap_or(0),
            remaining_chromosomes: chromosomes.len() as u16,
            detail: format!("Waiting in the profile installation queue (position {position})"),
            ..preparation::LivePreparationState::default()
        };
    }
    status
}

fn start_profile_preparation(profile_id: &str) -> Result<(), String> {
    let profile = annocat_core::source_catalog::profile(profile_id)
        .ok_or_else(|| format!("unknown profile '{profile_id}'"))?;
    let resources = portable_paths()?.resources;
    let actionable = profile
        .source_ids
        .iter()
        .map(String::as_str)
        .filter(|id| {
            preparation_available(id)
                && annocat_core::source_catalog::download_release(id).is_some()
        })
        .filter(|id| managed_preparation_status(id, &resources).state != "ready")
        .collect::<Vec<_>>();
    if actionable.is_empty() {
        return Err(format!(
            "profile '{profile_id}' has no sources with a verified streaming plan"
        ));
    }
    for resource_id in actionable {
        enqueue_preparation(resource_id)?;
    }
    Ok(())
}

fn set_preparation_concurrency(query: &str) -> Result<usize, String> {
    let Some(value) = query_parameter(query, "concurrency").transpose()? else {
        return Ok(install_queue::concurrency());
    };
    let concurrency = value
        .parse::<usize>()
        .map_err(|_| "installation concurrency must be between 1 and 4".to_string())?;
    install_queue::set_concurrency(concurrency)
}

fn set_source_input_mode(query: &str) -> Result<preparation::SourceInputMode, String> {
    let value = query_parameter(query, "sourceMode")
        .transpose()?
        .unwrap_or_else(|| "pure-streaming".to_string());
    preparation::set_source_input_mode(&value)
}

fn enqueue_preparation(resource_id: &str) -> Result<(), String> {
    if !preparation_available(resource_id) {
        return Err(format!(
            "resource '{resource_id}' has no verified streaming plan"
        ));
    }
    let paths = portable_paths()?;
    if managed_preparation_status(resource_id, &paths.resources).state == "ready" {
        return Ok(());
    }
    preparation::forget_live(resource_id);
    let resuming = install_queue::release_hold(resource_id);
    if preparation::live_status(resource_id).state == "running" {
        return Ok(());
    }
    let outcome = install_queue::enqueue(resource_id, resuming)?;
    if outcome.start_worker {
        start_preparation_queue_worker();
    }
    Ok(())
}

fn start_preparation_queue_worker() {
    std::thread::spawn(|| {
        loop {
            let resource_id = match install_queue::next(preparation::running_count()) {
                install_queue::NextWork::Start(resource_id) => resource_id,
                install_queue::NextWork::Wait => {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }
                install_queue::NextWork::Idle => return,
            };
            let Ok(paths) = portable_paths() else {
                continue;
            };
            if managed_preparation_status(&resource_id, &paths.resources).state == "ready" {
                continue;
            }
            let started = if resource_id == "dbnsfp" {
                start_dbnsfp_preparation()
            } else {
                start_catalog_preparation(&resource_id)
            };
            if let Err(error) = started {
                let expected = annocat_core::source_catalog::download_release(&resource_id)
                    .and_then(|release| release.download_bytes)
                    .unwrap_or(0);
                preparation::record_start_failure(&resource_id, error.clone(), expected);
                terminal_log(
                    "resources",
                    format!("{resource_id} could not start: {error}"),
                );
                continue;
            }
        }
    });
}

fn catalog_source_type(resource_id: &str) -> Option<&'static str> {
    annocat_core::source_catalog::resource(resource_id)?;
    annocat_core::source_catalog::source(resource_id)?
        .fastvep_source
        .as_deref()
}

fn preparation_available(resource_id: &str) -> bool {
    resource_id == "dbnsfp" || catalog_source_type(resource_id).is_some()
}

fn start_catalog_preparation(resource_id: &str) -> Result<(), String> {
    let release = resource_release(resource_id).map_err(|_| {
        format!("resource '{resource_id}' has no pinned per-object preparation metadata")
    })?;
    let source_type = catalog_source_type(resource_id).ok_or_else(|| {
        format!("resource '{resource_id}' is not yet connected to a fastSA schema")
    })?;
    let readiness = fastvep::readiness();
    if !readiness.ready {
        return Err(format!("fastVEP is not ready: {}", readiness.state));
    }
    let executable = readiness
        .executable
        .ok_or("fastVEP readiness omitted its executable path")?;
    if !fastvep::supports_sa_verify(&executable) {
        return Err(
            "the installed fastVEP predates pinned patch 0004; rebuild or repair it before streaming preparation"
                .into(),
        );
    }
    let resources = portable_paths()?.resources;
    if resource_id == "cadd" {
        return preparation::start_cadd_live(preparation::CaddLiveRequest {
            fastvep_executable: executable,
            resource_root: resources.join("cadd").join(release.version),
        });
    }
    if resource_id == "spliceai" {
        return preparation::start_spliceai_live(preparation::SpliceAiLiveRequest {
            fastvep_executable: executable,
            resource_root: resources.join("spliceai").join(release.version),
        });
    }
    if resource_id == "revel" {
        return preparation::start_revel_live(preparation::RevelLiveRequest {
            fastvep_executable: executable,
            resource_root: resources.join("revel").join(release.version),
        });
    }
    if matches!(resource_id, "gnomad" | "gnomad-genomes" | "phylop") {
        let source = preparation::pinned_sharded_source(resource_id)?;
        if source.release != release.version || source.source_type != source_type {
            return Err(format!(
                "resource '{resource_id}' release metadata differs from its shard catalog"
            ));
        }
        return preparation::start_sharded_live(preparation::ShardedLiveRequest {
            fastvep_executable: executable,
            resource_root: resources.join(resource_id).join(&source.release),
            source,
        });
    }
    if resource_id == "dbsnp" {
        let resolved = resolve_dbsnp_release()?;
        return preparation::start_dbsnp_live(preparation::DbsnpLiveRequest {
            fastvep_executable: executable,
            resource_root: resources.join("dbsnp").join(&resolved.version),
            artifact: preparation::DbsnpArtifact {
                release: resolved.version,
                data_url: resolved.url,
                data_bytes: resolved.download_bytes,
                data_md5: resolved
                    .etag
                    .and_then(|value| value.strip_prefix("md5:").map(str::to_owned))
                    .ok_or("resolved dbSNP release omitted its data checksum")?,
                data_last_modified: resolved.last_modified,
                index_url: resolved
                    .index_url
                    .ok_or("resolved dbSNP release omitted its tabix URL")?,
                index_bytes: resolved
                    .index_bytes
                    .ok_or("resolved dbSNP release omitted its tabix size")?,
                index_md5: resolved
                    .index_md5
                    .ok_or("resolved dbSNP release omitted its tabix checksum")?,
                index_last_modified: resolved.index_last_modified,
            },
        });
    }
    let resolved = match resource_id {
        "clinvar" => Some(resolve_clinvar_release()?),
        _ => None,
    };
    let release_version = resolved
        .as_ref()
        .map(|release| release.version.as_str())
        .unwrap_or(release.version);
    let source_url = resolved
        .as_ref()
        .map(|release| release.url.as_str())
        .unwrap_or(release.url);
    let expected_compressed_bytes = resolved
        .as_ref()
        .map(|release| release.download_bytes)
        .or(release.download_bytes)
        .ok_or("catalog object size is not pinned")?;
    preparation::start_live(preparation::LivePreparationRequest {
        fastvep_executable: executable,
        source_type: source_type.into(),
        resource_root: resources.join(resource_id).join(release_version),
        identity: preparation::PreparationIdentity {
            resource_id: resource_id.into(),
            release: release_version.into(),
            assembly: "GRCh38".into(),
            chromosome: "all".into(),
            source_url: source_url.into(),
            expected_compressed_bytes,
            source_etag: resolved.as_ref().and_then(|release| release.etag.clone()),
            source_last_modified: resolved
                .as_ref()
                .and_then(|release| release.last_modified.clone()),
            selected_schema: format!("{resource_id}-{release_version}"),
            fastvep_commit: preparation::LEGACY_PREPARATION_IDENTITY_COMMIT.into(),
            osa_schema_version: 1,
        },
    })
}

fn start_dbnsfp_preparation() -> Result<(), String> {
    let readiness = fastvep::readiness();
    if !readiness.ready {
        return Err(format!("fastVEP is not ready: {}", readiness.state));
    }
    let executable = readiness
        .executable
        .ok_or("fastVEP readiness omitted its executable path")?;
    if !fastvep::supports_sa_verify(&executable) {
        return Err("the installed fastVEP does not support verified shard preparation".into());
    }
    let paths = portable_paths()?;
    let release = resource_release("dbnsfp").expect("dbNSFP 4.9a is pinned");
    let local_archive = downloader::is_downloaded(&release, &paths.downloads)
        .then(|| downloader::final_path(&paths.downloads, &release));
    preparation::start_dbnsfp_live(preparation::DbnsfpLiveRequest {
        fastvep_executable: executable,
        resource_root: paths.resources.join("dbnsfp").join("4.9a"),
        local_archive,
    })
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

struct PortablePaths {
    home: std::path::PathBuf,
    resource_directory: std::path::PathBuf,
    resources: std::path::PathBuf,
    downloads: std::path::PathBuf,
    runs: std::path::PathBuf,
    config: std::path::PathBuf,
}

#[derive(Default, Deserialize, Serialize)]
struct AppConfig {
    resource_directory: Option<std::path::PathBuf>,
    results_directory: Option<std::path::PathBuf>,
}

fn portable_home() -> Result<std::path::PathBuf, String> {
    if let Some(home) = std::env::var_os("ANNOCAT_HOME") {
        Ok(std::path::PathBuf::from(home))
    } else {
        std::env::current_exe()
            .map_err(|error| format!("cannot locate AnnoCat executable: {error}"))?
            .parent()
            .ok_or_else(|| "AnnoCat executable has no parent directory".to_string())
            .map(std::path::Path::to_path_buf)
    }
}

fn config_file(home: &std::path::Path) -> std::path::PathBuf {
    home.join("config").join("annocat.json")
}

fn load_config(home: &std::path::Path) -> Result<AppConfig, String> {
    let file = config_file(home);
    if !file.exists() {
        return Ok(AppConfig::default());
    }
    let contents = std::fs::read_to_string(&file)
        .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid configuration {}: {error}", file.display()))
}

fn save_resource_directory(path: &std::path::Path) -> Result<(), String> {
    let home = portable_home()?;
    let mut config = load_config(&home)?;
    config.resource_directory = Some(path.to_path_buf());
    save_config(&home, &config)
}

fn save_results_directory(path: &std::path::Path) -> Result<(), String> {
    let home = portable_home()?;
    let mut config = load_config(&home)?;
    config.results_directory = Some(path.to_path_buf());
    save_config(&home, &config)
}

fn save_config(home: &std::path::Path, config: &AppConfig) -> Result<(), String> {
    let directory = home.join("config");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let contents = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("cannot serialize configuration: {error}"))?;
    let file = config_file(&home);
    std::fs::write(&file, format!("{contents}\n"))
        .map_err(|error| format!("cannot write {}: {error}", file.display()))
}

fn delete_managed_resource(resource_id: &str) -> Result<(), String> {
    if annotation::is_running()
        || managed_download_is_active(resource_id)
        || (resource_id == "grch38-reference" && reference::is_running())
        || (matches!(resource_id, "grch38-reference" | "ensembl-gff3") && transcript::is_running())
        || preparation::live_status(resource_id).state == "running"
    {
        return Err("cancel active annotation and resource jobs before removing data".into());
    }
    invalidate_core_preparation(resource_id);
    remove_managed_resource_files(resource_id)
}

fn managed_download_is_active(resource_id: &str) -> bool {
    downloader::is_resource_active(resource_id)
        || (resource_id == "grch38-reference" && downloader::is_resource_active("ensembl-gff3"))
}

fn invalidate_core_preparation(resource_id: &str) {
    if matches!(resource_id, "grch38-reference" | "ensembl-gff3") {
        CORE_PREPARATION_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn cancel_and_delete_managed_resource(resource_id: &str) -> Result<(), String> {
    let managed_id = annocat_core::source_catalog::download_release(resource_id)
        .map(|release| release.resource_id)
        .ok_or_else(|| format!("resource '{resource_id}' is not managed yet"))?;
    install_queue::remove(resource_id);
    let paths = portable_paths()?;
    let active = managed_download_is_active(resource_id)
        || annotation::is_running()
        || (resource_id == "grch38-reference" && reference::is_running())
        || (matches!(resource_id, "grch38-reference" | "ensembl-gff3") && transcript::is_running())
        || preparation::live_status(resource_id).state == "running";
    if !active {
        invalidate_core_preparation(resource_id);
        return remove_managed_resource_files(resource_id);
    }
    invalidate_core_preparation(resource_id);
    downloader::discard_resource(resource_id, &paths.downloads);
    if resource_id == "grch38-reference" {
        downloader::discard_resource("ensembl-gff3", &paths.downloads);
    }
    preparation::cancel_live(resource_id);
    if resource_id == "grch38-reference" {
        reference::cancel_background();
    }
    if matches!(resource_id, "grch38-reference" | "ensembl-gff3") {
        transcript::cancel_background();
    }
    let resource_id = managed_id;
    std::thread::spawn(move || {
        while managed_download_is_active(resource_id)
            || (resource_id == "grch38-reference" && reference::is_running())
            || (matches!(resource_id, "grch38-reference" | "ensembl-gff3")
                && transcript::is_running())
            || preparation::live_status(resource_id).state == "running"
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if let Err(error) = remove_managed_resource_files(resource_id) {
            eprintln!(
                "[resources] could not finish cancellation cleanup for {resource_id}: {error}"
            );
        }
    });
    Ok(())
}

fn remove_managed_resource_files(resource_id: &str) -> Result<(), String> {
    let release = annocat_core::source_catalog::download_release(resource_id);
    if release.is_none() {
        return Err(format!("resource '{resource_id}' is not managed yet"));
    }
    let paths = portable_paths()?;
    let mut targets = vec![paths.resources.join(resource_id)];
    if let Some(release) = release {
        targets.push(paths.downloads.join(release.filename));
        targets.push(
            paths
                .downloads
                .join(format!("{}.partial", release.filename)),
        );
        // Older AnnoCat download builds used this suffix while replacing a
        // partial download. Treat it as managed data so upgrades can always
        // remove a source completely.
        targets.push(
            paths
                .downloads
                .join(format!("{}.new-partial", release.filename)),
        );
        targets.push(
            paths
                .downloads
                .join(format!("{}.verified.json", release.filename)),
        );
    }
    if resource_id == "grch38-reference" {
        let release = release.expect("GRCh38 release is cataloged");
        let transcript_release =
            resource_release("ensembl-gff3").expect("Ensembl release is cataloged");
        targets.push(paths.downloads.join(transcript_release.filename));
        targets.push(
            paths
                .downloads
                .join(format!("{}.partial", transcript_release.filename)),
        );
        targets.push(
            paths
                .downloads
                .join(format!("{}.verified.json", transcript_release.filename)),
        );
        targets.push(paths.resources.join("ensembl-gff3"));
        targets.push(
            paths
                .resources
                .join("reference")
                .join("grch38")
                .join(release.version),
        );
        targets.push(paths.resources.join("transcript-cache"));
        targets.push(paths.resources.join("transcript-cache.partial"));
    }
    if resource_id == "ensembl-gff3" {
        targets.push(paths.resources.join("transcript-cache"));
        targets.push(paths.resources.join("transcript-cache.partial"));
    }
    for target in targets {
        if target.is_dir() {
            std::fs::remove_dir_all(&target)
                .map_err(|error| format!("cannot remove {}: {error}", target.display()))?;
        } else if target.is_file() {
            std::fs::remove_file(&target)
                .map_err(|error| format!("cannot remove {}: {error}", target.display()))?;
        }
    }
    preparation::forget_live(resource_id);
    if resource_id == "grch38-reference" {
        reference::forget();
    }
    if matches!(resource_id, "grch38-reference" | "ensembl-gff3") {
        transcript::forget();
    }
    println!("[resources] removed managed data for {resource_id}");
    Ok(())
}

fn portable_paths() -> Result<PortablePaths, String> {
    let home = portable_home()?;
    let config = load_config(&home)?;
    let resource_directory = config.resource_directory.unwrap_or_else(|| home.clone());
    let runs = config
        .results_directory
        .unwrap_or_else(|| home.join("runs"));
    Ok(PortablePaths {
        resources: resource_directory.join("resources"),
        downloads: resource_directory.join("downloads"),
        runs,
        config: home.join("config"),
        resource_directory,
        home,
    })
}

fn ensure_portable_layout() -> Result<(), String> {
    let paths = portable_paths()?;
    for path in [
        &paths.resources,
        &paths.downloads,
        &paths.runs,
        &paths.config,
    ] {
        std::fs::create_dir_all(path).map_err(|error| {
            format!(
                "cannot create portable directory {}: {error}",
                path.display()
            )
        })?;
    }
    if !config_file(&paths.home).exists() {
        save_resource_directory(&paths.resource_directory)?;
    }
    Ok(())
}

fn portable_paths_json() -> String {
    match portable_paths() {
        Ok(paths) => format!(
            "{{\"mode\":\"portable\",\"home\":\"{}\",\"resourceDirectory\":\"{}\",\"resources\":\"{}\",\"downloads\":\"{}\",\"runs\":\"{}\",\"config\":\"{}\"}}",
            json_escape(&paths.home.to_string_lossy()),
            json_escape(&paths.resource_directory.to_string_lossy()),
            json_escape(&paths.resources.to_string_lossy()),
            json_escape(&paths.downloads.to_string_lossy()),
            json_escape(&paths.runs.to_string_lossy()),
            json_escape(&paths.config.to_string_lossy())
        ),
        Err(error) => format!("{{\"error\":\"{}\"}}", json_escape(&error)),
    }
}

fn about_json() -> String {
    let pin: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/fastvep-pin.json")).unwrap_or_default();
    serde_json::json!({
        "name": "AnnoCAT",
        "version": env!("CARGO_PKG_VERSION"),
        "license": "Apache-2.0",
        "fastvepRepository": pin["repository"],
        "fastvepCommit": pin["commit"],
        "fastvepVersion": pin["upstreamVersion"]
    })
    .to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedRunSummary {
    id: String,
    name: String,
    original_name: String,
    completed_at: String,
    assembly: String,
    variant_count: u64,
    canonical_result_bytes: Option<u64>,
    annotated_vcf_bytes: Option<u64>,
}

fn completed_runs(runs_directory: &std::path::Path) -> Result<Vec<CompletedRunSummary>, String> {
    if !runs_directory.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(runs_directory).map_err(|error| {
        format!(
            "cannot inspect runs directory {}: {error}",
            runs_directory.display()
        )
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut runs = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let run_directory = entry.path();
        let manifest_path = run_directory.join("manifest.json");
        let Ok(metadata) = std::fs::metadata(&manifest_path) else {
            continue;
        };
        if metadata.len() == 0 || metadata.len() > 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if manifest["schemaVersion"] != 1 || manifest["state"] != "completed" {
            continue;
        }
        let Some(id) = manifest["runId"].as_str().filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let Some(name) = manifest["name"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 256)
        else {
            continue;
        };
        let Some(completed_at) = manifest["completedAt"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 64)
        else {
            continue;
        };
        let Some(assembly) = manifest["assembly"]
            .as_str()
            .filter(|value| matches!(*value, "GRCh37" | "GRCh38"))
        else {
            continue;
        };
        let Some(variant_count) = manifest["variantCount"].as_u64() else {
            continue;
        };
        let canonical_result_bytes = manifest["canonicalResultBytes"].as_u64();
        let annotated_vcf_bytes = manifest["annotatedVcfBytes"].as_u64();
        let Some(result_file) = manifest["resultFile"].as_str() else {
            continue;
        };
        let relative_result = std::path::Path::new(result_file);
        if relative_result.as_os_str().is_empty()
            || relative_result.is_absolute()
            || relative_result.extension().and_then(|value| value.to_str()) != Some("parquet")
            || !relative_result
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            continue;
        }
        let Ok(run_root) = run_directory.canonicalize() else {
            continue;
        };
        let Ok(result_path) = run_directory.join(relative_result).canonicalize() else {
            continue;
        };
        if !result_path.is_file() || !result_path.starts_with(&run_root) {
            continue;
        }
        let display_name =
            library_metadata::display_name(runs_directory, id).unwrap_or_else(|| name.to_owned());
        runs.push(CompletedRunSummary {
            id: id.into(),
            name: display_name,
            original_name: name.into(),
            completed_at: completed_at.into(),
            assembly: assembly.into(),
            variant_count,
            canonical_result_bytes,
            annotated_vcf_bytes,
        });
    }
    runs.sort_by(|left, right| right.completed_at.cmp(&left.completed_at));
    Ok(runs)
}

fn completed_runs_json(runs_directory: &std::path::Path) -> Result<String, String> {
    serde_json::to_string(&serde_json::json!({"runs": completed_runs(runs_directory)?}))
        .map_err(|error| error.to_string())
}

fn completed_run_result(
    runs_directory: &std::path::Path,
    requested_id: &str,
) -> Result<std::path::PathBuf, String> {
    if requested_id.is_empty()
        || requested_id.len() > 128
        || !requested_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid run identifier".into());
    }
    let entries = std::fs::read_dir(runs_directory)
        .map_err(|error| format!("cannot inspect completed runs: {error}"))?;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let directory = entry.path();
        let manifest_path = directory.join("manifest.json");
        let Ok(metadata) = std::fs::metadata(&manifest_path) else {
            continue;
        };
        if metadata.len() == 0 || metadata.len() > 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if manifest["schemaVersion"] != 1
            || manifest["state"] != "completed"
            || manifest["runId"] != requested_id
        {
            continue;
        }
        let Some(result_file) = manifest["resultFile"].as_str() else {
            continue;
        };
        let relative = std::path::Path::new(result_file);
        if relative.is_absolute()
            || !relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            continue;
        }
        let root = directory
            .canonicalize()
            .map_err(|error| format!("cannot resolve completed run: {error}"))?;
        let result = directory
            .join(relative)
            .canonicalize()
            .map_err(|error| format!("completed result is missing: {error}"))?;
        if result.is_file()
            && result.starts_with(root)
            && result.extension().and_then(|value| value.to_str()) == Some("parquet")
        {
            return Ok(result);
        }
    }
    Err(format!("completed run '{requested_id}' was not found"))
}

fn completed_run_file(
    runs_directory: &std::path::Path,
    requested_id: &str,
    manifest_key: &str,
    expected_name: &str,
    expected_extension: &str,
) -> Result<std::path::PathBuf, String> {
    let result = completed_run_result(runs_directory, requested_id)?;
    let root = result
        .parent()
        .ok_or("completed result has no containing directory")?
        .canonicalize()
        .map_err(|error| format!("cannot resolve completed run: {error}"))?;
    let manifest_path = root.join("manifest.json");
    let metadata = std::fs::metadata(&manifest_path)
        .map_err(|error| format!("completed run manifest is missing: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 1024 * 1024 {
        return Err("completed run manifest has an invalid size".into());
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .map_err(|error| format!("cannot read completed run manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid completed run manifest: {error}"))?;
    if manifest["runId"] != requested_id || manifest[manifest_key] != expected_name {
        return Err(format!("completed run does not declare {expected_name}"));
    }
    let file = root
        .join(expected_name)
        .canonicalize()
        .map_err(|error| format!("completed run file is missing: {error}"))?;
    if !file.is_file()
        || !file.starts_with(&root)
        || file.extension().and_then(|value| value.to_str()) != Some(expected_extension)
    {
        return Err("completed run file failed containment validation".into());
    }
    Ok(file)
}

fn setup_status_value(paths: &PortablePaths) -> serde_json::Value {
    let reference_ready = reference::is_ready(&paths.resources);
    let engine_ready = fastvep::readiness().ready;
    let transcript_cache_ready = transcript::is_ready(&paths.resources);
    serde_json::json!({
        "ready": reference_ready && engine_ready && transcript_cache_ready,
        "referenceReady": reference_ready,
        "engineReady": engine_ready,
        "transcriptCacheReady": transcript_cache_ready
    })
}

fn resources_status_json() -> Result<String, String> {
    let paths = portable_paths()?;
    let mut statuses = serde_json::Map::new();
    for release in annocat_core::source_catalog::download_releases() {
        let download = serde_json::to_value(downloader::status(&release, &paths.downloads))
            .map_err(|error| error.to_string())?;
        let prepare = match release.resource_id {
            "grch38-reference" => {
                serde_json::to_value(reference::status(&paths.downloads, &paths.resources))
            }
            "ensembl-gff3" => serde_json::to_value(transcript::status(&paths.resources)),
            id => serde_json::to_value(managed_preparation_status(id, &paths.resources)),
        };
        statuses.insert(
            release.resource_id.into(),
            serde_json::json!({
                "download": download,
                "prepare": prepare.map_err(|error| error.to_string())?
            }),
        );
    }
    serde_json::to_string(&serde_json::json!({
        "resources": statuses,
        "setup": setup_status_value(&paths)
    }))
    .map_err(|error| error.to_string())
}

fn resource_task_title(resource_id: &str) -> String {
    match resource_id {
        "grch38-reference" => "GRCh38 reference".into(),
        "ensembl-gff3" => "Ensembl transcript cache".into(),
        id => annocat_core::source_catalog::source(id)
            .map(|source| source.name.clone())
            .unwrap_or_else(|| id.to_owned()),
    }
}

fn task_snapshots(paths: &PortablePaths) -> Vec<tasks::TaskSnapshot> {
    let mut snapshots = annocat_core::source_catalog::download_releases()
        .filter_map(|release| {
            let title = resource_task_title(release.resource_id);
            let download = tasks::from_download(
                release.resource_id,
                &title,
                downloader::status(&release, &paths.downloads),
            );
            let installation = match release.resource_id {
                "grch38-reference" => tasks::from_reference(
                    release.resource_id,
                    &title,
                    reference::status(&paths.downloads, &paths.resources),
                ),
                "ensembl-gff3" => tasks::from_transcript(
                    release.resource_id,
                    &title,
                    transcript::status(&paths.resources),
                ),
                id => tasks::from_preparation(
                    id,
                    &title,
                    managed_preparation_status(id, &paths.resources),
                ),
            };
            tasks::choose_resource_task(download, installation)
        })
        .collect::<Vec<_>>();
    let annotation = tasks::from_annotation(annotation::status());
    if annotation.is_meaningful() {
        snapshots.push(annotation);
    }
    snapshots.sort_by(|left, right| {
        task_sort_rank(left)
            .cmp(&task_sort_rank(right))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.id.cmp(&right.id))
    });
    snapshots
}

fn task_sort_rank(task: &tasks::TaskSnapshot) -> u8 {
    match task.state.as_str() {
        "running" | "validating" | "cancelling" => 0,
        "queued" => 1,
        "failed" => 2,
        "paused" | "cancelled" | "downloaded" => 3,
        "ready" | "completed" => 4,
        _ => 5,
    }
}

fn tasks_json() -> Result<String, String> {
    let paths = portable_paths()?;
    let runs = completed_runs(&paths.runs)?;
    let completed_ids = runs
        .iter()
        .map(|run| run.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut snapshots = task_snapshots(&paths);
    snapshots.retain(|task| {
        task.state != "completed"
            || task
                .run_id
                .as_deref()
                .is_none_or(|run_id| !completed_ids.contains(run_id))
    });
    snapshots.extend(runs.iter().map(|run| {
        tasks::from_completed_run(
            &run.id,
            &run.name,
            &run.completed_at,
            &run.assembly,
            run.variant_count,
            run.canonical_result_bytes.unwrap_or(0),
        )
    }));
    snapshots.sort_by(|left, right| {
        task_sort_rank(left)
            .cmp(&task_sort_rank(right))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.title.cmp(&right.title))
    });
    serde_json::to_string(&serde_json::json!({"tasks": snapshots}))
        .map_err(|error| error.to_string())
}

fn pick_output_folder() -> Result<Option<String>, String> {
    let selection = run_native_dialog(|| {
        rfd::FileDialog::new()
            .set_title("Choose AnnoCat output folder")
            .pick_folder()
            .map(|path| path.to_string_lossy().into_owned())
    })?;
    Ok(selection)
}

fn share_completed_run_interactive(
    run_id: &str,
) -> Result<Option<report_package::PackageSummary>, String> {
    let paths = portable_paths()?;
    let result = completed_run_result(&paths.runs, run_id)?;
    let run_directory = result.parent().ok_or("completed run has no directory")?;
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(run_directory.join("manifest.json"))
            .map_err(|error| format!("cannot read completed run manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid completed run manifest: {error}"))?;
    let original_name = manifest["name"].as_str().unwrap_or("AnnoCat-report");
    let name = library_metadata::display_name(&paths.runs, run_id)
        .unwrap_or_else(|| original_name.to_owned());
    let completed = manifest["completedAt"]
        .as_str()
        .unwrap_or("unknown-time")
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect::<String>();
    let short_id = run_id.strip_prefix("run-").unwrap_or(run_id);
    let safe_name = report_safe_name(&name);
    let filename = format!(
        "{}--{}--{}.zip",
        safe_name,
        if completed.is_empty() {
            "unknown-time"
        } else {
            &completed
        },
        short_id.chars().take(12).collect::<String>()
    );
    let destination = run_native_dialog(move || {
        rfd::FileDialog::new()
            .set_title("Share AnnoCat report")
            .add_filter("AnnoCat report", &["zip"])
            .set_file_name(filename)
            .save_file()
    })?;
    let Some(destination) = destination else {
        return Ok(None);
    };
    let package =
        report_package::create_with_display_name(run_directory, &destination, Some(&name))?;
    if let Err(error) = worker::validate_report(&package.path) {
        let _ = std::fs::remove_file(&package.path);
        return Err(format!("created report failed import validation: {error}"));
    }
    Ok(Some(package))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilteredExportRequest {
    format: String,
    filters: results::PageRequest,
    #[serde(default)]
    columns: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FilteredExportSummary {
    path: std::path::PathBuf,
    rows: u64,
    genes: Option<u64>,
}

fn export_filtered_results_interactive(
    run_id: &str,
    request: &FilteredExportRequest,
) -> Result<Option<FilteredExportSummary>, String> {
    let paths = portable_paths()?;
    let result = completed_run_result(&paths.runs, run_id)?;
    let evidence = completed_run_file(
        &paths.runs,
        run_id,
        "evidenceFile",
        "evidence.parquet",
        "parquet",
    )
    .ok();
    let catalog = completed_run_file(
        &paths.runs,
        run_id,
        "fieldCatalogFile",
        "field-catalog.json",
        "json",
    )
    .ok();
    let name = library_metadata::display_name(&paths.runs, run_id)
        .unwrap_or_else(|| "AnnoCat-report".to_owned());
    let (title, extension, suffix) = match request.format.as_str() {
        "rowsCsv" => ("Export filtered variants", "csv", "filtered-variants"),
        "genesTxt" => ("Export filtered genes", "txt", "filtered-genes"),
        _ => return Err("export format must be rowsCsv or genesTxt".into()),
    };
    let default_name = format!("{}-{suffix}.{extension}", report_safe_name(&name));
    let destination = run_native_dialog(move || {
        rfd::FileDialog::new()
            .set_title(title)
            .add_filter(
                if extension == "csv" { "CSV" } else { "Text" },
                &[extension],
            )
            .set_file_name(default_name)
            .save_file()
    })?;
    let Some(destination) = destination else {
        return Ok(None);
    };
    match request.format.as_str() {
        "rowsCsv" => {
            let rows = results::export_filtered_rows_with_details(
                &result,
                evidence.as_deref(),
                catalog.as_deref(),
                &destination,
                &request.filters,
                &request.columns,
            )?;
            Ok(Some(FilteredExportSummary {
                path: destination,
                rows,
                genes: None,
            }))
        }
        "genesTxt" => {
            let genes = results::export_filtered_genes_with_details(
                &result,
                evidence.as_deref(),
                catalog.as_deref(),
                &destination,
                &request.filters,
            )?;
            let page: serde_json::Value = serde_json::from_str(&results::page_json_with_evidence(
                &result,
                evidence.as_deref(),
                catalog.as_deref(),
                0,
                1,
                &request.filters,
            )?)
            .map_err(|error| error.to_string())?;
            Ok(Some(FilteredExportSummary {
                path: destination,
                rows: page["total"].as_u64().unwrap_or(0),
                genes: Some(genes),
            }))
        }
        _ => unreachable!(),
    }
}

fn report_safe_name(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    let result = result
        .trim_matches('-')
        .chars()
        .take(80)
        .collect::<String>();
    if result.is_empty() {
        "AnnoCat-report".into()
    } else {
        result
    }
}

fn pick_resource_folder() -> Result<Option<String>, String> {
    if downloader::is_running() || reference::is_running() || preparation::running_count() > 0 {
        return Err(
            "cancel the active download or preparation before changing the resource directory"
                .into(),
        );
    }
    let selection = run_native_dialog(|| {
        rfd::FileDialog::new()
            .set_title("Choose AnnoCat resource folder")
            .pick_folder()
    })?;
    if let Some(path) = selection {
        save_resource_directory(&path)?;
        ensure_portable_layout()?;
        println!("[config] resource directory changed to {}", path.display());
        Ok(Some(path.to_string_lossy().into_owned()))
    } else {
        Ok(None)
    }
}

fn pick_results_folder() -> Result<Option<String>, String> {
    if annotation::is_running() {
        return Err("cancel the active annotation before changing the results directory".into());
    }
    let selection = run_native_dialog(|| {
        rfd::FileDialog::new()
            .set_title("Choose AnnoCat results folder")
            .pick_folder()
    })?;
    if let Some(path) = selection {
        save_results_directory(&path)?;
        std::fs::create_dir_all(&path).map_err(|error| {
            format!(
                "cannot create results directory {}: {error}",
                path.display()
            )
        })?;
        terminal_log(
            "config",
            format!("results directory changed to {}", path.display()),
        );
        Ok(Some(path.to_string_lossy().into_owned()))
    } else {
        Ok(None)
    }
}

static PREPARATION_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static CORE_PREPARATION_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn schedule_preparation(
    resource_id: &'static str,
    downloads: std::path::PathBuf,
    resources: std::path::PathBuf,
) {
    let core_epoch = CORE_PREPARATION_EPOCH.load(std::sync::atomic::Ordering::SeqCst);
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        let Some(release) = annocat_core::source_catalog::download_release(resource_id) else {
            return;
        };
        while downloader::is_resource_active(resource_id) {
            if CORE_PREPARATION_EPOCH.load(Ordering::SeqCst) != core_epoch {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if CORE_PREPARATION_EPOCH.load(Ordering::SeqCst) != core_epoch {
            return;
        }
        if !downloader::is_downloaded(&release, &downloads) {
            return;
        }
        while PREPARATION_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            if CORE_PREPARATION_EPOCH.load(Ordering::SeqCst) != core_epoch {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if CORE_PREPARATION_EPOCH.load(Ordering::SeqCst) != core_epoch {
            PREPARATION_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
        let started = match resource_id {
            "dbnsfp" => start_dbnsfp_preparation().is_ok(),
            "grch38-reference" if reference::should_prepare(&downloads, &resources) => {
                reference::start_background(downloads.clone(), resources.clone()).is_ok()
            }
            "ensembl-gff3" => {
                while downloader::is_resource_active("grch38-reference") || reference::is_running()
                {
                    if CORE_PREPARATION_EPOCH.load(Ordering::SeqCst) != core_epoch {
                        PREPARATION_ACTIVE.store(false, Ordering::SeqCst);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                if !reference::is_ready(&resources)
                    && reference::should_prepare(&downloads, &resources)
                    && reference::start_background(downloads.clone(), resources.clone()).is_ok()
                {
                    while reference::is_running() {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                }
                if !reference::is_ready(&resources) {
                    false
                } else {
                    let release =
                        resource_release("ensembl-gff3").expect("Ensembl release is cataloged");
                    fastvep::readiness().executable.is_some_and(|fastvep| {
                        transcript::start_background(
                            fastvep,
                            downloader::final_path(&downloads, &release),
                            reference::fasta_path(&resources),
                            resources.clone(),
                        )
                        .is_ok()
                    })
                }
            }
            _ => false,
        };
        if started && resource_id == "grch38-reference" {
            while reference::is_running() {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        PREPARATION_ACTIVE.store(false, Ordering::SeqCst);
    });
}

fn pick_vcf_files() -> Result<Vec<String>, String> {
    let selection = run_native_dialog(|| {
        rfd::FileDialog::new()
            .set_title("Choose one or more VCF files")
            .add_filter("Variant call files", &["vcf", "gz", "bgz"])
            .pick_files()
            .unwrap_or_default()
    })?;
    Ok(selection
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

fn pick_result_file() -> Result<Option<String>, String> {
    let selection = run_native_dialog(|| {
        rfd::FileDialog::new()
            .set_title("Open AnnoCat results")
            .add_filter("AnnoCat report", &["zip"])
            .pick_file()
    })?;
    Ok(selection.map(|path| path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod profile_status_tests {
    use super::*;

    #[test]
    fn server_port_parser_rejects_zero_and_unknown_options() {
        assert_eq!(parse_port(&[]).unwrap(), 8787);
        assert_eq!(parse_port(&["--port".into(), "8792".into()]).unwrap(), 8792);
        assert!(parse_port(&["--port".into(), "0".into()]).is_err());
        assert!(parse_port(&["--listen".into(), "8792".into()]).is_err());
    }

    #[test]
    fn storage_config_is_backward_compatible_and_keeps_both_directories() {
        let legacy: AppConfig =
            serde_json::from_str(r#"{"resource_directory":"D:\\resources"}"#).unwrap();
        assert_eq!(
            legacy.resource_directory,
            Some(std::path::PathBuf::from(r"D:\resources"))
        );
        assert!(legacy.results_directory.is_none());
        let current = AppConfig {
            resource_directory: Some(r"D:\resources".into()),
            results_directory: Some(r"E:\results".into()),
        };
        let encoded = serde_json::to_value(current).unwrap();
        assert_eq!(encoded["resource_directory"], r"D:\resources");
        assert_eq!(encoded["results_directory"], r"E:\results");
    }

    #[test]
    fn settings_owns_storage_locations_and_omits_removed_density_option() {
        let html = include_str!("../../../web/index.html");
        let app = include_str!("../../../web/src/app.js");
        assert!(html.contains("id=\"settings-resource-path\""));
        assert!(html.contains("id=\"settings-downloads-path\""));
        assert!(html.contains("id=\"settings-results-path\""));
        assert!(!html.contains("id=\"sources-resource-path\""));
        assert!(!html.contains("id=\"result-density\""));
        assert!(!html.contains("class=\"streaming-storage-note\""));
        assert!(html.contains("<option value=\"4\">4 — Maximum resource use</option>"));
        assert!(app.contains("formatDateTime(run.completedAt)"));
        assert!(app.contains("data-install-source-mode"));
        assert!(app.contains("data-install-concurrency"));
        assert!(app.contains("profileReviewResources(profile,installable)"));
        assert!(!app.contains("$('#result-density')"));
    }

    #[test]
    fn about_surface_and_metadata_use_the_project_apache_license() {
        let html = include_str!("../../../web/index.html");
        let manifest = include_str!("../../../Cargo.toml");
        let about: serde_json::Value = serde_json::from_str(&about_json()).unwrap();
        assert!(html.contains("id=\"about-button\""));
        assert!(html.contains("id=\"about-dialog\""));
        assert!(!html.contains("class=\"privacy\""));
        assert!(html.contains("Licensed under Apache License 2.0"));
        assert!(manifest.contains("license = \"Apache-2.0\""));
        assert_eq!(about["license"], "Apache-2.0");
        assert_eq!(about["version"], env!("CARGO_PKG_VERSION"));
        assert!(
            about["fastvepCommit"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[test]
    fn terminal_progress_compacts_replay_state_before_rendering() {
        let state = preparation::LivePreparationState {
            percent: 39.8,
            detail: "gnomad chromosome 7: replaying 0.18 GB of 9.04 GB retained hybrid part".into(),
            ..preparation::LivePreparationState::default()
        };
        assert_eq!(
            terminal_task_activity(&tasks::from_preparation("dbsnp", "dbSNP", state)),
            "replay 0.18 GB/9.04 GB"
        );
        assert_eq!(format_terminal_size(872_900_000), "872.9 MB");
        assert_eq!(format_terminal_size(39_000_000_000), "39.0 GB");
        assert_eq!(format_terminal_rate(12_300_000.0), "12.3 MB/s");
    }

    #[test]
    fn every_actionable_variant_release_uses_a_native_fastvep_parser() {
        assert_eq!(catalog_source_type("clinvar"), Some("clinvar"));
        assert_eq!(catalog_source_type("dbsnp"), Some("dbsnp"));
        assert_eq!(catalog_source_type("gnomad"), Some("gnomad"));
        assert_eq!(catalog_source_type("gnomad-genomes"), Some("gnomad"));
        assert_eq!(catalog_source_type("phylop"), Some("phylop"));
        assert_eq!(catalog_source_type("cadd"), Some("cadd"));
        assert_eq!(catalog_source_type("spliceai"), Some("spliceai"));
        assert_eq!(catalog_source_type("revel"), Some("revel"));
        assert_eq!(catalog_source_type("clingen"), None);
        assert_eq!(catalog_source_type("alphamissense"), None);
    }

    #[test]
    fn clinvar_discovery_selects_the_newest_dated_primary_vcf() {
        let listing = r#"href="clinvar_20260706.vcf.gz" href="clinvar_20260715_papu.vcf.gz" href="clinvar_20260715.vcf.gz.md5" href="clinvar_20260715.vcf.gz""#;
        assert_eq!(
            latest_clinvar_filename(listing).as_deref(),
            Some("clinvar_20260715.vcf.gz")
        );
        assert_eq!(latest_clinvar_filename("clinvar.vcf.gz"), None);
        assert_eq!(
            clinvar_md5("5151E8D3CA7FFB26ECFC296040BC4F48  /path/clinvar.vcf.gz\n").unwrap(),
            "5151e8d3ca7ffb26ecfc296040bc4f48"
        );
        assert!(clinvar_md5("not-a-digest clinvar.vcf.gz").is_err());
        let dbsnp_listing = r#"href="GCF_000001405.25.gz" href="GCF_000001405.40.gz.md5" href="GCF_000001405.40.gz""#;
        assert_eq!(
            latest_dbsnp_filename(dbsnp_listing).as_deref(),
            Some("GCF_000001405.40.gz")
        );
        assert_eq!(
            dbsnp_build("dbSNP build 157 release notes").as_deref(),
            Some("157")
        );
    }

    #[test]
    fn rolling_resolvers_read_content_length_from_head_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_LENGTH,
            reqwest::header::HeaderValue::from_static("3140346"),
        );
        assert_eq!(
            http_content_length_header(&headers, "missing").unwrap(),
            3_140_346
        );
        assert!(http_content_length_header(&reqwest::header::HeaderMap::new(), "missing").is_err());
    }

    #[test]
    fn mutable_latest_urls_must_use_the_rolling_checksum_policy() {
        for release in annocat_core::source_catalog::download_releases() {
            if release.url.contains("/latest") {
                assert!(
                    is_rolling_resource(release.resource_id),
                    "{} uses a mutable latest URL without rolling-release resolution",
                    release.resource_id
                );
                assert!(preparation_available(release.resource_id));
            }
        }
        for candidate in annocat_core::RESOURCE_CATALOG_CANDIDATES {
            let mutable_export = candidate.artifacts.iter().any(|artifact| {
                artifact.url_template.contains("/latest")
                    || artifact.url_template.contains("/download/action/")
            });
            if mutable_export {
                assert!(
                    is_rolling_resource(candidate.resource_id)
                        || !preparation_available(candidate.resource_id),
                    "{} became installable without rolling-release resolution",
                    candidate.resource_id
                );
            }
        }
        assert_eq!(ROLLING_RESOURCE_IDS, &["clinvar", "dbsnp"]);
    }

    #[test]
    fn profile_status_exposes_aggregate_progress_and_blockers() {
        let value: serde_json::Value =
            serde_json::from_str(&profile_preparation_status("wgs").unwrap()).unwrap();
        assert_eq!(value["profileId"], "wgs");
        assert!(value["expectedNetworkBytes"].as_u64().unwrap() > 0);
        assert!(value["networkBytes"].is_u64());
        assert!(value["preparedBytes"].is_u64());
        assert_eq!(value["disk"]["sourceDiskBytes"], 0);
        assert_eq!(value["disk"]["writerBufferBytes"], 1024 * 1024);
        assert!(value["throughputBytesPerSecond"].is_number());
        assert!(value["completedChromosomes"].is_u64());
        assert!(value["remainingChromosomes"].is_u64());
        assert!(value["percent"].is_number());
        assert!(
            !value["blockedResourceIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "spliceai")
        );
        assert_eq!(value["state"], "idle");
    }

    #[test]
    fn profiles_can_start_verified_sources_without_hiding_pending_sources() {
        let profile = annocat_core::source_catalog::profile("standard").unwrap();
        let actionable = profile
            .source_ids
            .iter()
            .filter(|id| preparation_available(id))
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            actionable,
            vec!["clinvar", "dbsnp", "gnomad", "phylop", "revel"]
        );
        assert!(!profile.source_ids.iter().any(|id| id == "dbnsfp"));
        assert!(profile.source_ids.iter().any(|id| id == "gnomad"));
    }

    #[test]
    fn download_routes_keep_pause_and_destructive_cancel_distinct() {
        assert_eq!(
            download_api_route("/api/resources/dbnsfp/download/pause"),
            Some(("dbnsfp", "pause"))
        );
        assert_eq!(
            download_api_route("/api/resources/dbnsfp/download/cancel"),
            Some(("dbnsfp", "cancel"))
        );
    }

    #[test]
    fn installation_concurrency_accepts_only_supported_worker_counts() {
        assert_eq!(set_preparation_concurrency("concurrency=4").unwrap(), 4);
        assert_eq!(install_queue::concurrency(), 4);
        assert!(set_preparation_concurrency("concurrency=0").is_err());
        assert!(set_preparation_concurrency("concurrency=5").is_err());
        assert!(set_preparation_concurrency("concurrency=lots").is_err());
        set_preparation_concurrency("concurrency=1").unwrap();
    }

    #[test]
    fn curated_dbnsfp_contract_is_versioned_unique_and_excludes_dedicated_sources() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../config/dbnsfp-4.9a-curated-fields.json"
        ))
        .unwrap();
        assert_eq!(contract["id"], "dbnsfp-4.9a-annocat-core-v1");
        assert_eq!(contract["release"], "4.9a");
        let fields = contract["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["fields"].as_array().unwrap())
            .map(|field| field.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(fields.len() >= 100);
        assert_eq!(
            fields
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            fields.len()
        );
        for required in [
            "Ensembl_transcriptid",
            "AlphaMissense_score",
            "REVEL_score",
            "MetaRNN_score",
            "GERP++_RS",
        ] {
            assert!(fields.contains(&required));
        }
        assert!(!fields.iter().any(|field| field.starts_with("gnomAD_")));
        assert!(!fields.iter().any(|field| field.starts_with("clinvar_")));
        let fastvep_pin: serde_json::Value =
            serde_json::from_str(include_str!("../../../config/fastvep-pin.json")).unwrap();
        assert_eq!(fastvep_pin["schemaVersion"], 2);
        assert_eq!(
            fastvep_pin["repository"],
            "https://github.com/annocat-project/fastVEP"
        );
        let changes = fastvep_pin["changes"].as_array().unwrap();
        assert!(changes.iter().any(|change| {
            change["commit"] == "231c1926a168800c3b9a0455d614699580871bee"
                && change["purpose"]
                    .as_str()
                    .is_some_and(|purpose| purpose.contains("135-field dbNSFP 4.9a contract"))
        }));
        assert_eq!(fastvep_pin["commit"], changes.last().unwrap()["commit"]);
    }

    #[test]
    fn browser_post_requests_include_the_local_csrf_header() {
        let app = include_str!("../../../web/src/app.js");
        let missing = app
            .lines()
            .filter(|line| line.contains("method:'POST'") && !line.contains("X-AnnoCat-CSRF"))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "POST request lines missing X-AnnoCat-CSRF: {missing:#?}"
        );
    }

    #[test]
    fn annotation_start_failures_use_the_unified_status_surface() {
        let app = include_str!("../../../web/src/app.js");
        let html = include_str!("../../../web/index.html");
        assert!(html.contains("id=\"global-status-button\""));
        assert!(html.contains("id=\"annotation-notice\""));
        assert!(app.contains("catch(error){setAnnotationStartError(error.message)}"));
        assert!(
            !app.contains("showResourceNotice(`Annotation could not start: ${error.message}`)")
        );
    }

    #[test]
    fn annotation_profiles_drive_selection_without_static_warning_cards() {
        let app = include_str!("../../../web/src/app.js");
        let html = include_str!("../../../web/index.html");
        assert!(html.contains("id=\"wizard-readiness\""));
        assert!(html.contains("id=\"review-readiness\""));
        assert!(!html.contains("download-note"));
        assert!(!html.contains("blocked-run"));
        assert!(app.contains("<option value=\"custom\">Custom</option>"));
        assert!(app.contains("$('#profile').value='custom'"));
        assert!(app.contains("selectedProfile()?.sourceIds.includes(id)"));
    }

    #[test]
    fn completed_runs_require_a_final_manifest_and_contained_result() {
        let root = std::env::temp_dir().join(format!(
            "annocat-completed-runs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let complete = root.join("complete");
        let incomplete = root.join("incomplete");
        std::fs::create_dir_all(&complete).unwrap();
        std::fs::create_dir_all(&incomplete).unwrap();
        std::fs::write(complete.join("variants.parquet"), b"fixture").unwrap();
        std::fs::write(complete.join("consequences.parquet"), b"fixture").unwrap();
        std::fs::write(complete.join("evidence.parquet"), b"fixture").unwrap();
        std::fs::write(complete.join("field-catalog.json"), br#"{"fields":[]}"#).unwrap();
        std::fs::write(
            complete.join("manifest.json"),
            br#"{"schemaVersion":1,"state":"completed","runId":"run-2","name":"HG002 chromosome 22","completedAt":"2026-07-16T20:30:00-04:00","assembly":"GRCh38","variantCount":123,"canonicalResultBytes":4567,"annotatedVcfBytes":8901,"resultFile":"variants.parquet","consequencesFile":"consequences.parquet","evidenceFile":"evidence.parquet","fieldCatalogFile":"field-catalog.json"}"#,
        )
        .unwrap();
        std::fs::write(
            incomplete.join("manifest.json"),
            br#"{"schemaVersion":1,"state":"running","runId":"run-1"}"#,
        )
        .unwrap();
        library_metadata::rename(&root, "run-2", "Renamed report").unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&completed_runs_json(&root).unwrap()).unwrap();
        assert_eq!(value["runs"].as_array().unwrap().len(), 1);
        assert_eq!(value["runs"][0]["id"], "run-2");
        assert_eq!(value["runs"][0]["name"], "Renamed report");
        assert_eq!(value["runs"][0]["originalName"], "HG002 chromosome 22");
        assert_eq!(value["runs"][0]["variantCount"], 123);
        assert_eq!(value["runs"][0]["canonicalResultBytes"], 4567);
        assert_eq!(value["runs"][0]["annotatedVcfBytes"], 8901);
        assert_eq!(
            completed_run_file(
                &root,
                "run-2",
                "fieldCatalogFile",
                "field-catalog.json",
                "json"
            )
            .unwrap()
            .file_name()
            .unwrap(),
            "field-catalog.json"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
