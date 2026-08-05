use duckdb::{Connection, params};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const CACHE_PREFIX: &str = ".annocat-evidence-";
const CACHE_VERSION: i32 = 1;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contract {
    groups: Vec<ContractGroup>,
    legacy_transcript_alignment: ContractAlignment,
    record_resolution: RecordResolutionContract,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractGroup {
    id: String,
    fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractAlignment {
    id: String,
    kind: String,
    source_id: String,
    scope: String,
    source_transcript_release: String,
    key_field: String,
    protein_field: String,
    canonical_field: String,
    separator: String,
    missing_values: Vec<String>,
    aligned_groups: Vec<String>,
    aligned_fields: Vec<String>,
    excluded_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordResolutionContract {
    id: String,
    source_id: String,
    raw_field_path: String,
    transcript_ids_unversioned: bool,
    stable_transcript_match_requires_peptide_compatibility: bool,
    missing_values: Vec<String>,
    record_identity: RecordIdentityContract,
    field_groups: Vec<RecordFieldGroup>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordIdentityContract {
    transcript_field: String,
    protein_field: String,
    uniprot_accession_field: String,
    uniprot_entry_field: String,
    reference_amino_acid_field: String,
    alternate_amino_acid_field: String,
    amino_acid_position_field: String,
    hgvsp_field: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum RecordCardinality {
    AlleleScalar,
    RecordScalar,
    AlignedVector,
    OpaqueList,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordFieldGroup {
    cardinality: RecordCardinality,
    #[serde(default)]
    identity_field: Option<String>,
    #[serde(default)]
    identity_separator: Option<String>,
    #[serde(default)]
    value_separator: Option<String>,
    fields: Vec<String>,
}

#[derive(Clone, Debug)]
struct AlignmentSpec {
    id: String,
    kind: String,
    source_id: String,
    scope: String,
    source_transcript_release: String,
    key_field: String,
    protein_field: String,
    canonical_field: String,
    separator: String,
    missing_values: Vec<String>,
    fields: BTreeSet<String>,
}

#[derive(Clone, Debug)]
struct RecordFieldSpec {
    cardinality: RecordCardinality,
    identity_field: Option<String>,
    identity_separator: Option<String>,
    value_separator: Option<String>,
}

#[derive(Clone, Debug)]
struct RecordResolutionSpec {
    id: String,
    source_id: String,
    raw_field_path: String,
    transcript_ids_unversioned: bool,
    stable_transcript_match_requires_peptide_compatibility: bool,
    missing_values: Vec<String>,
    identity: RecordIdentityContract,
    fields: BTreeMap<String, RecordFieldSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedRecordScope {
    Allele,
    Selected,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedRecordField {
    pub field_path: String,
    pub value: Value,
    pub scope: ResolvedRecordScope,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedRecordList {
    pub raw_field_path: String,
    pub raw_value: Value,
    pub fields: Vec<ResolvedRecordField>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestedResolutionKind {
    AlignedTranscriptVector,
    LegacyAllele,
    SelectedFeature,
}

#[derive(Clone, Debug)]
pub(crate) struct RequestedField {
    pub scope: String,
    pub biological_scope: String,
    pub source_id: String,
    pub field_path: String,
    pub kind: RequestedResolutionKind,
}

static BUNDLED_SPEC: OnceLock<Result<AlignmentSpec, String>> = OnceLock::new();
static BUNDLED_RECORD_SPEC: OnceLock<Result<RecordResolutionSpec, String>> = OnceLock::new();
static BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bundled_spec() -> Result<AlignmentSpec, String> {
    BUNDLED_SPEC
        .get_or_init(|| {
            let contract: Contract = serde_json::from_str(include_str!(
                "../../../config/dbnsfp-4.9a-curated-fields.json"
            ))
            .map_err(|error| format!("invalid bundled evidence alignment contract: {error}"))?;
            let alignment = contract.legacy_transcript_alignment;
            if alignment.kind != "parallelTranscriptVector"
                || alignment.separator.is_empty()
                || alignment.separator.len() > 4
            {
                return Err("bundled evidence alignment contract is unsupported".into());
            }
            let aligned_groups = alignment
                .aligned_groups
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let excluded = alignment
                .excluded_fields
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let mut fields = contract
                .groups
                .iter()
                .filter(|group| aligned_groups.contains(group.id.as_str()))
                .flat_map(|group| group.fields.iter().cloned())
                .filter(|field| !excluded.contains(field.as_str()))
                .collect::<BTreeSet<_>>();
            fields.extend(alignment.aligned_fields.iter().cloned());
            if fields.is_empty() || !fields.contains(&alignment.key_field) {
                return Err("bundled evidence alignment fields are incomplete".into());
            }
            Ok(AlignmentSpec {
                id: alignment.id,
                kind: alignment.kind,
                source_id: alignment.source_id,
                scope: alignment.scope,
                source_transcript_release: alignment.source_transcript_release,
                key_field: alignment.key_field,
                protein_field: alignment.protein_field,
                canonical_field: alignment.canonical_field,
                separator: alignment.separator,
                missing_values: alignment.missing_values,
                fields,
            })
        })
        .clone()
}

fn bundled_record_spec() -> Result<RecordResolutionSpec, String> {
    BUNDLED_RECORD_SPEC
        .get_or_init(|| {
            let contract: Contract = serde_json::from_str(include_str!(
                "../../../config/dbnsfp-4.9a-curated-fields.json"
            ))
            .map_err(|error| format!("invalid bundled record resolution contract: {error}"))?;
            let resolution = contract.record_resolution;
            if resolution.raw_field_path.is_empty()
                || resolution.raw_field_path.len() > 200
                || resolution.missing_values.len() > 16
            {
                return Err("bundled record resolution contract is unsupported".into());
            }
            let excluded = contract
                .legacy_transcript_alignment
                .excluded_fields
                .into_iter()
                .collect::<HashSet<_>>();
            let expected = contract
                .groups
                .into_iter()
                .flat_map(|group| group.fields)
                .filter(|field| !excluded.contains(field))
                .collect::<BTreeSet<_>>();
            let mut fields = BTreeMap::new();
            for group in resolution.field_groups {
                if group.cardinality == RecordCardinality::AlignedVector
                    && (group.identity_field.as_deref().is_none_or(str::is_empty)
                        || group
                            .identity_separator
                            .as_deref()
                            .is_none_or(str::is_empty)
                        || group.value_separator.as_deref().is_none_or(str::is_empty))
                {
                    return Err("aligned record fields require identity and delimiters".into());
                }
                for separator in [
                    group.identity_separator.as_deref(),
                    group.value_separator.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    if separator.len() > 4 {
                        return Err("record field delimiter is too long".into());
                    }
                }
                for field in group.fields {
                    if fields
                        .insert(
                            field,
                            RecordFieldSpec {
                                cardinality: group.cardinality,
                                identity_field: group.identity_field.clone(),
                                identity_separator: group.identity_separator.clone(),
                                value_separator: group.value_separator.clone(),
                            },
                        )
                        .is_some()
                    {
                        return Err(
                            "record resolution contract classifies a field more than once".into(),
                        );
                    }
                }
            }
            let actual = fields.keys().cloned().collect::<BTreeSet<_>>();
            if actual != expected {
                return Err("record resolution contract does not cover retained fields".into());
            }
            for identity_field in [
                &resolution.record_identity.transcript_field,
                &resolution.record_identity.protein_field,
                &resolution.record_identity.uniprot_accession_field,
                &resolution.record_identity.uniprot_entry_field,
                &resolution.record_identity.reference_amino_acid_field,
                &resolution.record_identity.alternate_amino_acid_field,
                &resolution.record_identity.amino_acid_position_field,
                &resolution.record_identity.hgvsp_field,
            ] {
                if !fields.contains_key(identity_field) {
                    return Err("record resolution identity field is not retained".into());
                }
            }
            Ok(RecordResolutionSpec {
                id: resolution.id,
                source_id: resolution.source_id,
                raw_field_path: resolution.raw_field_path,
                transcript_ids_unversioned: resolution.transcript_ids_unversioned,
                stable_transcript_match_requires_peptide_compatibility: resolution
                    .stable_transcript_match_requires_peptide_compatibility,
                missing_values: resolution.missing_values,
                identity: resolution.record_identity,
                fields,
            })
        })
        .clone()
}

pub(crate) fn bundled_record_resolution_contract() -> Option<(String, String)> {
    bundled_record_spec()
        .ok()
        .map(|spec| (spec.source_id, spec.id))
}

pub(crate) fn is_bundled_record_list(source_id: &str, value: &Value) -> bool {
    bundled_record_spec().is_ok_and(|spec| {
        spec.source_id == source_id
            && value
                .as_array()
                .is_some_and(|records| records.iter().all(Value::is_object))
    })
}

pub(crate) fn bundled_record_field_scope(
    source_id: &str,
    field_path: &str,
) -> Option<&'static str> {
    let spec = bundled_record_spec().ok()?;
    let field = (spec.source_id == source_id)
        .then(|| spec.fields.get(field_path))
        .flatten()?;
    match field.cardinality {
        RecordCardinality::AlleleScalar => Some("allele"),
        RecordCardinality::RecordScalar | RecordCardinality::AlignedVector => Some("transcript"),
        RecordCardinality::OpaqueList => None,
    }
}

pub(crate) fn bundled_record_field_is_aligned(source_id: &str, field_path: &str) -> bool {
    bundled_record_spec().is_ok_and(|spec| {
        spec.source_id == source_id
            && spec
                .fields
                .get(field_path)
                .is_some_and(|field| field.cardinality == RecordCardinality::AlignedVector)
    })
}

pub(crate) fn bundled_record_field_is_selected(source_id: &str, field_path: &str) -> bool {
    bundled_record_spec().is_ok_and(|spec| {
        spec.source_id == source_id
            && spec.fields.get(field_path).is_some_and(|field| {
                matches!(
                    field.cardinality,
                    RecordCardinality::RecordScalar | RecordCardinality::AlignedVector
                )
            })
    })
}

pub(crate) fn resolve_bundled_record_list(
    source_id: &str,
    value: &Value,
    selected_consequence: &Map<String, Value>,
) -> Result<Option<ResolvedRecordList>, String> {
    let spec = bundled_record_spec()?;
    if spec.source_id != source_id {
        return Ok(None);
    }
    let records = value
        .as_array()
        .ok_or("record-list source payload is not an array")?;
    if records.iter().any(|record| !record.is_object()) {
        return Err("record-list source payload contains a non-object record".into());
    }
    let mut raw_records = Vec::with_capacity(records.len());
    for (ordinal, record) in records.iter().enumerate() {
        let mut record = record.as_object().cloned().unwrap_or_default();
        if record.contains_key("sourceRecordOrdinal") {
            return Err("record-list source payload contains a reserved field".into());
        }
        record.insert(
            "sourceRecordOrdinal".into(),
            Value::from(u64::try_from(ordinal).map_err(|_| "record ordinal is too large")?),
        );
        raw_records.push(Value::Object(record));
    }

    let transcript = consequence_string(selected_consequence, "transcript_id");
    let eligible = transcript.map_or_else(Vec::new, |transcript| {
        records
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|record| eligible_record(&spec, record, transcript, selected_consequence))
            .collect::<Vec<_>>()
    });
    let mut fields = Vec::new();
    for (field_path, field_spec) in &spec.fields {
        let resolved = match field_spec.cardinality {
            RecordCardinality::AlleleScalar => common_record_value(
                &spec,
                records.iter().filter_map(Value::as_object),
                field_path,
            )
            .map(|value| (ResolvedRecordScope::Allele, value)),
            RecordCardinality::RecordScalar => common_record_value(
                &spec,
                eligible.iter().map(|record| record.object),
                field_path,
            )
            .map(|value| (ResolvedRecordScope::Selected, value)),
            RecordCardinality::AlignedVector => {
                common_aligned_value(&spec, &eligible, field_path, field_spec)
                    .map(|value| (ResolvedRecordScope::Selected, value))
            }
            RecordCardinality::OpaqueList => None,
        };
        if let Some((scope, value)) = resolved {
            fields.push(ResolvedRecordField {
                field_path: field_path.clone(),
                value,
                scope,
            });
        }
    }
    Ok(Some(ResolvedRecordList {
        raw_field_path: spec.raw_field_path,
        raw_value: Value::Array(raw_records),
        fields,
    }))
}

#[derive(Clone, Copy)]
struct EligibleRecord<'a> {
    object: &'a Map<String, Value>,
    transcript_index: usize,
}

fn eligible_record<'a>(
    spec: &RecordResolutionSpec,
    record: &'a Map<String, Value>,
    selected_transcript: &str,
    selected_consequence: &Map<String, Value>,
) -> Option<EligibleRecord<'a>> {
    let transcripts = split_record_field(spec, record.get(&spec.identity.transcript_field)?, ";")?;
    let transcript_index = unique_identity_index(
        &transcripts,
        selected_transcript,
        spec.transcript_ids_unversioned,
    )?;
    if spec.stable_transcript_match_requires_peptide_compatibility
        && !peptide_context_matches(
            spec,
            record,
            &transcripts,
            transcript_index,
            selected_consequence,
        )
    {
        return None;
    }
    Some(EligibleRecord {
        object: record,
        transcript_index,
    })
}

fn peptide_context_matches(
    spec: &RecordResolutionSpec,
    record: &Map<String, Value>,
    transcripts: &[&str],
    transcript_index: usize,
    selected: &Map<String, Value>,
) -> bool {
    if let Some((reference, alternate)) = consequence_amino_acids(selected) {
        if record_nonmissing_string(spec, record, &spec.identity.reference_amino_acid_field)
            .is_some_and(|value| !value.eq_ignore_ascii_case(reference))
            || record_nonmissing_string(spec, record, &spec.identity.alternate_amino_acid_field)
                .is_some_and(|value| !value.eq_ignore_ascii_case(alternate))
        {
            return false;
        }
    }
    let source_protein = vector_value_at_transcript(
        spec,
        record,
        transcripts,
        transcript_index,
        &spec.identity.protein_field,
    );
    if let Some(selected_protein) = consequence_string(selected, "protein_id")
        && source_protein.as_deref().is_some_and(|protein| {
            stable_transcript_id(protein) != stable_transcript_id(selected_protein)
        })
    {
        return false;
    }
    let Some(protein) = source_protein else {
        return true;
    };
    let Some(selected_position) = consequence_u64(selected, "protein_start") else {
        return hgvsp_context_matches(spec, record, &protein, selected);
    };
    let Some(positions) = record
        .get(&spec.identity.amino_acid_position_field)
        .and_then(|value| split_record_field(spec, value, ";"))
    else {
        return hgvsp_context_matches(spec, record, &protein, selected);
    };
    let Some(proteins) = record
        .get(&spec.identity.protein_field)
        .and_then(|value| split_record_field(spec, value, ";"))
    else {
        return hgvsp_context_matches(spec, record, &protein, selected);
    };
    if proteins.len() != positions.len() {
        return false;
    }
    let matching_positions = proteins
        .iter()
        .zip(positions)
        .filter(|(candidate, _)| **candidate == protein)
        .map(|(_, position)| position)
        .filter(|position| !is_missing(spec, position))
        .collect::<Vec<_>>();
    if !matching_positions.is_empty()
        && (matching_positions
            .iter()
            .any(|position| position.parse::<u64>().ok() != Some(selected_position))
            || matching_positions.len() > 1)
    {
        return false;
    }
    hgvsp_context_matches(spec, record, &protein, selected)
}

fn hgvsp_context_matches(
    spec: &RecordResolutionSpec,
    record: &Map<String, Value>,
    source_protein: &str,
    selected: &Map<String, Value>,
) -> bool {
    let Some(selected_hgvsp) = consequence_string(selected, "hgvsp") else {
        return true;
    };
    let Some(proteins) = record
        .get(&spec.identity.protein_field)
        .and_then(|value| split_record_field(spec, value, ";"))
    else {
        return true;
    };
    let Some(hgvsp_values) = record
        .get(&spec.identity.hgvsp_field)
        .and_then(|value| split_record_field(spec, value, ";"))
    else {
        return true;
    };
    if proteins.len() != hgvsp_values.len() {
        return false;
    }
    let selected_hgvsp = hgvsp_suffix(selected_hgvsp);
    proteins
        .iter()
        .zip(hgvsp_values)
        .filter(|(protein, _)| **protein == source_protein)
        .filter(|(_, hgvsp)| !is_missing(spec, hgvsp))
        .all(|(_, hgvsp)| hgvsp_suffix(hgvsp) == selected_hgvsp)
}

fn hgvsp_suffix(value: &str) -> &str {
    value.rsplit_once(':').map_or(value, |(_, suffix)| suffix)
}

fn common_record_value<'a>(
    spec: &RecordResolutionSpec,
    records: impl Iterator<Item = &'a Map<String, Value>>,
    field_path: &str,
) -> Option<Value> {
    common_value(
        spec,
        records.filter_map(|record| record.get(field_path).cloned()),
    )
}

fn common_aligned_value(
    spec: &RecordResolutionSpec,
    records: &[EligibleRecord<'_>],
    field_path: &str,
    field_spec: &RecordFieldSpec,
) -> Option<Value> {
    let identity_field = field_spec.identity_field.as_deref()?;
    let identity_separator = field_spec.identity_separator.as_deref()?;
    let value_separator = field_spec.value_separator.as_deref()?;
    let mut values = Vec::new();
    for record in records {
        let Some(field_value) = record.object.get(field_path) else {
            continue;
        };
        if normalized_value(spec, field_value).is_none() {
            continue;
        }
        let transcripts = split_record_field(
            spec,
            record.object.get(&spec.identity.transcript_field)?,
            ";",
        )?;
        let target = if identity_field == spec.identity.transcript_field {
            transcripts
                .get(record.transcript_index)
                .map(|value| (*value).to_owned())
        } else {
            vector_value_at_transcript(
                spec,
                record.object,
                &transcripts,
                record.transcript_index,
                identity_field,
            )
        }?;
        let identities =
            split_record_field(spec, record.object.get(identity_field)?, identity_separator)?;
        let field_values = split_record_field(spec, field_value, value_separator)?;
        if identities.len() != field_values.len() {
            return None;
        }
        let matched = identities
            .iter()
            .zip(field_values)
            .filter(|(identity, _)| **identity == target)
            .map(|(_, value)| Value::String(value.to_owned()))
            .collect::<Vec<_>>();
        if matched.is_empty() {
            return None;
        }
        let has_value = matched
            .iter()
            .any(|value| normalized_value(spec, value).is_some());
        if let Some(value) = common_value(spec, matched.into_iter()) {
            values.push(value);
        } else if has_value {
            return None;
        }
    }
    common_value(spec, values.into_iter())
}

fn vector_value_at_transcript(
    spec: &RecordResolutionSpec,
    record: &Map<String, Value>,
    transcripts: &[&str],
    transcript_index: usize,
    field_path: &str,
) -> Option<String> {
    let values = split_record_field(spec, record.get(field_path)?, ";")?;
    (values.len() == transcripts.len())
        .then(|| values.get(transcript_index).copied())
        .flatten()
        .filter(|value| !is_missing(spec, value))
        .map(str::to_owned)
}

fn common_value(spec: &RecordResolutionSpec, values: impl Iterator<Item = Value>) -> Option<Value> {
    let mut selected: Option<(String, Value)> = None;
    for value in values {
        let Some(normalized) = normalized_value(spec, &value) else {
            continue;
        };
        match &selected {
            None => selected = Some((normalized, value)),
            Some((previous, _)) if previous == &normalized => {}
            Some(_) => return None,
        }
    }
    selected.map(|(_, value)| value)
}

fn normalized_value(spec: &RecordResolutionSpec, value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            if is_missing(spec, value) {
                None
            } else if let Ok(number) = value.parse::<f64>()
                && number.is_finite()
            {
                Some(format!("number:{number:e}"))
            } else {
                Some(format!("string:{value}"))
            }
        }
        Value::Number(value) => value
            .as_f64()
            .filter(|number| number.is_finite())
            .map(|number| format!("number:{number:e}")),
        Value::Bool(value) => Some(format!("boolean:{value}")),
        Value::Array(_) | Value::Object(_) | Value::Null => None,
    }
}

fn split_record_field<'a>(
    spec: &RecordResolutionSpec,
    value: &'a Value,
    separator: &str,
) -> Option<Vec<&'a str>> {
    let value = value.as_str()?.trim();
    if is_missing(spec, value) {
        return None;
    }
    Some(value.split(separator).map(str::trim).collect())
}

fn unique_identity_index(values: &[&str], target: &str, allow_stable: bool) -> Option<usize> {
    let exact = values
        .iter()
        .enumerate()
        .filter(|(_, value)| **value == target)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return exact.first().copied();
    }
    if !allow_stable {
        return None;
    }
    let stable = stable_transcript_id(target);
    let matches = values
        .iter()
        .enumerate()
        .filter(|(_, value)| {
            stable_transcript_id(value) == stable && (!value.contains('.') || **value == target)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0])
}

