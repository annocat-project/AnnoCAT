use std::io::Write;
use std::process::{Command, Stdio};

fn annocat() -> Command {
    Command::new(env!("CARGO_BIN_EXE_annocat"))
}

fn run(args: &[&str]) -> std::process::Output {
    annocat().args(args).output().expect("run annocat command")
}

#[test]
fn public_read_only_commands_are_wired() {
    let help = run(&["help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for command in [
        "annotate",
        "share-report",
        "validate-report",
        "doctor",
        "fastvep",
        "sources",
        "inspect-vcf",
        "inspect-fastvep",
        "check-normalization",
        "resources",
        "launch",
        "serve",
        "interactive",
        "version",
    ] {
        assert!(help.contains(command), "help omitted {command}");
    }

    let version = run(&["version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("annocat "));

    let doctor = run(&["doctor", "--json"]);
    assert!(doctor.status.success());
    serde_json::from_slice::<serde_json::Value>(&doctor.stdout).expect("doctor JSON");

    let fastvep = run(&["fastvep", "status", "--json"]);
    assert!(fastvep.status.success());
    serde_json::from_slice::<serde_json::Value>(&fastvep.stdout).expect("fastVEP JSON");

    let sources = run(&["sources"]);
    assert!(sources.status.success());
    assert!(String::from_utf8_lossy(&sources.stdout).contains("clinvar"));

    let plan = run(&["resources", "plan", "comprehensive"]);
    assert!(plan.status.success());
    let plan = String::from_utf8_lossy(&plan.stdout);
    assert!(plan.contains("Comprehensive annotation resource plan"));
    assert!(plan.contains("dbsnp"));
    assert!(plan.contains("gnomad"));

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/preparation/tiny-custom.vcf");
    let inspected = annocat()
        .arg("inspect-vcf")
        .arg(fixture)
        .output()
        .expect("inspect fixture VCF");
    assert!(inspected.status.success());
    assert!(String::from_utf8_lossy(&inspected.stdout).contains("Records"));
}

#[test]
fn interactive_menu_and_command_errors_are_wired() {
    let mut child = annocat()
        .arg("interactive")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start interactive command");
    child
        .stdin
        .take()
        .expect("interactive stdin")
        .write_all(b"4\n")
        .expect("exit interactive command");
    let output = child
        .wait_with_output()
        .expect("wait for interactive command");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Start browser UI"));

    let unknown = run(&["not-a-command"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown command"));

    let bad_port = run(&["serve", "--port", "0"]);
    assert!(!bad_port.status.success());
    assert!(String::from_utf8_lossy(&bad_port.stderr).contains("1 to 65535"));
}
