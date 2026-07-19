use std::io::{BufRead, BufReader, Read};

fn next_complete_data_line<R: BufRead>(
    input: &mut R,
    decode_context: &str,
) -> Result<Option<String>, String> {
    loop {
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|error| format!("{decode_context}: {error}"))?;
        // Indexed BGZF ranges end at compressed block boundaries. An
        // unterminated tail belongs to the next range, so it is not a record.
        if read == 0 || !line.ends_with('\n') {
            return Ok(None);
        }
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        return Ok(Some(line));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CaddRecord {
    pub(super) position: u64,
    pub(super) reference: String,
    pub(super) alternate: String,
    pub(super) line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpliceAiRecord {
    pub(super) line: String,
}

pub(super) struct SpliceAiReader<R: Read> {
    pub(super) input: BufReader<flate2::read::MultiGzDecoder<R>>,
    pub(super) chromosome: Option<String>,
}

impl<R: Read> SpliceAiReader<R> {
    pub(super) fn next_record(&mut self) -> Result<Option<SpliceAiRecord>, String> {
        loop {
            let Some(line) =
                next_complete_data_line(&mut self.input, "cannot decode SpliceAI VCF")?
            else {
                return Ok(None);
            };
            let fields = line.trim_end().split('\t').collect::<Vec<_>>();
            if fields.len() < 8 {
                return Err("SpliceAI VCF row has fewer than eight columns".into());
            }
            if !fields[7]
                .split(';')
                .any(|field| field.starts_with("SpliceAI="))
            {
                return Err("SpliceAI VCF row is missing its SpliceAI INFO value".into());
            }
            let chromosome = fields[0].strip_prefix("chr").unwrap_or(fields[0]);
            if self
                .chromosome
                .as_deref()
                .is_some_and(|wanted| wanted != chromosome)
            {
                continue;
            }
            match chromosome {
                "X" => 23,
                "Y" => 24,
                "M" | "MT" => 25,
                value => value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| (1..=22).contains(value))
                    .ok_or_else(|| format!("unsupported SpliceAI chromosome '{chromosome}'"))?,
            };
            fields[1]
                .parse::<u64>()
                .map_err(|_| "SpliceAI VCF row has an invalid position".to_string())?;
            return Ok(Some(SpliceAiRecord {
                line: format!("{}\n", line.trim_end()),
            }));
        }
    }
}

pub(super) struct DbsnpReader<R: Read> {
    pub(super) input: BufReader<flate2::read::MultiGzDecoder<R>>,
    pub(super) source_contig: String,
    pub(super) chromosome: String,
}

impl<R: Read> DbsnpReader<R> {
    pub(super) fn next_record(&mut self) -> Result<Option<String>, String> {
        loop {
            let Some(line) =
                next_complete_data_line(&mut self.input, "cannot decode dbSNP BGZF range")?
            else {
                return Ok(None);
            };
            let trimmed = line.trim_end();
            let (contig, remainder) = trimmed
                .split_once('\t')
                .ok_or("dbSNP VCF row has no tab-delimited fields")?;
            if contig != self.source_contig {
                continue;
            }
            if remainder.split('\t').count() < 7 {
                return Err("dbSNP VCF row has fewer than eight columns".into());
            }
            return Ok(Some(format!("{}\t{remainder}\n", self.chromosome)));
        }
    }
}

pub(super) struct CaddChromosomeReader<R: Read> {
    input: BufReader<flate2::read::MultiGzDecoder<R>>,
    chromosome: String,
}

impl<R: Read> CaddChromosomeReader<R> {
    pub(super) fn new(
        mut input: flate2::read::MultiGzDecoder<R>,
        skip: u16,
        chromosome: &str,
    ) -> Result<Self, String> {
        if skip > 0 {
            std::io::copy(&mut input.by_ref().take(skip as u64), &mut std::io::sink())
                .map_err(|error| format!("cannot seek to CADD tabix virtual offset: {error}"))?;
        }
        Ok(Self {
            input: BufReader::new(input),
            chromosome: chromosome.to_string(),
        })
    }

    pub(super) fn next_record(&mut self) -> Result<Option<CaddRecord>, String> {
        loop {
            let Some(line) =
                next_complete_data_line(&mut self.input, "cannot decode CADD BGZF range")?
            else {
                return Ok(None);
            };
            let fields = line.trim_end().split('\t').collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(format!(
                    "CADD row has {} columns instead of 6",
                    fields.len()
                ));
            }
            let row_chromosome = fields[0].strip_prefix("chr").unwrap_or(fields[0]);
            if row_chromosome != self.chromosome {
                continue;
            }
            let position = fields[1]
                .parse::<u64>()
                .map_err(|_| "CADD row has an invalid position".to_string())?;
            return Ok(Some(CaddRecord {
                position,
                reference: fields[2].to_string(),
                alternate: fields[3].to_string(),
                line: format!("{}\n", line.trim_end()),
            }));
        }
    }
}
