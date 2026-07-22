use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

const BUFFER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    pub records: u64,
    pub valid_bytes: u64,
    pub total_bytes: u64,
    pub chromosome: Option<String>,
    pub identity_sha256: String,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_records: u64,
    pub chromosome: Option<String>,
    pub bytes_per_second: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityMismatch {
    pub record: u64,
    pub input_chromosome: String,
    pub output_chromosome: String,
}

pub fn scan_vcf(path: &Path, mut report: impl FnMut(Progress)) -> Result<ScanSummary, String> {
    let total = std::fs::metadata(path)
        .map_err(|error| format!("cannot measure partial VCF: {error}"))?
        .len();
    let mut reader = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(path)
            .map_err(|error| format!("cannot open partial VCF for recovery: {error}"))?,
    );
    let started = Instant::now();
    let mut buffer = Vec::new();
    let mut bytes = 0_u64;
    let mut valid_bytes = 0_u64;
    let mut records = 0_u64;
    let mut chromosome = None;
    let mut next_report = BUFFER_BYTES as u64;
    let mut identity = Sha256::new();
    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| format!("cannot scan partial VCF: {error}"))?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        let complete = buffer.last() == Some(&b'\n');
        if complete {
            valid_bytes = bytes;
            if buffer.first() != Some(&b'#') && !buffer.is_empty() && vcf_line_is_variant(&buffer)?
            {
                chromosome = Some(update_vcf_identity(&buffer, &mut identity)?);
                records = records.saturating_add(1);
            }
        }
        if bytes >= next_report || bytes == total {
            report(progress(started, bytes, total, records, chromosome.clone()));
            next_report = bytes.saturating_add(BUFFER_BYTES as u64);
        }
    }
    report(progress(started, bytes, total, records, chromosome.clone()));
    Ok(ScanSummary {
        records,
        valid_bytes,
        total_bytes: total,
        chromosome,
        identity_sha256: format!("{:x}", identity.finalize()),
    })
}

pub fn scan_ndjson(path: &Path, mut report: impl FnMut(Progress)) -> Result<ScanSummary, String> {
    let total = std::fs::metadata(path)
        .map_err(|error| format!("cannot measure partial structured output: {error}"))?
        .len();
    let mut reader = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(path)
            .map_err(|error| format!("cannot open partial structured output: {error}"))?,
    );
    let mut buffer = Vec::new();
    let mut bytes = 0_u64;
    let mut valid_bytes = 0_u64;
    let mut records = 0_u64;
    let started = Instant::now();
    let mut next_report = BUFFER_BYTES as u64;
    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| format!("cannot scan partial structured output: {error}"))?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        if buffer.last() == Some(&b'\n') {
            valid_bytes = bytes;
            if buffer.iter().any(|byte| !byte.is_ascii_whitespace())
                && structured_line_is_variant(&buffer)?
            {
                records = records.saturating_add(1);
            }
        }
        if bytes >= next_report || bytes == total {
            report(progress(started, bytes, total, records, None));
            next_report = bytes.saturating_add(BUFFER_BYTES as u64);
        }
    }
    report(progress(started, bytes, total, records, None));
    Ok(ScanSummary {
        records,
        valid_bytes,
        total_bytes: total,
        chromosome: None,
        identity_sha256: String::new(),
    })
}

pub fn vcf_prefix_bytes(path: &Path, records: u64) -> Result<u64, String> {
    prefix_bytes(path, records, PrefixKind::Vcf)
}

pub fn ndjson_prefix_bytes(path: &Path, records: u64) -> Result<u64, String> {
    prefix_bytes(path, records, PrefixKind::Structured)
}

pub fn vcf_prefix_identity_sha256(path: &Path, record_limit: u64) -> Result<String, String> {
    let mut reader = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(path).map_err(|error| format!("cannot open interrupted output: {error}"))?,
    );
    let mut buffer = Vec::new();
    let mut records = 0_u64;
    let mut identity = Sha256::new();
    while records < record_limit {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| format!("cannot verify the recovery boundary: {error}"))?;
        if read == 0 || buffer.last() != Some(&b'\n') {
            break;
        }
        if buffer.first() != Some(&b'#')
            && !buffer.iter().all(u8::is_ascii_whitespace)
            && vcf_line_is_variant(&buffer)?
        {
            update_vcf_identity(&buffer, &mut identity)?;
            records += 1;
        }
    }
    if records != record_limit {
        return Err("interrupted output ended before its verified recovery boundary".into());
    }
    Ok(format!("{:x}", identity.finalize()))
}