fn consequence_string<'a>(consequence: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    consequence
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn consequence_u64(consequence: &Map<String, Value>, field: &str) -> Option<u64> {
    consequence.get(field).and_then(|value| {
        value.as_u64().or_else(|| {
            value.as_str()?.split_once('-').map_or_else(
                || value.as_str()?.parse().ok(),
                |(start, _)| start.parse().ok(),
            )
        })
    })
}

fn consequence_amino_acids(consequence: &Map<String, Value>) -> Option<(&str, &str)> {
    consequence_string(consequence, "amino_acids")?.split_once('/')
}

fn record_nonmissing_string<'a>(
    spec: &RecordResolutionSpec,
    record: &'a Map<String, Value>,
    field: &str,
) -> Option<&'a str> {
    record
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !is_missing(spec, value))
}

fn is_missing(spec: &RecordResolutionSpec, value: &str) -> bool {
    spec.missing_values.iter().any(|missing| missing == value)
}

pub(crate) fn bundled_alignment_group(
    scope: &str,
    source_id: &str,
    field_path: &str,
) -> Option<String> {
    let spec = bundled_spec().ok()?;
    (spec.scope == scope && spec.source_id == source_id && spec.fields.contains(field_path))
        .then_some(spec.id)
}

pub(crate) fn alignment_key_field(scope: &str, source_id: &str) -> Option<String> {
    let spec = bundled_spec().ok()?;
    (spec.scope == scope && spec.source_id == source_id).then_some(spec.key_field)
}

