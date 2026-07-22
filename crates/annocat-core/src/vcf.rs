use crate::normalization::{IndexedReference, NormalizeError, canonicalize};
use flate2::read::MultiGzDecoder;
use noodles::vcf;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VcfSummary {
    pub assembly: Option<String>,
    pub samples: Vec<String>,
    pub source_records: u64,
    pub skipped_non_variant_records: u64,
    pub records: u64,
    pub alleles: u64,
    pub snps: u64,
    pub indels: u64,
    pub other_alleles: u64,
    pub multiallelic_records: u64,
    pub identity_sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VcfHeaderSummary {
    pub assembly: Option<String>,
    pub samples: Vec<String>,
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
        assembly: declared_assembly(path)?,
        ..VcfSummary::default()
    };
    let reader =
        open_vcf(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut vcf_reader = vcf::io::Reader::new(reader);
    let header = vcf_reader
        .read_header()
        .map_err(|error| format!("invalid VCF header in {}: {error}", path.display()))?;
    summary.samples = header.sample_names().iter().cloned().collect();
    let mut identity = Sha256::new();

    for result in vcf_reader.records() {
        let record = result.map_err(|error| {
            format!(
                "invalid VCF record {} in {}: {error}",
                summary.source_records + 1,
                path.display()
            )
        })?;
        summary.source_records += 1;
        let chromosome = record.reference_sequence_name();
        let position = record
            .variant_start()
            .transpose()
            .map_err(|error| error.to_string())?
            .ok_or("VCF record has no position")?
            .get() as u64;
        let reference = record.reference_bases();
        let alternate_bases = record.alternate_bases();
        let alt_values: Vec<&str> = alternate_bases.as_ref().split(',').collect();
        if !alt_values
            .iter()
            .any(|alternate| is_variant_alternate(alternate))
        {
            summary.skipped_non_variant_records += 1;
            continue;
        }
        summary.records += 1;
        if alt_values.len() > 1 {
            summary.multiallelic_records += 1;
        }
        for alternate in alt_values {
            update_identity_digest(&mut identity, chromosome, position, reference, alternate);
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
    summary.identity_sha256 = format!("{:x}", identity.finalize());
    Ok(summary)
}

pub fn update_identity_digest(
    digest: &mut Sha256,
    chromosome: &str,
    position: u64,
    reference: &str,
    alternate: &str,
) {
    for value in [
        chromosome.as_bytes(),
        reference.as_bytes(),
        alternate.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    digest.update(position.to_le_bytes());
}

pub fn declared_assembly(path: &Path) -> Result<Option<String>, String> {
    Ok(inspect_header(path)?.assembly)
}

pub fn inspect_header(path: &Path) -> Result<VcfHeaderSummary, String> {
    let reader =
        open_vcf(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut assembly = None;
    for line in reader.lines() {
        let line =
            line.map_err(|error| format!("cannot read VCF header in {}: {error}", path.display()))?;
        if let Some(value) = line.strip_prefix("##reference=") {
            assembly = assembly_name(value).or_else(|| Some(value.to_string()));
        }
        if line.starts_with("##contig=<")
            && let Some(value) = assembly_name(&line)
        {
            assembly = Some(value);
        }
        if line.starts_with("#CHROM\t") {
            let columns = line.split('\t').collect::<Vec<_>>();
            return Ok(VcfHeaderSummary {
                assembly,
                samples: columns
                    .get(9..)
                    .unwrap_or(&[])
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
            });
        }
    }
    Err(format!("VCF header is missing #CHROM in {}", path.display()))
}

pub fn is_variant_alternate(value: &str) -> bool {
    !matches!(value, "" | "." | "<NON_REF>" | "<*>")
}

pub fn has_variant_alternate(value: &str) -> bool {
    value.split(',').any(is_variant_alternate)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_counts_only_records_with_real_alternate_alleles() {
        let root = std::env::temp_dir().join(format!(
            "annocat-core-vcf-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("input.vcf");
        std::fs::write(
            &path,
            b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\t.\t.\tPASS\t.\n1\t11\t.\tC\t<NON_REF>\t.\tPASS\t.\n1\t12\t.\tG\tT\t.\tPASS\t.\n1\t13\t.\tA\tC,<NON_REF>\t.\tPASS\t.\n",
        )
        .unwrap();
        let summary = inspect(&path).unwrap();
        assert_eq!(summary.source_records, 4);
        assert_eq!(summary.skipped_non_variant_records, 2);
        assert_eq!(summary.records, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
