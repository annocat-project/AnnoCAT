use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

#[derive(Debug, Parser)]
#[command(
    name = "annocat",
    version,
    about = "Annotate and review genetic variants on this computer"
)]
pub(crate) struct Cli {
    /// Use a different portable AnnoCAT home folder.
    #[arg(long, global = true, value_name = "FOLDER")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: CliCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CliCommand {
    /// Create one AnnoCAT result for each input VCF.
    Annotate(AnnotateArgs),
    /// Start the local AnnoCAT application.
    Launch(LaunchArgs),
    /// Show whether AnnoCAT is ready to annotate.
    Status(StatusArgs),
    /// List, inspect, transfer, or validate results.
    Results {
        #[command(subcommand)]
        command: ResultsCommand,
    },
    /// List, install, and remove annotation data.
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// View or recover unfinished work.
    Tasks {
        #[command(subcommand)]
        command: TasksCommand,
    },
    /// Inspect VCF files and annotation output.
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsCommand,
    },
    #[command(hide = true, trailing_var_arg = true)]
    ReportWorker { args: Vec<String> },
}

#[derive(Debug, Args)]
#[command(
    group(
        ArgGroup::new("selection")
            .required(true)
            .multiple(false)
            .args(["profile", "source", "core_only"])
    ),
    after_help = "Example:\n  annocat annotate -i sample.vcf.gz --profile standard"
)]
pub(crate) struct AnnotateArgs {
    /// Input VCF or VCF.GZ. Repeat for a sequential batch.
    #[arg(short = 'i', long = "input", required = true, action = ArgAction::Append)]
    input: Vec<PathBuf>,
    /// Use the standard, comprehensive, or online profile.
    #[arg(
        long,
        value_parser = ["standard", "comprehensive", "online"]
    )]
    profile: Option<String>,
    /// Use this installed annotation source. Repeat to select more than one.
    #[arg(long, action = ArgAction::Append)]
    source: Vec<String>,
    /// Create a result with local transcript consequences only.
    #[arg(long)]
    core_only: bool,
    /// Result name. Valid only with one input.
    #[arg(long)]
    name: Option<String>,
    /// Put the result in this folder instead of the configured results folder.
    #[arg(short = 'o', long = "output-folder", value_name = "FOLDER")]
    output_folder: Option<PathBuf>,
    /// Retain the annotated VCF with the structured result.
    #[arg(long)]
    include_annotated_vcf: bool,
    /// Use only when the input is GRCh38 and its header does not identify it. This option does not convert the assembly.
    #[arg(long)]
    confirm_grch38: bool,
    /// Print one machine-readable JSON document.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct LaunchArgs {
    /// Listen on this local TCP port.
    #[arg(long, default_value_t = 8787, value_parser = clap::value_parser!(u16).range(1..))]
    pub(crate) port: u16,
    /// Start the service without opening the browser.
    #[arg(long)]
    no_open: bool,
}