pub(crate) fn select_aligned_value(
    scope: &str,
    source_id: &str,
    field_path: &str,
    transcript_vector: &str,
    value_vector: &str,
    selected_transcript: &str,
) -> Option<String> {
    let spec = bundled_spec().ok()?;
    if spec.scope != scope || spec.source_id != source_id || !spec.fields.contains(field_path) {
        return None;
    }
    let transcripts = transcript_vector
        .split(&spec.separator)
        .map(str::trim)
        .collect::<Vec<_>>();
    let values = value_vector
        .split(&spec.separator)
        .map(str::trim)
        .collect::<Vec<_>>();
    if transcripts.len() != values.len() {
        return None;
    }
    let exact = transcripts
        .iter()
        .enumerate()
        .filter(|(_, transcript)| **transcript == selected_transcript)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let index = if exact.len() == 1 {
        exact[0]
    } else if transcripts
        .iter()
        .all(|transcript| !transcript.contains('.'))
    {
        let stable = stable_transcript_id(selected_transcript);
        let matches = transcripts
            .iter()
            .enumerate()
            .filter(|(_, transcript)| stable_transcript_id(transcript) == stable)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        *matches.first().filter(|_| matches.len() == 1)?
    } else {
        return None;
    };
    let value = values[index];
    (!spec.missing_values.iter().any(|missing| missing == value)).then(|| value.to_owned())
}

