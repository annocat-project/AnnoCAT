use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const DBNSFP_CURATED_SCHEMA: &str = "dbnsfp-4.9a-annocat-core-v2";
pub(super) const DBNSFP_LEGACY_CURATED_SCHEMA: &str = "dbnsfp-4.9a-annocat-core-v1";
pub(super) const DBNSFP_FIELD_SELECTION_SCHEMA_VERSION: u16 = 1;
const DBNSFP_COORDINATE_FIELDS: &[&str] = &["chr", "pos(1-based)", "ref", "alt"];
const DBNSFP_V2_IDENTITY_FIELDS: &[&str] = &["Uniprot_entry", "MutPred_protID", "MutPred_AAchange"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbnsfpFieldSelection {
    pub schema_version: u16,
    pub contract_id: String,
    pub fields: Vec<String>,
}

pub(super) fn dbnsfp_contract() -> Result<serde_json::Value, String> {
    serde_json::from_str(include_str!(
        "../../../../config/dbnsfp-4.9a-curated-fields.json"
    ))
    .map_err(|error| format!("invalid bundled dbNSFP field contract: {error}"))
}

pub(super) fn dbnsfp_contract_fields(
    contract: &serde_json::Value,
) -> Result<(Vec<String>, Vec<String>), String> {
    let groups = contract["groups"]
        .as_array()
        .ok_or("dbNSFP field contract has no groups")?;
    let mut allowed = Vec::new();
    let mut required = Vec::new();
    for group in groups {
        let is_required = group["required"].as_bool().unwrap_or(false);
        for field in group["fields"]
            .as_array()
            .ok_or("dbNSFP field group has no fields")?
        {
            let field = field
                .as_str()
                .ok_or("dbNSFP field contract contains a non-string field")?;
            if DBNSFP_COORDINATE_FIELDS.contains(&field) {
                continue;
            }
            if !allowed.iter().any(|candidate| candidate == field) {
                allowed.push(field.to_owned());
            }
            if is_required && !required.iter().any(|candidate| candidate == field) {
                required.push(field.to_owned());
            }
        }
    }
    if allowed.is_empty() || required.is_empty() {
        return Err("dbNSFP field contract is unexpectedly empty".into());
    }
    Ok((allowed, required))
}

fn dbnsfp_selection_path(resource_root: &Path) -> PathBuf {
    resource_root.join("field-selection.json")
}

fn dbnsfp_selection_locked(resource_root: &Path) -> Result<bool, String> {
    field_selection_locked(resource_root)
}

pub(super) fn full_dbnsfp_field_selection() -> Result<DbnsfpFieldSelection, String> {
    let contract = dbnsfp_contract()?;
    let (fields, _) = dbnsfp_contract_fields(&contract)?;
    Ok(DbnsfpFieldSelection {
        schema_version: DBNSFP_FIELD_SELECTION_SCHEMA_VERSION,
        contract_id: DBNSFP_CURATED_SCHEMA.into(),
        fields,
    })
}

pub fn default_dbnsfp_field_selection() -> Result<DbnsfpFieldSelection, String> {
    let contract = dbnsfp_contract()?;
    let (allowed, required) = dbnsfp_contract_fields(&contract)?;
    let recommended = contract["recommendedFields"]
        .as_array()
        .ok_or("dbNSFP field contract has no recommended fields")?
        .iter()
        .map(|field| {
            field
                .as_str()
                .map(str::to_owned)
                .ok_or("dbNSFP recommended field is not a string")
        })
        .collect::<Result<HashSet<_>, _>>()?;
    if recommended.is_empty() || recommended.iter().any(|field| !allowed.contains(field)) {
        return Err("dbNSFP recommended fields contain an unknown field".into());
    }
    let required = required.into_iter().collect::<HashSet<_>>();
    let fields = allowed
        .into_iter()
        .filter(|field| required.contains(field) || recommended.contains(field))
        .collect();
    Ok(DbnsfpFieldSelection {
        schema_version: DBNSFP_FIELD_SELECTION_SCHEMA_VERSION,
        contract_id: DBNSFP_CURATED_SCHEMA.into(),
        fields,
    })
}

fn validate_dbnsfp_field_selection(
    mut selection: DbnsfpFieldSelection,
) -> Result<DbnsfpFieldSelection, String> {
    if selection.schema_version != DBNSFP_FIELD_SELECTION_SCHEMA_VERSION {
        return Err("dbNSFP field selection uses an unsupported contract".into());
    }
    let contract = dbnsfp_contract()?;
    let (mut allowed, mut required) = dbnsfp_contract_fields(&contract)?;
    if selection.contract_id == DBNSFP_LEGACY_CURATED_SCHEMA {
        allowed.retain(|field| !DBNSFP_V2_IDENTITY_FIELDS.contains(&field.as_str()));
        required.retain(|field| !DBNSFP_V2_IDENTITY_FIELDS.contains(&field.as_str()));
    } else if selection.contract_id != DBNSFP_CURATED_SCHEMA {
        return Err("dbNSFP field selection uses an unsupported contract".into());
    }
    if selection.fields.len() > allowed.len() {
        return Err("dbNSFP field selection contains too many fields".into());
    }
    let supplied = selection.fields.iter().collect::<HashSet<_>>();
    if supplied.len() != selection.fields.len()
        || selection
            .fields
            .iter()
            .any(|field| !allowed.contains(field))
    {
        return Err("dbNSFP field selection contains duplicate or unknown fields".into());
    }
    if let Some(missing) = required.iter().find(|field| !supplied.contains(field)) {
        return Err(format!(
            "required dbNSFP field '{missing}' cannot be removed"
        ));
    }
    // Store fields in the contract's stable order so equivalent UI selections
    // always produce the same cache identity.
    selection.fields = allowed
        .into_iter()
        .filter(|field| supplied.contains(field))
        .collect();
    Ok(selection)
}

fn migrate_legacy_dbnsfp_field_selection(
    selection: &DbnsfpFieldSelection,
) -> Result<DbnsfpFieldSelection, String> {
    let supplied = selection
        .fields
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let contract = dbnsfp_contract()?;
    let (allowed, required) = dbnsfp_contract_fields(&contract)?;
    let required = required.iter().map(String::as_str).collect::<HashSet<_>>();
    Ok(DbnsfpFieldSelection {
        schema_version: DBNSFP_FIELD_SELECTION_SCHEMA_VERSION,
        contract_id: DBNSFP_CURATED_SCHEMA.into(),
        fields: allowed
            .into_iter()
            .filter(|field| supplied.contains(field.as_str()) || required.contains(field.as_str()))
            .collect(),
    })
}

fn write_dbnsfp_field_selection(
    resource_root: &Path,
    selection: &DbnsfpFieldSelection,
) -> Result<(), String> {
    fs::create_dir_all(resource_root)
        .map_err(|error| format!("cannot create dbNSFP resource directory: {error}"))?;
    let path = dbnsfp_selection_path(resource_root);
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(selection)
            .map_err(|error| format!("cannot encode dbNSFP field selection: {error}"))?,
    )
    .map_err(|error| format!("cannot write dbNSFP field selection: {error}"))?;
    if path.is_file() {
        fs::remove_file(&path)
            .map_err(|error| format!("cannot replace dbNSFP field selection: {error}"))?;
    }
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot publish dbNSFP field selection: {error}"))
}