#[derive(Debug, Args)]
pub(crate) struct StatusArgs {
    /// Show readiness for the standard, comprehensive, or online profile.
    #[arg(
        long,
        value_parser = ["standard", "comprehensive", "online"]
    )]
    profile: Option<String>,
    /// Print the complete readiness state as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ResultsCommand {
    /// List completed results in the configured results folder.
    List(OutputArgs),
    /// Show stored metadata for one result without reading variant rows.
    Show {
        /// Result ID from `annocat results list`.
        result_id: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Create a validated ZIP that contains one result and its candidate bookmarks.
    #[command(after_help = "Example:\n  annocat results export run-123 -o sample.zip")]
    Export {
        /// Result ID from `annocat results list`.
        result_id: String,
        /// New ZIP file to create. An existing file is never replaced.
        #[arg(short = 'o', long, value_name = "FILE.zip")]
        output: PathBuf,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Validate and import an AnnoCAT result ZIP.
    Import {
        /// AnnoCAT result ZIP to validate and import.
        file: PathBuf,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Validate result files and hashes. Do not change the result.
    Validate {
        /// Result ID from the configured library or a result ZIP path.
        result_or_file: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct OutputArgs {
    /// Print one machine-readable JSON document.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SourcesCommand {
    /// List annotation sources and their installation state.
    List {
        /// Show only sources in this profile.
        #[arg(
            long,
            value_parser = ["standard", "comprehensive", "online"]
        )]
        profile: Option<String>,
        /// Show only installed sources.
        #[arg(long)]
        installed: bool,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Show detailed state and storage use for one annotation source.
    Status {
        /// Annotation source ID. Use `annocat sources list` to find IDs.
        source_id: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// List fields available for one annotation source.
    Fields {
        /// Annotation source ID with configurable fields.
        source_id: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Fully verify installed annotation data without changing it.
    Verify {
        /// Annotation source IDs. Omit to verify all installed data.
        #[arg(value_name = "SOURCE_ID", action = ArgAction::Append)]
        source_ids: Vec<String>,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Install and verify annotation data.
    #[command(group(
        ArgGroup::new("install_selection")
            .required(true)
            .multiple(false)
            .args(["profile", "source_ids"])
    ), after_help = "Example:\n  annocat sources install --profile standard --dry-run")]
    Install {
        /// One or more annotation source IDs.
        #[arg(value_name = "SOURCE_ID", action = ArgAction::Append)]
        source_ids: Vec<String>,
        /// Install all data in the standard, comprehensive, or online profile.
        #[arg(
            long,
            value_parser = ["standard", "comprehensive", "online"]
        )]
        profile: Option<String>,
        /// Keep the recommended fields or all available fields.
        #[arg(long, value_enum)]
        field_set: Option<FieldSet>,
        /// Keep this field. Repeat for each field to keep. This replaces the recommended field set.
        #[arg(long = "field", action = ArgAction::Append)]
        fields: Vec<String>,
        /// Show the installation plan and leave files unchanged.
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Remove installed annotation data and its retained download. Existing results are not changed.
    Remove {
        /// Annotation source ID to remove.
        source_id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FieldSet {
    Recommended,
    All,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TasksCommand {
    /// List active tasks and tasks that need attention.
    List {
        /// Include completed tasks.
        #[arg(long)]
        all: bool,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Show progress, errors, and available actions for one task.
    Show {
        /// Stable task ID from `annocat tasks list`.
        task_id: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Resume a recoverable task and wait for it to finish.
    Resume {
        /// Stable task ID whose available actions include `resume`.
        task_id: String,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Discard a recoverable task and its partial data.
    Cancel {
        /// Stable task ID to discard.
        task_id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Print one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DiagnosticsCommand {
    /// Summarize assembly, samples, records, and alleles in a VCF.
    Vcf {
        /// Input VCF or VCF.GZ to scan.
        file: PathBuf,
    },
    /// Summarize CSQ fields and record counts in fastVEP VCF output.
    Fastvep {
        /// fastVEP VCF output to scan.
        file: PathBuf,
    },
    /// Validate allele normalization and REF bases against the installed GRCh38 reference.
    #[command(
        after_help = "Example:\n  annocat diagnostics normalization sample.vcf.gz --limit 10000"
    )]
    Normalization {
        /// Input VCF or VCF.GZ to check.
        file: PathBuf,
        /// Limit the check to one chromosome or contig.
        #[arg(long)]
        chromosome: Option<String>,
        /// Stop after this many alternate alleles.
        #[arg(long)]
        limit: Option<u64>,
    },
}

#[derive(Clone, Copy)]
enum LockMode {
    None,
    Shared,
    Exclusive,
}

struct HomeLock(Option<File>);

impl HomeLock {
    fn acquire(mode: LockMode) -> Result<Self, String> {
        if matches!(mode, LockMode::None) {
            return Ok(Self(None));
        }
        let home = portable_home()?;
        std::fs::create_dir_all(&home)
            .map_err(|error| format!("cannot create AnnoCAT home {}: {error}", home.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(home.join(".annocat.lock"))
            .map_err(|error| format!("cannot open the AnnoCAT home lock: {error}"))?;
        let result = match mode {
            LockMode::Shared => file.try_lock_shared(),
            LockMode::Exclusive => file.try_lock(),
            LockMode::None => unreachable!(),
        };
        result.map_err(|_| {
            "another AnnoCAT process is using this home folder; wait for it to finish".to_string()
        })?;
        Ok(Self(Some(file)))
    }
}

impl Drop for HomeLock {
    fn drop(&mut self) {
        if let Some(file) = &self.0 {
            let _ = file.unlock();
        }
    }
}

pub(crate) fn main_entry() {
    let cli = Cli::parse();
    let exit_code = match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("annocat: {error}");
            1
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn run(cli: Cli) -> Result<i32, String> {
    if let Some(home) = cli.home {
        set_portable_home(home)?;
    }
    let mode = lock_mode(&cli.command);
    let _lock = HomeLock::acquire(mode)?;
    match cli.command {
        CliCommand::Annotate(args) => annotate(args),
        CliCommand::Launch(args) => {
            ensure_portable_layout()?;
            serve(args.port, !args.no_open)?;
            Ok(0)
        }
        CliCommand::Status(args) => status(args),
        CliCommand::Results { command } => results(command),
        CliCommand::Sources { command } => sources(command),
        CliCommand::Tasks { command } => tasks(command),
        CliCommand::Diagnostics { command } => diagnostics(command),
        CliCommand::ReportWorker { args } => {
            report_worker_command(&args)?;
            Ok(0)
        }
    }
}

fn lock_mode(command: &CliCommand) -> LockMode {
    match command {
        CliCommand::Launch(_)
        | CliCommand::Annotate(_)
        | CliCommand::Results {
            command: ResultsCommand::Import { .. },
        }
        | CliCommand::Sources {
            command: SourcesCommand::Install { dry_run: false, .. } | SourcesCommand::Remove { .. },
        }
        | CliCommand::Tasks {
            command: TasksCommand::Resume { .. } | TasksCommand::Cancel { .. },
        } => LockMode::Exclusive,
        CliCommand::Status(_)
        | CliCommand::Results {
            command: ResultsCommand::Export { .. },
        }
        | CliCommand::Sources { .. }
        | CliCommand::Tasks {
            command: TasksCommand::List { .. } | TasksCommand::Show { .. },
        } => LockMode::Shared,
        CliCommand::Results {
            command: ResultsCommand::List(_) | ResultsCommand::Show { .. },
        } => LockMode::None,
        CliCommand::Results {
            command: ResultsCommand::Validate { result_or_file, .. },
        } if Path::new(result_or_file).is_file() => LockMode::None,
        CliCommand::Results {
            command: ResultsCommand::Validate { .. },
        } => LockMode::Shared,
        CliCommand::Diagnostics { .. } | CliCommand::ReportWorker { .. } => LockMode::None,
    }
}

fn public_profile(value: &str) -> Result<&'static str, String> {
    match value {
        "standard" => Ok("standard"),
        "comprehensive" => Ok("wgs"),
        "online" => Ok("online"),
        _ => Err(format!(
            "profile '{value}' does not exist; use standard, comprehensive, or online"
        )),
    }
}

fn profile_source_ids(value: &str) -> Result<Vec<String>, String> {
    let internal = public_profile(value)?;
    let profile = annocat_core::source_catalog::profile(internal)
        .ok_or_else(|| format!("profile '{value}' is missing from the source catalog"))?;
    annotation::normalize_source_ids(profile.source_ids.clone())
}

fn annotate(args: AnnotateArgs) -> Result<i32, String> {
    if args.input.len() > 1 && args.name.is_some() {
        return Err("--name can be used only with one input VCF".into());
    }
    let profile = args.profile.as_deref();
    let source_ids = if let Some(profile) = profile {
        profile_source_ids(profile)?
    } else if args.core_only {
        Vec::new()
    } else {
        let mut seen = std::collections::HashSet::new();
        if let Some(duplicate) = args
            .source
            .iter()
            .find(|source| !seen.insert(source.as_str()))
        {
            return Err(format!("source '{duplicate}' was supplied more than once"));
        }
        annotation::normalize_source_ids(args.source.clone())?
    };

    let paths = portable_paths()?;
    let requests = args
        .input
        .into_iter()
        .map(|input| annotation::AnnotationRequest {
            input,
            requested_profile: profile.map(str::to_owned),
            name: args.name.clone(),
            output_directory: args.output_folder.clone(),
            source_ids: source_ids.clone(),
            include_annotated_vcf: args.include_annotated_vcf,
            run_mode: annotation::RunMode::Annotation,
            add_local_consequences: false,
            confirm_grch38: args.confirm_grch38,
        })
        .collect::<Vec<_>>();
    for request in &requests {
        annotation::validate_request(request, &paths.resources)
            .map_err(|error| format!("{}: {error}", request.input.display()))?;
    }

    let mut created = Vec::with_capacity(requests.len());
    for request in requests {
        match annotation::run_blocking(request, paths.runs.clone(), paths.resources.clone()) {
            Ok(output) => created.push(output),
            Err(error) => {
                for path in &created {
                    eprintln!("Completed before failure: {}", path.display());
                }
                return Err(error);
            }
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "results": created.iter().map(|path| {
                    serde_json::json!({"path": path, "onlineAnnotationsIncluded": false})
                }).collect::<Vec<_>>()
            })
        );
    } else {
        for path in created {
            println!("Created AnnoCAT result: {}", path.display());
        }
        if profile == Some("online") {
            println!("Online annotations can be added later in the result viewer.");
        }
    }
    Ok(0)
}

fn status(args: StatusArgs) -> Result<i32, String> {
    ensure_portable_layout()?;
    let paths = portable_paths()?;
    let status = app_status()?;
    let fastvep = fastvep::readiness();
    let requested_profile = args.profile.as_deref();
    let profile = args
        .profile
        .as_deref()
        .map(public_profile)
        .transpose()?
        .map(profile_preparation_status)
        .transpose()?;
    let profile_field_selections = requested_profile
        .map(|profile| {
            resolve_install_request(Vec::new(), Some(profile), None, Vec::new(), &paths)
                .map(|request| request.field_selections)
        })
        .transpose()?
        .unwrap_or_default();
    let profile_ready = profile.as_ref().is_none_or(|profile| {
        if profile.profile_id == "online" {
            status.resources.setup.ready
        } else {
            status.resources.setup.ready && profile.state == "ready"
        }
    });
    let ready = status.resources.setup.ready && profile_ready;
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ready": ready,
                "folders": {
                    "home": paths.home,
                    "resources": paths.resources,
                    "downloads": paths.downloads,
                    "results": paths.runs
                },
                "application": status,
                "profile": profile,
                "profileFieldSelections": profile_field_selections,
                "fastvep": fastvep
            })
        );
    } else {
        println!("AnnoCAT status");
        println!("  Ready       : {}", yes_no(ready));
        println!("  Home        : {}", paths.home.display());
        println!("  Data        : {}", paths.resources.display());
        println!("  Downloads   : {}", paths.downloads.display());
        println!("  Results     : {}", paths.runs.display());
        println!(
            "  fastVEP     : {}",
            if fastvep.ready {
                "Ready".to_string()
            } else {
                format!("Not ready ({})", fastvep.state)
            }
        );
        println!(
            "  Reference   : {}",
            ready_text(status.resources.setup.reference_ready)
        );
        println!(
            "  Transcripts : {}",
            ready_text(status.resources.setup.transcript_cache_ready)
        );
        if !fastvep.ready {
            println!("  Blocker     : {}", fastvep.next_action);
            if let Some(path) = &fastvep.executable {
                println!("  Expected at : {}", path.display());
            }
        }
        if !status.resources.setup.reference_ready {
            println!("  Blocker     : Install the GRCh38 reference");
            println!("  Run         : annocat sources install grch38-reference");
        }
        if !status.resources.setup.transcript_cache_ready {
            println!("  Blocker     : Install the Ensembl transcript cache");
            println!("  Run         : annocat sources install ensembl-gff3");
        }
        if let Some(profile) = profile {
            println!(
                "  Profile     : {} ({})",
                requested_profile.unwrap_or(&profile.profile_id),
                profile.state
            );
            if profile.profile_id == "online" {
                println!("  Online data : Available later in the result viewer");
            }
            for selection in &profile_field_selections {
                println!(
                    "  {} fields: {} selected ({})",
                    selection.source_id,
                    selection.fields.len(),
                    selection.contract_id
                );
            }
        }
        let attention = status
            .tasks
            .iter()
            .filter(|task| matches!(task.state.as_str(), "failed" | "paused" | "interrupted"))
            .count();
        println!("  Tasks       : {} need attention", attention);
    }
    Ok(if ready { 0 } else { 1 })
}

fn results(command: ResultsCommand) -> Result<i32, String> {
    ensure_portable_layout()?;
    let paths = portable_paths()?;
    match command {
        ResultsCommand::List(args) => {
            let runs = completed_runs(&paths.runs)?;
            if args.json {
                println!("{}", serialize_json(&runs));
            } else if runs.is_empty() {
                println!("No AnnoCAT results.");
            } else {
                println!(
                    "{:<32}  {:<28}  {:<6}  {:>12}  Completed",
                    "Result ID", "Name", "Build", "Variants"
                );
                for run in runs {
                    println!(
                        "{:<32}  {:<28}  {:<6}  {:>12}  {}",
                        run.id,
                        truncate(&run.name, 28),
                        run.assembly,
                        run.variant_count,
                        run.completed_at
                    );
                }
            }
        }
        ResultsCommand::Show { result_id, json } => {
            let result = completed_run_result(&paths.runs, &result_id)?;
            let root = result.parent().ok_or("AnnoCAT result has no folder")?;
            let manifest = read_json(root.join("manifest.json"))?;
            if json {
                println!("{}", manifest);
            } else {
                println!(
                    "AnnoCAT result {}",
                    manifest["runId"].as_str().unwrap_or(&result_id)
                );
                println!(
                    "  Name       : {}",
                    manifest["name"].as_str().unwrap_or("Not available")
                );
                println!(
                    "  State      : {}",
                    manifest["state"].as_str().unwrap_or("Not available")
                );
                if let Some(result_type) = manifest["reportKind"].as_str() {
                    println!("  Type       : {result_type}");
                }
                println!(
                    "  Assembly   : {}",
                    manifest["assembly"].as_str().unwrap_or("Not available")
                );
                println!(
                    "  Variants   : {}",
                    manifest["variantCount"]
                        .as_u64()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "Not available".into())
                );
                println!(
                    "  Completed  : {}",
                    manifest["completedAt"].as_str().unwrap_or("Not available")
                );
                if let Some(profile) = manifest["requestedProfile"].as_str() {
                    println!("  Profile    : {profile}");
                }
                let sources = manifest["sourceIds"].as_array().map(|sources| {
                    let sources = sources
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>();
                    if sources.is_empty() {
                        "Core only".to_string()
                    } else {
                        sources.join(", ")
                    }
                });
                println!(
                    "  Sources    : {}",
                    sources.as_deref().unwrap_or("Not available")
                );
                let stored_bytes = manifest_data_bytes(&manifest);
                if stored_bytes > 0 {
                    println!("  Stored data: {stored_bytes} bytes");
                }
                let retained_vcf = manifest["includeAnnotatedVcf"]
                    .as_bool()
                    .unwrap_or_else(|| manifest["annotatedVcfFile"].is_string());
                println!("  VCF retained: {}", yes_no(retained_vcf));
                println!("  Folder     : {}", root.display());
            }
        }
        ResultsCommand::Export {
            result_id,
            output,
            json,
        } => {
            if output.exists() {
                return Err(format!("output file already exists: {}", output.display()));
            }
            let result = completed_run_result(&paths.runs, &result_id)?;
            let root = result.parent().ok_or("AnnoCAT result has no folder")?;
            let candidates = library_metadata::candidate_snapshot(&paths.runs, &result_id)?;
            let package = report_package::create(root, &output, &candidates)?;
            if let Err(error) = worker::validate_report(&package.path) {
                let _ = std::fs::remove_file(&package.path);
                return Err(format!("the exported result failed validation: {error}"));
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "resultId": package.run_id,
                        "path": package.path,
                        "bytes": package.bytes
                    })
                );
            } else {
                println!(
                    "Exported AnnoCAT result: {} ({} bytes)",
                    package.path.display(),
                    package.bytes
                );
            }
        }
        ResultsCommand::Import { file, json } => {
            let imported = report_library::import(&file, &paths.runs)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "resultId": imported.run_id,
                        "name": imported.name,
                        "folder": imported.directory
                    })
                );
            } else {
                println!("Imported AnnoCAT result: {}", imported.run_id);
            }
        }
        ResultsCommand::Validate {
            result_or_file,
            json,
        } => {
            if completed_run_result(&paths.runs, &result_or_file).is_ok() {
                let manifest = validate_local_result(&paths.runs, &result_or_file)?;
                let schema = manifest["schemaVersion"].as_u64().unwrap_or(0);
                let files = manifest_file_count(&manifest);
                let bytes = manifest_data_bytes(&manifest);
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "valid": true,
                            "resultId": result_or_file,
                            "schemaVersion": schema,
                            "fileCount": files,
                            "bytes": bytes
                        })
                    );
                } else {
                    println!(
                        "Valid AnnoCAT result: {result_or_file} \
                         (schema {schema}, {files} files, {bytes} bytes)"
                    );
                }
            } else {
                let file = PathBuf::from(&result_or_file);
                let message = worker::validate_report(&file)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"valid": true, "path": file, "detail": message})
                    );
                } else {
                    println!("{message}");
                }
            }
        }
    }
    Ok(0)
}

