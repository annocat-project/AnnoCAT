use serde::Serialize;
use std::path::{Path, PathBuf};

pub const RUN_ID: &str = "demo-giab-hg002-v1";
const DIRECTORY_NAME: &str = ".annocat-demo-v1";
const FILES: &[(&str, &[u8])] = &[
    (
        "variants.parquet",
        include_bytes!("../assets/demo/variants.parquet"),
    ),
    (
        "consequences.parquet",
        include_bytes!("../assets/demo/consequences.parquet"),
    ),
    (
        "evidence.parquet",
        include_bytes!("../assets/demo/evidence.parquet"),
    ),
    (
        "field-catalog.json",
        include_bytes!("../assets/demo/field-catalog.json"),
    ),
    (
        "manifest.json",
        include_bytes!("../assets/demo/manifest.json"),
    ),
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoRun {
    pub id: &'static str,
    pub name: &'static str,
    pub assembly: &'static str,
    pub variant_count: u64,
    pub demo: bool,
    pub read_only: bool,
}

#[derive(Serialize)]
pub struct DemoStatus {
    pub run: DemoRun,
}

pub fn ensure(runs_directory: &Path) -> Result<DemoStatus, String> {
    std::fs::create_dir_all(runs_directory)
        .map_err(|error| format!("cannot create the results directory: {error}"))?;
    let directory = runs_directory.join(DIRECTORY_NAME);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create the demo result: {error}"))?;

    // Write the manifest last so normal result discovery never sees a partial demo.
    for (name, bytes) in FILES.iter().filter(|(name, _)| *name != "manifest.json") {
        write_if_changed(&directory.join(name), bytes)?;
    }
    let manifest = FILES
        .iter()
        .find(|(name, _)| *name == "manifest.json")
        .expect("embedded demo manifest");
    write_if_changed(&directory.join(manifest.0), manifest.1)?;

    Ok(DemoStatus {
        run: DemoRun {
            id: RUN_ID,
            name: "GIAB HG002 demo",
            assembly: "GRCh38",
            variant_count: 200,
            demo: true,
            read_only: false,
        },
    })
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let temporary = temporary_path(path);
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write the demo result: {error}"))?;
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| format!("cannot replace the demo result: {error}"))?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot publish the demo result: {error}"))
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("demo asset name").to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::{Connection, params};

    #[test]
    fn materializes_a_hidden_interactive_result() {
        let root = std::env::temp_dir().join(format!("annocat-demo-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let status = ensure(&root).unwrap();
        assert!(!status.run.read_only);
        assert_eq!(status.run.variant_count, 200);
        let directory = root.join(DIRECTORY_NAME);
        assert!(directory.join("variants.parquet").is_file());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["runId"], RUN_ID);
        assert_eq!(manifest["reportKind"], "demo");

        let connection = Connection::open_in_memory().unwrap();
        let variants = directory.join("variants.parquet");
        let high_impact: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?) WHERE impact = 'HIGH'",
                params![variants.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(high_impact > 0);
        let represented_impacts: i64 = connection
            .query_row(
                "SELECT count(DISTINCT impact) FROM read_parquet(?) \
                 WHERE impact IN ('HIGH', 'MODERATE', 'LOW', 'MODIFIER')",
                params![variants.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(represented_impacts, 4);

        let evidence = directory.join("evidence.parquet");
        let pathogenic: i64 = connection
            .query_row(
                "SELECT count(*) FROM read_parquet(?) \
                 WHERE source_id = 'clinvar' \
                   AND field_path = 'significance' \
                   AND contains(lower(coalesce(string_value, json_value, '')), 'pathogenic')",
                params![evidence.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(pathogenic > 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