pub fn load_dbnsfp_field_selection(resource_root: &Path) -> Result<DbnsfpFieldSelection, String> {
    let path = dbnsfp_selection_path(resource_root);
    if !path.is_file() {
        return if dbnsfp_selection_locked(resource_root)? {
            // Before field configuration existed, dbNSFP retained the full
            // curated contract. Preserve that identity for existing shards.
            full_dbnsfp_field_selection()
        } else {
            default_dbnsfp_field_selection()
        };
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read dbNSFP field selection: {error}"))?;
    let selection = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid dbNSFP field selection: {error}"))?;
    let selection = validate_dbnsfp_field_selection(selection)?;
    if selection.contract_id == DBNSFP_LEGACY_CURATED_SCHEMA
        && !dbnsfp_selection_locked(resource_root)?
    {
        let migrated = migrate_legacy_dbnsfp_field_selection(&selection)?;
        discard_field_staging(resource_root, "dbNSFP")?;
        write_dbnsfp_field_selection(resource_root, &migrated)?;
        return Ok(migrated);
    }
    Ok(selection)
}

pub fn save_dbnsfp_field_selection(
    resource_root: &Path,
    selection: DbnsfpFieldSelection,
) -> Result<DbnsfpFieldSelection, String> {
    let selection = validate_dbnsfp_field_selection(selection)?;
    if selection.contract_id != DBNSFP_CURATED_SCHEMA {
        return Err("dbNSFP field selection must use the current contract".into());
    }
    let current = load_dbnsfp_field_selection(resource_root)?;
    let path = dbnsfp_selection_path(resource_root);
    let locked = dbnsfp_selection_locked(resource_root)?;
    if current == selection && (path.is_file() || locked) {
        return Ok(selection);
    }
    if locked {
        return Err("remove the installed dbNSFP cache before changing retained fields".into());
    }
    discard_field_staging(resource_root, "dbNSFP")?;
    write_dbnsfp_field_selection(resource_root, &selection)?;
    Ok(selection)
}

pub fn dbnsfp_field_configuration(
    resource_root: &Path,
) -> Result<FieldConfiguration<DbnsfpFieldSelection>, String> {
    Ok(FieldConfiguration {
        contract: dbnsfp_contract()?,
        selection: load_dbnsfp_field_selection(resource_root)?,
        locked: dbnsfp_selection_locked(resource_root)?,
    })
}

pub(super) fn dbnsfp_schema_identity(selection: &DbnsfpFieldSelection) -> String {
    let full_fields = if selection.contract_id == DBNSFP_CURATED_SCHEMA {
        full_dbnsfp_field_selection().map(|full| full.fields)
    } else if selection.contract_id == DBNSFP_LEGACY_CURATED_SCHEMA {
        dbnsfp_contract().and_then(|contract| {
            dbnsfp_contract_fields(&contract).map(|(mut allowed, _)| {
                allowed.retain(|field| !DBNSFP_V2_IDENTITY_FIELDS.contains(&field.as_str()));
                allowed
            })
        })
    } else {
        Err("unsupported dbNSFP field selection contract".into())
    };
    if full_fields.is_ok_and(|fields| fields == selection.fields) {
        // Preserve compatibility with chromosomes built before the field
        // selector existed; that release already retained this exact set.
        return selection.contract_id.clone();
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for field in &selection.fields {
        hasher.update(field.as_bytes());
        hasher.update([0]);
    }
    let digest = format!("{:x}", hasher.finalize());
    format!("{}:{}", selection.contract_id, &digest[..16])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupplementaryFieldSelection {
    pub schema_version: u16,
    pub contract_id: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConfiguration<T> {
    pub contract: serde_json::Value,
    pub selection: T,
    pub locked: bool,
}

fn supplementary_field_catalog() -> Result<serde_json::Value, String> {
    serde_json::from_str(include_str!(
        "../../../../config/supplementary-source-fields.json"
    ))
    .map_err(|error| format!("invalid bundled supplementary field catalog: {error}"))
}

pub(super) fn supplementary_field_contract(resource_id: &str) -> Result<serde_json::Value, String> {
    let catalog = supplementary_field_catalog()?;
    if catalog["schemaVersion"] != 1 {
        return Err("unsupported supplementary field catalog version".into());
    }
    catalog["sources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| source["resourceId"] == resource_id)
        })
        .cloned()
        .ok_or_else(|| format!("resource '{resource_id}' has no configurable field contract"))
}

type SupplementaryContractFields = (Vec<String>, Vec<String>, Vec<String>);

fn supplementary_contract_fields(
    contract: &serde_json::Value,
) -> Result<SupplementaryContractFields, String> {
    let mut allowed = Vec::new();
    let mut defaults = Vec::new();
    let mut required = Vec::new();
    for group in contract["groups"]
        .as_array()
        .ok_or("supplementary field contract has no groups")?
    {
        let is_default = group["default"].as_bool().unwrap_or(false);
        let is_required = group["required"].as_bool().unwrap_or(false);
        for field in group["fields"]
            .as_array()
            .ok_or("supplementary field group has no fields")?
        {
            let field = field
                .as_str()
                .ok_or("supplementary field contract contains a non-string field")?;
            if allowed.iter().any(|candidate| candidate == field) {
                return Err(format!("supplementary field '{field}' is duplicated"));
            }
            allowed.push(field.to_owned());
            if is_default || is_required {
                defaults.push(field.to_owned());
            }
            if is_required {
                required.push(field.to_owned());
            }
        }
    }
    if allowed.is_empty() || defaults.is_empty() {
        return Err("supplementary field contract is unexpectedly empty".into());
    }
    Ok((allowed, defaults, required))
}

pub fn default_supplementary_field_selection(
    resource_id: &str,
) -> Result<SupplementaryFieldSelection, String> {
    let contract = supplementary_field_contract(resource_id)?;
    let (allowed, defaults, _) = supplementary_contract_fields(&contract)?;
    let fields = if matches!(resource_id, "gnomad" | "gnomad-genomes") {
        allowed
    } else {
        defaults
    };
    Ok(SupplementaryFieldSelection {
        schema_version: 1,
        contract_id: contract["contractId"]
            .as_str()
            .ok_or("supplementary contract has no ID")?
            .into(),
        fields,
    })
}

fn contract_default_supplementary_fields(resource_id: &str) -> Result<Vec<String>, String> {
    let contract = supplementary_field_contract(resource_id)?;
    let (_, defaults, _) = supplementary_contract_fields(&contract)?;
    Ok(defaults)
}

fn validate_supplementary_field_selection(
    resource_id: &str,
    mut selection: SupplementaryFieldSelection,
) -> Result<SupplementaryFieldSelection, String> {
    let contract = supplementary_field_contract(resource_id)?;
    let contract_id = contract["contractId"]
        .as_str()
        .ok_or("supplementary contract has no ID")?;
    let (allowed, _, required) = supplementary_contract_fields(&contract)?;
    if selection.schema_version != 1 || selection.contract_id != contract_id {
        return Err(format!(
            "{resource_id} field selection uses an unsupported contract"
        ));
    }
    let supplied = selection.fields.iter().collect::<HashSet<_>>();
    if supplied.is_empty()
        || supplied.len() != selection.fields.len()
        || selection
            .fields
            .iter()
            .any(|field| !allowed.contains(field))
    {
        return Err(format!(
            "{resource_id} field selection is empty or contains duplicate or unsupported fields"
        ));
    }
    if let Some(missing) = required.iter().find(|field| !supplied.contains(field)) {
        return Err(format!(
            "required {resource_id} field '{missing}' cannot be removed"
        ));
    }
    selection.fields = allowed
        .into_iter()
        .filter(|field| supplied.contains(field))
        .collect();
    Ok(selection)
}

pub fn load_supplementary_field_selection(
    resource_id: &str,
    resource_root: &Path,
) -> Result<SupplementaryFieldSelection, String> {
    let path = resource_root.join("field-selection.json");
    if !path.is_file() {
        if field_selection_locked(resource_root)? {
            let mut inferred = default_supplementary_field_selection(resource_id)?;
            inferred.fields = contract_default_supplementary_fields(resource_id)?;
            return Ok(inferred);
        }
        return default_supplementary_field_selection(resource_id);
    }
    let selection = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("cannot read {resource_id} fields: {error}"))?,
    )
    .map_err(|error| format!("invalid {resource_id} field selection: {error}"))?;
    validate_supplementary_field_selection(resource_id, selection)
}

fn directory_has_entries(directory: &Path) -> Result<bool, String> {
    if !directory.is_dir() {
        return Ok(false);
    }
    Ok(fs::read_dir(directory)
        .map_err(|error| format!("cannot inspect prepared resource data: {error}"))?
        .next()
        .is_some())
}

fn field_selection_locked(resource_root: &Path) -> Result<bool, String> {
    // Only promoted shards lock a schema. An interrupted staging directory is
    // not an installed cache and may be invalidated by an explicit field edit.
    if directory_has_entries(&resource_root.join("shards"))? {
        return Ok(true);
    }
    let Ok(entries) = fs::read_dir(resource_root) else {
        return Ok(false);
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect resource versions: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("cannot inspect resource version: {error}"))?
            .is_dir()
            && directory_has_entries(&entry.path().join("shards"))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn discard_field_staging(resource_root: &Path, resource_id: &str) -> Result<(), String> {
    for staging in std::iter::once(resource_root.join("staging")).chain(
        fs::read_dir(resource_root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| entry)
            })
            .map(|entry| entry.path().join("staging")),
    ) {
        if staging.is_dir() {
            fs::remove_dir_all(&staging).map_err(|error| {
                format!(
                    "cannot discard incomplete {resource_id} cache after changing fields: {error}"
                )
            })?;
        }
    }
    Ok(())
}