fn validate_local_result(runs: &Path, result_id: &str) -> Result<serde_json::Value, String> {
    let variants = completed_run_result(runs, result_id)?;
    let root = variants.parent().ok_or("AnnoCAT result has no folder")?;
    let manifest = read_json(root.join("manifest.json"))?;
    let declared = [
        ("resultFile", "resultSha256"),
        ("consequencesFile", "consequencesSha256"),
        ("evidenceFile", "evidenceSha256"),
        ("fieldCatalogFile", "fieldCatalogSha256"),
    ];
    for (file_key, hash_key) in declared {
        let name = manifest[file_key]
            .as_str()
            .ok_or_else(|| format!("result manifest omits {file_key}"))?;
        let expected = manifest[hash_key]
            .as_str()
            .ok_or_else(|| format!("result manifest omits {hash_key}"))?;
        let actual = fastvep::sha256_file(&root.join(name))?;
        if actual != expected {
            return Err(format!("{name} does not match its declared SHA-256"));
        }
    }
    let expected = manifest["variantCount"]
        .as_u64()
        .ok_or("result manifest omits variantCount")?;
    let consequences = root.join(
        manifest["consequencesFile"]
            .as_str()
            .ok_or("result manifest omits consequencesFile")?,
    );
    let evidence = root.join(
        manifest["evidenceFile"]
            .as_str()
            .ok_or("result manifest omits evidenceFile")?,
    );
    let catalog = root.join(
        manifest["fieldCatalogFile"]
            .as_str()
            .ok_or("result manifest omits fieldCatalogFile")?,
    );
    if manifest["reportKind"] == "vcf-only" {
        results::validate_report_tables_allow_empty_consequences(
            &variants,
            &consequences,
            &evidence,
            &catalog,
            expected,
        )?;
    } else {
        results::validate_report_tables(&variants, &consequences, &evidence, &catalog, expected)?;
    }
    Ok(manifest)
}