fn stable_transcript_id(value: &str) -> &str {
    value.split_once('.').map_or(value, |(stable, _)| stable)
}

pub(crate) fn catalog_alignment_groups(fields: &[Value]) -> Vec<Value> {
    let Ok(mut spec) = bundled_spec() else {
        return Vec::new();
    };
    let present = fields
        .iter()
        .filter_map(|field| {
            (field["scope"].as_str()? == spec.scope
                && field["sourceId"].as_str()? == spec.source_id)
                .then(|| field["fieldPath"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect::<HashSet<_>>();
    spec.fields.retain(|field| present.contains(field));
    if !present.contains(&spec.key_field) || spec.fields.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "id": spec.id,
        "kind": spec.kind,
        "scope": spec.scope,
        "sourceId": spec.source_id,
        "sourceTranscriptRelease": spec.source_transcript_release,
        "keyField": spec.key_field,
        "proteinField": spec.protein_field,
        "canonicalField": spec.canonical_field,
        "separator": spec.separator,
        "missingValues": spec.missing_values,
        "fields": spec.fields,
    })]
}

fn bounded_string(value: &Value, name: &str) -> Result<String, String> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("evidence alignment has no {name}"))?;
    if value.is_empty() || value.len() > 200 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(format!("evidence alignment {name} is invalid"));
    }
    Ok(value.to_owned())
}

