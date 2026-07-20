#![cfg(windows)]

use sha2::{Digest, Sha256};
use std::io::Write;
use std::process::Command;
use zip::write::SimpleFileOptions;

#[test]
fn packaged_report_worker_validates_inside_appcontainer() {
    let root = std::env::temp_dir().join(format!(
        "annocat-appcontainer-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let archive_path = root.join("valid-report.zip");
    let files = [
        ("variants.parquet", "variants", b"PAR1variants".as_slice()),
        (
            "consequences.parquet",
            "consequences",
            b"PAR1consequences".as_slice(),
        ),
        ("evidence.parquet", "evidence", b"PAR1evidence".as_slice()),
        (
            "field-catalog.json",
            "field-catalog",
            br#"{"schemaVersion":1,"fields":[]}"#.as_slice(),
        ),
    ];
    let declarations = files
        .iter()
        .map(|(path, role, bytes)| {
            serde_json::json!({
                "path": path,
                "role": role,
                "bytes": bytes.len(),
                "sha256": format!("{:x}", Sha256::digest(bytes))
            })
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "packageFormat": "annocat-report",
        "packageVersion": 1,
        "schemaVersion": 1,
        "runId": "appcontainer-fixture",
        "displayName": "AppContainer fixture",
        "completedAt": "2026-07-16T00:00:00Z",
        "assembly": "GRCh38",
        "variantCount": 1,
        "files": declarations
    }))
    .unwrap();
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive
        .start_file("annocat-manifest.json", options)
        .unwrap();
    archive.write_all(&manifest).unwrap();
    for (name, _, bytes) in files {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_annocat"))
        .arg("validate-report")
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("Valid AnnoCAT report: appcontainer-fixture (schema 1, 5 files")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn report_worker_refuses_direct_execution() {
    let output = Command::new(env!("CARGO_BIN_EXE_annocat-report-worker"))
        .args(["validate-handle", "123"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("refused to parse untrusted data outside AppContainer")
    );
}
