//! Reference-aware VCF allele normalization following Algorithm 1 from
//! Tan et al., Bioinformatics 31(13), 2015. This module is an independent
//! implementation of the published algorithm.

use noodles::{
    core::{Position, Region},
    fasta,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalAllele {
    pub chromosome: String,
    pub position: u64,
    pub reference: String,
    pub alternate: String,
}

pub fn canonical_chromosome(chromosome: &str) -> String {
    match chromosome.strip_prefix("chr").unwrap_or(chromosome) {
        "M" => "MT".into(),
        value => value.into(),
    }
}

pub trait ReferenceSequence {
    fn base(&mut self, chromosome: &str, position: u64) -> Result<u8, NormalizeError>;
    fn sequence(
        &mut self,
        chromosome: &str,
        position: u64,
        length: usize,
    ) -> Result<Vec<u8>, NormalizeError>;
}

pub struct IndexedReference {
    path: PathBuf,
    cache: Option<(String, Vec<u8>)>,
}

impl IndexedReference {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NormalizeError> {
        let path = path.as_ref().to_path_buf();
        std::fs::File::open(&path).map_err(|error| {
            NormalizeError::Reference(format!("cannot open {}: {error}", path.display()))
        })?;
        let fai = PathBuf::from(format!("{}.fai", path.display()));
        std::fs::File::open(&fai).map_err(|error| {
            NormalizeError::Reference(format!("cannot open {}: {error}", fai.display()))
        })?;
        Ok(Self { path, cache: None })
    }

    fn load_chromosome(&mut self, chromosome: &str) -> Result<&[u8], NormalizeError> {
        if self
            .cache
            .as_ref()
            .is_some_and(|(name, _)| name == chromosome)
        {
            return Ok(&self.cache.as_ref().expect("checked cache").1);
        }
        let aliases = chromosome_aliases(chromosome);
        let mut indexed = fasta::io::indexed_reader::Builder::default()
            .build_from_path(&self.path)
            .map_err(|error| {
                NormalizeError::Reference(format!("cannot open indexed FASTA: {error}"))
            })?;
        for alias in aliases {
            let region = Region::new(alias.as_bytes(), Position::MIN..=Position::MAX);
            if let Ok(record) = indexed.query(&region) {
                let mut sequence = record.sequence().as_ref().to_vec();
                sequence.make_ascii_uppercase();
                self.cache = Some((chromosome.to_string(), sequence));
                return Ok(&self.cache.as_ref().expect("just populated cache").1);
            }
        }
        Err(NormalizeError::MissingChromosome(chromosome.into()))
    }
}

impl ReferenceSequence for IndexedReference {
    fn base(&mut self, chromosome: &str, position: u64) -> Result<u8, NormalizeError> {
        self.sequence(chromosome, position, 1)
            .map(|sequence| sequence[0])
    }

    fn sequence(
        &mut self,
        chromosome: &str,
        position: u64,
        length: usize,
    ) -> Result<Vec<u8>, NormalizeError> {
        if position == 0 {
            return Err(NormalizeError::InvalidPosition);
        }
        let sequence = self.load_chromosome(chromosome)?;
        let start = usize::try_from(position - 1).map_err(|_| NormalizeError::InvalidPosition)?;
        sequence
            .get(start..start.saturating_add(length))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| NormalizeError::ReferenceOutOfRange {
                chromosome: chromosome.into(),
                position,
            })
    }
}

fn chromosome_aliases(chromosome: &str) -> Vec<String> {
    let bare = chromosome.strip_prefix("chr").unwrap_or(chromosome);
    let mut aliases = vec![
        chromosome.to_string(),
        bare.to_string(),
        format!("chr{bare}"),
    ];
    if bare == "MT" {
        aliases.push("chrM".into());
        aliases.push("M".into());
    }
    aliases.dedup();
    aliases
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeError {
    InvalidPosition,
    InvalidAllele(String),
    MissingChromosome(String),
    ReferenceOutOfRange { chromosome: String, position: u64 },
    ReferenceMismatch { expected: String, observed: String },
    Reference(String),
}

impl std::fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPosition => write!(f, "VCF positions must be one-based"),
            Self::InvalidAllele(value) => write!(f, "invalid small-variant allele: {value}"),
            Self::MissingChromosome(value) => write!(f, "reference chromosome is missing: {value}"),
            Self::ReferenceOutOfRange {
                chromosome,
                position,
            } => write!(
                f,
                "reference position is out of range: {chromosome}:{position}"
            ),
            Self::ReferenceMismatch { expected, observed } => write!(
                f,
                "VCF REF does not match the reference: expected {expected}, observed {observed}"
            ),
            Self::Reference(value) => f.write_str(value),
        }
    }
}