fn catalog_specs(catalog: &Path) -> Result<Vec<AlignmentSpec>, String> {
    let metadata =
        fs::metadata(catalog).map_err(|error| format!("cannot inspect field catalog: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
        return Err("field catalog has an invalid size".into());
    }
    let catalog: Value = serde_json::from_slice(
        &fs::read(catalog).map_err(|error| format!("cannot read field catalog: {error}"))?,
    )
    .map_err(|error| format!("invalid field catalog: {error}"))?;
    let present = catalog["fields"]
        .as_array()
        .ok_or("field catalog has no fields array")?
        .iter()
        .filter_map(|field| {
            Some((
                field["scope"].as_str()?.to_owned(),
                field["sourceId"].as_str()?.to_owned(),
                field["fieldPath"].as_str()?.to_owned(),
            ))
        })
        .collect::<HashSet<_>>();
    let Some(groups) = catalog["alignmentGroups"].as_array() else {
        let mut spec = bundled_spec()?;
        spec.fields.retain(|field| {
            present.contains(&(spec.scope.clone(), spec.source_id.clone(), field.clone()))
        });
        return Ok((present.contains(&(
            spec.scope.clone(),
            spec.source_id.clone(),
            spec.key_field.clone(),
        )) && !spec.fields.is_empty())
        .then_some(spec)
        .into_iter()
        .collect());
    };
    if groups.len() > 16 {
        return Err("field catalog has too many evidence alignment groups".into());
    }
    groups
        .iter()
        .map(|group| {
            let kind = bounded_string(&group["kind"], "kind")?;
            if kind != "parallelTranscriptVector" {
                return Err(format!("unsupported evidence alignment kind: {kind}"));
            }
            let scope = bounded_string(&group["scope"], "scope")?;
            let source_id = bounded_string(&group["sourceId"], "source ID")?;
            let separator = bounded_string(&group["separator"], "separator")?;
            if separator.len() > 4 {
                return Err("evidence alignment separator is too long".into());
            }
            let values = |name: &str, maximum: usize| -> Result<Vec<String>, String> {
                let values = group[name]
                    .as_array()
                    .ok_or_else(|| format!("evidence alignment has no {name}"))?;
                if values.len() > maximum {
                    return Err(format!("evidence alignment has too many {name}"));
                }
                values
                    .iter()
                    .map(|value| bounded_string(value, name))
                    .collect()
            };
            let missing_values = group["missingValues"]
                .as_array()
                .ok_or("evidence alignment has no missingValues")?;
            if missing_values.len() > 16 {
                return Err("evidence alignment has too many missingValues".into());
            }
            let missing_values = missing_values
                .iter()
                .map(|value| {
                    let value = value
                        .as_str()
                        .ok_or("evidence alignment missingValues is invalid")?;
                    if value.len() > 16 || value.bytes().any(|byte| byte.is_ascii_control()) {
                        return Err("evidence alignment missingValues is invalid".into());
                    }
                    Ok(value.to_owned())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let fields = values("fields", 2048)?.into_iter().collect::<BTreeSet<_>>();
            let key_field = bounded_string(&group["keyField"], "key field")?;
            if !fields.contains(&key_field)
                || fields.iter().any(|field| {
                    !present.contains(&(scope.clone(), source_id.clone(), field.clone()))
                })
            {
                return Err("evidence alignment references unavailable fields".into());
            }
            Ok(AlignmentSpec {
                id: bounded_string(&group["id"], "ID")?,
                kind,
                scope,
                source_id,
                source_transcript_release: bounded_string(
                    &group["sourceTranscriptRelease"],
                    "source transcript release",
                )?,
                key_field,
                protein_field: bounded_string(&group["proteinField"], "protein field")?,
                canonical_field: bounded_string(&group["canonicalField"], "canonical field")?,
                separator,
                missing_values,
                fields,
            })
        })
        .collect()
}

pub(crate) fn validate_catalog(catalog: &Path) -> Result<(), String> {
    catalog_specs(catalog).map(|_| ())
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_list(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .map(|value| sql_literal(&value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn report_table_path(evidence: &Path, name: &str) -> Result<PathBuf, String> {
    let path = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join(name);
    path.is_file()
        .then_some(path)
        .ok_or_else(|| format!("AnnoCAT result is missing {name}"))
}

fn input_fingerprint(evidence: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    let report_root = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?;
    let selection_contract = fs::read(report_root.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|manifest| {
            manifest
                .get("representativeSelectionContract")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "legacy-global-per-allele-v0".to_owned());
    digest.update(b"representativeSelectionContract\0");
    digest.update(selection_contract.as_bytes());
    let mut paths = vec![
        report_table_path(evidence, "variants.parquet")?,
        evidence.to_path_buf(),
    ];
    let consequences = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join("consequences.parquet");
    if consequences.is_file() {
        paths.insert(1, consequences);
    }
    for path in paths {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        digest.update(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        digest.update(metadata.len().to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize())[..16].to_owned())
}

fn kind_tag(kind: RequestedResolutionKind) -> &'static str {
    match kind {
        RequestedResolutionKind::AlignedTranscriptVector => "vector",
        RequestedResolutionKind::LegacyAllele => "allele",
        RequestedResolutionKind::SelectedFeature => "selected",
    }
}

fn field_hash(field: &RequestedField) -> String {
    let mut digest = Sha256::new();
    for value in [
        kind_tag(field.kind),
        &field.scope,
        &field.biological_scope,
        &field.source_id,
        &field.field_path,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_owned()
}

fn cache_path(
    evidence: &Path,
    fingerprint: &str,
    field: &RequestedField,
) -> Result<PathBuf, String> {
    Ok(evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join(format!(
            "{CACHE_PREFIX}{fingerprint}-{}.parquet",
            field_hash(field)
        )))
}

pub(crate) fn available_path(evidence: &Path) -> Option<PathBuf> {
    let fingerprint = input_fingerprint(evidence).ok()?;
    let parent = evidence.parent()?;
    let prefix = format!("{CACHE_PREFIX}{fingerprint}-");
    let mut exists = false;
    for entry in fs::read_dir(parent).ok()?.filter_map(Result::ok) {
        let matches = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".parquet"));
        if !matches {
            continue;
        }
        if cache_schema_is_valid(&entry.path()) {
            exists = true;
        } else {
            let _ = fs::remove_file(entry.path());
        }
    }
    exists.then(|| parent.join(format!("{prefix}*.parquet")))
}

fn cache_schema_is_valid(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    let schema = builder.schema();
    [
        "schema_version",
        "input_fingerprint",
        "allele_id",
        "source_id",
        "field_path",
        "resolution_kind",
        "resolved_string",
        "resolved_number",
    ]
    .into_iter()
    .all(|name| schema.index_of(name).is_ok())
}

fn valid_requested_field(field: &RequestedField) -> bool {
    [
        &field.scope,
        &field.biological_scope,
        &field.source_id,
        &field.field_path,
    ]
    .into_iter()
    .all(|value| {
        !value.is_empty()
            && value.len() <= 200
            && !value.bytes().any(|byte| byte.is_ascii_control())
    })
}

fn cache_is_valid(path: &Path, fingerprint: &str, field: &RequestedField) -> bool {
    if !cache_schema_is_valid(path) {
        return false;
    }
    let Ok(connection) = Connection::open_in_memory() else {
        return false;
    };
    connection
        .query_row(
            "SELECT coalesce(bool_and(
                      schema_version=? AND input_fingerprint=?
                      AND source_id=? AND field_path=?
                    ), true)
             FROM (SELECT * FROM read_parquet(?) LIMIT 1)",
            params![
                CACHE_VERSION,
                fingerprint,
                field.source_id,
                field.field_path,
                path.to_string_lossy().as_ref()
            ],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

pub(crate) fn prepare(
    variants: &Path,
    evidence: &Path,
    catalog: &Path,
    requested: &[RequestedField],
) -> Result<Option<PathBuf>, String> {
    if requested.is_empty() {
        return Ok(None);
    }
    validate_catalog(catalog)?;
    let fingerprint = input_fingerprint(evidence)?;
    let mut seen = HashSet::new();
    let fields = requested
        .iter()
        .filter(|field| {
            seen.insert((
                kind_tag(field.kind),
                field.scope.as_str(),
                field.biological_scope.as_str(),
                field.source_id.as_str(),
                field.field_path.as_str(),
            ))
        })
        .collect::<Vec<_>>();
    if fields.iter().any(|field| !valid_requested_field(field)) {
        return Err("requested evidence field is invalid".into());
    }
    let consequences = evidence
        .parent()
        .ok_or("evidence table has no parent directory")?
        .join("consequences.parquet");
    if fields
        .iter()
        .any(|field| field.kind == RequestedResolutionKind::SelectedFeature)
        && !consequences.is_file()
    {
        return Err("AnnoCAT result is missing consequences.parquet".into());
    }
    // ponytail: one process-wide lock is enough until concurrent report queries are measured.
    let _guard = BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "evidence cache build lock failed")?;
    for field in fields {
        let path = cache_path(evidence, &fingerprint, field)?;
        if cache_is_valid(&path, &fingerprint, field) {
            continue;
        }
        build_cache(
            &path,
            &fingerprint,
            variants,
            &consequences,
            evidence,
            field,
        )?;
    }
    available_path(evidence)
        .ok_or_else(|| "requested evidence cache was not published".into())
        .map(Some)
}

fn build_cache(
    path: &Path,
    fingerprint: &str,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<(), String> {
    let partial = path.with_extension("parquet.partial");
    let _ = fs::remove_file(&partial);
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "SET threads=4;
             SET preserve_insertion_order=false;
             SET memory_limit='1GB';",
        )
        .map_err(|error| format!("cannot configure evidence resolution: {error}"))?;
    let query = match field.kind {
        RequestedResolutionKind::AlignedTranscriptVector => {
            aligned_query(fingerprint, variants, evidence, field)?
        }
        RequestedResolutionKind::LegacyAllele => legacy_allele_query(fingerprint, evidence, field),
        RequestedResolutionKind::SelectedFeature => {
            selected_feature_query(fingerprint, variants, consequences, evidence, field)?
        }
    };
    let sql = format!(
        "COPY ({query}) TO {} (FORMAT PARQUET, COMPRESSION ZSTD, ROW_GROUP_SIZE 100000)",
        sql_literal(&partial.to_string_lossy())
    );
    if let Err(error) = connection.execute_batch(&sql) {
        let _ = fs::remove_file(&partial);
        return Err(format!("cannot build requested evidence cache: {error}"));
    }
    if !cache_is_valid(&partial, fingerprint, field) {
        let _ = fs::remove_file(&partial);
        return Err("requested evidence cache failed validation".into());
    }
    if let Err(error) = crate::library_metadata::publish_atomic_file(&partial, path) {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    Ok(())
}

fn cache_projection(
    fingerprint: &str,
    source_id: &str,
    field_path: &str,
    release: &str,
    body: &str,
) -> String {
    format!(
        "SELECT {CACHE_VERSION}::INTEGER AS schema_version,
                {}::VARCHAR AS input_fingerprint,
                allele_id,
                {}::VARCHAR AS source_id,
                {}::VARCHAR AS field_path,
                {}::VARCHAR AS source_transcript_release,
                resolution_kind,
                resolved_string,
                try_cast(resolved_string AS DOUBLE) AS resolved_number,
                cast(selected_index AS SMALLINT) AS selected_index,
                cast(source_canonical_index AS SMALLINT) AS source_canonical_index,
                cast(least(reported_value_count, 32767) AS SMALLINT) AS reported_value_count,
                cast(least(distinct_value_count, 32767) AS SMALLINT) AS distinct_value_count
         FROM ({body}) resolved",
        sql_literal(fingerprint),
        sql_literal(source_id),
        sql_literal(field_path),
        sql_literal(release),
    )
}

fn aligned_query(
    fingerprint: &str,
    variants: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<String, String> {
    if field.scope == "selected" {
        return selected_record_vector_query(fingerprint, variants, evidence, field);
    }
    let spec = bundled_spec()?;
    if spec.scope != field.scope
        || spec.source_id != field.source_id
        || !spec.fields.contains(&field.field_path)
    {
        return Err("requested aligned evidence field is outside its contract".into());
    }
    let missing = sql_list(spec.missing_values.iter().cloned());
    let separator = sql_literal(&spec.separator);
    let variants = crate::results::resolved_variants_relation(variants)?;
    let body = format!(
        r#"
        WITH vectors AS MATERIALIZED (
          SELECT allele_id,
                 max(coalesce(string_value, cast(integer_value AS VARCHAR),
                              cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR)))
                   FILTER (WHERE field_path={key}) AS transcripts,
                 max(coalesce(string_value, cast(integer_value AS VARCHAR),
                              cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR)))
                   FILTER (WHERE field_path={field_path}) AS values
          FROM read_parquet({evidence})
          WHERE scope={scope} AND source_id={source}
            AND field_path IN ({key}, {field_path})
          GROUP BY allele_id
        ), joined AS MATERIALIZED (
          SELECT vectors.allele_id,
                 list_transform(string_split(vectors.transcripts, {separator}), x -> trim(x))
                   AS transcript_ids,
                 string_split(vectors.values, {separator}) AS values,
                 trim(coalesce(variants.transcript_id, '')) AS selected_transcript
          FROM vectors
          JOIN {variants} variants USING (allele_id)
          WHERE vectors.transcripts IS NOT NULL AND vectors.values IS NOT NULL
        ), indexed AS (
          SELECT *,
                 CASE
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> x=selected_transcript))=1
                     THEN list_position(transcript_ids, selected_transcript)
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> strpos(x, '.')>0))=0
                     AND list_count(list_filter(
                       transcript_ids,
                       x -> split_part(x, '.', 1)=split_part(selected_transcript, '.', 1)
                     ))=1
                     THEN list_position(
                       list_transform(transcript_ids, x -> split_part(x, '.', 1)),
                       split_part(selected_transcript, '.', 1)
                     )
                 END AS selected_index,
                 CASE
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> x=selected_transcript))=1
                     THEN 'exact_transcript'
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, x -> strpos(x, '.')>0))=0
                     AND list_count(list_filter(
                       transcript_ids,
                       x -> split_part(x, '.', 1)=split_part(selected_transcript, '.', 1)
                     ))=1
                     THEN 'stable_id_match'
                 END AS selected_match_kind
          FROM joined
        ), classified AS (
          SELECT allele_id,
                 selected_index,
                 NULL::INTEGER AS source_canonical_index,
                 list_count(list_filter(values, x -> trim(x) NOT IN ({missing})))
                   AS reported_value_count,
                 list_unique(list_filter(values, x -> trim(x) NOT IN ({missing})))
                   AS distinct_value_count,
                 CASE
                   WHEN list_count(values)<>list_count(transcript_ids) THEN 'invalid_vector'
                   WHEN selected_index IS NULL THEN
                     CASE WHEN list_count(list_filter(values, x -> trim(x) NOT IN ({missing})))=0
                          THEN 'not_reported' ELSE 'unresolved_transcript' END
                   WHEN trim(list_extract(values, selected_index)) IN ({missing})
                     THEN 'exact_missing'
                   ELSE selected_match_kind
                 END AS resolution_kind,
                 CASE
                   WHEN selected_index IS NOT NULL
                     AND list_count(values)=list_count(transcript_ids)
                     AND trim(list_extract(values, selected_index)) NOT IN ({missing})
                     THEN trim(list_extract(values, selected_index))
                 END AS resolved_string
          FROM indexed
        )
        SELECT * FROM classified
        "#,
        key = sql_literal(&spec.key_field),
        field_path = sql_literal(&field.field_path),
        evidence = sql_literal(&evidence.to_string_lossy()),
        variants = variants,
        scope = sql_literal(&field.scope),
        source = sql_literal(&field.source_id),
    );
    Ok(cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        &spec.source_transcript_release,
        &body,
    ))
}

