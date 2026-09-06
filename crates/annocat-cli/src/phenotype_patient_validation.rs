use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidationManifest {
    schema_version: u16,
    source: ValidationSource,
    ranking_algorithm_version: String,
    selection: Value,
    cases: Vec<ValidationCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidationSource {
    name: String,
    repository: String,
    release: String,
    commit: String,
    phenopacket_file_count: usize,
    phenopacket_tree_hash_algorithm: String,
    phenopacket_tree_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidationCase {
    id: String,
    relative_path: String,
    sha256: String,
    publication_ids: Vec<String>,
    disease_ids: Vec<String>,
    target_hgnc_id: String,
    target_symbol: String,
    positive_hpo_ids: Vec<String>,
    excluded_hpo_ids: Vec<String>,
    #[serde(rename = "sentinel")]
    challenge_case: bool,
    #[serde(rename = "maximumWorstTieRank")]
    historical_maximum_worst_tie_rank: Option<usize>,
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("config/phenotype-patient-validation.json")
}

fn load_manifest() -> ValidationManifest {
    let bytes = fs::read(manifest_path()).expect("cannot read patient validation manifest");
    let manifest: ValidationManifest =
        serde_json::from_slice(&bytes).expect("patient validation manifest is invalid");
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(
        manifest.ranking_algorithm_version,
        PHENOTYPE_RANKING_ALGORITHM_VERSION
    );
    assert_eq!(manifest.source.name, "Phenopacket Store");
    assert_eq!(
        manifest.source.repository,
        "https://github.com/monarch-initiative/phenopacket-store.git"
    );
    assert_eq!(manifest.source.release, "0.1.27");
    assert_eq!(
        manifest.source.commit,
        "3f3619800b2c949f8bfb457a122346c2fae7e482"
    );
    assert_eq!(
        manifest.source.phenopacket_tree_hash_algorithm,
        "sha256(relative-path + NUL + lowercase-file-sha256 + LF)-v1"
    );
    assert!(manifest.selection.is_object());
    manifest
}

fn collect_phenopackets(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot inspect {}: {error}", directory.display()))
    {
        let entry = entry.expect("cannot inspect Phenopacket Store entry");
        let path = entry.path();
        let kind = entry
            .file_type()
            .expect("cannot inspect Phenopacket Store entry type");
        if kind.is_dir() {
            collect_phenopackets(&path, files);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "phenopackets")
        {
            files.push(path);
        }
    }
}

fn normalized_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("Phenopacket path must be inside the source root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn verify_snapshot(root: &Path, source: &ValidationSource) {
    let mut files = Vec::new();
    collect_phenopackets(&root.join("notebooks"), &mut files);
    files.sort_by_key(|path| normalized_relative_path(root, path));
    assert_eq!(files.len(), source.phenopacket_file_count);

    let mut tree = Sha256::new();
    for path in files {
        let relative = normalized_relative_path(root, &path);
        let sha256 = crate::fastvep::sha256_file(&path).expect("cannot hash phenopacket");
        tree.update(relative.as_bytes());
        tree.update([0]);
        tree.update(sha256.as_bytes());
        tree.update(b"\n");
    }
    assert_eq!(
        format!("{:x}", tree.finalize()),
        source.phenopacket_tree_sha256
    );
}

fn sorted_unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn case_terms(document: &Value, excluded: bool) -> Vec<String> {
    sorted_unique(
        document["phenotypicFeatures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|feature| feature["excluded"].as_bool().unwrap_or(false) == excluded)
            .filter_map(|feature| feature["type"]["id"].as_str())
            .filter(|id| id.starts_with("HP:"))
            .map(str::to_owned),
    )
}

fn solved_interpretations(document: &Value) -> impl Iterator<Item = &Value> {
    document["interpretations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|interpretation| interpretation["progressStatus"] == "SOLVED")
}

fn case_diseases(document: &Value) -> Vec<String> {
    sorted_unique(
        solved_interpretations(document)
            .filter_map(|interpretation| interpretation["diagnosis"]["disease"]["id"].as_str())
            .map(str::to_owned),
    )
}

fn case_genes(document: &Value) -> Vec<(String, String)> {
    solved_interpretations(document)
        .flat_map(|interpretation| {
            interpretation["diagnosis"]["genomicInterpretations"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter(|interpretation| interpretation["interpretationStatus"] == "CAUSATIVE")
        .filter_map(|interpretation| {
            let context =
                &interpretation["variantInterpretation"]["variationDescriptor"]["geneContext"];
            Some((
                context["valueId"].as_str()?.to_owned(),
                context["symbol"].as_str()?.to_owned(),
            ))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn case_publications(document: &Value) -> Vec<String> {
    sorted_unique(
        document["metaData"]["externalReferences"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|reference| reference["id"].as_str())
            .filter(|id| id.starts_with("PMID:"))
            .map(str::to_owned),
    )
}

fn read_case(root: &Path, expected: &ValidationCase) -> Value {
    assert!(!expected.relative_path.contains('\\'));
    assert!(!expected.relative_path.split('/').any(|part| part == ".."));
    let path = root.join(&expected.relative_path);
    assert_eq!(
        crate::fastvep::sha256_file(&path).expect("cannot hash validation case"),
        expected.sha256
    );
    let document: Value = serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
    assert_eq!(document["id"], expected.id);
    assert_eq!(case_terms(&document, false), expected.positive_hpo_ids);
    assert_eq!(case_terms(&document, true), expected.excluded_hpo_ids);
    assert_eq!(case_diseases(&document), expected.disease_ids);
    assert_eq!(case_publications(&document), expected.publication_ids);
    assert_eq!(
        case_genes(&document),
        vec![(
            expected.target_hgnc_id.clone(),
            expected.target_symbol.clone()
        )]
    );
    document
}

fn query_indexes(knowledge: &HpoKnowledge, case: &ValidationCase) -> Vec<usize> {
    let excluded = case.excluded_hpo_ids.iter().collect::<BTreeSet<_>>();
    case.positive_hpo_ids
        .iter()
        .map(|id| {
            assert!(!excluded.contains(id), "{id} is both positive and excluded");
            let index = *knowledge
                .term_index
                .get(id)
                .unwrap_or_else(|| panic!("{} contains unknown HPO term {id}", case.id));
            assert!(
                knowledge.active_terms.binary_search(&index).is_ok(),
                "{} contains inactive HPO term {id}",
                case.id
            );
            index
        })
        .collect()
}

fn target_key(resolver: &crate::gene_identity::Resolver, case: &ValidationCase) -> String {
    let gene = resolver
        .resolve_pair(&case.target_hgnc_id, &case.target_symbol)
        .resolved()
        .unwrap_or_else(|| panic!("{} target does not resolve uniquely", case.id));
    assert_eq!(
        gene.canonical_gene_id.as_deref(),
        Some(case.target_hgnc_id.as_str())
    );
    assert_eq!(gene.symbol, case.target_symbol);
    assert_eq!(gene.identity_status, "hgnc");
    gene.comparison_key()
}

fn ranking_fingerprint(ranking: &PhenotypeRanking) -> String {
    let mut genes = ranking.genes.iter().collect::<Vec<_>>();
    genes.sort_by(|left, right| left.1.rank.cmp(&right.1.rank).then(left.0.cmp(right.0)));
    genes
        .into_iter()
        .take(20)
        .map(|(gene, rank)| format!("{gene}:{}:{}", rank.rank, rank.tie_count))
        .collect::<Vec<_>>()
        .join("|")
}

#[derive(Debug)]
struct RankedValidationCase {
    id: String,
    target_key: String,
    target_hgnc_id: String,
    target_symbol: String,
    disease_id: String,
    publication_id: String,
    query: Vec<usize>,
    denominator: usize,
    target_rank: Option<usize>,
    target_tie_count: Option<usize>,
    target_worst_tie_rank: Option<usize>,
    target_raw_score: Option<f64>,
    target_score_key: Option<u64>,
    target_best_disease_id: Option<String>,
    gene_worst_tie_ranks: BTreeMap<String, usize>,
    runtime_millis: u128,
}

impl RankedValidationCase {
    fn report(&self) -> Value {
        json!({
            "caseId": self.id,
            "targetHgncId": self.target_hgnc_id,
            "targetSymbol": self.target_symbol,
            "diseaseId": self.disease_id,
            "publicationId": self.publication_id,
            "positiveHpoTermCount": self.query.len(),
            "covered": self.target_rank.is_some(),
            "displayedCompetitionRank": self.target_rank,
            "tieCount": self.target_tie_count,
            "worstTieRank": self.target_worst_tie_rank,
            "rawResnikScore": self.target_raw_score,
            "scoreKey": self.target_score_key,
            "bestDiseaseId": self.target_best_disease_id,
            "eligibleGeneDenominator": self.denominator,
            "runtimeMillis": self.runtime_millis,
        })
    }
}

fn rank_validation_case(
    phenopacket_root: &Path,
    knowledge: &HpoKnowledge,
    resolver: &crate::gene_identity::Resolver,
    case: &ValidationCase,
) -> RankedValidationCase {
    let _document = read_case(phenopacket_root, case);
    let query = query_indexes(knowledge, case);
    let target_key = target_key(resolver, case);
    let started = Instant::now();
    let ranking = build_phenotype_ranking(knowledge, resolver, &query, "2026-06-23".into());
    let runtime_millis = started.elapsed().as_millis();
    let target = ranking.genes.get(&target_key);
    RankedValidationCase {
        id: case.id.clone(),
        target_key,
        target_hgnc_id: case.target_hgnc_id.clone(),
        target_symbol: case.target_symbol.clone(),
        disease_id: case.disease_ids[0].clone(),
        publication_id: case.publication_ids[0].clone(),
        query,
        denominator: ranking.denominator,
        target_rank: target.map(|rank| rank.rank),
        target_tie_count: target.map(|rank| rank.tie_count),
        target_worst_tie_rank: target.map(|rank| rank.rank + rank.tie_count - 1),
        target_raw_score: target.map(|rank| rank.raw_score),
        target_score_key: target.map(|rank| rank.score_key),
        target_best_disease_id: target.map(|rank| rank.best_disease_id.clone()),
        gene_worst_tie_ranks: ranking
            .genes
            .into_iter()
            .map(|(gene, rank)| (gene, rank.rank + rank.tie_count - 1))
            .collect(),
        runtime_millis,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricSummary {
    count: usize,
    covered: usize,
    coverage: f64,
    top_1: f64,
    top_3: f64,
    top_5: f64,
    top_10: f64,
    top_20: f64,
    mean_reciprocal_rank: f64,
    median_evaluation_rank: f64,
}

fn metric_summary(ranks: &[Option<usize>], missing_rank: usize) -> MetricSummary {
    assert!(!ranks.is_empty());
    let count = ranks.len();
    let covered = ranks.iter().filter(|rank| rank.is_some()).count();
    let proportion = |limit| {
        ranks
            .iter()
            .filter(|rank| rank.is_some_and(|rank| rank <= limit))
            .count() as f64
            / count as f64
    };
    let mean_reciprocal_rank = ranks
        .iter()
        .map(|rank| rank.map_or(0.0, |rank| 1.0 / rank as f64))
        .sum::<f64>()
        / count as f64;
    let mut ordered = ranks
        .iter()
        .map(|rank| rank.unwrap_or(missing_rank))
        .collect::<Vec<_>>();
    ordered.sort_unstable();
    let middle = ordered.len() / 2;
    let median_evaluation_rank = if ordered.len() % 2 == 0 {
        (ordered[middle - 1] + ordered[middle]) as f64 / 2.0
    } else {
        ordered[middle] as f64
    };
    MetricSummary {
        count,
        covered,
        coverage: covered as f64 / count as f64,
        top_1: proportion(1),
        top_3: proportion(3),
        top_5: proportion(5),
        top_10: proportion(10),
        top_20: proportion(20),
        mean_reciprocal_rank,
        median_evaluation_rank,
    }
}

fn balanced_summary(
    cases: &[RankedValidationCase],
    group: impl Fn(&RankedValidationCase) -> &str,
) -> Value {
    let mut groups = BTreeMap::<String, Vec<Option<usize>>>::new();
    for case in cases {
        groups
            .entry(group(case).to_owned())
            .or_default()
            .push(case.target_worst_tie_rank);
    }
    let group_values = groups
        .iter()
        .map(|(id, ranks)| {
            let reciprocal_rank = ranks
                .iter()
                .map(|rank| rank.map_or(0.0, |rank| 1.0 / rank as f64))
                .sum::<f64>()
                / ranks.len() as f64;
            json!({
                "id": id,
                "caseCount": ranks.len(),
                "top1": ranks.iter().filter(|rank| rank.is_some_and(|rank| rank <= 1)).count() as f64 / ranks.len() as f64,
                "top3": ranks.iter().filter(|rank| rank.is_some_and(|rank| rank <= 3)).count() as f64 / ranks.len() as f64,
                "top5": ranks.iter().filter(|rank| rank.is_some_and(|rank| rank <= 5)).count() as f64 / ranks.len() as f64,
                "top10": ranks.iter().filter(|rank| rank.is_some_and(|rank| rank <= 10)).count() as f64 / ranks.len() as f64,
                "top20": ranks.iter().filter(|rank| rank.is_some_and(|rank| rank <= 20)).count() as f64 / ranks.len() as f64,
                "meanReciprocalRank": reciprocal_rank,
            })
        })
        .collect::<Vec<_>>();
    let average = |field: &str| {
        group_values
            .iter()
            .filter_map(|group| group[field].as_f64())
            .sum::<f64>()
            / group_values.len() as f64
    };
    json!({
        "groupCount": group_values.len(),
        "top1": average("top1"),
        "top3": average("top3"),
        "top5": average("top5"),
        "top10": average("top10"),
        "top20": average("top20"),
        "meanReciprocalRank": average("meanReciprocalRank"),
        "groups": group_values,
    })
}

#[derive(Debug)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(2_685_821_657_736_338_717)
    }

    fn index(&mut self, limit: usize) -> usize {
        (self.next_u64() % limit as u64) as usize
    }

    fn shuffle(&mut self, values: &mut [usize]) {
        for index in (1..values.len()).rev() {
            values.swap(index, self.index(index + 1));
        }
    }
}

fn target_label_null(cases: &[RankedValidationCase], replicates: usize, seed: u64) -> Value {
    let mut targets = cases
        .iter()
        .map(|case| case.target_key.clone())
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    assert_eq!(
        targets.len(),
        cases.len(),
        "cohort targets must be HGNC-distinct"
    );
    assert!(targets.len() >= 7, "at least seven targets are required");
    let target_indexes = targets
        .iter()
        .enumerate()
        .map(|(index, target)| (target.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut rng = DeterministicRng::new(seed);
    let mut accepted = BTreeSet::new();
    let mut assignments = Sha256::new();
    let mut null_values = Vec::with_capacity(replicates);
    while null_values.len() < replicates {
        let mut permutation = (0..targets.len()).collect::<Vec<_>>();
        rng.shuffle(&mut permutation);
        if permutation
            .iter()
            .enumerate()
            .any(|(index, mapped)| index == *mapped)
            || !accepted.insert(permutation.clone())
        {
            continue;
        }
        for mapped in &permutation {
            assignments.update((*mapped as u64).to_le_bytes());
        }
        let value = cases
            .iter()
            .map(|case| {
                let original = target_indexes[&case.target_key];
                let mapped = &targets[permutation[original]];
                case.gene_worst_tie_ranks
                    .get(mapped)
                    .map_or(0.0, |rank| 1.0 / *rank as f64)
            })
            .sum::<f64>()
            / cases.len() as f64;
        null_values.push(value);
    }
    null_values.sort_by(f64::total_cmp);
    let percentile_95 = null_values[(replicates * 95 / 100).saturating_sub(1)];
    let observed = cases
        .iter()
        .map(|case| {
            case.target_worst_tie_rank
                .map_or(0.0, |rank| 1.0 / rank as f64)
        })
        .sum::<f64>()
        / cases.len() as f64;
    json!({
        "algorithm": "xorshift64star-fisher-yates-derangement-v1",
        "seed": seed,
        "replicates": replicates,
        "orderedTargetHgncIds": targets,
        "assignmentStreamSha256": format!("{:x}", assignments.finalize()),
        "statistic": "gene-balanced mean reciprocal worst-tie rank",
        "observed": observed,
        "null95thPercentile": percentile_95,
        "passed": observed > percentile_95,
    })
}

fn percentile_interval(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let count = values.len();
    let lower = (count * 25 / 1000).saturating_sub(1);
    let upper = (count * 975 / 1000).saturating_sub(1).min(count - 1);
    json!({
        "lower": values[lower],
        "upper": values[upper],
        "lowerOrderedPosition": lower + 1,
        "upperOrderedPosition": upper + 1,
    })
}

fn clustered_bootstrap(
    cases: &[RankedValidationCase],
    group: impl Fn(&RankedValidationCase) -> &str,
    replicates: usize,
    seed: u64,
) -> Value {
    assert!(replicates >= 40);
    let mut grouped = BTreeMap::<String, Vec<usize>>::new();
    for (index, case) in cases.iter().enumerate() {
        grouped
            .entry(group(case).to_owned())
            .or_default()
            .push(index);
    }
    let ordered_cluster_ids = grouped.keys().cloned().collect::<Vec<_>>();
    let clusters = grouped.into_values().collect::<Vec<_>>();
    let missing_rank = cases[0].denominator + 1;
    let mut rng = DeterministicRng::new(seed);
    let mut assignments = Sha256::new();
    let mut coverage = Vec::with_capacity(replicates);
    let mut top_1 = Vec::with_capacity(replicates);
    let mut top_3 = Vec::with_capacity(replicates);
    let mut top_5 = Vec::with_capacity(replicates);
    let mut top_10 = Vec::with_capacity(replicates);
    let mut top_20 = Vec::with_capacity(replicates);
    let mut mrr = Vec::with_capacity(replicates);
    let mut median = Vec::with_capacity(replicates);
    for _ in 0..replicates {
        let mut sampled = Vec::new();
        for _ in 0..clusters.len() {
            let cluster = rng.index(clusters.len());
            assignments.update((cluster as u64).to_le_bytes());
            sampled.extend(
                clusters[cluster]
                    .iter()
                    .map(|index| cases[*index].target_worst_tie_rank),
            );
        }
        let metrics = metric_summary(&sampled, missing_rank);
        coverage.push(metrics.coverage);
        top_1.push(metrics.top_1);
        top_3.push(metrics.top_3);
        top_5.push(metrics.top_5);
        top_10.push(metrics.top_10);
        top_20.push(metrics.top_20);
        mrr.push(metrics.mean_reciprocal_rank);
        median.push(metrics.median_evaluation_rank);
    }
    json!({
        "algorithm": "xorshift64star-cluster-bootstrap-v1",
        "seed": seed,
        "replicates": replicates,
        "orderedClusterIds": ordered_cluster_ids,
        "assignmentStreamSha256": format!("{:x}", assignments.finalize()),
        "intervals": {
            "coverage": percentile_interval(coverage),
            "top1": percentile_interval(top_1),
            "top3": percentile_interval(top_3),
            "top5": percentile_interval(top_5),
            "top10": percentile_interval(top_10),
            "top20": percentile_interval(top_20),
            "meanReciprocalRank": percentile_interval(mrr),
            "medianEvaluationRank": percentile_interval(median),
        },
    })
}

fn rank_for_query(
    knowledge: &HpoKnowledge,
    resolver: &crate::gene_identity::Resolver,
    target_key: &str,
    query: &[usize],
) -> Option<usize> {
    build_phenotype_ranking(knowledge, resolver, query, "2026-06-23".into())
        .genes
        .get(target_key)
        .map(|rank| rank.rank + rank.tie_count - 1)
}

fn unrelated_noise_term(
    knowledge: &HpoKnowledge,
    supported: &[usize],
    query: &[usize],
    rng: &mut DeterministicRng,
) -> usize {
    let start = rng.index(supported.len());
    for offset in 0..supported.len() {
        let candidate = supported[(start + offset) % supported.len()];
        if query.contains(&candidate) {
            continue;
        }
        let related = query.iter().any(|term| {
            knowledge.terms[candidate].ancestors.iter().any(|ancestor| {
                *ancestor != knowledge.phenotypic_abnormality_root
                    && !knowledge.terms[*ancestor].parents.is_empty()
                    && knowledge.terms[*term].ancestors.contains(ancestor)
            })
        });
        if !related {
            return candidate;
        }
    }
    panic!("no unrelated supported HPO noise term is available")
}

fn robustness_report(
    cases: &[RankedValidationCase],
    knowledge: &HpoKnowledge,
    resolver: &crate::gene_identity::Resolver,
    seed: u64,
    dropout_fraction: f64,
    noise_term_count: usize,
) -> Value {
    assert!((0.0..1.0).contains(&dropout_fraction));
    assert_eq!(noise_term_count, 1);
    let mut supported = knowledge
        .diseases
        .iter()
        .flat_map(|disease| disease.positive.iter().copied())
        .collect::<Vec<_>>();
    supported.sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
    supported.dedup();
    let mut rng = DeterministicRng::new(seed);
    let mut dropout_ranks = Vec::with_capacity(cases.len());
    let mut ancestor_ranks = Vec::with_capacity(cases.len());
    let mut noise_ranks = Vec::with_capacity(cases.len());
    let mut records = Vec::with_capacity(cases.len());
    for case in cases {
        let mut positions = (0..case.query.len()).collect::<Vec<_>>();
        rng.shuffle(&mut positions);
        let remove_count = ((case.query.len() as f64 * dropout_fraction).ceil() as usize)
            .max(1)
            .min(case.query.len() - 1);
        let removed = positions
            .into_iter()
            .take(remove_count)
            .collect::<BTreeSet<_>>();
        let dropout = case
            .query
            .iter()
            .enumerate()
            .filter_map(|(index, term)| (!removed.contains(&index)).then_some(*term))
            .collect::<Vec<_>>();

        let mut ancestor = case
            .query
            .iter()
            .map(|term| {
                knowledge.terms[*term]
                    .parents
                    .iter()
                    .filter(|parent| knowledge.active_terms.binary_search(parent).is_ok())
                    .min_by_key(|parent| &knowledge.terms[**parent].id)
                    .copied()
                    .unwrap_or(*term)
            })
            .collect::<Vec<_>>();
        ancestor.sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
        ancestor.dedup();

        let noise_term = unrelated_noise_term(knowledge, &supported, &case.query, &mut rng);
        let mut noise = case.query.clone();
        noise.push(noise_term);
        noise.sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
        noise.dedup();

        let dropout_rank = rank_for_query(knowledge, resolver, &case.target_key, &dropout);
        let ancestor_rank = rank_for_query(knowledge, resolver, &case.target_key, &ancestor);
        let noise_rank = rank_for_query(knowledge, resolver, &case.target_key, &noise);
        dropout_ranks.push(dropout_rank);
        ancestor_ranks.push(ancestor_rank);
        noise_ranks.push(noise_rank);
        let ids = |query: &[usize]| {
            query
                .iter()
                .map(|term| knowledge.terms[*term].id.clone())
                .collect::<Vec<_>>()
        };
        records.push(json!({
            "caseId": case.id,
            "dropout": {"positiveHpoIds": ids(&dropout), "targetWorstTieRank": dropout_rank},
            "lessSpecific": {"positiveHpoIds": ids(&ancestor), "targetWorstTieRank": ancestor_rank},
            "unrelatedNoise": {"positiveHpoIds": ids(&noise), "addedHpoId": knowledge.terms[noise_term].id, "targetWorstTieRank": noise_rank},
        }));
    }
    let missing_rank = cases[0].denominator + 1;
    json!({
        "randomizationAlgorithm": "xorshift64star-v1",
        "seed": seed,
        "dropoutFraction": dropout_fraction,
        "noiseTermCount": noise_term_count,
        "lessSpecificRule": "replace each term by the lexically first active direct HPO parent; retain the term when no active direct parent exists",
        "unrelatedNoiseRule": "add an active source-supported HPO term that shares no informative ancestor below Phenotypic abnormality with any query term",
        "baseline": metric_summary(&cases.iter().map(|case| case.target_worst_tie_rank).collect::<Vec<_>>(), missing_rank),
        "dropout": metric_summary(&dropout_ranks, missing_rank),
        "lessSpecific": metric_summary(&ancestor_ranks, missing_rank),
        "unrelatedNoise": metric_summary(&noise_ranks, missing_rank),
        "cases": records,
    })
}

fn normalized_patient_term(
    knowledge: &HpoKnowledge,
    id: &str,
) -> Result<(usize, Option<String>), String> {
    let mut index = *knowledge
        .term_index
        .get(id)
        .ok_or_else(|| format!("unknown HPO identifier {id}"))?;
    let mut visited = BTreeSet::new();
    while knowledge.terms[index].obsolete {
        if !visited.insert(index) {
            return Err(format!("replacement cycle for {id}"));
        }
        index = knowledge.terms[index]
            .replacement
            .ok_or_else(|| format!("obsolete HPO identifier {id} has no single replacement"))?;
    }
    if knowledge.active_terms.binary_search(&index).is_err() {
        return Err(format!("{id} is not an active phenotypic abnormality"));
    }
    let replacement = (knowledge.terms[index].id != id).then(|| knowledge.terms[index].id.clone());
    Ok((index, replacement))
}

fn cohort_eligibility_report(
    root: &Path,
    knowledge: &HpoKnowledge,
    resolver: &crate::gene_identity::Resolver,
    selected_cases: &[ValidationCase],
) -> Value {
    let selected = selected_cases
        .iter()
        .map(|case| case.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let mut paths = Vec::new();
    collect_phenopackets(&root.join("notebooks"), &mut paths);
    paths.sort_by_key(|path| normalized_relative_path(root, path));
    let mut eligible = Vec::<(String, String, Vec<Value>)>::new();
    let mut ineligible = Vec::new();
    for path in paths {
        let relative_path = normalized_relative_path(root, &path);
        let document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let positive = case_terms(&document, false);
        let solved = solved_interpretations(&document).count();
        let genes = case_genes(&document);
        let diseases = case_diseases(&document);
        let publications = case_publications(&document);
        let subject = document["subject"]["id"].as_str().unwrap_or("").trim();
        let mut reasons = Vec::new();
        let mut replacements = Vec::new();
        if subject.is_empty() {
            reasons.push("missing individual identifier".to_owned());
        }
        if positive.is_empty() {
            reasons.push("no observed positive HPO feature".to_owned());
        }
        for id in &positive {
            match normalized_patient_term(knowledge, id) {
                Ok((_, Some(replacement))) => replacements.push(json!({
                    "original": id,
                    "replacement": replacement,
                })),
                Ok((_, None)) => {}
                Err(reason) => reasons.push(reason),
            }
        }
        if solved == 0 {
            reasons.push("no solved interpretation".to_owned());
        }
        if genes.len() != 1 {
            reasons.push(format!(
                "expected one causative HGNC gene, found {}",
                genes.len()
            ));
        }
        if diseases.len() != 1 {
            reasons.push(format!(
                "expected one diagnosed disease, found {}",
                diseases.len()
            ));
        }
        if publications.is_empty() {
            reasons.push("no PMID publication reference".to_owned());
        }
        if let Some((hgnc_id, symbol)) = genes.first() {
            let resolved = resolver.resolve_pair(hgnc_id, symbol).resolved();
            if resolved.as_ref().is_none_or(|gene| {
                gene.canonical_gene_id.as_deref() != Some(hgnc_id) || gene.identity_status != "hgnc"
            }) {
                reasons.push(format!(
                    "causative target {hgnc_id} is not an approved HGNC identity"
                ));
            }
        }
        if reasons.is_empty() {
            let (hgnc_id, _) = &genes[0];
            let identity = format!("{}|{}|{}", publications.join(","), hgnc_id, subject);
            eligible.push((relative_path, identity, replacements));
        } else {
            ineligible.push(json!({
                "relativePath": relative_path,
                "reasons": reasons,
            }));
        }
    }

    let mut identities = BTreeMap::<String, Vec<String>>::new();
    for (path, identity, _) in &eligible {
        identities
            .entry(identity.clone())
            .or_default()
            .push(path.clone());
    }
    let mut duplicates = Vec::new();
    let mut retained = BTreeSet::new();
    for paths in identities.values_mut() {
        paths.sort();
        retained.insert(paths[0].clone());
        for duplicate in paths.iter().skip(1) {
            duplicates.push(json!({
                "relativePath": duplicate,
                "reason": "duplicate publication, target, and subject identifier",
                "retainedRelativePath": paths[0],
            }));
        }
    }
    for selected_path in &selected {
        assert!(
            retained.contains(*selected_path),
            "selected cohort case is not eligible after deduplication: {selected_path}"
        );
    }
    let replacement_records = eligible
        .into_iter()
        .filter(|(path, _, replacements)| retained.contains(path) && !replacements.is_empty())
        .map(|(path, _, replacements)| {
            json!({
                "relativePath": path,
                "replacements": replacements,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "contract": "one individual; at least one positive HPO term; solved interpretation; exactly one causative approved HGNC gene; exactly one disease; PMID provenance",
        "deduplicationKey": "publication IDs, target HGNC ID, and subject ID",
        "sourceCaseCount": retained.len() + duplicates.len() + ineligible.len(),
        "eligiblePoolCount": retained.len(),
        "selectedCaseCount": selected.len(),
        "eligibleNotSelectedCount": retained.len() - selected.len(),
        "ineligibleCaseCount": ineligible.len(),
        "duplicateCaseCount": duplicates.len(),
        "ineligibleCases": ineligible,
        "duplicateCases": duplicates,
        "obsoleteTermReplacements": replacement_records,
    })
}

fn peak_memory_bytes() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            let kib = line.strip_prefix("VmHWM:")?.trim().strip_suffix(" kB")?;
            kib.trim().parse::<u64>().ok()?.checked_mul(1024)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CandidateAlgorithm {
    QueryToDiseaseResnik,
    SymmetricResnikSizeWeightedBestMatchAverage,
    SymmetricResnikDirectionalMean,
    Phrank,
}

impl CandidateAlgorithm {
    const ALL: [Self; 4] = [
        Self::QueryToDiseaseResnik,
        Self::SymmetricResnikSizeWeightedBestMatchAverage,
        Self::SymmetricResnikDirectionalMean,
        Self::Phrank,
    ];

    fn version(self) -> &'static str {
        match self {
            Self::QueryToDiseaseResnik => "resnik-query-disease-v1",
            Self::SymmetricResnikSizeWeightedBestMatchAverage => {
                "resnik-symmetric-size-weighted-bma-candidate-v1"
            }
            Self::SymmetricResnikDirectionalMean => {
                "resnik-symmetric-directional-mean-candidate-v1"
            }
            Self::Phrank => "phrank-candidate-v1",
        }
    }
}

#[derive(Debug)]
struct CandidateDisease {
    id: String,
    positive: Vec<usize>,
    closure: BTreeSet<usize>,
    genes: Vec<String>,
}

#[derive(Debug)]
struct CandidateCorpus {
    diseases: Vec<CandidateDisease>,
    information_content: Vec<Option<f64>>,
    phrank_weights: Vec<f64>,
    denominator: usize,
}

#[derive(Debug, Clone)]
struct CandidateBest {
    score: f64,
    disease_id: String,
    disease_term_count: usize,
    query_to_disease_score: f64,
    disease_to_query_score: f64,
}

#[derive(Debug, Clone)]
struct CandidateGeneRank {
    rank: usize,
    tie_count: usize,
    score_key: u64,
    best: CandidateBest,
}

#[derive(Debug)]
struct CandidateRanking {
    denominator: usize,
    genes: BTreeMap<String, CandidateGeneRank>,
    disease_worst_tie_ranks: BTreeMap<String, usize>,
    omim_disease_worst_tie_ranks: BTreeMap<String, usize>,
    omim_gene_worst_tie_ranks: BTreeMap<String, usize>,
}

impl CandidateCorpus {
    fn new(knowledge: &HpoKnowledge, resolver: &crate::gene_identity::Resolver) -> Self {
        let mut diseases = knowledge
            .diseases
            .iter()
            .filter_map(|disease| {
                let genes = disease
                    .genes
                    .iter()
                    .filter(|association| {
                        association
                            .association_type
                            .eq_ignore_ascii_case("MENDELIAN")
                    })
                    .filter_map(|association| resolved_association_gene(resolver, association))
                    .map(|gene| gene.comparison_key())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                if disease.positive.is_empty() || genes.is_empty() {
                    return None;
                }
                let mut positive = disease.positive.clone();
                positive.sort_by(|left, right| {
                    knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id)
                });
                positive.dedup();
                let closure = positive
                    .iter()
                    .flat_map(|term| knowledge.terms[*term].ancestors.iter().copied())
                    .collect();
                Some(CandidateDisease {
                    id: disease.id.clone(),
                    positive,
                    closure,
                    genes,
                })
            })
            .collect::<Vec<_>>();
        diseases.sort_by(|left, right| left.id.cmp(&right.id));

        let disease_count = diseases.len();
        let mut profile_counts = vec![0_u64; knowledge.terms.len()];
        for disease in &diseases {
            for term in &disease.closure {
                profile_counts[*term] += 1;
            }
        }
        let information_content = profile_counts
            .iter()
            .map(|count| (*count > 0).then(|| (disease_count as f64 / *count as f64).ln().max(0.0)))
            .collect::<Vec<_>>();

        let gene_indexes = diseases
            .iter()
            .flat_map(|disease| disease.genes.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, gene)| (gene, index))
            .collect::<BTreeMap<_, _>>();
        let mut term_genes = vec![BTreeSet::<usize>::new(); knowledge.terms.len()];
        for disease in &diseases {
            let genes = disease
                .genes
                .iter()
                .map(|gene| gene_indexes[gene])
                .collect::<Vec<_>>();
            for term in &disease.closure {
                term_genes[*term].extend(genes.iter().copied());
            }
        }
        let phrank_weights = knowledge
            .terms
            .iter()
            .enumerate()
            .map(|(index, term)| {
                if term.parents.is_empty() || term_genes[index].is_empty() {
                    return 0.0;
                }
                let parent_genes = term
                    .parents
                    .iter()
                    .flat_map(|parent| term_genes[*parent].iter().copied())
                    .collect::<BTreeSet<_>>();
                assert!(term_genes[index].len() <= parent_genes.len());
                if parent_genes.is_empty() {
                    0.0
                } else {
                    -(term_genes[index].len() as f64 / parent_genes.len() as f64).log2()
                }
            })
            .collect();

        Self {
            denominator: gene_indexes.len(),
            diseases,
            information_content,
            phrank_weights,
        }
    }

    fn rank(
        &self,
        knowledge: &HpoKnowledge,
        observed: &[usize],
    ) -> BTreeMap<CandidateAlgorithm, CandidateRanking> {
        let mut query = observed.to_vec();
        query.sort_by(|left, right| knowledge.terms[*left].id.cmp(&knowledge.terms[*right].id));
        query.dedup();
        assert!(!query.is_empty());
        let query_closure = query
            .iter()
            .flat_map(|term| knowledge.terms[*term].ancestors.iter().copied())
            .collect::<BTreeSet<_>>();
        let mut best_by_algorithm = CandidateAlgorithm::ALL
            .map(|algorithm| (algorithm, BTreeMap::<String, CandidateBest>::new()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut best_by_omim_algorithm = CandidateAlgorithm::ALL
            .map(|algorithm| (algorithm, BTreeMap::<String, CandidateBest>::new()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut disease_scores_by_algorithm = CandidateAlgorithm::ALL
            .map(|algorithm| (algorithm, BTreeMap::<String, f64>::new()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        for disease in &self.diseases {
            let mut query_sum = 0.0;
            let mut disease_best = vec![0.0_f64; disease.positive.len()];
            for query_term in &query {
                let mut query_best = 0.0_f64;
                for (position, disease_term) in disease.positive.iter().enumerate() {
                    let score = resnik_similarity(
                        knowledge,
                        &self.information_content,
                        *query_term,
                        *disease_term,
                    )
                    .0;
                    query_best = query_best.max(score);
                    disease_best[position] = disease_best[position].max(score);
                }
                query_sum += query_best;
            }
            let query_to_disease_score = query_sum / query.len() as f64;
            let disease_to_query_score =
                disease_best.iter().sum::<f64>() / disease.positive.len() as f64;
            let symmetric_score = (query_sum + disease_best.iter().sum::<f64>())
                / (query.len() + disease.positive.len()) as f64;
            let directional_mean_score = (query_to_disease_score + disease_to_query_score) / 2.0;
            let phrank_score = query_closure
                .intersection(&disease.closure)
                .map(|term| self.phrank_weights[*term])
                .sum::<f64>();
            let scores = [
                query_to_disease_score,
                symmetric_score,
                directional_mean_score,
                phrank_score,
            ];

            for (algorithm, score) in CandidateAlgorithm::ALL.into_iter().zip(scores) {
                disease_scores_by_algorithm
                    .get_mut(&algorithm)
                    .expect("candidate algorithm initialized")
                    .insert(disease.id.clone(), score);
                let candidate = CandidateBest {
                    score,
                    disease_id: disease.id.clone(),
                    disease_term_count: disease.positive.len(),
                    query_to_disease_score,
                    disease_to_query_score,
                };
                let genes = best_by_algorithm
                    .get_mut(&algorithm)
                    .expect("candidate algorithm initialized");
                for gene in &disease.genes {
                    genes
                        .entry(gene.clone())
                        .and_modify(|current| {
                            if candidate.score > current.score
                                || candidate.score == current.score
                                    && candidate.disease_id < current.disease_id
                            {
                                *current = candidate.clone();
                            }
                        })
                        .or_insert_with(|| candidate.clone());
                    if disease.id.starts_with("OMIM:") {
                        best_by_omim_algorithm
                            .get_mut(&algorithm)
                            .expect("candidate algorithm initialized")
                            .entry(gene.clone())
                            .and_modify(|current| {
                                if candidate.score > current.score
                                    || candidate.score == current.score
                                        && candidate.disease_id < current.disease_id
                                {
                                    *current = candidate.clone();
                                }
                            })
                            .or_insert_with(|| candidate.clone());
                    }
                }
            }
        }

        best_by_algorithm
            .into_iter()
            .map(|(algorithm, genes)| {
                let disease_scores = disease_scores_by_algorithm
                    .remove(&algorithm)
                    .expect("candidate disease scores initialized");
                let omim_genes = candidate_gene_ranks(
                    best_by_omim_algorithm
                        .remove(&algorithm)
                        .expect("candidate OMIM genes initialized"),
                );
                (
                    algorithm,
                    CandidateRanking {
                        denominator: self.denominator,
                        genes: candidate_gene_ranks(genes),
                        omim_disease_worst_tie_ranks: candidate_worst_tie_ranks(
                            disease_scores
                                .iter()
                                .filter(|(id, _)| id.starts_with("OMIM:"))
                                .map(|(id, score)| (id.clone(), *score))
                                .collect(),
                        ),
                        disease_worst_tie_ranks: candidate_worst_tie_ranks(disease_scores),
                        omim_gene_worst_tie_ranks: omim_genes
                            .into_iter()
                            .map(|(gene, rank)| (gene, rank.rank + rank.tie_count - 1))
                            .collect(),
                    },
                )
            })
            .collect()
    }
}

fn candidate_worst_tie_ranks(scores: BTreeMap<String, f64>) -> BTreeMap<String, usize> {
    let mut ordered = scores
        .into_iter()
        .map(|(id, score)| (id, (score * 1_000_000_000_000.0 + 0.5).floor() as u64))
        .collect::<Vec<_>>();
    let tie_counts = ordered
        .iter()
        .fold(BTreeMap::<u64, usize>::new(), |mut counts, row| {
            *counts.entry(row.1).or_default() += 1;
            counts
        });
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let mut previous_score = None;
    let mut competition_rank = 0;
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, (id, score_key))| {
            if previous_score != Some(score_key) {
                competition_rank = position + 1;
                previous_score = Some(score_key);
            }
            (id, competition_rank + tie_counts[&score_key] - 1)
        })
        .collect()
}

fn candidate_gene_ranks(
    best_by_gene: BTreeMap<String, CandidateBest>,
) -> BTreeMap<String, CandidateGeneRank> {
    let mut ordered = best_by_gene
        .into_iter()
        .map(|(gene, best)| {
            let score_key = (best.score * 1_000_000_000_000.0 + 0.5).floor() as u64;
            (gene, score_key, best)
        })
        .collect::<Vec<_>>();
    let tie_counts = ordered
        .iter()
        .fold(BTreeMap::<u64, usize>::new(), |mut counts, row| {
            *counts.entry(row.1).or_default() += 1;
            counts
        });
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let mut previous_score = None;
    let mut competition_rank = 0;
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, (gene, score_key, best))| {
            if previous_score != Some(score_key) {
                competition_rank = position + 1;
                previous_score = Some(score_key);
            }
            (
                gene,
                CandidateGeneRank {
                    rank: competition_rank,
                    tie_count: tie_counts[&score_key],
                    score_key,
                    best,
                },
            )
        })
        .collect()
}

#[test]
#[ignore = "set ANNOCAT_HPO_FIXTURE_ROOT and ANNOCAT_PHENOPACKET_ROOT to pinned validation snapshots"]
fn phenopacket_store_candidate_algorithm_comparison() {
    let started = Instant::now();
    let hpo_root = PathBuf::from(std::env::var("ANNOCAT_HPO_FIXTURE_ROOT").unwrap());
    let phenopacket_root = PathBuf::from(std::env::var("ANNOCAT_PHENOPACKET_ROOT").unwrap());
    let manifest = load_manifest();
    verify_snapshot(&phenopacket_root, &manifest.source);
    let resources = hpo_root
        .parent()
        .and_then(Path::parent)
        .expect("HPO fixture root must be resources/hpo/release");
    let knowledge = load_knowledge_from_root(&hpo_root).unwrap();
    let resolver = crate::gene_identity::Resolver::new(resources, &[]);
    let corpus = CandidateCorpus::new(&knowledge, &resolver);
    let mut candidate_rankings = Vec::with_capacity(manifest.cases.len());

    for case in &manifest.cases {
        let _document = read_case(&phenopacket_root, case);
        let query = query_indexes(&knowledge, case);
        let production =
            build_phenotype_ranking(&knowledge, &resolver, &query, "2026-06-23".into());
        let candidates = corpus.rank(&knowledge, &query);
        let baseline = &candidates[&CandidateAlgorithm::QueryToDiseaseResnik];
        assert_eq!(baseline.denominator, production.denominator);
        assert_eq!(baseline.genes.len(), production.genes.len());
        for (gene, expected) in &production.genes {
            let actual = &baseline.genes[gene];
            assert_eq!(actual.score_key, expected.score_key, "{gene} score key");
            assert_eq!(actual.rank, expected.rank, "{gene} rank");
            assert_eq!(actual.tie_count, expected.tie_count, "{gene} tie count");
            assert_eq!(
                actual.best.disease_id, expected.best_disease_id,
                "{gene} best disease"
            );
        }
        candidate_rankings.push((query, candidates));
    }

    let null_replicates = manifest.selection["targetLabelNull"]["derangements"]
        .as_u64()
        .expect("null derangement count is missing") as usize;
    let null_seed = manifest.selection["targetLabelNull"]["seed"]
        .as_u64()
        .expect("null seed is missing");
    let mut algorithms = BTreeMap::<String, Value>::new();
    for algorithm in CandidateAlgorithm::ALL {
        let mut ranked = Vec::with_capacity(manifest.cases.len());
        for (case, (query, candidates)) in manifest.cases.iter().zip(&candidate_rankings) {
            let ranking = &candidates[&algorithm];
            let target_key = target_key(&resolver, case);
            let target = ranking.genes.get(&target_key);
            ranked.push(RankedValidationCase {
                id: case.id.clone(),
                target_key,
                target_hgnc_id: case.target_hgnc_id.clone(),
                target_symbol: case.target_symbol.clone(),
                disease_id: case.disease_ids[0].clone(),
                publication_id: case.publication_ids[0].clone(),
                query: query.clone(),
                denominator: ranking.denominator,
                target_rank: target.map(|rank| rank.rank),
                target_tie_count: target.map(|rank| rank.tie_count),
                target_worst_tie_rank: target.map(|rank| rank.rank + rank.tie_count - 1),
                target_raw_score: target.map(|rank| rank.best.score),
                target_score_key: target.map(|rank| rank.score_key),
                target_best_disease_id: target.map(|rank| rank.best.disease_id.clone()),
                gene_worst_tie_ranks: ranking
                    .genes
                    .iter()
                    .map(|(gene, rank)| (gene.clone(), rank.rank + rank.tie_count - 1))
                    .collect(),
                runtime_millis: 0,
            });
        }
        let missing_rank = corpus.denominator + 1;
        let metrics = metric_summary(
            &ranked
                .iter()
                .map(|case| case.target_worst_tie_rank)
                .collect::<Vec<_>>(),
            missing_rank,
        );
        let challenge_case_reference_misses = manifest
            .cases
            .iter()
            .zip(&ranked)
            .filter(|(case, _)| case.challenge_case)
            .filter_map(|(case, result)| {
                let maximum = case.historical_maximum_worst_tie_rank?;
                let actual = result.target_worst_tie_rank?;
                (actual > maximum).then(|| {
                    json!({
                        "caseId": case.id,
                        "targetSymbol": case.target_symbol,
                        "maximumWorstTieRank": maximum,
                        "actualWorstTieRank": actual,
                    })
                })
            })
            .collect::<Vec<_>>();
        let cases = manifest
            .cases
            .iter()
            .zip(&candidate_rankings)
            .map(|(case, (_, candidates))| {
                let ranking = &candidates[&algorithm];
                let target_key = target_key(&resolver, case);
                let target = ranking.genes.get(&target_key);
                json!({
                    "caseId": case.id,
                    "targetSymbol": case.target_symbol,
                    "targetDiseaseWorstTieRank": ranking.disease_worst_tie_ranks.get(&case.disease_ids[0]),
                    "targetOmimDiseaseWorstTieRank": ranking.omim_disease_worst_tie_ranks.get(&case.disease_ids[0]),
                    "targetOmimGeneWorstTieRank": ranking.omim_gene_worst_tie_ranks.get(&target_key),
                    "targetWorstTieRank": target.map(|rank| rank.rank + rank.tie_count - 1),
                    "targetRawScore": target.map(|rank| rank.best.score),
                    "bestDiseaseId": target.map(|rank| rank.best.disease_id.clone()),
                    "bestDiseasePositiveTermCount": target.map(|rank| rank.best.disease_term_count),
                    "bestDiseaseQueryToDiseaseScore": target.map(|rank| rank.best.query_to_disease_score),
                    "bestDiseaseDiseaseToQueryScore": target.map(|rank| rank.best.disease_to_query_score),
                })
            })
            .collect::<Vec<_>>();
        algorithms.insert(
            algorithm.version().into(),
            json!({
                "cohort": metrics,
                "targetLabelNull": target_label_null(&ranked, null_replicates, null_seed),
                "challengeCaseReferenceMisses": challenge_case_reference_misses,
                "cases": cases,
            }),
        );
    }

    let report = json!({
        "schemaVersion": 1,
        "purpose": "validation-only candidate comparison; no product algorithm changes",
        "sourceRelease": manifest.source.release,
        "hpoRelease": "2026-06-23",
        "hgncRelease": resolver.identity_release(),
        "baselineParity": "passed for every gene in all 40 cases",
        "algorithms": algorithms,
        "runtimeMillis": started.elapsed().as_millis(),
        "peakMemoryBytes": peak_memory_bytes(),
    });
    if let Ok(path) = std::env::var("ANNOCAT_PHENOTYPE_CANDIDATE_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        eprintln!("wrote {}", path.display());
    }
    eprintln!(
        "compared {} candidate algorithms across {} frozen cases in {} ms",
        CandidateAlgorithm::ALL.len(),
        manifest.cases.len(),
        report["runtimeMillis"]
    );
}

#[test]
#[ignore = "set ANNOCAT_HPO_FIXTURE_ROOT and ANNOCAT_PHENOPACKET_ROOT to pinned validation snapshots"]
fn phenopacket_store_snapshot_and_manifest_are_valid() {
    let hpo_root = PathBuf::from(std::env::var("ANNOCAT_HPO_FIXTURE_ROOT").unwrap());
    let phenopacket_root = PathBuf::from(std::env::var("ANNOCAT_PHENOPACKET_ROOT").unwrap());
    let manifest = load_manifest();
    verify_snapshot(&phenopacket_root, &manifest.source);
    let resources = hpo_root
        .parent()
        .and_then(Path::parent)
        .expect("HPO fixture root must be resources/hpo/release");
    let knowledge = load_knowledge_from_root(&hpo_root).unwrap();
    let resolver = crate::gene_identity::Resolver::new(resources, &[]);
    for case in &manifest.cases {
        let _document = read_case(&phenopacket_root, case);
        let _query = query_indexes(&knowledge, case);
        let _target = target_key(&resolver, case);
    }
    let eligibility =
        cohort_eligibility_report(&phenopacket_root, &knowledge, &resolver, &manifest.cases);
    assert_eq!(eligibility["selectedCaseCount"], manifest.cases.len());
    assert_eq!(
        eligibility["sourceCaseCount"],
        manifest.source.phenopacket_file_count
    );
    eprintln!(
        "validated {} Phenopacket Store cases: {} eligible, {} ineligible, {} selected",
        eligibility["sourceCaseCount"],
        eligibility["eligiblePoolCount"],
        eligibility["ineligibleCaseCount"],
        eligibility["selectedCaseCount"],
    );
}

#[test]
#[ignore = "set ANNOCAT_HPO_FIXTURE_ROOT and ANNOCAT_PHENOPACKET_ROOT to pinned validation snapshots"]
fn phenopacket_store_frozen_challenge_case_report() {
    let hpo_root = PathBuf::from(std::env::var("ANNOCAT_HPO_FIXTURE_ROOT").unwrap());
    let phenopacket_root = PathBuf::from(std::env::var("ANNOCAT_PHENOPACKET_ROOT").unwrap());
    let manifest = load_manifest();
    verify_snapshot(&phenopacket_root, &manifest.source);

    let resources = hpo_root
        .parent()
        .and_then(Path::parent)
        .expect("HPO fixture root must be resources/hpo/release");
    let knowledge = load_knowledge_from_root(&hpo_root).unwrap();
    let resolver = crate::gene_identity::Resolver::new(resources, &[]);
    assert_eq!(resolver.identity_release(), Some("2026-08-07"));
    let challenge_cases = manifest
        .cases
        .iter()
        .filter(|case| case.challenge_case)
        .collect::<Vec<_>>();
    assert_eq!(challenge_cases.len(), 8);
    let mut fingerprints = BTreeSet::new();
    let mut denominators = BTreeSet::new();
    for case in challenge_cases {
        let _document = read_case(&phenopacket_root, case);
        let ranking = build_phenotype_ranking(
            &knowledge,
            &resolver,
            &query_indexes(&knowledge, case),
            "2026-06-23".into(),
        );
        let target = ranking
            .genes
            .get(&target_key(&resolver, case))
            .unwrap_or_else(|| panic!("{} target is outside the ranking universe", case.id));
        let worst_tie_rank = target.rank + target.tie_count - 1;
        let maximum = case
            .historical_maximum_worst_tie_rank
            .expect("challenge case must retain its historical reference threshold");
        eprintln!(
            "{} {}: rank {}, tie {}, group end {}, historical reference {}, denominator {}",
            case.id,
            case.target_symbol,
            target.rank,
            target.tie_count,
            worst_tie_rank,
            maximum,
            ranking.denominator
        );
        assert!(fingerprints.insert(ranking_fingerprint(&ranking)));
        denominators.insert(ranking.denominator);
    }
    assert_eq!(denominators.len(), 1);
}

#[test]
#[ignore = "set ANNOCAT_HPO_FIXTURE_ROOT and ANNOCAT_PHENOPACKET_ROOT to pinned validation snapshots"]
fn phenopacket_store_public_cohort_report() {
    let started = Instant::now();
    let hpo_root = PathBuf::from(std::env::var("ANNOCAT_HPO_FIXTURE_ROOT").unwrap());
    let phenopacket_root = PathBuf::from(std::env::var("ANNOCAT_PHENOPACKET_ROOT").unwrap());
    let manifest = load_manifest();
    verify_snapshot(&phenopacket_root, &manifest.source);
    assert_eq!(manifest.cases.len(), 40);

    let resources = hpo_root
        .parent()
        .and_then(Path::parent)
        .expect("HPO fixture root must be resources/hpo/release");
    let knowledge = load_knowledge_from_root(&hpo_root).unwrap();
    let resolver = crate::gene_identity::Resolver::new(resources, &[]);
    assert_eq!(resolver.identity_release(), Some("2026-08-07"));
    let eligibility =
        cohort_eligibility_report(&phenopacket_root, &knowledge, &resolver, &manifest.cases);

    let mut ranked = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        let result = rank_validation_case(&phenopacket_root, &knowledge, &resolver, case);
        eprintln!(
            "{} {}: rank {:?}, group end {:?}, {} ms",
            result.id,
            result.target_symbol,
            result.target_rank,
            result.target_worst_tie_rank,
            result.runtime_millis
        );
        ranked.push(result);
    }
    let denominator = ranked
        .first()
        .expect("cohort must contain cases")
        .denominator;
    assert!(ranked.iter().all(|case| case.denominator == denominator));
    let case_metrics = metric_summary(
        &ranked
            .iter()
            .map(|case| case.target_worst_tie_rank)
            .collect::<Vec<_>>(),
        denominator + 1,
    );
    let gene_balanced = balanced_summary(&ranked, |case| &case.target_hgnc_id);
    let disease_balanced = balanced_summary(&ranked, |case| &case.disease_id);
    let publication_balanced = balanced_summary(&ranked, |case| &case.publication_id);
    let null_replicates = manifest.selection["targetLabelNull"]["derangements"]
        .as_u64()
        .expect("null derangement count is missing") as usize;
    let null_seed = manifest.selection["targetLabelNull"]["seed"]
        .as_u64()
        .expect("null seed is missing");
    let null_control = target_label_null(&ranked, null_replicates, null_seed);
    let bootstrap_replicates = manifest.selection["bootstrap"]["replicates"]
        .as_u64()
        .expect("bootstrap replicate count is missing") as usize;
    let bootstrap_seed = manifest.selection["bootstrap"]["seed"]
        .as_u64()
        .expect("bootstrap seed is missing");
    let gene_bootstrap = clustered_bootstrap(
        &ranked,
        |case| &case.target_hgnc_id,
        bootstrap_replicates,
        bootstrap_seed,
    );
    let publication_bootstrap = clustered_bootstrap(
        &ranked,
        |case| &case.publication_id,
        bootstrap_replicates,
        bootstrap_seed ^ 0x5055_424c_4943_4154,
    );
    let robustness = robustness_report(
        &ranked,
        &knowledge,
        &resolver,
        manifest.selection["robustness"]["seed"]
            .as_u64()
            .expect("robustness seed is missing"),
        manifest.selection["robustness"]["dropoutFraction"]
            .as_f64()
            .expect("dropout fraction is missing"),
        manifest.selection["robustness"]["noiseTermCount"]
            .as_u64()
            .expect("noise term count is missing") as usize,
    );

    let first = &ranked[0];
    let mut reordered = first.query.iter().rev().copied().collect::<Vec<_>>();
    reordered.push(first.query[0]);
    let invariant = build_phenotype_ranking(&knowledge, &resolver, &reordered, "2026-06-23".into())
        .genes
        .into_iter()
        .map(|(gene, rank)| (gene, rank.rank + rank.tie_count - 1))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(invariant, first.gene_worst_tie_ranks);

    let challenge_case_reference_misses = manifest
        .cases
        .iter()
        .zip(&ranked)
        .filter(|(case, _)| case.challenge_case)
        .filter_map(|(case, result)| {
            let maximum = case.historical_maximum_worst_tie_rank?;
            let actual = result.target_worst_tie_rank?;
            (actual > maximum).then(|| {
                json!({
                    "caseId": case.id,
                    "targetHgncId": case.target_hgnc_id,
                    "targetSymbol": case.target_symbol,
                    "maximumWorstTieRank": maximum,
                    "actualWorstTieRank": actual,
                })
            })
        })
        .collect::<Vec<_>>();
    let null_passed = null_control["passed"] == true;

    let report = json!({
        "schemaVersion": 1,
        "software": {
            "name": "AnnoCAT",
            "version": env!("CARGO_PKG_VERSION"),
            "commit": std::env::var("GITHUB_SHA").ok(),
        },
        "qualifiedForPhenotypeRankRelease": null_passed,
        "releaseQualification": {
            "scope": "exploratory search feature; not clinical validation",
            "cohortRole": "frozen retrospective public reference cohort",
            "aggregateGate": "observed gene-balanced mean reciprocal worst-tie rank strictly exceeds the predeclared 95th percentile of target-label derangements",
            "passed": null_passed,
            "absoluteTopKThreshold": null,
            "clinicalValidation": false,
        },
        "source": {
            "name": manifest.source.name,
            "repository": manifest.source.repository,
            "release": manifest.source.release,
            "commit": manifest.source.commit,
            "phenopacketFileCount": manifest.source.phenopacket_file_count,
            "phenopacketTreeSha256": manifest.source.phenopacket_tree_sha256,
        },
        "hpoRelease": "2026-06-23",
        "hgncRelease": resolver.identity_release(),
        "rankingAlgorithmVersion": PHENOTYPE_RANKING_ALGORITHM_VERSION,
        "associationPolicy": "MENDELIAN only; POLYGENIC, UNKNOWN, other, and missing association types excluded",
        "queryPolicy": "unique observed positive HPO terms; excluded Phenopacket findings retained for audit and omitted from ranking",
        "eligibleGeneDenominator": denominator,
        "tiePolicy": "evaluation rank is displayed competition rank plus tie count minus one",
        "cohort": {
            "selectedCases": ranked.len(),
            "uniqueTargetGenes": ranked.iter().map(|case| &case.target_key).collect::<BTreeSet<_>>().len(),
            "uniqueDiseases": ranked.iter().map(|case| &case.disease_id).collect::<BTreeSet<_>>().len(),
            "uniquePublications": ranked.iter().map(|case| &case.publication_id).collect::<BTreeSet<_>>().len(),
            "caseWeighted": case_metrics,
            "geneBalanced": gene_balanced,
            "diseaseBalanced": disease_balanced,
            "publicationBalanced": publication_balanced,
        },
        "eligibility": eligibility,
        "targetLabelNull": null_control,
        "uncertainty": {
            "geneClusterBootstrap": gene_bootstrap,
            "publicationClusterBootstrap": publication_bootstrap,
        },
        "robustness": robustness,
        "invariance": {
            "queryOrderAndDuplicates": "passed",
            "excludedPhenopacketFindings": "not included in any query",
        },
        "challengeCaseReferenceMisses": challenge_case_reference_misses,
        "cases": ranked.iter().map(RankedValidationCase::report).collect::<Vec<_>>(),
        "runtimeMillis": started.elapsed().as_millis(),
        "peakMemoryBytes": peak_memory_bytes(),
    });

    if let Ok(path) = std::env::var("ANNOCAT_PHENOTYPE_VALIDATION_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        eprintln!("wrote {}", path.display());
    }
    if let Ok(path) = std::env::var("ANNOCAT_VALIDATION_GENE_RANKS") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut rows = String::from("case_id\ttarget_hgnc_id\tgene_hgnc_id\tworst_tie_rank\n");
        for case in &ranked {
            for (gene, rank) in &case.gene_worst_tie_ranks {
                if gene.starts_with("HGNC:") {
                    rows.push_str(&format!(
                        "{}\t{}\t{}\t{}\n",
                        case.id, case.target_hgnc_id, gene, rank
                    ));
                }
            }
        }
        fs::write(&path, rows).unwrap();
        eprintln!("wrote {}", path.display());
    }

    assert!(null_passed, "target-label null gate failed");
}