impl std::error::Error for NormalizeError {}

pub fn canonicalize<R: ReferenceSequence>(
    reference_source: &mut R,
    chromosome: &str,
    position: u64,
    reference: &str,
    alternate: &str,
) -> Result<CanonicalAllele, NormalizeError> {
    if position == 0 {
        return Err(NormalizeError::InvalidPosition);
    }
    let chromosome = canonical_chromosome(chromosome);
    let mut ref_bases = allele_bytes(reference)?;
    let mut alt_bases = allele_bytes(alternate)?;
    let observed = reference_source.sequence(&chromosome, position, ref_bases.len())?;
    if !reference_bases_are_compatible(&ref_bases, &observed) {
        return Err(NormalizeError::ReferenceMismatch {
            expected: String::from_utf8_lossy(&observed).into_owned(),
            observed: reference.to_ascii_uppercase(),
        });
    }
    ref_bases.make_ascii_uppercase();
    alt_bases.make_ascii_uppercase();

    // Ambiguous bases cannot be shifted safely. Preserve their VCF identity after
    // confirming that the declared REF is compatible with the installed reference.
    if ref_bases
        .iter()
        .chain(&alt_bases)
        .chain(&observed)
        .any(|base| !matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T'))
    {
        return Ok(CanonicalAllele {
            chromosome,
            position,
            reference: String::from_utf8(ref_bases).expect("validated ASCII allele"),
            alternate: String::from_utf8(alt_bases).expect("validated ASCII allele"),
        });
    }
    let mut position = position;

    loop {
        let same_suffix = ref_bases.last() == alt_bases.last();
        if !same_suffix {
            break;
        }
        ref_bases.pop();
        alt_bases.pop();
        if ref_bases.is_empty() || alt_bases.is_empty() {
            if position == 1 {
                return Err(NormalizeError::ReferenceOutOfRange {
                    chromosome: chromosome.clone(),
                    position: 0,
                });
            }
            position -= 1;
            let preceding = reference_source
                .base(&chromosome, position)?
                .to_ascii_uppercase();
            ref_bases.insert(0, preceding);
            alt_bases.insert(0, preceding);
        }
    }

    while ref_bases.len() >= 2 && alt_bases.len() >= 2 && ref_bases[0] == alt_bases[0] {
        ref_bases.remove(0);
        alt_bases.remove(0);
        position += 1;
    }

    Ok(CanonicalAllele {
        chromosome,
        position,
        reference: String::from_utf8(ref_bases).expect("validated ASCII allele"),
        alternate: String::from_utf8(alt_bases).expect("validated ASCII allele"),
    })
}

fn reference_bases_are_compatible(declared: &[u8], observed: &[u8]) -> bool {
    declared.len() == observed.len()
        && declared.iter().zip(observed).all(|(&declared, &observed)| {
            iupac_mask(declared)
                .zip(iupac_mask(observed))
                .is_some_and(|(declared, observed)| declared & observed != 0)
        })
}

fn iupac_mask(base: u8) -> Option<u8> {
    match base.to_ascii_uppercase() {
        b'A' => Some(0b0001),
        b'C' => Some(0b0010),
        b'G' => Some(0b0100),
        b'T' => Some(0b1000),
        b'R' => Some(0b0101),
        b'Y' => Some(0b1010),
        b'S' => Some(0b0110),
        b'W' => Some(0b1001),
        b'K' => Some(0b1100),
        b'M' => Some(0b0011),
        b'B' => Some(0b1110),
        b'D' => Some(0b1101),
        b'H' => Some(0b1011),
        b'V' => Some(0b0111),
        b'N' => Some(0b1111),
        _ => None,
    }
}

fn allele_bytes(value: &str) -> Result<Vec<u8>, NormalizeError> {
    if value.is_empty()
        || value == "."
        || value.starts_with('<')
        || value.contains('[')
        || value.contains(']')
        || !value
            .bytes()
            .all(|base| matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T' | b'N'))
    {
        return Err(NormalizeError::InvalidAllele(value.into()));
    }
    Ok(value.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemoryReference {
        chromosome: &'static str,
        bases: &'static [u8],
    }

    impl ReferenceSequence for MemoryReference {
        fn base(&mut self, chromosome: &str, position: u64) -> Result<u8, NormalizeError> {
            self.sequence(chromosome, position, 1).map(|value| value[0])
        }

        fn sequence(
            &mut self,
            chromosome: &str,
            position: u64,
            length: usize,
        ) -> Result<Vec<u8>, NormalizeError> {
            if chromosome != self.chromosome {
                return Err(NormalizeError::MissingChromosome(chromosome.into()));
            }
            let start =
                usize::try_from(position - 1).map_err(|_| NormalizeError::InvalidPosition)?;
            self.bases
                .get(start..start + length)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| NormalizeError::ReferenceOutOfRange {
                    chromosome: chromosome.into(),
                    position,
                })
        }
    }

    #[test]
    fn snv_is_unchanged_and_chr_prefix_is_removed() {
        let mut genome = MemoryReference {
            chromosome: "1",
            bases: b"AACCGGTT",
        };
        let allele = canonicalize(&mut genome, "chr1", 3, "C", "T").unwrap();
        assert_eq!(
            allele,
            CanonicalAllele {
                chromosome: "1".into(),
                position: 3,
                reference: "C".into(),
                alternate: "T".into()
            }
        );
    }

    #[test]
    fn deletion_is_shifted_to_leftmost_repeat_position() {
        let mut genome = MemoryReference {
            chromosome: "1",
            bases: b"CAAAAG",
        };
        let allele = canonicalize(&mut genome, "1", 4, "AA", "A").unwrap();
        assert_eq!(
            (
                allele.position,
                allele.reference.as_str(),
                allele.alternate.as_str()
            ),
            (1, "CA", "C")
        );
    }

    #[test]
    fn rejects_reference_mismatch() {
        let mut genome = MemoryReference {
            chromosome: "1",
            bases: b"ACGT",
        };
        assert!(matches!(
            canonicalize(&mut genome, "1", 2, "G", "A"),
            Err(NormalizeError::ReferenceMismatch { .. })
        ));
    }

    #[test]
    fn accepts_compatible_ambiguous_reference_without_shifting() {
        let mut genome = MemoryReference {
            chromosome: "3",
            bases: b"ATGBGCA",
        };
        let allele = canonicalize(&mut genome, "3", 2, "TGNGC", "T").unwrap();
        assert_eq!(
            allele,
            CanonicalAllele {
                chromosome: "3".into(),
                position: 2,
                reference: "TGNGC".into(),
                alternate: "T".into(),
            }
        );
    }

    #[test]
    fn rejects_incompatible_iupac_reference_base() {
        let mut genome = MemoryReference {
            chromosome: "1",
            bases: b"AB",
        };
        assert!(matches!(
            canonicalize(&mut genome, "1", 2, "A", "T"),
            Err(NormalizeError::ReferenceMismatch { .. })
        ));
    }

    #[test]
    fn canonicalization_is_idempotent() {
        let mut genome = MemoryReference {
            chromosome: "1",
            bases: b"CAAAAG",
        };
        let first = canonicalize(&mut genome, "1", 4, "AA", "A").unwrap();
        let second = canonicalize(
            &mut genome,
            &first.chromosome,
            first.position,
            &first.reference,
            &first.alternate,
        )
        .unwrap();
        assert_eq!(first, second);
    }
}