fn selected_record_vector_query(
    fingerprint: &str,
    variants: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<String, String> {
    let spec = bundled_record_spec()?;
    let field_spec = spec
        .fields
        .get(&field.field_path)
        .filter(|field| field.cardinality == RecordCardinality::AlignedVector)
        .ok_or("requested selected record field is not an aligned vector")?;
    if spec.source_id != field.source_id {
        return Err("requested selected record field is outside its contract".into());
    }
    let value_separator = field_spec
        .value_separator
        .as_deref()
        .ok_or("selected record field has no value separator")?;
    let transcript_identity =
        field_spec.identity_field.as_deref() == Some(spec.identity.transcript_field.as_str());
    let missing = sql_list(spec.missing_values.iter().cloned());
    let variants = crate::results::resolved_variants_relation(variants)?;
    let transcript_path = sql_literal(&json_field_path(&spec.identity.transcript_field));
    let field_path = sql_literal(&json_field_path(&field.field_path));
    let body = format!(
        r#"
        WITH stored AS MATERIALIZED (
          SELECT allele_id,
                 min(coalesce(string_value, cast(integer_value AS VARCHAR),
                              cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR)))
                   AS stored_value,
                 count(DISTINCT coalesce(string_value, cast(integer_value AS VARCHAR),
                                         cast(number_value AS VARCHAR),
                                         cast(boolean_value AS VARCHAR))) AS stored_count
          FROM read_parquet({evidence})
          WHERE scope='selected' AND source_id={source} AND field_path={requested_field}
          GROUP BY allele_id
        ), prepared AS MATERIALIZED (
          SELECT *, string_split(stored_value, {value_separator}) AS stored_values
          FROM stored
          WHERE stored_value IS NOT NULL
        ), scalar_rows AS (
          SELECT allele_id,
                 1 AS selected_index,
                 NULL::INTEGER AS source_canonical_index,
                 1 AS reported_value_count,
                 1 AS distinct_value_count,
                 'policy_selected' AS resolution_kind,
                 trim(stored_value) AS resolved_string
          FROM prepared
          WHERE stored_count=1 AND list_count(stored_values)=1
            AND trim(stored_value) NOT IN ({missing})
        ), raw_records AS MATERIALIZED (
          SELECT prepared.allele_id,
                 prepared.stored_values,
                 list_transform(
                   string_split(json_extract_string(record.value, {transcript_path}), ';'),
                   value -> trim(value)
                 ) AS transcript_ids,
                 list_transform(
                   string_split(json_extract_string(record.value, {field_path}), {value_separator}),
                   value -> trim(value)
                 ) AS record_values,
                 trim(coalesce(variants.transcript_id, '')) AS selected_transcript
          FROM prepared
          JOIN {variants} variants USING (allele_id)
          JOIN read_parquet({evidence}) raw USING (allele_id)
          CROSS JOIN json_each(raw.json_value) record
          WHERE prepared.stored_count=1 AND list_count(prepared.stored_values)>1
            AND raw.scope='source_records' AND raw.source_id={source}
            AND raw.field_path={raw_field}
            AND json_extract_string(record.value, {transcript_path}) IS NOT NULL
            AND trim(json_extract_string(record.value, {field_path}))=trim(prepared.stored_value)
        ), indexed AS (
          SELECT *,
                 CASE
                   WHEN selected_transcript<>''
                     AND list_count(list_filter(transcript_ids, value -> value=selected_transcript))=1
                     THEN list_position(transcript_ids, selected_transcript)
                 END AS selected_index
          FROM raw_records
        ), recovered AS (
          SELECT allele_id,
                 min(selected_index) AS selected_index,
                 count(DISTINCT trim(list_extract(record_values, selected_index)))
                   AS resolved_count,
                 min(trim(list_extract(record_values, selected_index))) AS resolved_string
          FROM indexed
          WHERE {transcript_identity}
            AND selected_index IS NOT NULL
            AND list_count(record_values)=list_count(transcript_ids)
            AND trim(list_extract(record_values, selected_index)) NOT IN ({missing})
          GROUP BY allele_id
        ), vector_rows AS (
          SELECT prepared.allele_id,
                 recovered.selected_index,
                 NULL::INTEGER AS source_canonical_index,
                 list_count(list_filter(prepared.stored_values, value -> trim(value) NOT IN ({missing})))
                   AS reported_value_count,
                 list_unique(list_filter(prepared.stored_values, value -> trim(value) NOT IN ({missing})))
                   AS distinct_value_count,
                 CASE
                   WHEN recovered.resolved_count=1 THEN 'exact_transcript'
                   ELSE 'invalid_vector'
                 END AS resolution_kind,
                 CASE WHEN recovered.resolved_count=1 THEN recovered.resolved_string END
                   AS resolved_string
          FROM prepared
          LEFT JOIN recovered USING (allele_id)
          WHERE prepared.stored_count=1 AND list_count(prepared.stored_values)>1
        )
        SELECT * FROM scalar_rows
        UNION ALL
        SELECT * FROM vector_rows
        "#,
        evidence = sql_literal(&evidence.to_string_lossy()),
        source = sql_literal(&field.source_id),
        requested_field = sql_literal(&field.field_path),
        value_separator = sql_literal(value_separator),
        raw_field = sql_literal(&spec.raw_field_path),
        variants = variants,
        transcript_identity = if transcript_identity { "true" } else { "false" },
    );
    Ok(cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        "legacy selected dbNSFP record",
        &body,
    ))
}

fn json_field_path(field: &str) -> String {
    format!("$.\"{}\"", field.replace('\\', "\\\\").replace('"', "\\\""))
}

