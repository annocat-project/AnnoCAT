use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

const MAX_CHROMOSOME_BYTES: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub chromosome: Option<String>,
    pub records: u64,
    pub output_bytes: u64,
    pub valid_output_bytes: u64,
    pub bytes_per_second: f64,
    pub records_per_second: f64,
}

pub struct VcfTail {
    offset: u64,
    valid_offset: u64,
    records: u64,
    chromosome: Option<String>,
    carry: Vec<u8>,
    last_sample: Instant,
    last_bytes: u64,
    last_records: u64,
    bytes_per_second: f64,
    records_per_second: f64,
}

impl Default for VcfTail {
    fn default() -> Self {
        Self {
            offset: 0,
            valid_offset: 0,
            records: 0,
            chromosome: None,
            carry: Vec::new(),
            last_sample: Instant::now(),
            last_bytes: 0,
            last_records: 0,
            bytes_per_second: 0.0,
            records_per_second: 0.0,
        }
    }
}

impl VcfTail {
    pub fn update(&mut self, path: &Path) -> Result<Snapshot, String> {
        let length = std::fs::metadata(path)
            .map_err(|error| format!("cannot measure annotation output: {error}"))?
            .len();
        if length < self.offset {
            *self = Self::default();
        }
        if length > self.offset {
            let mut file = File::open(path)
                .map_err(|error| format!("cannot inspect annotation progress: {error}"))?;
            file.seek(SeekFrom::Start(self.offset))
                .map_err(|error| format!("cannot seek annotation progress: {error}"))?;
            let limit = (length - self.offset).min(8 * 1024 * 1024);
            let mut appended = Vec::with_capacity(limit as usize);
            file.take(limit)
                .read_to_end(&mut appended)
                .map_err(|error| format!("cannot read annotation progress: {error}"))?;
            self.offset = self.offset.saturating_add(appended.len() as u64);
            self.consume(&appended);
        }
        self.sample_rates(self.offset);
        Ok(Snapshot {
            chromosome: self.chromosome.clone(),
            records: self.records,
            output_bytes: self.offset,
            valid_output_bytes: self.valid_offset,
            bytes_per_second: self.bytes_per_second,
            records_per_second: self.records_per_second,
        })
    }

    fn consume(&mut self, appended: &[u8]) {
        self.carry.extend_from_slice(appended);
        let mut start = 0;
        for index in 0..self.carry.len() {
            if self.carry[index] != b'\n' {
                continue;
            }
            let line = &self.carry[start..index];
            self.valid_offset += (index + 1 - start) as u64;
            start = index + 1;
            if line.first() == Some(&b'#') || line.is_empty() {
                continue;
            }
            if let Some(tab) = line.iter().position(|byte| *byte == b'\t') {
                if tab > 0 && tab <= MAX_CHROMOSOME_BYTES && line[..tab].is_ascii() {
                    self.chromosome = Some(String::from_utf8_lossy(&line[..tab]).into_owned());
                }
                self.records = self.records.saturating_add(1);
            }
        }
        if start > 0 {
            self.carry.drain(..start);
        }
    }

    fn sample_rates(&mut self, bytes: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample).as_secs_f64();
        if elapsed < 0.75 {
            return;
        }
        let bytes_rate = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
        let records_rate = self.records.saturating_sub(self.last_records) as f64 / elapsed;
        self.bytes_per_second = smooth(self.bytes_per_second, bytes_rate);
        self.records_per_second = smooth(self.records_per_second, records_rate);
        self.last_sample = now;
        self.last_bytes = bytes;
        self.last_records = self.records;
    }
}

fn smooth(previous: f64, current: f64) -> f64 {
    if previous <= 0.0 {
        current
    } else {
        previous * 0.7 + current * 0.3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn counts_only_complete_vcf_records_and_tracks_chromosome() {
        let root = std::env::temp_dir().join(format!(
            "annocat-progress-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("synthetic.vcf");
        std::fs::write(&path, b"##fileformat=VCFv4.2\n#CHROM\tPOS\n1\t10\n2\t20").unwrap();
        let mut tail = VcfTail::default();
        let first = tail.update(&path).unwrap();
        assert_eq!(first.records, 1);
        assert_eq!(first.chromosome.as_deref(), Some("1"));
        File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let second = tail.update(&path).unwrap();
        assert_eq!(second.records, 2);
        assert_eq!(second.chromosome.as_deref(), Some("2"));
        assert_eq!(second.records, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