#[derive(Clone, Copy)]
enum PrefixKind {
    Vcf,
    Structured,
}

fn prefix_bytes(path: &Path, record_limit: u64, kind: PrefixKind) -> Result<u64, String> {
    let mut reader = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(path).map_err(|error| format!("cannot open interrupted output: {error}"))?,
    );
    let mut buffer = Vec::new();
    let mut bytes = 0_u64;
    let mut records = 0_u64;
    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| format!("cannot find the common recovery boundary: {error}"))?;
        if read == 0 || buffer.last() != Some(&b'\n') {
            break;
        }
        let header = matches!(kind, PrefixKind::Vcf) && buffer.first() == Some(&b'#');
        if !header {
            if records == record_limit {
                break;
            }
            let complete_record = !buffer.iter().all(u8::is_ascii_whitespace)
                && match kind {
                    PrefixKind::Vcf => vcf_line_is_variant(&buffer)?,
                    PrefixKind::Structured => structured_line_is_variant(&buffer)?,
                };
            if complete_record {
                records = records.saturating_add(1);
            }
        }
        bytes = bytes.saturating_add(read as u64);
        if !header && records == record_limit {
            break;
        }
    }
    if records != record_limit {
        return Err("interrupted output ended before the common recovery boundary".into());
    }
    Ok(bytes)
}

fn update_vcf_identity(line: &[u8], digest: &mut Sha256) -> Result<String, String> {
    let columns = line
        .split(|byte| *byte == b'\t')
        .take(5)
        .collect::<Vec<_>>();
    if columns.len() < 5 {
        return Err("annotated VCF contains a row with fewer than five columns".into());
    }
    let chromosome =
        std::str::from_utf8(columns[0]).map_err(|_| "annotated VCF chromosome is not UTF-8")?;
    let position = std::str::from_utf8(columns[1])
        .map_err(|_| "annotated VCF position is not UTF-8")?
        .parse::<u64>()
        .map_err(|_| "annotated VCF position is invalid")?;
    let reference = std::str::from_utf8(columns[3])
        .map_err(|_| "annotated VCF reference allele is not UTF-8")?;
    let alternates = std::str::from_utf8(columns[4])
        .map_err(|_| "annotated VCF alternate allele is not UTF-8")?
        .trim_end_matches(&['\r', '\n'][..]);
    for alternate in alternates.split(',') {
        annocat_core::vcf::update_identity_digest(
            digest, chromosome, position, reference, alternate,
        );
    }
    Ok(chromosome.to_owned())
}

fn vcf_line_is_variant(line: &[u8]) -> Result<bool, String> {
    let alternate = line
        .split(|byte| *byte == b'\t')
        .nth(4)
        .ok_or("VCF row has fewer than five columns")?;
    let alternate = std::str::from_utf8(alternate)
        .map_err(|_| "VCF alternate allele is not UTF-8")?
        .trim_end_matches(&['\r', '\n'][..]);
    Ok(annocat_core::vcf::has_variant_alternate(alternate))
}

fn structured_line_is_variant(line: &[u8]) -> Result<bool, String> {
    let record: serde_json::Value = serde_json::from_slice(line)
        .map_err(|error| format!("invalid structured recovery record: {error}"))?;
    let allele_string = record
        .get("allele_string")
        .and_then(serde_json::Value::as_str)
        .ok_or("structured recovery record has no allele_string")?;
    Ok(allele_string
        .split('/')
        .skip(1)
        .any(annocat_core::vcf::is_variant_alternate))
}

