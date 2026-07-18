use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static CANCEL: AtomicBool = AtomicBool::new(false);
static BATCH_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationRequest {
    pub input: PathBuf,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub include_annotated_vcf: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAnnotationRequest {
    pub inputs: Vec<PathBuf>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub include_annotated_vcf: bool,
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
    pub output_bytes: u64,
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
    chromosomes: Vec<String>,
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

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(idle_state()))
}

fn idle_state() -> State {
    State {
        state: "idle",
        phase: "Waiting",
        detail: "No annotation run is active".into(),
        run_id: None,
        name: None,
        input: None,
        output: None,
        records: None,
        output_bytes: 0,
        cancel_requested: false,
        error: None,
    }
}

pub fn status_json() -> String {
    serde_json::to_string(
        &state()
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| idle_state()),
    )
    .unwrap_or_else(|_| {
        r#"{"state":"failed","phase":"Failed","detail":"Annotation state unavailable"}"#.into()
    })
}

pub fn is_running() -> bool {
    state().lock().is_ok_and(|value| value.state == "running")
}

pub fn cancel() -> bool {
    if !is_running() && !BATCH_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    BATCH_ACTIVE.store(false, Ordering::SeqCst);
    CANCEL.store(true, Ordering::SeqCst);
    if let Ok(mut current) = state().lock() {
        current.cancel_requested = true;
        current.detail = "Stopping fastVEP and discarding partial output".into();
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
            name: None,
            output_directory: request.output_directory.clone(),
            source_ids: request.source_ids.clone(),
            include_annotated_vcf: request.include_annotated_vcf,
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
                        current.phase = "Failed";
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
    {
        let mut current = state().lock().map_err(|_| "annotation state lock failed")?;
        if current.state == "running" {
            return Err("an annotation run is already active".into());
        }
        *current = State {
            state: "running",
            phase: "Queued",
            detail: "Validating the input VCF".into(),
            run_id: Some(run_id.clone()),
            name: Some(name.clone()),
            input: Some(request.input.clone()),
            output: Some(final_directory.clone()),
            records: None,
            output_bytes: 0,
            cancel_requested: false,
            error: None,
        };
    }
    CANCEL.store(false, Ordering::SeqCst);
    crate::terminal_log(
        "annotation",
        format!(
            "{run_id} queued: input={} sources={}",
            request.input.display(),
            if request.source_ids.is_empty() {
                "core".into()
            } else {
                request.source_ids.join(",")
            }
        ),
    );
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
        if result.as_ref().is_err_and(|error| error != "cancelled") && staging_directory.exists() {
            let failed = staging_directory.with_extension("failed");
            if failed.exists() {
                let _ = fs::remove_dir_all(&failed);
            }
            let _ = fs::rename(&staging_directory, failed);
        }
        if let Ok(mut current) = state().lock() {
            match result {
                Ok((records, bytes)) => {
                    crate::terminal_log(
                        "annotation",
                        format!("{run_id} completed: {records} variants, {bytes} bytes"),
                    );
                    current.state = "completed";
                    current.phase = "Completed";
                    current.detail = format!("Published {records} canonical variants");
                    current.records = Some(records);
                    current.output_bytes = bytes;
                    current.output = Some(final_directory);
                    current.cancel_requested = false;
                }
                Err(error) if error == "cancelled" => {
                    crate::terminal_log("annotation", format!("{run_id} cancelled"));
                    current.state = "cancelled";
                    current.phase = "Cancelled";
                    current.detail = "Annotation cancelled; partial result discarded".into();
                    current.cancel_requested = false;
                    current.error = None;
                }
                Err(error) => {
                    crate::terminal_log("annotation", format!("{run_id} failed: {error}"));
                    current.state = "failed";
                    current.phase = "Failed";
                    current.detail = "Annotation did not produce a publishable result".into();
                    current.cancel_requested = false;
                    current.error = Some(error);
                }
            }
        }
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
                    .ok_or("completed run has no output path".into());
            }
            "failed" => return Err(current.error.unwrap_or(current.detail)),
            "cancelled" => return Err("annotation was cancelled".into()),
            _ => {}
        }
    }
}

