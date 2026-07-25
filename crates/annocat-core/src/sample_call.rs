use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AllelePresence {
    Carried,
    NotCarried,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenotypeRelation {
    Reference,
    OtherAlternate,
    Heterozygous,
    HomozygousAlternate,
    HaploidAlternate,
    MixedAlternate,
    PartiallyCalled,
    NotCalled,
    Unavailable,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhaseState {
    Phased,
    Unphased,
    PartiallyPhased,
    Haploid,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenotypeFilterState {
    Passed,
    Failed,
    NotApplied,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCall {
    pub sample_name: String,
    pub genotype: Option<String>,
    pub allele_presence: AllelePresence,
    pub genotype_relation: GenotypeRelation,
    pub phase: PhaseState,
    pub ploidy: usize,
    pub called_allele_count: usize,
    pub missing_allele_count: usize,
    pub selected_alt_copy_count: usize,
    pub depth: Option<u64>,
    pub genotype_quality: Option<f64>,
    pub genotype_filter: Option<String>,
    pub genotype_filter_state: GenotypeFilterState,
    pub reference_depth: Option<u64>,
    pub selected_alt_depth: Option<u64>,
    pub other_alt_depth: Option<u64>,
    pub selected_alt_fraction: Option<f64>,
    pub allelic_depths_valid: Option<bool>,
}

pub fn parse_sample_call(
    sample_name: impl Into<String>,
    format: Option<&str>,
    value: &str,
    selected_alt_index: usize,
    alternate_count: usize,
) -> SampleCall {
    let sample_name = sample_name.into();
    let fields = format
        .filter(|format| !format.is_empty() && *format != ".")
        .map(|format| format.split(':').zip(value.split(':')).collect::<Vec<_>>())
        .unwrap_or_default();
    let raw_field = |name: &str| {
        fields
            .iter()
            .find_map(|(key, value)| (*key == name).then_some(*value))
    };
    let field = |name: &str| raw_field(name).filter(|value| !value.is_empty() && *value != ".");
    let raw_genotype = raw_field("GT").filter(|value| !value.is_empty());
    let parsed = raw_genotype.map(parse_genotype);
    let (allele_presence, genotype_relation, phase, ploidy, called, missing, copies) =
        genotype_facts(parsed.as_ref(), selected_alt_index, alternate_count);
    let raw_allelic_depths = field("AD");
    let parsed_allelic_depths = raw_allelic_depths.and_then(parse_unsigned_list);
    let expected_depth_count = alternate_count.saturating_add(1);
    let allelic_depths_valid = raw_allelic_depths.map(|_| {
        parsed_allelic_depths
            .as_ref()
            .is_some_and(|depths| depths.len() == expected_depth_count)
    });
    let allelic_depths =
        parsed_allelic_depths.filter(|depths| depths.len() == expected_depth_count);
    let reference_depth = allelic_depths
        .as_ref()
        .and_then(|depths| depths.first().copied().flatten());
    let selected_alt_depth = allelic_depths
        .as_ref()
        .and_then(|depths| depths.get(selected_alt_index).copied().flatten());
    let other_alt_depth = allelic_depths.as_ref().and_then(|depths| {
        depths
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(index, _)| *index != selected_alt_index)
            .try_fold(0_u64, |total, (_, value)| {
                value.map(|value| total.saturating_add(value))
            })
    });
    let selected_alt_fraction = allelic_depths.as_ref().and_then(|depths| {
        let total = depths.iter().try_fold(0_u64, |total, value| {
            value.map(|value| total.saturating_add(value))
        })?;
        let selected = depths.get(selected_alt_index).copied().flatten()?;
        (total > 0).then_some(selected as f64 / total as f64)
    });
    let genotype_filter = field("FT").map(str::to_owned);
    let genotype_filter_state = match genotype_filter.as_deref() {
        Some("PASS") => GenotypeFilterState::Passed,
        Some(_) => GenotypeFilterState::Failed,
        None if format.is_some_and(|format| format.split(':').any(|key| key == "FT")) => {
            GenotypeFilterState::NotApplied
        }
        None => GenotypeFilterState::Unavailable,
    };

    SampleCall {
        sample_name,
        genotype: raw_genotype.map(str::to_owned),
        allele_presence,
        genotype_relation,
        phase,
        ploidy,
        called_allele_count: called,
        missing_allele_count: missing,
        selected_alt_copy_count: copies,
        depth: field("DP").and_then(parse_unsigned),
        genotype_quality: field("GQ").and_then(parse_finite),
        genotype_filter,
        genotype_filter_state,
        reference_depth,
        selected_alt_depth,
        other_alt_depth,
        selected_alt_fraction,
        allelic_depths_valid,
    }
}

#[derive(Debug)]
struct ParsedGenotype {
    alleles: Vec<Option<usize>>,
    separators: Vec<char>,
    valid: bool,
}

fn parse_genotype(raw: &str) -> ParsedGenotype {
    let separators = raw
        .chars()
        .filter(|character| matches!(character, '/' | '|'))
        .collect::<Vec<_>>();
    let raw_alleles = raw.split(['/', '|']).collect::<Vec<_>>();
    let valid = !raw.is_empty()
        && raw_alleles.len() == separators.len().saturating_add(1)
        && raw_alleles.iter().all(|allele| {
            *allele == "."
                || (!allele.is_empty() && allele.bytes().all(|byte| byte.is_ascii_digit()))
        });
    let alleles = raw_alleles
        .into_iter()
        .map(parse_allele)
        .collect::<Vec<_>>();
    ParsedGenotype {
        alleles,
        separators,
        valid,
    }
}

fn parse_allele(value: &str) -> Option<usize> {
    if value == "." || value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

fn genotype_facts(
    parsed: Option<&ParsedGenotype>,
    selected_alt_index: usize,
    alternate_count: usize,
) -> (
    AllelePresence,
    GenotypeRelation,
    PhaseState,
    usize,
    usize,
    usize,
    usize,
) {
    let Some(parsed) = parsed else {
        return (
            AllelePresence::Unknown,
            GenotypeRelation::Unavailable,
            PhaseState::Unknown,
            0,
            0,
            0,
            0,
        );
    };
    let ploidy = parsed.alleles.len();
    let called = parsed.alleles.iter().flatten().count();
    let missing = ploidy.saturating_sub(called);
    let copies = parsed
        .alleles
        .iter()
        .filter(|allele| **allele == Some(selected_alt_index))
        .count();
    if !parsed.valid
        || selected_alt_index == 0
        || selected_alt_index > alternate_count
        || parsed
            .alleles
            .iter()
            .flatten()
            .any(|allele| *allele > alternate_count)
    {
        return (
            AllelePresence::Unknown,
            GenotypeRelation::Invalid,
            PhaseState::Unknown,
            ploidy,
            called,
            missing,
            copies,
        );
    }
    let presence = if copies > 0 {
        AllelePresence::Carried
    } else if called == ploidy && ploidy > 0 {
        AllelePresence::NotCarried
    } else {
        AllelePresence::Unknown
    };
    let phase = if called == 0 {
        PhaseState::Unknown
    } else if ploidy == 1 {
        PhaseState::Haploid
    } else if parsed.separators.iter().all(|separator| *separator == '|') {
        PhaseState::Phased
    } else if parsed.separators.iter().all(|separator| *separator == '/') {
        PhaseState::Unphased
    } else {
        PhaseState::PartiallyPhased
    };
    let relation = if called == 0 {
        GenotypeRelation::NotCalled
    } else if missing > 0 {
        GenotypeRelation::PartiallyCalled
    } else if copies == 0 {
        if parsed.alleles.iter().all(|allele| *allele == Some(0)) {
            GenotypeRelation::Reference
        } else {
            GenotypeRelation::OtherAlternate
        }
    } else if ploidy == 1 {
        GenotypeRelation::HaploidAlternate
    } else if copies == ploidy {
        GenotypeRelation::HomozygousAlternate
    } else if ploidy == 2 {
        GenotypeRelation::Heterozygous
    } else {
        GenotypeRelation::MixedAlternate
    };
    (presence, relation, phase, ploidy, called, missing, copies)
}

fn parse_unsigned(value: &str) -> Option<u64> {
    value.parse().ok()
}

fn parse_finite(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn parse_unsigned_list(value: &str) -> Option<Vec<Option<u64>>> {
    let parsed = value
        .split(',')
        .map(|item| {
            if item == "." {
                Some(None)
            } else {
                item.parse::<u64>().ok().map(Some)
            }
        })
        .collect::<Option<Vec<_>>>()?;
    (!parsed.is_empty()).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_alt_presence_is_specific_to_each_multiallelic_row() {
        let first = parse_sample_call("CASE", Some("GT:AD"), "0/1:20,8,3", 1, 2);
        let second = parse_sample_call("CASE", Some("GT:AD"), "0/1:20,8,3", 2, 2);
        assert_eq!(first.allele_presence, AllelePresence::Carried);
        assert_eq!(first.genotype_relation, GenotypeRelation::Heterozygous);
        assert_eq!(first.selected_alt_depth, Some(8));
        assert_eq!(first.other_alt_depth, Some(3));
        assert_eq!(second.allele_presence, AllelePresence::NotCarried);
        assert_eq!(second.genotype_relation, GenotypeRelation::OtherAlternate);
        assert_eq!(second.selected_alt_depth, Some(3));
        assert_eq!(second.other_alt_depth, Some(8));
    }

    #[test]
    fn one_two_carries_each_alt_once_without_combining_depths() {
        let first = parse_sample_call("CASE", Some("GT:AD"), "1/2:20,8,3", 1, 2);
        let second = parse_sample_call("CASE", Some("GT:AD"), "1/2:20,8,3", 2, 2);
        assert_eq!(first.selected_alt_copy_count, 1);
        assert_eq!(second.selected_alt_copy_count, 1);
        assert_eq!(first.selected_alt_depth, Some(8));
        assert_eq!(second.selected_alt_depth, Some(3));
        assert_eq!(first.selected_alt_fraction, Some(8.0 / 31.0));
        assert_eq!(second.selected_alt_fraction, Some(3.0 / 31.0));
    }

    #[test]
    fn two_two_is_homozygous_only_for_the_second_alt() {
        let first = parse_sample_call("CASE", Some("GT"), "2/2", 1, 2);
        let second = parse_sample_call("CASE", Some("GT"), "2/2", 2, 2);
        assert_eq!(first.allele_presence, AllelePresence::NotCarried);
        assert_eq!(first.genotype_relation, GenotypeRelation::OtherAlternate);
        assert_eq!(
            second.genotype_relation,
            GenotypeRelation::HomozygousAlternate
        );
        assert_eq!(second.selected_alt_copy_count, 2);
    }

    #[test]
    fn missing_haploid_polyploid_and_filters_remain_explicit() {
        let missing = parse_sample_call("CASE", Some("GT:FT"), "./.:.", 1, 1);
        assert_eq!(missing.allele_presence, AllelePresence::Unknown);
        assert_eq!(missing.genotype_relation, GenotypeRelation::NotCalled);
        assert_eq!(
            missing.genotype_filter_state,
            GenotypeFilterState::NotApplied
        );
        let missing_haploid = parse_sample_call("CASE", Some("GT"), ".", 1, 1);
        assert_eq!(
            missing_haploid.genotype_relation,
            GenotypeRelation::NotCalled
        );

        let partial = parse_sample_call("CASE", Some("GT"), "1/.", 1, 1);
        assert_eq!(partial.allele_presence, AllelePresence::Carried);
        assert_eq!(partial.genotype_relation, GenotypeRelation::PartiallyCalled);

        let haploid = parse_sample_call("CASE", Some("GT"), "1", 1, 1);
        assert_eq!(
            haploid.genotype_relation,
            GenotypeRelation::HaploidAlternate
        );
        assert_eq!(haploid.phase, PhaseState::Haploid);

        let triploid = parse_sample_call("CASE", Some("GT:FT"), "0/1/2:LowGQ", 2, 2);
        assert_eq!(triploid.genotype_relation, GenotypeRelation::MixedAlternate);
        assert_eq!(triploid.ploidy, 3);
        assert_eq!(triploid.genotype_filter_state, GenotypeFilterState::Failed);
    }

    #[test]
    fn malformed_genotype_separators_are_not_treated_as_calls() {
        for genotype in ["/1", "0/", "0//1", "0|/1"] {
            let call = parse_sample_call("CASE", Some("GT"), genotype, 1, 1);
            assert_eq!(call.allele_presence, AllelePresence::Unknown);
            assert_eq!(call.genotype_relation, GenotypeRelation::Invalid);
        }
    }

    #[test]
    fn allele_numbers_outside_the_record_alt_count_are_invalid() {
        let call = parse_sample_call("CASE", Some("GT:AD"), "1/3:10,4", 1, 1);
        assert_eq!(call.allele_presence, AllelePresence::Unknown);
        assert_eq!(call.genotype_relation, GenotypeRelation::Invalid);
        assert_eq!(call.allelic_depths_valid, Some(true));
    }

    #[test]
    fn malformed_number_r_allelic_depths_are_not_used() {
        let call = parse_sample_call("CASE", Some("GT:AD"), "0/2:10,4", 2, 2);
        assert_eq!(call.allele_presence, AllelePresence::Carried);
        assert_eq!(call.allelic_depths_valid, Some(false));
        assert_eq!(call.reference_depth, None);
        assert_eq!(call.selected_alt_depth, None);
        assert_eq!(call.selected_alt_fraction, None);
    }
}
