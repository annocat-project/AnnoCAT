use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const BUFFER_BYTES: usize = 1024 * 1024;
const REPORT_BYTES: u64 = 8 * 1024 * 1024;

pub fn validate_declared_assembly(assembly: Option<&str>) -> Result<(), String> {
    match assembly {
        None | Some("GRCh38") => Ok(()),
        Some("GRCh37") => Err(
            "AnnoCAT does not support GRCh37, b37, or hg19 inputs in this release; select a GRCh38 VCF"
                .into(),
        ),
        Some(assembly) => Err(format!(
            "AnnoCAT supports GRCh38 inputs only; this file declares {assembly}"
        )),
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_records: u64,
    pub chromosome: Option<String>,
    pub bytes_per_second: f64,
    pub records_per_second: f64,
}

#[derive(Debug, Clone)]
pub struct ProjectedStream {
    pub summary: annocat_core::vcf::VcfSummary,
    pub skipped_identity_sha256: String,
    pub written_records: u64,
}

struct CountingReader<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

pub fn stream_variants(
    path: &Path,
    output: impl Write,
    cancelled: impl Fn() -> bool,
    report: impl FnMut(Progress),
) -> Result<annocat_core::vcf::VcfSummary, String> {
    Ok(stream_variants_after(path, output, 0, cancelled, report)?.summary)
}

pub fn stream_variants_after(
    path: &Path,
    mut output: impl Write,
    skip_records: u64,
    cancelled: impl Fn() -> bool,
    mut report: impl FnMut(Progress),
) -> Result<ProjectedStream, String> {
    let total_bytes = std::fs::metadata(path)
        .map_err(|error| format!("cannot measure input VCF: {error}"))?
        .len();
    let assembly = annocat_core::vcf::declared_assembly(path)?;
    validate_declared_assembly(assembly.as_deref())?;
    let compressed = is_gzip(path)?;
    let compressed_bytes = Arc::new(AtomicU64::new(0));
    let file = File::open(path).map_err(|error| format!("cannot open input VCF: {error}"))?;
    let counted = CountingReader {
        inner: file,
        bytes: Arc::clone(&compressed_bytes),
    };
    let input: Box<dyn Read> = if compressed {
        Box::new(MultiGzDecoder::new(counted))
    } else {
        Box::new(counted)
    };
    let mut reader = BufReader::with_capacity(BUFFER_BYTES, input);
    let mut summary = annocat_core::vcf::VcfSummary {
        assembly,
        ..Default::default()
    };
    let mut identity = Sha256::new();
    let mut skipped_identity = Sha256::new();
    let mut line = Vec::new();
    let mut saw_columns = false;
    let started = Instant::now();
    let mut previous_at = started;
    let mut previous_bytes = 0_u64;
    let mut previous_records = 0_u64;
    let mut next_report = REPORT_BYTES;
    let mut chromosome = None;

    loop {
        if cancelled() {
            return Err("cancelled".into());
        }
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("cannot read input VCF: {error}"))?;
        if read == 0 {
            break;
        }
        if line.first() == Some(&b'#') {
            if line.starts_with(b"#CHROM\t") {
                saw_columns = true;
                let columns = line.split(|byte| *byte == b'\t').collect::<Vec<_>>();
                if columns.len() > 9 {
                    summary.samples = columns[9..]
                        .iter()
                        .map(|value| {
                            String::from_utf8_lossy(value)
                                .trim_end_matches(&['\r', '\n'][..])
                                .to_owned()
                        })
                        .collect();
                }
            }
            write_line(&mut output, &line, "VCF header")?;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        summary.source_records += 1;
        let columns = line
            .split(|byte| *byte == b'\t')
            .take(6)
            .collect::<Vec<_>>();
        if columns.len() < 5 {
            return Err(format!(
                "input VCF record {} has fewer than five columns",
                summary.source_records
            ));
        }
        let alternate_field = std::str::from_utf8(columns[4])
            .map_err(|_| "input VCF alternate allele is not UTF-8")?
            .trim_end_matches(&['\r', '\n'][..]);
        if !annocat_core::vcf::has_variant_alternate(alternate_field) {
            summary.skipped_non_variant_records += 1;
            continue;
        }
        let chromosome_value =
            std::str::from_utf8(columns[0]).map_err(|_| "input VCF chromosome is not UTF-8")?;
        let position = std::str::from_utf8(columns[1])
            .map_err(|_| "input VCF position is not UTF-8")?
            .parse::<u64>()
            .map_err(|_| {
                format!(
                    "input VCF record {} has an invalid position",
                    summary.source_records
                )
            })?;
        let reference = std::str::from_utf8(columns[3])
            .map_err(|_| "input VCF reference allele is not UTF-8")?;
        let skipped = summary.records < skip_records;
        let real_alternates = alternate_field
            .split(',')
            .filter(|alternate| annocat_core::vcf::is_variant_alternate(alternate))
            .collect::<Vec<_>>();
        for alternate in &real_alternates {
            annocat_core::vcf::update_identity_digest(
                &mut identity,
                chromosome_value,
                position,
                reference,
                alternate,
            );
            if skipped {
                annocat_core::vcf::update_identity_digest(
                    &mut skipped_identity,
                    chromosome_value,
                    position,
                    reference,
                    alternate,
                );
            }
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
        if real_alternates.len() > 1 {
            summary.multiallelic_records += 1;
        }
        summary.records += 1;
        chromosome = Some(chromosome_value.to_owned());
        if !skipped {
            write_line(&mut output, &line, "VCF record")?;
        }

        let bytes = compressed_bytes.load(Ordering::Relaxed).min(total_bytes);
        if bytes >= next_report {
            let now = Instant::now();
            let elapsed = now.duration_since(previous_at).as_secs_f64();
            report(Progress {
                completed_bytes: bytes,
                total_bytes,
                completed_records: summary.records,
                chromosome: chromosome.clone(),
                bytes_per_second: rate(bytes.saturating_sub(previous_bytes), elapsed),
                records_per_second: rate(summary.records.saturating_sub(previous_records), elapsed),
            });
            previous_at = now;
            previous_bytes = bytes;
            previous_records = summary.records;
            next_report = bytes.saturating_add(REPORT_BYTES);
        }
    }
    output
        .flush()
        .map_err(|error| format!("cannot finish fastVEP input: {error}"))?;
    if !saw_columns {
        return Err("input VCF has no #CHROM header".into());
    }
    if summary.records < skip_records {
        return Err("interrupted output contains more variants than the original input".into());
    }
    summary.identity_sha256 = format!("{:x}", identity.finalize());
    let bytes = compressed_bytes.load(Ordering::Relaxed).min(total_bytes);
    let elapsed = started.elapsed().as_secs_f64();
    report(Progress {
        completed_bytes: bytes,
        total_bytes,
        completed_records: summary.records,
        chromosome,
        bytes_per_second: rate(bytes, elapsed),
        records_per_second: rate(summary.records, elapsed),
    });
    let written_records = summary.records - skip_records;
    Ok(ProjectedStream {
        summary,
        skipped_identity_sha256: format!("{:x}", skipped_identity.finalize()),
        written_records,
    })
}

fn is_gzip(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| format!("cannot open input VCF: {error}"))?;
    let mut magic = [0_u8; 2];
    let read = file
        .read(&mut magic)
        .map_err(|error| format!("cannot inspect input VCF: {error}"))?;
    Ok(read == 2 && magic == [0x1f, 0x8b])
}

fn write_line(output: &mut impl Write, line: &[u8], label: &str) -> Result<(), String> {
    output
        .write_all(line)
        .map_err(|error| format!("cannot stream {label} to fastVEP: {error}"))?;
    if line.last() != Some(&b'\n') {
        output
            .write_all(b"\n")
            .map_err(|error| format!("cannot stream {label} to fastVEP: {error}"))?;
    }
    Ok(())
}

fn rate(value: u64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        value as f64 / seconds
    } else {
        0.0
    }
}