fn validate_request(request: &AnnotationRequest, resources: &Path) -> Result<(), String> {
    if !request.input.is_file() {
        return Err(format!("input VCF is missing: {}", request.input.display()));
    }
    validate_source_ids(&request.source_ids)?;
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

fn execute(
    request: &AnnotationRequest,
    resources: &Path,
    name: &str,
    run_id: &str,
    staging: &Path,
    final_directory: &Path,
) -> Result<(u64, u64), String> {
    fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    set_phase("Validating", "Validating the complete input VCF");
    let input_summary = annocat_core::vcf::inspect(&request.input)?;
    if input_summary.records == 0 {
        return fail_staging(staging, "input VCF contains no variant records".into());
    }
    if input_summary
        .assembly
        .as_deref()
        .is_some_and(|value| value != "GRCh38")
    {
        return fail_staging(
            staging,
            format!(
                "input declares {0}; AnnoCat currently requires GRCh38",
                input_summary.assembly.unwrap()
            ),
        );
    }
    if CANCEL.load(Ordering::SeqCst) {
        let _ = fs::remove_dir_all(staging);
        return Err("cancelled".into());
    }
    if let Ok(mut current) = state().lock() {
        current.records = Some(input_summary.records);
    }
    let readiness = super::fastvep::readiness();
    let executable = readiness
        .executable
        .ok_or("fastVEP executable disappeared")?;
    let output = staging.join("annotated.vcf");
    let structured_output = staging.join("fastvep.ndjson");
    let stdout =
        File::create(staging.join("fastvep.stdout.log")).map_err(|error| error.to_string())?;
    let stderr =
        File::create(staging.join("fastvep.stderr.log")).map_err(|error| error.to_string())?;
    let (provider_directory, source_bindings) =
        compose_provider_set(resources, run_id, &request.source_ids)?;
    set_phase("Annotating", "fastVEP is annotating variants");
    let mut command = Command::new(executable);
    command
        .arg("annotate")
        .arg("--input")
        .arg(&request.input)
        .arg("--output")
        .arg(&output)
        .arg("--output-format")
        .arg("vcf")
        .arg("--structured-output")
        .arg(&structured_output)
        .arg("--fasta")
        .arg(super::reference::fasta_path(resources))
        .arg("--transcript-cache")
        .arg(super::transcript::cache_path(resources))
        .args(["--symbol", "--hgvs", "--canonical", "--no-progress"]);
    if let Some(directory) = provider_directory.as_ref() {
        command.arg("--sa-dir").arg(directory);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn();
    // `Command` retains its configured stdio handles after spawning. Drop it now so
    // Windows can atomically rename the staging directory after the child exits.
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
    let status = loop {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&output);
            if let Some(directory) = provider_directory.as_ref() {
                let _ = fs::remove_dir_all(directory);
            }
            let _ = fs::remove_dir_all(staging);
            return Err("cancelled".into());
        }
        if let Ok(bytes) = fs::metadata(&output).map(|value| value.len())
            && let Ok(mut current) = state().lock()
        {
            current.output_bytes = bytes;
        }
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for fastVEP: {error}"))?
        {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(150)),
        }
    };
    if let Some(directory) = provider_directory.as_ref() {
        let _ = fs::remove_dir_all(directory);
    }
    if !status.success() {
        return fail_staging(
            staging,
            format!("fastVEP annotation exited with {status}; see fastvep.stderr.log"),
        );
    }
    set_phase(
        "Verifying",
        "Checking record counts and the dynamic CSQ schema",
    );
    let output_summary = super::csq::inspect(&output)
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
    if output_summary.records_without_csq > 0 {
        return fail_staging(
            staging,
            format!(
                "{} output records have no CSQ annotation",
                output_summary.records_without_csq
            ),
        );
    }
    set_phase("Indexing", "Building the typed, paged Parquet result");
    let parquet = staging.join("variants.parquet");
    let temporary_database = staging.join("result-build.duckdb");
    let canonical =
        match super::results::convert_vcf(&output, &parquet, &temporary_database, || {
            CANCEL.load(Ordering::SeqCst)
        }) {
            Ok(summary) => summary,
            Err(error) if error == "cancelled" => {
                let _ = fs::remove_dir_all(staging);
                return Err(error);
            }
            Err(error) => {
                return fail_staging(staging, format!("result conversion failed: {error}"));
            }
        };
    let consequences = staging.join("consequences.parquet");
    let evidence = staging.join("evidence.parquet");
    let field_catalog = staging.join("field-catalog.json");
    let structured_database = staging.join("structured-result-build.duckdb");
    let structured = match super::results::convert_structured(
        &structured_output,
        &consequences,
        &evidence,
        &field_catalog,
        &structured_database,
        || CANCEL.load(Ordering::SeqCst),
    ) {
        Ok(summary) => summary,
        Err(error) if error == "cancelled" => {
            let _ = fs::remove_dir_all(staging);
            return Err(error);
        }
        Err(error) => {
            return fail_staging(
                staging,
                format!("structured result conversion failed: {error}"),
            );
        }
    };
    if let Err(error) = fs::remove_file(&structured_output) {
        return fail_staging(
            staging,
            format!("cannot remove temporary structured output: {error}"),
        );
    }
    let output_bytes = fs::metadata(&parquet)
        .map_err(|error| error.to_string())?
        .len();
    let consequences_bytes = file_bytes(&consequences)?;
    let evidence_bytes = file_bytes(&evidence)?;
    let field_catalog_bytes = file_bytes(&field_catalog)?;
    let annotated_vcf = if request.include_annotated_vcf {
        Some((file_bytes(&output)?, super::fastvep::sha256_file(&output)?))
    } else {
        fs::remove_file(&output)
            .map_err(|error| format!("cannot remove temporary annotated VCF: {error}"))?;
        None
    };
    let canonical_result_bytes = output_bytes
        .checked_add(consequences_bytes)
        .and_then(|bytes| bytes.checked_add(evidence_bytes))
        .and_then(|bytes| bytes.checked_add(field_catalog_bytes))
        .ok_or("canonical result size overflow")?;
    let mut manifest = serde_json::json!({
        "schemaVersion": 1,
        "canonicalSchemaVersion": super::results::SCHEMA_VERSION,
        "fastvepStructuredFormat": "ndjson-v1",
        "state": "completed",
        "runId": run_id,
        "name": name,
        "completedAt": current_timestamp(),
        "assembly": "GRCh38",
        "variantCount": canonical.rows,
        "vcfRecordCount": output_summary.records,
        "alleleCount": output_summary.alternate_alleles,
        "csqEntryCount": output_summary.csq_entries,
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
        "input": request.input,
        "inputSha256": super::fastvep::sha256_file(&request.input)?,
        "resultSha256": super::fastvep::sha256_file(&parquet)?,
        "consequencesSha256": super::fastvep::sha256_file(&consequences)?,
        "evidenceSha256": super::fastvep::sha256_file(&evidence)?,
        "fieldCatalogSha256": super::fastvep::sha256_file(&field_catalog)?,
        "fastvepVersion": readiness.version,
        "fastvepSha256": readiness.sha256,
        "sourceIds": request.source_ids,
        "sources": source_bindings,
    });
    if let Some((bytes, sha256)) = annotated_vcf {
        let object = manifest
            .as_object_mut()
            .ok_or("completed manifest is not an object")?;
        object.insert("annotatedVcfFile".into(), "annotated.vcf".into());
        object.insert("annotatedVcfBytes".into(), bytes.into());
        object.insert("annotatedVcfSha256".into(), sha256.into());
    }
    fs::write(
        staging.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(staging, final_directory)
        .map_err(|error| format!("cannot publish completed run: {error}"))?;
    Ok((canonical.rows, output_bytes))
}