fn manifest_data_bytes(manifest: &serde_json::Value) -> u64 {
    [
        "canonicalResultBytes",
        "consequencesBytes",
        "evidenceBytes",
        "fieldCatalogBytes",
        "annotatedVcfBytes",
    ]
    .iter()
    .filter_map(|key| manifest[*key].as_u64())
    .sum()
}

fn manifest_file_count(manifest: &serde_json::Value) -> usize {
    1 + [
        "resultFile",
        "consequencesFile",
        "evidenceFile",
        "fieldCatalogFile",
        "annotatedVcfFile",
    ]
    .iter()
    .filter(|key| manifest[**key].is_string())
    .count()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceSummary {
    id: String,
    name: String,
    required: bool,
    installed: bool,
    state: String,
    release: Option<String>,
    download_bytes: Option<u64>,
    retained_download_bytes: u64,
    cache_bytes: u64,
    field_contract_id: Option<String>,
    selected_field_count: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FieldCatalogEntry {
    id: String,
    display_name: String,
    description: String,
    value_type: &'static str,
    raw_name: String,
    required: bool,
    recommended: bool,
    selected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceVerificationResult {
    source_id: String,
    name: String,
    verified: bool,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<serde_json::Value>,
}

fn sources(command: SourcesCommand) -> Result<i32, String> {
    ensure_portable_layout()?;
    let paths = portable_paths()?;
    match command {
        SourcesCommand::List {
            profile,
            installed,
            json,
        } => {
            let allowed = profile.as_deref().map(profile_source_ids).transpose()?;
            let mut rows = all_source_ids()
                .into_iter()
                .filter(|id| {
                    allowed.as_ref().is_none_or(|allowed| {
                        matches!(id.as_str(), "grch38-reference" | "ensembl-gff3")
                            || allowed.contains(id)
                    })
                })
                .map(|id| source_summary(&id, &paths))
                .collect::<Result<Vec<_>, _>>()?;
            if installed {
                rows.retain(|row| row.installed);
            }
            if json {
                println!("{}", serialize_json(&rows));
            } else {
                println!(
                    "{:<20}  {:<30}  {:<8}  {:<18}  Release",
                    "Source ID", "Name", "Role", "State"
                );
                for row in rows {
                    println!(
                        "{:<20}  {:<30}  {:<8}  {:<18}  {}",
                        row.id,
                        truncate(&row.name, 30),
                        if row.required { "Required" } else { "Optional" },
                        row.state,
                        row.release.as_deref().unwrap_or("Not available")
                    );
                }
            }
        }
        SourcesCommand::Status { source_id, json } => {
            let row = source_summary(&source_id, &paths)?;
            if json {
                println!("{}", serialize_json(&row));
            } else {
                print_source_summary(&row);
            }
        }
        SourcesCommand::Fields { source_id, json } => {
            let value = field_configuration(&source_id, &paths)?;
            if json {
                println!("{}", value);
            } else {
                print_field_configuration(&value)?;
            }
        }
        SourcesCommand::Verify { source_ids, json } => {
            return verify_sources(source_ids, json, &paths);
        }
        SourcesCommand::Install {
            source_ids,
            profile,
            field_set,
            fields,
            dry_run,
            yes,
            json,
        } => {
            let request =
                resolve_install_request(source_ids, profile.as_deref(), field_set, fields, &paths)?;
            if dry_run {
                print_install_preview(&request, json)?;
                return Ok(0);
            }
            require_json_confirmation(json, yes)?;
            if !yes && !confirm("Install the listed annotation data?")? {
                return Err("installation canceled".into());
            }
            apply_field_selection(&request, &paths)?;
            for source_id in &request.source_ids {
                install_source_foreground(source_id, &paths)?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "installed": request.source_ids,
                        "profile": request.profile,
                        "onlineServices": request.online_services
                    })
                );
            } else {
                println!("Annotation data is ready.");
            }
        }
        SourcesCommand::Remove {
            source_id,
            yes,
            json,
        } => {
            let summary = source_summary(&source_id, &paths)?;
            require_json_confirmation(json, yes)?;
            if !yes
                && !confirm(&format!(
                    "Remove {}? This deletes {} cache bytes and {} retained download bytes.",
                    summary.name, summary.cache_bytes, summary.retained_download_bytes
                ))?
            {
                return Err("removal canceled".into());
            }
            delete_managed_resource(&source_id)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"removed": true, "sourceId": source_id})
                );
            } else {
                println!("Removed annotation data: {source_id}");
            }
        }
    }
    Ok(0)
}