pub fn first_vcf_identity_mismatch(
    input: &Path,
    output: &Path,
    record_limit: u64,
    mut report: impl FnMut(u64, Option<String>, f64),
) -> Result<Option<IdentityMismatch>, String> {
    let mut input = annocat_core::vcf::open_vcf(input)
        .map_err(|error| format!("cannot open original VCF for comparison: {error}"))?;
    let mut output = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(output)
            .map_err(|error| format!("cannot open annotated VCF for comparison: {error}"))?,
    );
    let mut input_line = Vec::new();
    let mut output_line = Vec::new();
    let started = Instant::now();
    let mut record = 0_u64;
    while record < record_limit {
        let input_record = next_vcf_record(&mut input, &mut input_line, "original")?;
        let output_record = next_vcf_record(&mut output, &mut output_line, "annotated")?;
        let (Some(input_record), Some(output_record)) = (input_record, output_record) else {
            return Err("one VCF ended before the verified comparison boundary".into());
        };
        record += 1;
        let input_columns = identity_columns(input_record, "original")?;
        let output_columns = identity_columns(output_record, "annotated")?;
        if input_columns != output_columns {
            return Ok(Some(IdentityMismatch {
                record,
                input_chromosome: String::from_utf8_lossy(input_columns[0]).into_owned(),
                output_chromosome: String::from_utf8_lossy(output_columns[0]).into_owned(),
            }));
        }
        if record.is_multiple_of(10_000) || record == record_limit {
            let elapsed = started.elapsed().as_secs_f64();
            report(
                record,
                Some(String::from_utf8_lossy(input_columns[0]).into_owned()),
                if elapsed > 0.0 {
                    record as f64 / elapsed
                } else {
                    0.0
                },
            );
        }
    }
    Ok(None)
}

fn next_vcf_record<'a>(
    reader: &mut dyn BufRead,
    buffer: &'a mut Vec<u8>,
    label: &str,
) -> Result<Option<&'a [u8]>, String> {
    loop {
        buffer.clear();
        if reader
            .read_until(b'\n', buffer)
            .map_err(|error| format!("cannot compare {label} VCF: {error}"))?
            == 0
        {
            return Ok(None);
        }
        if buffer.first() != Some(&b'#')
            && !buffer.iter().all(u8::is_ascii_whitespace)
            && vcf_line_is_variant(buffer)?
        {
            return Ok(Some(buffer));
        }
    }
}

fn identity_columns<'a>(line: &'a [u8], label: &str) -> Result<[&'a [u8]; 4], String> {
    let columns = line
        .split(|byte| *byte == b'\t')
        .take(5)
        .collect::<Vec<_>>();
    if columns.len() < 5 {
        return Err(format!(
            "{label} VCF contains a row with fewer than five columns"
        ));
    }
    Ok([
        columns[0],
        columns[1],
        columns[3],
        columns[4].trim_ascii_end(),
    ])
}

pub fn copy_prefix(
    source: &Path,
    bytes: u64,
    destination: &Path,
    mut report: impl FnMut(Progress),
) -> Result<(), String> {
    let input =
        File::open(source).map_err(|error| format!("cannot open recovered prefix: {error}"))?;
    let output = File::create(destination)
        .map_err(|error| format!("cannot create recovered output: {error}"))?;
    let started = Instant::now();
    let mut reader = BufReader::with_capacity(BUFFER_BYTES, input.take(bytes));
    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, output);
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("cannot copy recovered prefix: {error}"))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot copy recovered prefix: {error}"))?;
        copied = copied.saturating_add(read as u64);
        report(progress(started, copied, bytes, 0, None));
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish recovered prefix: {error}"))
}

pub fn append_vcf_records(source: &Path, destination: &Path) -> Result<(), String> {
    let mut reader = BufReader::with_capacity(
        BUFFER_BYTES,
        File::open(source).map_err(|error| format!("cannot open continuation VCF: {error}"))?,
    );
    let mut writer = BufWriter::with_capacity(
        BUFFER_BYTES,
        OpenOptions::new()
            .append(true)
            .open(destination)
            .map_err(|error| format!("cannot append recovered VCF: {error}"))?,
    );
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if buffer.first() != Some(&b'#') {
            writer
                .write_all(&buffer)
                .map_err(|error| error.to_string())?;
        }
    }
    writer.flush().map_err(|error| error.to_string())
}

pub fn append_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut input =
        File::open(source).map_err(|error| format!("cannot open continuation output: {error}"))?;
    let mut output = OpenOptions::new()
        .append(true)
        .open(destination)
        .map_err(|error| format!("cannot append continuation output: {error}"))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|error| format!("cannot join continuation output: {error}"))?;
    Ok(())
}

