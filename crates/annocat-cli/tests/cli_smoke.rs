use std::process::Command;

fn annocat() -> Command {
    Command::new(env!("CARGO_BIN_EXE_annocat"))
}

fn run(args: &[&str]) -> std::process::Output {
    annocat().args(args).output().expect("run annocat command")
}

fn test_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("annocat-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    home
}

#[test]
fn public_command_tree_matches_the_documented_workflow() {
    let help = run(&["help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for command in [
        "annotate",
        "launch",
        "status",
        "results",
        "sources",
        "tasks",
        "diagnostics",
    ] {
        assert!(help.contains(command), "help omitted {command}");
    }
    for removed in [
        "doctor",
        "fastvep status",
        "export-result",
        "validate-result",
        "inspect-vcf",
        "serve",
        "interactive",
        "report-worker",
    ] {
        assert!(!help.contains(removed), "help exposed {removed}");
    }

    for flag in ["-V", "--version"] {
        let version = run(&[flag]);
        assert!(version.status.success());
        assert!(String::from_utf8_lossy(&version.stdout).starts_with("annocat "));
    }
}

#[test]
fn read_only_commands_use_the_new_namespaces() {
    let home = test_home("read-only");
    let home = home.to_string_lossy();

    let status = run(&["--home", &home, "status", "--json"]);
    assert!(matches!(status.status.code(), Some(0 | 1)));
    serde_json::from_slice::<serde_json::Value>(&status.stdout).expect("status JSON");

    let sources = run(&["--home", &home, "sources", "list", "--json"]);
    assert!(sources.status.success());
    let sources =
        serde_json::from_slice::<serde_json::Value>(&sources.stdout).expect("source JSON");
    assert!(
        sources
            .as_array()
            .is_some_and(|rows| { rows.iter().any(|row| row["id"].as_str() == Some("clinvar")) })
    );

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/preparation/tiny-custom.vcf");
    let inspected = annocat()
        .args(["diagnostics", "vcf"])
        .arg(fixture)
        .output()
        .expect("inspect fixture VCF");
    assert!(inspected.status.success());
    assert!(String::from_utf8_lossy(&inspected.stdout).contains("Records"));
}

#[test]
fn human_status_explains_a_missing_engine() {
    let home = test_home("missing-engine");
    std::fs::create_dir_all(&home).unwrap();
    let output = annocat()
        .current_dir(&home)
        .args(["--home"])
        .arg(&home)
        .arg("status")
        .output()
        .expect("run status without fastVEP");
    assert_eq!(output.status.code(), Some(1));
    let status = String::from_utf8_lossy(&output.stdout);
    assert!(status.contains("Not ready (missing)"));
    assert!(status.contains("Blocker"));
    assert!(status.contains("Expected at"));
}

#[test]
fn usage_errors_return_clap_exit_status() {
    assert_eq!(run(&["not-a-command"]).status.code(), Some(2));
    assert_eq!(run(&["launch", "--port", "0"]).status.code(), Some(2));
    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(run(&["version"]).status.code(), Some(2));
    assert_eq!(
        run(&["annotate", "-i", "sample.vcf"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&[
            "annotate",
            "-i",
            "sample.vcf",
            "--profile",
            "standard",
            "--core-only",
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        run(&["sources", "install", "clinvar", "--dry-run", "--yes"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn every_public_command_has_help() {
    for args in [
        vec!["annotate", "--help"],
        vec!["launch", "--help"],
        vec!["status", "--help"],
        vec!["results", "list", "--help"],
        vec!["results", "show", "--help"],
        vec!["results", "export", "--help"],
        vec!["results", "import", "--help"],
        vec!["results", "validate", "--help"],
        vec!["sources", "list", "--help"],
        vec!["sources", "status", "--help"],
        vec!["sources", "fields", "--help"],
        vec!["sources", "install", "--help"],
        vec!["sources", "remove", "--help"],
        vec!["tasks", "list", "--help"],
        vec!["tasks", "show", "--help"],
        vec!["tasks", "resume", "--help"],
        vec!["tasks", "cancel", "--help"],
        vec!["diagnostics", "vcf", "--help"],
        vec!["diagnostics", "fastvep", "--help"],
        vec!["diagnostics", "normalization", "--help"],
    ] {
        let output = run(&args);
        assert!(
            output.status.success(),
            "{} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn global_home_and_source_contract_outputs_are_discoverable() {
    let home = test_home("source-contracts");
    let home = home.to_string_lossy();

    let sources = run(&["sources", "--home", &home, "list", "--json"]);
    assert!(sources.status.success());
    serde_json::from_slice::<serde_json::Value>(&sources.stdout).expect("source list JSON");

    let fields = run(&["--home", &home, "sources", "fields", "dbnsfp", "--json"]);
    assert!(fields.status.success());
    let fields =
        serde_json::from_slice::<serde_json::Value>(&fields.stdout).expect("field catalog JSON");
    let first = &fields["fieldCatalog"][0];
    for key in [
        "id",
        "displayName",
        "description",
        "valueType",
        "rawName",
        "required",
        "recommended",
        "selected",
    ] {
        assert!(!first[key].is_null(), "field catalog omitted {key}");
    }

    let core = run(&[
        "--home",
        &home,
        "sources",
        "install",
        "--profile",
        "core",
        "--dry-run",
        "--json",
    ]);
    assert!(core.status.success());
    let core =
        serde_json::from_slice::<serde_json::Value>(&core.stdout).expect("install preview JSON");
    assert_eq!(core["onlineServices"][0], "favor-variant-annotation");
    assert!(core["availableDiskBytes"].as_u64().is_some());
}

#[test]
fn public_options_have_help_text() {
    let annotate = run(&["annotate", "--help"]);
    let annotate = String::from_utf8_lossy(&annotate.stdout);
    assert!(annotate.contains("Print one machine-readable JSON document"));
    assert!(annotate.contains("possible values: standard, comprehensive, core"));
    assert!(annotate.contains("does not convert the assembly"));
    assert!(annotate.contains("Example:"));

    let install = run(&["sources", "install", "--help"]);
    let install = String::from_utf8_lossy(&install.stdout);
    for description in [
        "One or more annotation source IDs",
        "recommended fields or all available fields",
        "leave files unchanged",
        "Skip the confirmation prompt",
        "Example:",
    ] {
        assert!(
            install.contains(description),
            "install help omitted {description}"
        );
    }

    let validate = run(&["results", "validate", "--help"]);
    assert!(String::from_utf8_lossy(&validate.stdout).contains("Do not change the result"));

    let normalization = run(&["diagnostics", "normalization", "--help"]);
    let normalization = String::from_utf8_lossy(&normalization.stdout);
    assert!(normalization.contains("alternate alleles"));
    assert!(normalization.contains("Example:"));
}

#[test]
fn source_status_requires_one_source_id() {
    assert_eq!(
        run(&["sources", "status"]).status.code(),
        Some(2),
        "source status without an ID must be a usage error"
    );
}