fn verify_sources(
    mut source_ids: Vec<String>,
    json: bool,
    paths: &PortablePaths,
) -> Result<i32, String> {
    if source_ids.is_empty() {
        source_ids = all_source_ids()
            .into_iter()
            .filter(|id| has_source_data(id, &paths.resources))
            .collect();
    } else {
        source_ids.sort();
        source_ids.dedup();
    }

    let engine = fastvep::readiness();
    let mut rows = vec![SourceVerificationResult {
        source_id: "fastvep".into(),
        name: "fastVEP".into(),
        verified: engine.ready,
        detail: if engine.ready {
            "Pinned executable SHA-256 and version match".into()
        } else {
            format!("fastVEP is not ready: {}", engine.state)
        },
        summary: Some(serde_json::json!({
            "version": engine.version,
            "sha256": engine.sha256,
            "expectedSha256": engine.expected_sha256
        })),
    }];

    for source_id in source_ids {
        let name = resource_task_title(&source_id);
        if !json {
            eprintln!("Verifying {name}...");
        }
        let verified = verify_source(&source_id, paths, engine.executable.as_deref(), json);
        rows.push(match verified {
            Ok(summary) => SourceVerificationResult {
                source_id,
                name,
                verified: true,
                detail: verification_detail(&summary),
                summary: Some(summary),
            },
            Err(error) => SourceVerificationResult {
                source_id,
                name,
                verified: false,
                detail: error,
                summary: None,
            },
        });
    }

    if json {
        println!("{}", serialize_json(&rows));
    } else {
        for row in &rows {
            println!(
                "{:<30}  {:<8}  {}",
                truncate(&row.name, 30),
                if row.verified { "Verified" } else { "Failed" },
                row.detail
            );
        }
    }
    Ok(if rows.iter().all(|row| row.verified) {
        0
    } else {
        1
    })
}

fn verify_source(
    source_id: &str,
    paths: &PortablePaths,
    fastvep: Option<&Path>,
    json: bool,
) -> Result<serde_json::Value, String> {
    match source_id {
        "grch38-reference" => reference::verify(&paths.resources),
        "ensembl-gff3" => transcript::verify(
            fastvep.ok_or("fastVEP is required to verify the transcript cache")?,
            &paths.resources,
        ),
        "hpo" => phenotype::verify_assets(&paths.resources),
        _ => {
            if annocat_core::source_catalog::source(source_id).is_none() {
                return Err(format!("annotation source '{source_id}' does not exist"));
            }
            let root = active_source_root(source_id, &paths.resources)
                .ok_or_else(|| format!("{source_id} is not installed or is not ready"))?;
            let executable = fastvep.ok_or("fastVEP is required to verify OSA caches")?;
            let report = preparation::verify_source_cache(
                executable,
                &root,
                source_id,
                &resource_chromosomes(source_id),
                |chromosome| {
                    if !json {
                        eprintln!("  {source_id} chromosome {chromosome}");
                    }
                },
            )?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        }
    }
}

fn all_source_ids() -> Vec<String> {
    let mut ids = vec!["grch38-reference".to_string(), "ensembl-gff3".to_string()];
    ids.extend(
        annocat_core::source_catalog::catalog()
            .sources
            .iter()
            .filter(|source| source.id != favor::SERVICE_ID)
            .map(|source| source.id.clone()),
    );
    ids.sort();
    ids.dedup();
    ids
}

fn has_source_data(source_id: &str, resources: &Path) -> bool {
    match source_id {
        "grch38-reference" => resources.join("reference").is_dir(),
        "ensembl-gff3" => resources.join("transcript-cache").is_dir(),
        "hpo" => resources.join("hpo").is_dir(),
        _ => active_source_root(source_id, resources).is_some(),
    }
}

fn active_source_root(source_id: &str, resources: &Path) -> Option<PathBuf> {
    let chromosomes = resource_chromosomes(source_id);
    if is_rolling_resource(source_id) {
        let mut fallback = None;
        for version in installed_resource_versions(source_id, resources)
            .into_iter()
            .rev()
        {
            let root = resources.join(source_id).join(version);
            if root.join("shards").is_dir() {
                fallback.get_or_insert_with(|| root.clone());
            }
            if preparation::verified_storage_status(source_id, &root, &chromosomes).state == "ready"
            {
                return Some(root);
            }
        }
        return fallback;
    }
    let release = annocat_core::source_catalog::download_release(source_id)?;
    let root = resources.join(source_id).join(release.version);
    root.join("shards").is_dir().then_some(root)
}

fn verification_detail(summary: &serde_json::Value) -> String {
    if let Some(shards) = summary["shardCount"].as_u64() {
        let hashes = summary["hashedFileCount"].as_u64().unwrap_or(0);
        let structural = summary["structurallyVerifiedShardCount"]
            .as_u64()
            .unwrap_or(0);
        format!("{shards} shards; {hashes} file hashes; {structural} structural checks")
    } else {
        summary["scope"]
            .as_str()
            .unwrap_or("full verification passed")
            .replace('-', " ")
    }
}

fn source_summary(id: &str, paths: &PortablePaths) -> Result<SourceSummary, String> {
    let release = annocat_core::source_catalog::download_release(id);
    if release.is_none()
        && !matches!(id, "grch38-reference" | "ensembl-gff3")
        && annocat_core::source_catalog::source(id).is_none()
    {
        return Err(format!("annotation source '{id}' does not exist"));
    }
    let state = match id {
        "grch38-reference" => reference::status(&paths.downloads, &paths.resources).state,
        "ensembl-gff3" => transcript::status(&paths.resources).state.into(),
        _ => managed_preparation_status(id, &paths.resources).state,
    };
    let name = resource_task_title(id);
    let field_configuration = configurable_source(id)
        .then(|| field_configuration(id, paths))
        .transpose()?;
    Ok(SourceSummary {
        id: id.into(),
        name,
        required: matches!(id, "grch38-reference" | "ensembl-gff3"),
        installed: state == "ready",
        state,
        release: release.map(|release| release.version.into()),
        download_bytes: release.and_then(|release| release.download_bytes),
        retained_download_bytes: release
            .as_ref()
            .map(|release| downloader::status(release, &paths.downloads).downloaded_bytes)
            .unwrap_or(0),
        cache_bytes: directory_bytes(&paths.resources.join(id)),
        field_contract_id: field_configuration
            .as_ref()
            .and_then(|value| value["selection"]["contractId"].as_str())
            .map(str::to_owned),
        selected_field_count: field_configuration
            .as_ref()
            .and_then(|value| value["selection"]["fields"].as_array())
            .map(Vec::len),
    })
}

