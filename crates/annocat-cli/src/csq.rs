use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

const CSQ_PREFIX: &str = "##INFO=<ID=CSQ,";
const FORMAT_MARKER: &str = "Format: ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub fields: Vec<String>,
    pub source_records: u64,
    pub skipped_non_variant_records: u64,
    pub records: u64,
    pub alternate_alleles: u64,
    pub csq_entries: u64,
    pub records_without_csq: u64,
    pub identity_sha256: String,
}

pub fn inspect(path: &Path) -> Result<Summary, String> {
    let reader = open(path)?;
    let mut fields = None;
    let mut source_records = 0_u64;
    let mut skipped_non_variant_records = 0_u64;
    let mut records = 0_u64;
    let mut alternate_alleles = 0_u64;
    let mut csq_entries = 0_u64;
    let mut records_without_csq = 0_u64;
    let mut identity = Sha256::new();

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
        source_records += 1;
        if !annocat_core::vcf::has_variant_alternate(columns[4]) {
            skipped_non_variant_records += 1;
            continue;
        }
        records += 1;
        alternate_alleles += columns[4].split(',').count() as u64;
        let position = columns[1]
            .parse::<u64>()
            .map_err(|_| format!("VCF record on line {} has an invalid position", index + 1))?;
        for alternate in columns[4].split(',') {
            annocat_core::vcf::update_identity_digest(
                &mut identity,
                columns[0],
                position,
                columns[3],
                alternate,
            );
        }
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
        source_records,
        skipped_non_variant_records,
        records,
        alternate_alleles,
        csq_entries,
        records_without_csq,
        identity_sha256: format!("{:x}", identity.finalize()),
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

    #[test]
    fn output_identity_matches_the_core_vcf_inspector() {
        let root = std::env::temp_dir().join(format!("annocat-csq-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("annotated.vcf");
        std::fs::write(
            &path,
            b"##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\tC,G\t.\tPASS\tCSQ=C|missense_variant,G|intron_variant\n2\t20\t.\tTT\tT\t.\tPASS\tCSQ=T|frameshift_variant\n",
        )
        .unwrap();
        let output = inspect(&path).unwrap();
        let input = annocat_core::vcf::inspect(&path).unwrap();
        assert_eq!(output.identity_sha256, input.identity_sha256);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reference_only_records_do_not_require_csq_annotations() {
        let root = std::env::temp_dir().join(format!("annocat-csq-ref-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("annotated.vcf");
        std::fs::write(
            &path,
            b"##fileformat=VCFv4.2\n##INFO=<ID=CSQ,Number=.,Type=String,Description=\"Format: Allele|Consequence\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\t.\t.\tPASS\t.\n1\t11\t.\tC\tT\t.\tPASS\tCSQ=T|missense_variant\n",
        )
        .unwrap();
        let summary = inspect(&path).unwrap();
        assert_eq!(summary.source_records, 2);
        assert_eq!(summary.skipped_non_variant_records, 1);
        assert_eq!(summary.records, 1);
        assert_eq!(summary.records_without_csq, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