fn file_bytes(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("cannot measure {}: {error}", path.display()))
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

fn display_name(request: &AnnotationRequest) -> String {
    request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            request
                .input
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Annotation".into())
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
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop().ok_or_else(|| {
        format!("{source_id} is selected but has no complete verified fastSA provider")
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
            || checkpoint.identity.osa_schema_version != 1
        {
            return Err(format!(
                "{source_id} chromosome {} verification identity is invalid",
                entry.chromosome
            ));
        }
        let index = shard_directory.join("source.osa.idx");
        require_nonempty(&osa, source_id)?;
        require_nonempty(&index, source_id)?;
        let destination_relative = PathBuf::from(source_id)
            .join("shards")
            .join(format!("chr{}", entry.chromosome))
            .join("source.osa");
        let destination_osa = destination.join(&destination_relative);
        fs::create_dir_all(destination_osa.parent().expect("provider shard has parent"))
            .map_err(|error| format!("cannot create {source_id} provider directory: {error}"))?;
        fs::hard_link(&osa, &destination_osa)
            .map_err(|error| format!("cannot link verified {source_id} shard: {error}"))?;
        fs::hard_link(&index, destination_osa.with_extension("osa.idx"))
            .map_err(|error| format!("cannot link verified {source_id} index: {error}"))?;
        provider_shards.push(serde_json::json!({
            "chromosome": entry.chromosome,
            "file": destination_relative.to_string_lossy().replace('\\', "/"),
        }));
        match binding.as_mut() {
            Some(binding)
                if binding.release != checkpoint.identity.release
                    || binding.selected_schema != checkpoint.identity.selected_schema =>
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
    let osa = shard.join("source.osa");
    let index = shard.join("source.osa.idx");
    let checkpoint: super::preparation::PreparationCheckpoint = serde_json::from_slice(
        &fs::read(shard.join("verified.json"))
            .map_err(|_| format!("{source_id} provider is not verified"))?,
    )
    .map_err(|error| format!("invalid {source_id} verification: {error}"))?;
    if checkpoint.state != super::preparation::CheckpointState::Verified
        || checkpoint.identity.resource_id != source_id
        || checkpoint.identity.assembly != "GRCh38"
        || checkpoint.identity.chromosome != "all"
        || checkpoint.identity.osa_schema_version != 1
    {
        return Err(format!("{source_id} verification identity is invalid"));
    }
    require_nonempty(&osa, source_id)?;
    require_nonempty(&index, source_id)?;
    let destination_osa = destination.join(format!("{source_id}.osa"));
    fs::hard_link(&osa, &destination_osa)
        .map_err(|error| format!("cannot link verified {source_id} provider: {error}"))?;
    fs::hard_link(&index, destination.join(format!("{source_id}.osa.idx")))
        .map_err(|error| format!("cannot link verified {source_id} index: {error}"))?;
    Ok(SourceBinding {
        resource_id: source_id.into(),
        release: checkpoint.identity.release,
        assembly: checkpoint.identity.assembly,
        selected_schema: checkpoint.identity.selected_schema,
        chromosomes: vec!["all".into()],
    })
}

fn require_nonempty(path: &Path, source_id: &str) -> Result<(), String> {
    if path.is_file() && fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
        Ok(())
    } else {
        Err(format!(
            "verified {source_id} file is missing: {}",
            path.display()
        ))
    }
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
        fs::write(
            shard.join("verified.json"),
            serde_json::to_vec(&super::super::preparation::PreparationCheckpoint {
                schema_version: 1,
                identity: super::super::preparation::PreparationIdentity {
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
                },
                state: super::super::preparation::CheckpointState::Verified,
                compressed_bytes_read: 10,
                parsed_records: 1,
                prepared_bytes: 11,
                prepared_index_bytes: 13,
            })
            .unwrap(),
        )
        .unwrap();

        let (directory, bindings) =
            compose_provider_set(&root, "run-fixture", &["clinvar".into()]).unwrap();
        let directory = directory.unwrap();
        assert_eq!(bindings[0].release, "20260715");
        assert_eq!(bindings[0].chromosomes, ["all"]);
        assert!(directory.join("clinvar.osa").is_file());
        assert_eq!(
            fs::read(directory.join("clinvar.osa")).unwrap(),
            b"osa fixture"
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
}