pub fn save_supplementary_field_selection(
    resource_id: &str,
    resource_root: &Path,
    selection: SupplementaryFieldSelection,
) -> Result<SupplementaryFieldSelection, String> {
    let selection = validate_supplementary_field_selection(resource_id, selection)?;
    let current = load_supplementary_field_selection(resource_id, resource_root)?;
    let path = resource_root.join("field-selection.json");
    let locked = field_selection_locked(resource_root)?;
    if current == selection && (path.is_file() || locked) {
        return Ok(selection);
    }
    if locked {
        return Err(format!(
            "remove the installed {resource_id} cache before changing retained fields"
        ));
    }
    // A different field contract cannot safely resume an incomplete shard.
    // Staging contains only disposable partial cache output, never source data.
    discard_field_staging(resource_root, resource_id)?;
    fs::create_dir_all(resource_root)
        .map_err(|error| format!("cannot create {resource_id} resource directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&selection)
            .map_err(|error| format!("cannot encode {resource_id} fields: {error}"))?,
    )
    .map_err(|error| format!("cannot write {resource_id} fields: {error}"))?;
    if path.is_file() {
        fs::remove_file(&path)
            .map_err(|error| format!("cannot replace {resource_id} fields: {error}"))?;
    }
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot publish {resource_id} fields: {error}"))?;
    Ok(selection)
}

pub fn supplementary_field_configuration(
    resource_id: &str,
    resource_root: &Path,
) -> Result<FieldConfiguration<SupplementaryFieldSelection>, String> {
    Ok(FieldConfiguration {
        contract: supplementary_field_contract(resource_id)?,
        selection: load_supplementary_field_selection(resource_id, resource_root)?,
        locked: field_selection_locked(resource_root)?,
    })
}

pub fn supplementary_schema_identity(
    base: &str,
    resource_id: &str,
    selection: &SupplementaryFieldSelection,
) -> Result<String, String> {
    // The unqualified schema identity historically represented the field
    // groups marked default in the contract. Keep that identity stable even
    // when the UI chooses a broader default for a new installation.
    if contract_default_supplementary_fields(resource_id)? == selection.fields {
        return Ok(base.into());
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for field in &selection.fields {
        hasher.update(field.as_bytes());
        hasher.update([0]);
    }
    let digest = format!("{:x}", hasher.finalize());
    Ok(format!("{base}:{}", &digest[..16]))
}
