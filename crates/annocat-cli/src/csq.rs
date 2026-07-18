use flate2::read::MultiGzDecoder;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

const CSQ_PREFIX: &str = "##INFO=<ID=CSQ,";
const FORMAT_MARKER: &str = "Format: ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub fields: Vec<String>,
    pub records: u64,
    pub alternate_alleles: u64,
    pub csq_entries: u64,
    pub records_without_csq: u64,
}

pub fn inspect(path: &Path) -> Result<Summary, String> {
    let reader = open(path)?;
    let mut fields = None;
    let mut records = 0_u64;
    let mut alternate_alleles = 0_u64;
    let mut csq_entries = 0_u64;
    let mut records_without_csq = 0_u64;

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if line.starts_with(CSQ_PREFIX) {
            if fields.is_some() {
                return Err("VCF contains more than one CSQ header definition".into());
            }
            fields = Some(parse_header(&line)?);
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let columns = line.split('\t').collect::<Vec<_>>();
        if columns.len() < 8 {
            return Err(format!(
                "VCF record on line {} has fewer than 8 columns",
                index + 1
            ));
        }
        records += 1;
        alternate_alleles += columns[4].split(',').count() as u64;
        let csq = columns[7]
            .split(';')
            .find_map(|item| item.strip_prefix("CSQ="));
        if let Some(value) = csq.filter(|value| !value.is_empty() && *value != ".") {
            csq_entries += value.split(',').count() as u64;
        } else {
            records_without_csq += 1;
        }
    }

    let fields = fields.ok_or("VCF does not define an INFO/CSQ schema")?;
    Ok(Summary {
        fields,
        records,
        alternate_alleles,
        csq_entries,
        records_without_csq,
    })
}

pub(crate) fn parse_header(line: &str) -> Result<Vec<String>, String> {
    let format = line
        .split_once(FORMAT_MARKER)
        .map(|(_, value)| value)
        .ok_or("CSQ header does not contain a Format declaration")?;
    let format = format
        .strip_suffix("\">")
        .or_else(|| format.strip_suffix('>'))
        .unwrap_or(format);
    let fields = format
        .split('|')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if fields.is_empty() || fields.iter().any(String::is_empty) {
        return Err("CSQ header contains an empty field name".into());
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(duplicate) = fields.iter().find(|field| !seen.insert(field.as_str())) {
        return Err(format!("CSQ header contains duplicate field '{duplicate}'"));
    }
    Ok(fields)
}

pub(crate) fn open(path: &Path) -> Result<Box<dyn BufRead>, String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut magic = [0_u8; 2];
    let read = file
        .read(&mut magic)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    drop(file);
    let file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    if read == 2 && magic == [0x1f, 0x8b] {
        Ok(Box::new(BufReader::new(MultiGzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_smoke_output_has_dynamic_csq_schema() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fastvep/expected.vcf");
        let summary = inspect(&path).unwrap();
        assert_eq!(summary.fields.len(), 49);
        assert_eq!(summary.fields.first().unwrap(), "Allele");
        assert_eq!(summary.fields.last().unwrap(), "ACMG_CRITERIA");
        assert_eq!(summary.records, 8);
        assert_eq!(summary.alternate_alleles, 8);
        assert!(summary.csq_entries >= summary.records);
        assert_eq!(summary.records_without_csq, 0);
    }

    #[test]
    fn duplicate_csq_fields_are_rejected() {
        let error = parse_header(
            "##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Allele\">",
        )
        .unwrap_err();
        assert!(error.contains("duplicate"));
    }
}