fn legacy_allele_query(fingerprint: &str, evidence: &Path, field: &RequestedField) -> String {
    let body = format!(
        r#"
        WITH candidates AS MATERIALIZED (
          SELECT allele_id,
                 scope IN ('allele', 'variant') AS direct,
                 consequence_id,
                 coalesce(string_value, cast(integer_value AS VARCHAR),
                          cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                          json_value) AS candidate_value,
                 CASE
                   WHEN string_value IS NOT NULL THEN 's:' || trim(string_value)
                   WHEN integer_value IS NOT NULL THEN 'i:' || cast(integer_value AS VARCHAR)
                   WHEN number_value IS NOT NULL THEN 'n:' || cast(number_value AS VARCHAR)
                   WHEN boolean_value IS NOT NULL THEN 'b:' || cast(boolean_value AS VARCHAR)
                   WHEN json_value IS NOT NULL THEN 'j:' || trim(json_value)
                 END AS comparison_value
          FROM read_parquet({evidence})
          WHERE source_id={source} AND field_path={field_path}
            AND scope<>'selected'
            AND coalesce(string_value, cast(integer_value AS VARCHAR),
                         cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                         json_value) IS NOT NULL
        ), aggregated AS (
          SELECT allele_id,
                 count(*) FILTER (WHERE direct) AS direct_count,
                 count(DISTINCT comparison_value) FILTER (WHERE direct) AS direct_distinct,
                 count(*) AS reported_value_count,
                 count(DISTINCT comparison_value) AS distinct_value_count,
                 first(candidate_value ORDER BY consequence_id NULLS FIRST)
                   FILTER (WHERE direct) AS direct_value,
                 first(candidate_value ORDER BY consequence_id NULLS FIRST) AS any_value
          FROM candidates
          GROUP BY allele_id
        )
        SELECT allele_id,
               NULL::INTEGER AS selected_index,
               NULL::INTEGER AS source_canonical_index,
               reported_value_count,
               distinct_value_count,
               CASE
                 WHEN direct_count>0 AND direct_distinct=1 THEN 'direct_allele'
                 WHEN direct_count>0 THEN 'conflicting_allele_values'
                 WHEN distinct_value_count=1 THEN 'legacy_allele_scope_recovered'
                 ELSE 'conflicting_legacy_values'
               END AS resolution_kind,
               CASE
                 WHEN direct_count>0 AND direct_distinct=1 THEN direct_value
                 WHEN direct_count=0 AND distinct_value_count=1 THEN any_value
               END AS resolved_string
        FROM aggregated
        "#,
        evidence = sql_literal(&evidence.to_string_lossy()),
        source = sql_literal(&field.source_id),
        field_path = sql_literal(&field.field_path),
    );
    cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        "legacy report",
        &body,
    )
}