fn is_sequence(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|base| matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T' | b'N'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn streams_only_records_with_real_alternate_alleles_from_gzip() {
        let root = std::env::temp_dir().join(format!(
            "annocat-annotation-input-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("input.vcf.gz");
        let mut encoder = GzEncoder::new(File::create(&input).unwrap(), Compression::fast());
        encoder
            .write_all(
                b"##fileformat=VCFv4.2\n##reference=GRCh38\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\t.\t.\tPASS\t.\n1\t11\t.\tC\t<NON_REF>\t.\tPASS\t.\n1\t12\t.\tG\tT\t.\tPASS\t.\n1\t13\t.\tA\tC,<NON_REF>\t.\tPASS\t.\n1\t14\t.\tA\t*\t.\tPASS\t.\n",
            )
            .unwrap();
        encoder.finish().unwrap();
        let mut output = Vec::new();
        let summary = stream_variants(&input, &mut output, || false, |_| {}).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(summary.source_records, 5);
        assert_eq!(summary.skipped_non_variant_records, 3);
        assert_eq!(summary.records, 2);
        assert_eq!(summary.alleles, 2);
        assert_eq!(summary.multiallelic_records, 0);
        assert!(!output.contains("\t10\t"));
        assert!(!output.contains("\t11\t"));
        assert!(output.contains("\t12\t"));
        assert!(output.contains("\t13\t"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