fn progress(
    started: Instant,
    completed_bytes: u64,
    total_bytes: u64,
    completed_records: u64,
    chromosome: Option<String>,
) -> Progress {
    let elapsed = started.elapsed().as_secs_f64();
    Progress {
        completed_bytes,
        total_bytes,
        completed_records,
        chromosome,
        bytes_per_second: if elapsed > 0.0 {
            completed_bytes as f64 / elapsed
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_ignores_an_unterminated_private_record() {
        let root = std::env::temp_dir().join(format!("annocat-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("partial.vcf");
        std::fs::write(
            &path,
            b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\tG\t.\tPASS\t.\n2\t20\t.\tC\tT\t.\tPASS\t.",
        )
        .unwrap();
        let summary = scan_vcf(&path, |_| {}).unwrap();
        assert_eq!(summary.records, 1);
        assert_eq!(summary.chromosome.as_deref(), Some("1"));
        assert!(summary.valid_bytes < summary.total_bytes);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn common_prefix_boundaries_keep_vcf_headers_and_matching_records() {
        let root = std::env::temp_dir().join(format!("annocat-prefix-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let vcf = root.join("partial.vcf");
        let ndjson = root.join("partial.ndjson");
        std::fs::write(
            &vcf,
            b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\tG\n2\t20\t.\tC\tT\n",
        )
        .unwrap();
        std::fs::write(
            &ndjson,
            b"{\"allele_string\":\"A/G\"}\n{\"allele_string\":\"C/T\"}\n",
        )
        .unwrap();
        assert_eq!(
            vcf_prefix_bytes(&vcf, 1).unwrap(),
            b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\tG\n".len() as u64
        );
        assert_eq!(
            ndjson_prefix_bytes(&ndjson, 1).unwrap(),
            b"{\"allele_string\":\"A/G\"}\n".len() as u64
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_identity_matches_the_core_vcf_scan() {
        let root = std::env::temp_dir().join(format!("annocat-identity-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let vcf = root.join("identity.vcf");
        std::fs::write(
            &vcf,
            b"##fileformat=VCFv4.2\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\tC,G\t.\tPASS\t.\n2\t20\t.\tTT\tT\t.\tPASS\t.\n",
        )
        .unwrap();
        let recovered = scan_vcf(&vcf, |_| {}).unwrap();
        let original = annocat_core::vcf::inspect(&vcf).unwrap();
        assert_eq!(recovered.records, original.records);
        assert_eq!(recovered.identity_sha256, original.identity_sha256);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_boundaries_count_only_real_variant_records() {
        let root = std::env::temp_dir().join(format!(
            "annocat-recovery-projection-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let vcf = root.join("partial.vcf");
        let ndjson = root.join("partial.ndjson");
        let vcf_bytes = b"#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\t.\n1\t11\t.\tC\tT\n1\t12\t.\tG\t<NON_REF>\n1\t13\t.\tA\tC\n";
        let ndjson_bytes = b"{\"allele_string\":\"A\"}\n{\"allele_string\":\"C/T\"}\n{\"allele_string\":\"G/<NON_REF>\"}\n{\"allele_string\":\"A/C\"}\n";
        std::fs::write(&vcf, vcf_bytes).unwrap();
        std::fs::write(&ndjson, ndjson_bytes).unwrap();
        assert_eq!(scan_vcf(&vcf, |_| {}).unwrap().records, 2);
        assert_eq!(scan_ndjson(&ndjson, |_| {}).unwrap().records, 2);
        assert_eq!(
            vcf_prefix_bytes(&vcf, 1).unwrap(),
            b"#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\t.\n1\t11\t.\tC\tT\n".len() as u64
        );
        assert_eq!(
            ndjson_prefix_bytes(&ndjson, 1).unwrap(),
            b"{\"allele_string\":\"A\"}\n{\"allele_string\":\"C/T\"}\n".len() as u64
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatch_locator_reports_only_record_and_chromosomes() {
        let root = std::env::temp_dir().join(format!("annocat-mismatch-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("input.vcf");
        let output = root.join("output.vcf");
        std::fs::write(
            &input,
            b"#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\tG\n2\t20\t.\tC\tT\n",
        )
        .unwrap();
        std::fs::write(
            &output,
            b"#CHROM\tPOS\tID\tREF\tALT\n1\t10\t.\tA\tG\n3\t20\t.\tC\tT\n",
        )
        .unwrap();
        let mismatch = first_vcf_identity_mismatch(&input, &output, 2, |_, _, _| {})
            .unwrap()
            .unwrap();
        assert_eq!(mismatch.record, 2);
        assert_eq!(mismatch.input_chromosome, "2");
        assert_eq!(mismatch.output_chromosome, "3");
        std::fs::remove_dir_all(root).unwrap();
    }
}
