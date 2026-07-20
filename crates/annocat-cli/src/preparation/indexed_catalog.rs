use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaddArtifact {
    pub id: String,
    pub data_url: String,
    pub data_bytes: u64,
    pub data_etag: String,
    pub data_last_modified: String,
    pub data_md5: String,
    pub index_url: String,
    pub index_bytes: u64,
    pub index_etag: String,
    pub index_last_modified: String,
    pub index_md5: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpliceAiArtifact {
    pub data_url: String,
    pub data_bytes: u64,
    pub data_etag: String,
    pub data_last_modified: String,
    pub index_url: String,
    pub index_bytes: u64,
    pub index_etag: String,
    pub index_last_modified: String,
    pub index_md5: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexedCatalog {
    schema_version: u32,
    cadd: Vec<CaddArtifact>,
    spliceai: SpliceAiArtifact,
}

static CATALOG: OnceLock<Result<IndexedCatalog, String>> = OnceLock::new();

fn load() -> Result<&'static IndexedCatalog, String> {
    CATALOG
        .get_or_init(|| {
            let catalog: IndexedCatalog = serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../config/indexed-sources.json"
            )))
            .map_err(|error| format!("invalid indexed source catalog: {error}"))?;
            validate(&catalog)?;
            Ok(catalog)
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub fn cadd_artifacts() -> Result<[CaddArtifact; 2], String> {
    let catalog = load()?;
    Ok([catalog.cadd[0].clone(), catalog.cadd[1].clone()])
}

pub fn spliceai_artifact() -> Result<SpliceAiArtifact, String> {
    Ok(load()?.spliceai.clone())
}

fn validate(catalog: &IndexedCatalog) -> Result<(), String> {
    if catalog.schema_version != 1 {
        return Err(format!(
            "unsupported indexed source catalog schema {}",
            catalog.schema_version
        ));
    }
    if catalog.cadd.len() != 2 || catalog.cadd[0].id != "snv" || catalog.cadd[1].id != "indel" {
        return Err("indexed source catalog must contain CADD snv and indel artifacts".into());
    }
    for artifact in &catalog.cadd {
        validate_url(&artifact.data_url, &format!("CADD {} data", artifact.id))?;
        validate_url(&artifact.index_url, &format!("CADD {} index", artifact.id))?;
        validate_size(artifact.data_bytes, &format!("CADD {} data", artifact.id))?;
        validate_size(artifact.index_bytes, &format!("CADD {} index", artifact.id))?;
        validate_md5(&artifact.data_md5, &format!("CADD {} data", artifact.id))?;
        validate_md5(&artifact.index_md5, &format!("CADD {} index", artifact.id))?;
    }
    validate_url(&catalog.spliceai.data_url, "SpliceAI data")?;
    validate_url(&catalog.spliceai.index_url, "SpliceAI index")?;
    validate_size(catalog.spliceai.data_bytes, "SpliceAI data")?;
    validate_size(catalog.spliceai.index_bytes, "SpliceAI index")?;
    validate_md5(&catalog.spliceai.index_md5, "SpliceAI index")?;
    Ok(())
}

fn validate_url(url: &str, label: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("{label} URL must use HTTPS"));
    }
    Ok(())
}

fn validate_size(bytes: u64, label: &str) -> Result<(), String> {
    if bytes == 0 {
        return Err(format!("{label} size must be positive"));
    }
    Ok(())
}

fn validate_md5(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} MD5 is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_strict_and_complete() {
        let cadd = cadd_artifacts().unwrap();
        assert_eq!(cadd[0].id, "snv");
        assert_eq!(cadd[1].id, "indel");
        assert!(spliceai_artifact().unwrap().data_bytes > 0);
    }
}