fn print_source_summary(row: &SourceSummary) {
    println!("{}", row.name);
    println!("  Source ID   : {}", row.id);
    println!(
        "  Role        : {}",
        if row.required { "Required" } else { "Optional" }
    );
    println!("  State       : {}", row.state);
    println!(
        "  Release     : {}",
        row.release.as_deref().unwrap_or("Not available")
    );
    println!("  Cache bytes : {}", row.cache_bytes);
    println!("  Retained download bytes: {}", row.retained_download_bytes);
    if let (Some(count), Some(contract)) = (row.selected_field_count, &row.field_contract_id) {
        println!("  Field set   : {count} selected ({contract})");
    }
}

fn field_configuration(
    source_id: &str,
    paths: &PortablePaths,
) -> Result<serde_json::Value, String> {
    let mut value = if source_id == "dbnsfp" {
        let configuration =
            preparation::dbnsfp_field_configuration(&paths.resources.join("dbnsfp").join("4.9a"))?;
        serde_json::to_value(configuration).map_err(|error| error.to_string())
    } else {
        let configuration = preparation::supplementary_field_configuration(
            source_id,
            &paths.resources.join(source_id),
        )?;
        serde_json::to_value(configuration).map_err(|error| error.to_string())
    }?;
    let catalog = build_field_catalog(source_id, &value)?;
    value
        .as_object_mut()
        .ok_or("field configuration is not an object")?
        .insert(
            "fieldCatalog".into(),
            serde_json::to_value(catalog).map_err(|error| error.to_string())?,
        );
    Ok(value)
}

fn print_field_configuration(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "Field set: {}{}",
        value["selection"]["contractId"]
            .as_str()
            .unwrap_or("Not available"),
        if value["locked"].as_bool().unwrap_or(false) {
            " (locked)"
        } else {
            ""
        }
    );
    for field in value["fieldCatalog"]
        .as_array()
        .ok_or("field configuration omits its field catalog")?
    {
        let mut states = Vec::new();
        for (key, label) in [
            ("required", "required"),
            ("recommended", "recommended"),
            ("selected", "selected"),
        ] {
            if field[key].as_bool().unwrap_or(false) {
                states.push(label);
            }
        }
        println!(
            "  {} [{}]{}",
            field["displayName"].as_str().unwrap_or("Unnamed field"),
            field["valueType"].as_str().unwrap_or("source-defined"),
            if states.is_empty() {
                String::new()
            } else {
                format!(" - {}", states.join(", "))
            }
        );
        println!(
            "    {}",
            field["description"]
                .as_str()
                .unwrap_or("No description is available.")
        );
        println!(
            "    ID: {}  Raw: {}",
            field["id"].as_str().unwrap_or("Not available"),
            field["rawName"].as_str().unwrap_or("Not available")
        );
    }
    Ok(())
}

fn build_field_catalog(
    source_id: &str,
    value: &serde_json::Value,
) -> Result<Vec<FieldCatalogEntry>, String> {
    let selected = value["selection"]["fields"]
        .as_array()
        .ok_or("field configuration omits selected fields")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<std::collections::HashSet<_>>();
    let recommended = value["contract"]["recommendedFields"]
        .as_array()
        .or_else(|| value["selection"]["fields"].as_array())
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect::<std::collections::HashSet<_>>();
    let source_name = resource_task_title(source_id);
    let mut catalog = Vec::new();
    for group in value["contract"]["groups"]
        .as_array()
        .ok_or("field contract has no groups")?
    {
        let required = group["required"].as_bool().unwrap_or(false);
        let group_name = group["label"]
            .as_str()
            .or_else(|| group["id"].as_str())
            .unwrap_or("annotation");
        for field in group["fields"].as_array().into_iter().flatten() {
            let Some(id) = field_id(field) else {
                continue;
            };
            catalog.push(FieldCatalogEntry {
                id: id.into(),
                display_name: readable_field_id(id),
                description: format!("{group_name} field retained from {source_name}."),
                value_type: "source-defined",
                raw_name: id.into(),
                required,
                recommended: recommended.contains(id),
                selected: selected.contains(id),
            });
        }
    }
    Ok(catalog)
}