fn selected_feature_query(
    fingerprint: &str,
    variants: &Path,
    consequences: &Path,
    evidence: &Path,
    field: &RequestedField,
) -> Result<String, String> {
    let mode = match field.biological_scope.as_str() {
        "gene" => "gene",
        "feature"
            if annocat_core::source_catalog::feature_identity(&field.source_id) == Some("gene") =>
        {
            "gene"
        }
        "feature" => "feature",
        _ => "transcript",
    };
    let match_rank = match mode {
        "gene" => {
            "CASE
               WHEN selected_gene<>'' AND candidate_gene=selected_gene
                 AND selected_transcript<>'' AND candidate_transcript=selected_transcript THEN 0
               WHEN selected_gene<>'' AND candidate_gene=selected_gene THEN 1
               ELSE 99
             END"
        }
        "feature" => {
            "CASE
               WHEN selected_transcript<>'' AND candidate_transcript=selected_transcript THEN 0
               ELSE 99
             END"
        }
        _ => {
            "CASE
               WHEN selected_transcript<>'' AND candidate_transcript=selected_transcript THEN 0
               WHEN selected_transcript<>'' AND candidate_transcript<>''
                 AND strpos(candidate_transcript, '.')=0
                 AND split_part(candidate_transcript, '.', 1)
                     = split_part(selected_transcript, '.', 1)
                 AND versioned_candidates=0 AND stable_matches=1 THEN 1
               ELSE 99
             END"
        }
    };
    let success_kind = match mode {
        "gene" => "exact_transcript",
        "feature" => "policy_selected",
        _ => "exact_transcript",
    };
    let rank_one_kind = if mode == "gene" {
        "exact_gene"
    } else {
        "stable_id_match"
    };
    let variants = crate::results::resolved_variants_relation(variants)?;
    let body = format!(
        r#"
        WITH raw AS MATERIALIZED (
          SELECT allele_id, consequence_id,
                 coalesce(string_value, cast(integer_value AS VARCHAR),
                          cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                          json_value) AS candidate_value,
                 CASE
                   WHEN string_value IS NOT NULL THEN 's:' || trim(string_value)
                   WHEN integer_value IS NOT NULL THEN 'i:' || cast(integer_value AS VARCHAR)
                   WHEN number_value IS NOT NULL THEN 'n:' || cast(number_value AS VARCHAR)
                   WHEN boolean_value IS NOT NULL THEN 'b:' || cast(boolean_value AS VARCHAR)
                   WHEN json_value IS NOT NULL THEN 'j:' || trim(json_value)
                 END AS comparison_value
          FROM read_parquet({evidence})
          WHERE scope={scope} AND source_id={source} AND field_path={field_path}
            AND consequence_id IS NOT NULL
            AND coalesce(string_value, cast(integer_value AS VARCHAR),
                         cast(number_value AS VARCHAR), cast(boolean_value AS VARCHAR),
                         json_value) IS NOT NULL
        ), joined AS MATERIALIZED (
          SELECT raw.*,
                 c.ordinal,
                 trim(coalesce(c.transcript_id, '')) AS candidate_transcript,
                 trim(coalesce(c.gene_id, '')) AS candidate_gene,
                 trim(coalesce(v.transcript_id, '')) AS selected_transcript,
                 trim(coalesce(v.gene_id, '')) AS selected_gene
          FROM raw
          JOIN read_parquet({consequences}) c USING (allele_id, consequence_id)
          JOIN {variants} v USING (allele_id)
        ), measured AS (
          SELECT *,
                 count(*) FILTER (
                   WHERE candidate_transcript<>''
                     AND strpos(candidate_transcript, '.')=0
                     AND split_part(candidate_transcript, '.', 1)
                         = split_part(selected_transcript, '.', 1)
                 ) OVER (PARTITION BY allele_id) AS stable_matches,
                 count(*) FILTER (
                   WHERE candidate_transcript<>'' AND strpos(candidate_transcript, '.')>0
                 ) OVER (PARTITION BY allele_id) AS versioned_candidates
          FROM joined
        ), ranked AS (
          SELECT *, {match_rank} AS match_rank
          FROM measured
        ), chosen AS (
          SELECT *, min(match_rank) OVER (PARTITION BY allele_id) AS best_rank
          FROM ranked
        ), aggregated AS (
          SELECT allele_id,
                 min(best_rank) AS best_rank,
                 count(*) FILTER (WHERE match_rank=best_rank) AS reported_value_count,
                 count(DISTINCT comparison_value) FILTER (WHERE match_rank=best_rank)
                   AS distinct_value_count,
                 first(candidate_value ORDER BY ordinal)
                   FILTER (WHERE match_rank=best_rank) AS candidate_value
          FROM chosen
          GROUP BY allele_id
        )
        SELECT allele_id,
               NULL::INTEGER AS selected_index,
               NULL::INTEGER AS source_canonical_index,
               reported_value_count,
               distinct_value_count,
               CASE
                 WHEN best_rank>=99 THEN 'unresolved_feature'
                 WHEN distinct_value_count>1 THEN 'conflicting_selected_values'
                 WHEN best_rank=1 THEN {rank_one_kind}
                 ELSE {success_kind}
               END AS resolution_kind,
               CASE WHEN best_rank<99 AND distinct_value_count=1 THEN candidate_value END
                 AS resolved_string
        FROM aggregated
        "#,
        evidence = sql_literal(&evidence.to_string_lossy()),
        consequences = sql_literal(&consequences.to_string_lossy()),
        variants = variants,
        scope = sql_literal(&field.scope),
        source = sql_literal(&field.source_id),
        field_path = sql_literal(&field.field_path),
        rank_one_kind = sql_literal(rank_one_kind),
        success_kind = sql_literal(success_kind),
    );
    Ok(cache_projection(
        fingerprint,
        &field.source_id,
        &field.field_path,
        "legacy report",
        &body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_consequence() -> Map<String, Value> {
        json!({
            "transcript_id": "ENST2.4",
            "protein_id": "ENSP2.1",
            "amino_acids": "R/H",
            "protein_start": 20,
            "hgvsp": "ENSP2.1:p.Arg20His"
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    fn resolved_field<'a>(
        resolved: &'a ResolvedRecordList,
        field: &str,
    ) -> Option<&'a ResolvedRecordField> {
        resolved
            .fields
            .iter()
            .find(|value| value.field_path == field)
    }

    #[test]
    fn bundled_contract_marks_transcript_vectors_but_not_scalar_scores() {
        assert!(bundled_alignment_group("allele", "dbnsfp", "AlphaMissense_score").is_some());
        assert!(bundled_alignment_group("allele", "dbnsfp", "REVEL_score").is_some());
        assert!(bundled_alignment_group("allele", "dbnsfp", "CADD_phred").is_none());
        assert!(bundled_alignment_group("allele", "dbnsfp", "GERP++_RS").is_none());
    }

    #[test]
    fn aligned_values_require_an_exact_or_safe_unique_transcript() {
        assert_eq!(
            select_aligned_value(
                "allele",
                "dbnsfp",
                "REVEL_score",
                "ENST1;ENST2",
                "0.1;0.8",
                "ENST2.4"
            )
            .as_deref(),
            Some("0.8")
        );
        assert!(
            select_aligned_value(
                "allele",
                "dbnsfp",
                "REVEL_score",
                "ENST1.2;ENST2.4",
                "0.1;0.8",
                "ENST2"
            )
            .is_none()
        );
    }

    #[test]
    fn record_lists_resolve_each_field_by_its_declared_identity() {
        let payload = json!([{
            "Ensembl_transcriptid": "ENST1;ENST2",
            "Ensembl_proteinid": "ENSP1;ENSP2",
            "Uniprot_acc": "U1;U2",
            "Uniprot_entry": "ENTRY1;ENTRY2",
            "aaref": "R",
            "aaalt": "H",
            "aapos": "10;20",
            "HGVSp_VEP": "p.Gly10Arg;p.Arg20His",
            "REVEL_score": "0.1;0.8",
            "Polyphen2_HDIV_score": "0.2;0.95",
            "MutationAssessor_score": "1.0;3.0",
            "SIFT4G_score": "0.4,0.01",
            "AlphaMissense_score": "0.900",
            "CADD_phred": "20",
            "Interpro_domain": "raw-only"
        }]);
        let resolved = resolve_bundled_record_list("dbnsfp", &payload, &selected_consequence())
            .unwrap()
            .unwrap();

        assert_eq!(
            resolved_field(&resolved, "REVEL_score").map(|field| &field.value),
            Some(&Value::String("0.8".into()))
        );
        assert_eq!(
            resolved_field(&resolved, "Polyphen2_HDIV_score").map(|field| &field.value),
            Some(&Value::String("0.95".into()))
        );
        assert_eq!(
            resolved_field(&resolved, "MutationAssessor_score").map(|field| &field.value),
            Some(&Value::String("3.0".into()))
        );
        assert_eq!(
            resolved_field(&resolved, "SIFT4G_score").map(|field| &field.value),
            Some(&Value::String("0.01".into()))
        );
        assert_eq!(
            resolved_field(&resolved, "CADD_phred").map(|field| field.scope),
            Some(ResolvedRecordScope::Allele)
        );
        assert!(resolved_field(&resolved, "Interpro_domain").is_none());
        assert_eq!(resolved.raw_value[0]["sourceRecordOrdinal"], Value::from(0));
    }

    #[test]
    fn dbnsfp_osa2_vectors_follow_each_mrpl39_transcript() {
        let payload = json!([{
            "Ensembl_transcriptid": "ENST00000352957;ENST00000307301;ENST00000419219",
            "Ensembl_proteinid": "ENSP00000284967;ENSP00000305682;ENSP00000404426",
            "aaref": "S",
            "aaalt": "P",
            "aapos": "31;31;31",
            "HGVSp_VEP": "p.Ser31Pro;p.Ser31Pro;p.Ser31Pro",
            "REVEL_score": "0.036;0.036;0.036",
            "AlphaMissense_score": "0.0478;0.0461;0.0456",
            "AlphaMissense_pred": "B;B;B",
            "PrimateAI_score": "0.347287714481"
        }]);

        for (transcript, protein, alpha) in [
            ("ENST00000352957.9", "ENSP00000284967", "0.0478"),
            ("ENST00000307301", "ENSP00000305682", "0.0461"),
            ("ENST00000419219", "ENSP00000404426", "0.0456"),
        ] {
            let selected = json!({
                "transcript_id": transcript,
                "protein_id": protein,
                "amino_acids": "S/P",
                "protein_start": 31,
                "hgvsp": format!("{protein}:p.Ser31Pro")
            })
            .as_object()
            .cloned()
            .unwrap();
            let resolved = resolve_bundled_record_list("dbnsfp", &payload, &selected)
                .unwrap()
                .unwrap();

            assert_eq!(
                resolved_field(&resolved, "REVEL_score").map(|field| &field.value),
                Some(&Value::String("0.036".into()))
            );
            assert_eq!(
                resolved_field(&resolved, "AlphaMissense_score").map(|field| &field.value),
                Some(&Value::String(alpha.into()))
            );
            assert_eq!(
                resolved_field(&resolved, "AlphaMissense_pred").map(|field| &field.value),
                Some(&Value::String("B".into()))
            );
            assert_eq!(
                resolved_field(&resolved, "PrimateAI_score").map(|field| &field.value),
                Some(&Value::String("0.347287714481".into()))
            );
            assert_eq!(resolved.raw_value[0]["sourceRecordOrdinal"], Value::from(0));
        }
    }

    #[test]
    fn record_lists_keep_zero_and_omit_conflicting_selected_values() {
        let payload = json!([
            {
                "Ensembl_transcriptid": "ENST2",
                "Ensembl_proteinid": "ENSP2",
                "aaref": "R",
                "aaalt": "H",
                "aapos": "20",
                "HGVSp_VEP": "p.Arg20His",
                "REVEL_score": "0.1",
                "AlphaMissense_score": "0",
                "CADD_phred": "20"
            },
            {
                "Ensembl_transcriptid": "ENST2",
                "Ensembl_proteinid": "ENSP2",
                "aaref": "R",
                "aaalt": "H",
                "aapos": "20",
                "HGVSp_VEP": "p.Arg20His",
                "REVEL_score": "0.2",
                "AlphaMissense_score": "0.0",
                "CADD_phred": "20.0"
            }
        ]);
        let resolved = resolve_bundled_record_list("dbnsfp", &payload, &selected_consequence())
            .unwrap()
            .unwrap();

        assert!(resolved_field(&resolved, "REVEL_score").is_none());
        assert_eq!(
            resolved_field(&resolved, "AlphaMissense_score").map(|field| &field.value),
            Some(&Value::String("0".into()))
        );
        assert!(resolved_field(&resolved, "CADD_phred").is_some());
    }

    #[test]
    fn record_lists_reject_peptide_mismatches_without_losing_allele_values() {
        let payload = json!([{
            "Ensembl_transcriptid": "ENST2",
            "Ensembl_proteinid": "ENSP2",
            "aaref": "R",
            "aaalt": "Q",
            "aapos": "20",
            "HGVSp_VEP": "p.Arg20Gln",
            "REVEL_score": "0.8",
            "AlphaMissense_score": "0.9",
            "CADD_phred": "25"
        }]);
        let resolved = resolve_bundled_record_list("dbnsfp", &payload, &selected_consequence())
            .unwrap()
            .unwrap();

        assert!(resolved_field(&resolved, "REVEL_score").is_none());
        assert!(resolved_field(&resolved, "AlphaMissense_score").is_none());
        assert_eq!(
            resolved_field(&resolved, "CADD_phred").map(|field| field.scope),
            Some(ResolvedRecordScope::Allele)
        );
    }
}
