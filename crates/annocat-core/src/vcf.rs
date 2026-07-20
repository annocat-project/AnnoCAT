use crate::normalization::{IndexedReference, NormalizeError, canonicalize};
use flate2::read::MultiGzDecoder;
use noodles::vcf;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VcfSummary {
    pub assembly: Option<String>,
    pub samples: Vec<String>,
    pub records: u64,
    pub alleles: u64,
    pub snps: u64,
    pub indels: u64,
    pub other_alleles: u64,
    pub multiallelic_records: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizationSummary {
    pub records_scanned: u64,
    pub alleles_scanned: u64,
    pub canonicalized: u64,
    pub changed: u64,
    pub reference_mismatches: u64,
    pub unsupported: u64,
    pub examples: Vec<String>,
}

pub fn check_normalization(
    path: &Path,
    fasta: &Path,
    chromosome: Option<&str>,
    limit: Option<u64>,
) -> Result<NormalizationSummary, String> {
    let reader =
        open_vcf(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut vcf_reader = vcf::io::Reader::new(reader);
    let _header = vcf_reader
        .read_header()
        .map_err(|error| format!("invalid VCF header: {error}"))?;
    let mut reference = IndexedReference::open(fasta).map_err(|error| error.to_string())?;
    let wanted = chromosome.map(|value| value.strip_prefix("chr").unwrap_or(value));
    let mut summary = NormalizationSummary::default();
    for result in vcf_reader.records() {
        let record = result.map_err(|error| format!("invalid VCF record: {error}"))?;
        let record_chromosome = record.reference_sequence_name();
        let bare = record_chromosome
            .strip_prefix("chr")
            .unwrap_or(record_chromosome);
        if wanted.is_some_and(|value| value != bare) {
            continue;
        }
        summary.records_scanned += 1;
        let position = record
            .variant_start()
            .transpose()
            .map_err(|error| error.to_string())?
            .ok_or("VCF record has no position")?
            .get() as u64;
        let reference_bases = record.reference_bases();
        for alternate in record.alternate_bases().as_ref().split(',') {
            if limit.is_some_and(|value| summary.alleles_scanned >= value) {
                return Ok(summary);
            }
            summary.alleles_scanned += 1;
            match canonicalize(
                &mut reference,
                record_chromosome,
                position,
                reference_bases,
                alternate,
            ) {
                Ok(canonical) => {
                    summary.canonicalized += 1;
                    if canonical.position != position
                        || canonical.reference != reference_bases
                        || canonical.alternate != alternate
                    {
                        summary.changed += 1;
                        if summary.examples.len() < 20 {
                            summary.examples.push(format!(
                                "{bare}:{position}:{reference_bases}>{alternate} -> {}:{}>{}",
                                canonical.position, canonical.reference, canonical.alternate
                            ));
                        }
                    }
                }
                Err(NormalizeError::ReferenceMismatch { .. }) => summary.reference_mismatches += 1,
                Err(NormalizeError::InvalidAllele(_)) => summary.unsupported += 1,
                Err(error) => {
                    return Err(format!(
                        "normalization failed at {record_chromosome}:{position}: {error}"
                    ));
                }
            }
        }
    }
    Ok(summary)
}

pub fn open_vcf(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    let compressed = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "gz" | "bgz"));
    let reader: Box<dyn Read> = if compressed {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    Ok(Box::new(BufReader::with_capacity(1024 * 1024, reader)))
}

pub fn inspect(path: &Path) -> Result<VcfSummary, String> {
    let mut summary = VcfSummary {
        assembly: read_declared_assembly(path)?,
        ..VcfSummary::default()
    };
    let reader =
        open_vcf(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut vcf_reader = vcf::io::Reader::new(reader);
    let header = vcf_reader
        .read_header()
        .map_err(|error| format!("invalid VCF header in {}: {error}", path.display()))?;
    summary.samples = header.sample_names().iter().cloned().collect();

    for result in vcf_reader.records() {
        let record = result.map_err(|error| {
            format!(
                "invalid VCF record {} in {}: {error}",
                summary.records + 1,
                path.display()
            )
        })?;
        summary.records += 1;
        let reference = record.reference_bases();
        let alternate_bases = record.alternate_bases();
        let alt_values: Vec<&str> = alternate_bases.as_ref().split(',').collect();
        if alt_values.len() > 1 {
            summary.multiallelic_records += 1;
        }
        for alternate in alt_values {
            summary.alleles += 1;
            if reference.len() == 1
                && alternate.len() == 1
                && is_sequence(reference)
                && is_sequence(alternate)
            {
                summary.snps += 1;
            } else if is_sequence(reference) && is_sequence(alternate) {
                summary.indels += 1;
            } else {
                summary.other_alleles += 1;
            }
        }
    }
    Ok(summary)
}

fn read_declared_assembly(path: &Path) -> Result<Option<String>, String> {
    let reader =
        open_vcf(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    for line in reader.lines() {
        let line =
            line.map_err(|error| format!("cannot read VCF header in {}: {error}", path.display()))?;
        if let Some(value) = line.strip_prefix("##reference=") {
            return Ok(assembly_name(value).or_else(|| Some(value.to_string())));
        }
        if line.starts_with("##contig=<")
            && let Some(assembly) = assembly_name(&line)
        {
            return Ok(Some(assembly));
        }
        if line.starts_with("#CHROM") {
            break;
        }
    }
    Ok(None)
}

fn is_sequence(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|base| matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T' | b'N'))
}

fn assembly_name(value: &str) -> Option<String> {
    let uppercase = value.to_ascii_uppercase();
    if uppercase.contains("GRCH38") || uppercase.contains("HG38") {
        Some("GRCh38".into())
    } else if uppercase.contains("GRCH37") || uppercase.contains("HG19") {
        Some("GRCh37".into())
    } else {
        None
    }
}