fn readable_field_id(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_lowercase = false;
    for character in value.chars() {
        if matches!(character, '_' | '-') {
            output.push(' ');
            previous_lowercase = false;
        } else {
            if character.is_ascii_uppercase() && previous_lowercase {
                output.push(' ');
            }
            output.push(character);
            previous_lowercase = character.is_ascii_lowercase();
        }
    }
    output
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallRequest {
    profile: Option<String>,
    source_ids: Vec<String>,
    online_services: Vec<String>,
    field_selections: Vec<ResolvedFieldSelection>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedFieldSelection {
    source_id: String,
    contract_id: String,
    fields: Vec<String>,
}

fn resolve_install_request(
    direct_sources: Vec<String>,
    profile: Option<&str>,
    field_set: Option<FieldSet>,
    fields: Vec<String>,
    paths: &PortablePaths,
) -> Result<InstallRequest, String> {
    let (mut sources, online_services) = if let Some(profile) = profile {
        if field_set.is_some() || !fields.is_empty() {
            return Err("--field and --field-set cannot be used with --profile".into());
        }
        let sources = profile_source_ids(profile)?;
        let services = if profile == "online" {
            vec![favor::SERVICE_ID.into()]
        } else {
            Vec::new()
        };
        (sources, services)
    } else {
        let mut seen = std::collections::HashSet::new();
        if let Some(duplicate) = direct_sources
            .iter()
            .find(|source| !seen.insert(source.as_str()))
        {
            return Err(format!("source '{duplicate}' was supplied more than once"));
        }
        (
            annotation::normalize_source_ids(direct_sources)?,
            Vec::new(),
        )
    };

    let field_selections = if field_set.is_some() || !fields.is_empty() {
        if sources.len() != 1 {
            return Err("--field and --field-set require exactly one direct source".into());
        }
        vec![resolve_field_selection(
            &sources[0],
            field_set,
            fields,
            paths,
        )?]
    } else {
        sources
            .iter()
            .filter(|source_id| configurable_source(source_id))
            .map(|source_id| {
                resolve_field_selection(
                    source_id,
                    profile.map(|_| FieldSet::Recommended),
                    Vec::new(),
                    paths,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    if profile.is_some() {
        sources.insert(0, "ensembl-gff3".into());
        sources.insert(0, "grch38-reference".into());
    }
    Ok(InstallRequest {
        profile: profile.map(str::to_owned),
        source_ids: sources,
        online_services,
        field_selections,
    })
}

fn resolve_field_selection(
    source_id: &str,
    field_set: Option<FieldSet>,
    fields: Vec<String>,
    paths: &PortablePaths,
) -> Result<ResolvedFieldSelection, String> {
    if field_set.is_some() && !fields.is_empty() {
        return Err("--field and --field-set cannot be used together".into());
    }
    let configuration = field_configuration(source_id, paths)?;
    let contract_id = configuration["selection"]["contractId"]
        .as_str()
        .ok_or("field configuration omits its contract ID")?
        .to_owned();
    let groups = configuration["contract"]["groups"]
        .as_array()
        .ok_or("field contract has no groups")?;
    let mut allowed = Vec::new();
    let mut required = std::collections::HashSet::new();
    for group in groups {
        let group_required = group["required"].as_bool().unwrap_or(false);
        for field in group["fields"].as_array().into_iter().flatten() {
            if let Some(id) = field_id(field) {
                if !allowed.iter().any(|candidate| candidate == id) {
                    allowed.push(id.to_owned());
                }
                if group_required {
                    required.insert(id.to_owned());
                }
            }
        }
    }
    let requested = match field_set {
        Some(FieldSet::Recommended) => configuration["contract"]["recommendedFields"]
            .as_array()
            .or_else(|| configuration["selection"]["fields"].as_array())
            .ok_or("field contract has no recommended selection")?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        Some(FieldSet::All) => allowed.clone(),
        None if fields.is_empty() => configuration["selection"]["fields"]
            .as_array()
            .ok_or("field configuration omits its selection")?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        None => fields,
    };
    if let Some(unknown) = requested.iter().find(|field| !allowed.contains(field)) {
        return Err(format!(
            "field '{unknown}' is not available for {source_id}"
        ));
    }
    let requested = requested
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let fields = allowed
        .into_iter()
        .filter(|field| required.contains(field) || requested.contains(field))
        .collect();
    Ok(ResolvedFieldSelection {
        source_id: source_id.into(),
        contract_id,
        fields,
    })
}

fn field_id(value: &serde_json::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value["id"].as_str())
        .or_else(|| value["rawKey"].as_str())
}

fn configurable_source(source_id: &str) -> bool {
    source_id == "dbnsfp" || preparation::default_supplementary_field_selection(source_id).is_ok()
}

fn apply_field_selection(request: &InstallRequest, paths: &PortablePaths) -> Result<(), String> {
    for selection in &request.field_selections {
        if selection.source_id == "dbnsfp" {
            let current = preparation::default_dbnsfp_field_selection()?;
            preparation::save_dbnsfp_field_selection(
                &paths.resources.join("dbnsfp").join("4.9a"),
                preparation::DbnsfpFieldSelection {
                    schema_version: current.schema_version,
                    contract_id: selection.contract_id.clone(),
                    fields: selection.fields.clone(),
                },
            )?;
        } else {
            let current = preparation::default_supplementary_field_selection(&selection.source_id)?;
            preparation::save_supplementary_field_selection(
                &selection.source_id,
                &paths.resources.join(&selection.source_id),
                preparation::SupplementaryFieldSelection {
                    schema_version: current.schema_version,
                    contract_id: selection.contract_id.clone(),
                    fields: selection.fields.clone(),
                },
            )?;
        }
    }
    Ok(())
}

fn print_install_preview(request: &InstallRequest, json: bool) -> Result<(), String> {
    let paths = portable_paths()?;
    let available_disk_bytes =
        fs2::available_space(&paths.resources).map_err(|error| error.to_string())?;
    let rows = request
        .source_ids
        .iter()
        .map(|source_id| source_summary(source_id, &paths))
        .collect::<Result<Vec<_>, _>>()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "profile": request.profile,
                "sources": rows,
                "fieldSelections": request.field_selections,
                "onlineServices": request.online_services,
                "availableDiskBytes": available_disk_bytes
            })
        );
    } else {
        println!("Installation plan");
        println!("  Available disk: {available_disk_bytes} bytes");
        for row in rows {
            println!(
                "  {:<20} {:<18} {}",
                row.id,
                row.state,
                row.download_bytes
                    .map(|bytes| format!("{bytes} bytes"))
                    .unwrap_or_else(|| "size not available".into())
            );
        }
        for selection in &request.field_selections {
            println!(
                "  {} fields: {} selected ({})",
                selection.source_id,
                selection.fields.len(),
                selection.contract_id
            );
        }
        for service in &request.online_services {
            println!("  Online service: {service} (used later in the result viewer)");
        }
    }
    Ok(())
}

fn install_source_foreground(source_id: &str, paths: &PortablePaths) -> Result<(), String> {
    if source_summary(source_id, paths)?.installed {
        return Ok(());
    }
    match source_id {
        "grch38-reference" | "ensembl-gff3" => start_core_install(source_id, paths)?,
        _ => enqueue_preparation(source_id, false, false)?,
    }
    let mut last = String::new();
    loop {
        let status = source_summary(source_id, paths)?;
        if status.state != last {
            eprintln!("{}: {}", status.name, status.state);
            last = status.state.clone();
        }
        match status.state.as_str() {
            "ready" => return Ok(()),
            "failed" | "rebuild-required" => {
                return Err(format!("{} installation {}", status.name, status.state));
            }
            "paused" | "cancelled" => {
                return Err(format!("{} installation was paused", status.name));
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn start_core_install(source_id: &str, paths: &PortablePaths) -> Result<(), String> {
    let release = resource_release(source_id)?;
    if downloader::is_downloaded(&release, &paths.downloads) {
        match source_id {
            "grch38-reference" => {
                reference::start_background(paths.downloads.clone(), paths.resources.clone())?
            }
            "ensembl-gff3" => {
                if !reference::is_ready(&paths.resources) {
                    return Err("install the GRCh38 reference before the transcript cache".into());
                }
                let executable = fastvep::readiness()
                    .executable
                    .ok_or("fastVEP executable is unavailable")?;
                transcript::start_background(
                    executable,
                    downloader::final_path(&paths.downloads, &release),
                    reference::fasta_path(&paths.resources),
                    paths.resources.clone(),
                )?;
            }
            _ => unreachable!(),
        }
    } else {
        downloader::start_background(release, paths.downloads.clone())?;
        let id = match source_id {
            "grch38-reference" => "grch38-reference",
            "ensembl-gff3" => "ensembl-gff3",
            _ => unreachable!(),
        };
        schedule_preparation(id, paths.downloads.clone(), paths.resources.clone());
    }
    Ok(())
}

fn tasks(command: TasksCommand) -> Result<i32, String> {
    ensure_portable_layout()?;
    let paths = portable_paths()?;
    match command {
        TasksCommand::List { all, json } => {
            let mut tasks = task_status_for(&paths)?.tasks;
            if !all {
                tasks.retain(|task| task.is_active() || task_sort_rank(task) == 1);
            }
            if json {
                println!("{}", serialize_json(&tasks));
            } else if tasks.is_empty() {
                println!("No active tasks or tasks that need attention.");
            } else {
                println!(
                    "{:<36}  {:<24}  {:<18}  Progress",
                    "Task ID", "Name", "State"
                );
                for task in tasks {
                    println!(
                        "{:<36}  {:<24}  {:<18}  {:>5.1}%",
                        task.id,
                        truncate(&task.title, 24),
                        task.state,
                        task.percent
                    );
                }
            }
        }
        TasksCommand::Show { task_id, json } => {
            let task = find_task(&paths, &task_id)?;
            if json {
                println!("{}", serialize_json(&task));
            } else {
                print_task(&task);
            }
        }
        TasksCommand::Resume { task_id, json } => {
            let task = find_task(&paths, &task_id)?;
            if !task.available_actions.contains(&"resume") {
                return Err(format!("task '{task_id}' cannot be resumed"));
            }
            if task.kind == "annotation" {
                let run_id = task
                    .run_id
                    .as_deref()
                    .ok_or("annotation task has no run ID")?;
                annotation::resume_background(run_id, paths.runs.clone(), paths.resources.clone())?;
                wait_for_annotation(run_id)?;
            } else {
                let source_id = task
                    .resource_id
                    .as_deref()
                    .ok_or("source task has no source ID")?;
                install_source_foreground(source_id, &paths)?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({"taskId": task_id, "state": "completed"})
                );
            } else {
                println!("Task completed: {task_id}");
            }
        }
        TasksCommand::Cancel { task_id, yes, json } => {
            let task = find_task(&paths, &task_id)?;
            if !task_can_be_discarded(&task) {
                return Err(format!(
                    "task '{task_id}' does not have a cancel or discard action"
                ));
            }
            require_json_confirmation(json, yes)?;
            if !yes && !confirm("Discard this task and its partial managed data?")? {
                return Err("cancellation canceled".into());
            }
            if task.kind == "annotation" {
                annotation::discard_interrupted_run(
                    &paths.runs,
                    task.run_id
                        .as_deref()
                        .ok_or("annotation task has no run ID")?,
                )?;
            } else {
                cancel_and_delete_managed_resource(
                    task.resource_id
                        .as_deref()
                        .ok_or("source task has no source ID")?,
                )?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({"taskId": task_id, "discarded": true})
                );
            } else {
                println!("Discarded task: {task_id}");
            }
        }
    }
    Ok(0)
}

fn find_task(paths: &PortablePaths, task_id: &str) -> Result<tasks::TaskSnapshot, String> {
    task_status_for(paths)?
        .tasks
        .into_iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| format!("task '{task_id}' was not found"))
}

fn task_can_be_discarded(task: &tasks::TaskSnapshot) -> bool {
    task.available_actions
        .iter()
        .any(|action| matches!(*action, "cancel" | "discard"))
}

fn print_task(task: &tasks::TaskSnapshot) {
    println!("{}", task.title);
    println!("  Task ID     : {}", task.id);
    println!("  State       : {}", task.state);
    println!("  Activity    : {}", task.phase);
    println!("  Progress    : {:.1}%", task.percent);
    println!("  Detail      : {}", task.detail);
    if let Some(error) = &task.error {
        println!("  Error       : {error}");
    }
    println!("  Actions     : {}", task.available_actions.join(", "));
}

fn wait_for_annotation(run_id: &str) -> Result<(), String> {
    loop {
        let state = annotation::status();
        if state.run_id.as_deref() != Some(run_id) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        match state.state {
            "completed" => return Ok(()),
            "failed" | "paused" | "cancelled" => {
                return Err(state.error.unwrap_or(state.detail));
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn diagnostics(command: DiagnosticsCommand) -> Result<i32, String> {
    match command {
        DiagnosticsCommand::Vcf { file } => inspect_vcf_command(&file)?,
        DiagnosticsCommand::Fastvep { file } => inspect_fastvep_command(&file)?,
        DiagnosticsCommand::Normalization {
            file,
            chromosome,
            limit,
        } => check_normalization_command(&file, chromosome.as_deref(), limit)?,
    }
    Ok(0)
}

fn read_json(path: PathBuf) -> Result<serde_json::Value, String> {
    serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))
}

fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            entry
                .metadata()
                .ok()
                .map(|metadata| {
                    if metadata.is_dir() {
                        directory_bytes(&entry.path())
                    } else {
                        metadata.len()
                    }
                })
                .unwrap_or(0)
        })
        .sum()
}

fn confirm(prompt: &str) -> Result<bool, String> {
    if !io::stdin().is_terminal() {
        return Err("confirmation requires an interactive terminal or --yes".into());
    }
    eprint!("{prompt} [y/N] ");
    io::stderr().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("cannot read confirmation: {error}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn require_json_confirmation(json: bool, yes: bool) -> Result<(), String> {
    if json && !yes {
        Err("--json requires --yes for this operation".into())
    } else {
        Ok(())
    }
}

fn ready_text(ready: bool) -> &'static str {
    if ready { "Ready" } else { "Not ready" }
}

fn yes_no(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.into()
    } else if width <= 3 {
        ".".repeat(width)
    } else {
        value.chars().take(width - 3).collect::<String>() + "..."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_an_explicit_annotation_selection() {
        assert!(Cli::try_parse_from(["annocat", "annotate", "-i", "sample.vcf"]).is_err());
        assert!(
            Cli::try_parse_from([
                "annocat",
                "annotate",
                "-i",
                "sample.vcf",
                "--profile",
                "standard"
            ])
            .is_ok()
        );
    }

    #[test]
    fn parser_rejects_zero_ports_and_removed_root_commands() {
        assert!(Cli::try_parse_from(["annocat", "launch", "--port", "0"]).is_err());
        assert!(Cli::try_parse_from(["annocat", "doctor"]).is_err());
        assert!(Cli::try_parse_from(["annocat", "serve"]).is_err());
    }

    #[test]
    fn parser_rejects_unknown_profiles_and_source_status_without_an_id() {
        assert!(
            Cli::try_parse_from([
                "annocat",
                "annotate",
                "-i",
                "sample.vcf",
                "--profile",
                "custom"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["annocat", "sources", "status"]).is_err());
    }

    #[test]
    fn task_discard_requires_an_available_action() {
        let mut task = tasks::from_completed_run("run-test", "Test", "2026-07-30", "GRCh38", 1, 1);
        assert!(!task_can_be_discarded(&task));
        task.available_actions = vec!["discard"];
        assert!(task_can_be_discarded(&task));
    }

    #[test]
    fn table_values_truncate_to_the_requested_width() {
        assert_eq!(truncate("abcdefgh", 6), "abc...");
        assert_eq!(truncate("abcdefgh", 2), "..");
    }

    #[test]
    fn result_manifest_summary_counts_declared_files_and_bytes() {
        let manifest = serde_json::json!({
            "resultFile": "variants.parquet",
            "consequencesFile": "consequences.parquet",
            "evidenceFile": "evidence.parquet",
            "fieldCatalogFile": "field-catalog.json",
            "canonicalResultBytes": 10,
            "consequencesBytes": 20,
            "evidenceBytes": 30,
            "fieldCatalogBytes": 40
        });
        assert_eq!(manifest_file_count(&manifest), 5);
        assert_eq!(manifest_data_bytes(&manifest), 100);
    }

    #[test]
    fn public_profiles_map_to_catalog_profiles() {
        assert_eq!(public_profile("standard").unwrap(), "standard");
        assert_eq!(public_profile("comprehensive").unwrap(), "wgs");
        assert_eq!(public_profile("online").unwrap(), "online");
        assert!(public_profile("custom").is_err());
    }
}
