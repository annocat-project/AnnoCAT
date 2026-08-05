use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const MONDO_ID_PREFIX: &str = "http://purl.obolibrary.org/obo/MONDO_";
const EXACT_MATCH: &str = "http://www.w3.org/2004/02/skos/core#exactMatch";
const REPLACED_BY: &str = "http://purl.obolibrary.org/obo/IAO_0100001";
const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MondoAssetManifest {
    schema_version: u16,
    release: String,
    release_url: String,
    assets: Vec<MondoAsset>,
}

impl MondoAssetManifest {
    pub(crate) fn release(&self) -> &str {
        &self.release
    }

    pub(crate) fn release_url(&self) -> &str {
        &self.release_url
    }

    pub(crate) fn asset(&self) -> &MondoAsset {
        &self.assets[0]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MondoAsset {
    pub(crate) kind: String,
    pub(crate) filename: String,
    pub(crate) url: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MondoDocument {
    graphs: Vec<MondoGraph>,
}

#[derive(Debug, Deserialize)]
struct MondoGraph {
    #[serde(default)]
    nodes: Vec<MondoNode>,
    #[serde(default)]
    edges: Vec<MondoEdge>,
}

#[derive(Debug, Deserialize)]
struct MondoNode {
    id: String,
    #[serde(default)]
    lbl: String,
    #[serde(default)]
    meta: MondoMeta,
}

#[derive(Debug, Default, Deserialize)]
struct MondoMeta {
    #[serde(default)]
    synonyms: Vec<MondoSynonym>,
    #[serde(default, rename = "basicPropertyValues")]
    basic_property_values: Vec<MondoProperty>,
    #[serde(default)]
    deprecated: bool,
}

#[derive(Debug, Deserialize)]
struct MondoSynonym {
    pred: String,
    val: String,
}

#[derive(Debug, Deserialize)]
struct MondoProperty {
    pred: String,
    val: String,
}

#[derive(Debug, Deserialize)]
struct MondoEdge {
    sub: String,
    pred: String,
    obj: String,
}

#[derive(Debug)]
struct MondoTerm {
    id: String,
    label: String,
    synonyms: Vec<(String, String)>,
    exact_external_ids: Vec<String>,
    search_text: String,
    ancestors: Vec<usize>,
    subtype_count: usize,
    deprecated: bool,
    replacement: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct MondoKnowledge {
    terms: Vec<MondoTerm>,
    term_index: HashMap<String, usize>,
    exact_external_index: HashMap<String, usize>,
    active_terms: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct TermMatch {
    pub(crate) score: usize,
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) matched_text: String,
    pub(crate) match_kind: String,
    pub(crate) synonym_scope: Option<String>,
    pub(crate) subtype_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CanonicalCondition {
    pub(crate) id: String,
    pub(crate) label: String,
    index: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct DiseaseConditionMatch {
    pub(crate) selected_id: String,
    pub(crate) selected_label: String,
    pub(crate) matched_id: String,
    pub(crate) matched_label: String,
    pub(crate) relation: &'static str,
}

fn knowledge_cache() -> &'static Mutex<HashMap<PathBuf, Arc<MondoKnowledge>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<MondoKnowledge>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
pub(crate) fn embedded_asset_manifest() -> Result<MondoAssetManifest, String> {
    let manifest: MondoAssetManifest = serde_json::from_str(
        annocat_core::source_catalog::resource_manifest_json("mondo")?,
    )
    .map_err(|error| format!("invalid embedded MONDO bootstrap manifest: {error}"))?;
    validate_asset_manifest(&manifest)?;
    Ok(manifest)
}

pub(crate) fn resolve_latest_asset_manifest() -> Result<MondoAssetManifest, String> {
    let url = annocat_core::source_catalog::resolver_api_url("mondo")
        .ok_or("MONDO resolver API URL is missing from the source catalog")?;
    let client = super::http_client::source()
        .map_err(|error| format!("cannot create the MONDO release resolver: {error}"))?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("cannot discover the latest MONDO release: {error}"))?;
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_RELEASE_METADATA_BYTES)
    {
        return Err("the MONDO release metadata exceeded its safety limit".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RELEASE_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read the latest MONDO release metadata: {error}"))?;
    if bytes.len() as u64 > MAX_RELEASE_METADATA_BYTES {
        return Err("the MONDO release metadata exceeded its safety limit".into());
    }
    parse_github_release(&bytes)
}

fn parse_github_release(bytes: &[u8]) -> Result<MondoAssetManifest, String> {
    let release: GitHubRelease = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid MONDO release metadata from GitHub: {error}"))?;
    if release.draft || release.prerelease {
        return Err("GitHub returned a draft or prerelease as the latest MONDO release".into());
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .filter(|value| valid_release_version(value))
        .ok_or("the latest MONDO release has an invalid version tag")?
        .to_owned();
    let expected_release_url = format!(
        "https://github.com/monarch-initiative/mondo/releases/tag/{}",
        release.tag_name
    );
    if release.html_url != expected_release_url {
        return Err("the latest MONDO release points outside the official repository".into());
    }
    let matching = release
        .assets
        .iter()
        .filter(|asset| asset.name == "mondo.json")
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err("the latest MONDO release did not contain exactly one mondo.json asset".into());
    }
    let asset = matching[0];
    let expected_url = format!(
        "https://github.com/monarch-initiative/mondo/releases/download/{}/mondo.json",
        release.tag_name
    );
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("the latest MONDO asset has no valid GitHub SHA-256 digest")?
        .to_ascii_lowercase();
    let manifest = MondoAssetManifest {
        schema_version: 1,
        release: version,
        release_url: release.html_url,
        assets: vec![MondoAsset {
            kind: "condition-ontology".into(),
            filename: "mondo.json".into(),
            url: asset.browser_download_url.clone(),
            bytes: asset.size,
            sha256,
        }],
    };
    if asset.browser_download_url != expected_url {
        return Err("the latest MONDO asset points outside the official release".into());
    }
    validate_asset_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_asset_manifest(manifest: &MondoAssetManifest) -> Result<(), String> {
    let tag = format!("v{}", manifest.release);
    let expected_release =
        format!("https://github.com/monarch-initiative/mondo/releases/tag/{tag}");
    let expected_asset =
        format!("https://github.com/monarch-initiative/mondo/releases/download/{tag}/mondo.json");
    if manifest.schema_version != 1
        || !valid_release_version(&manifest.release)
        || manifest.release_url != expected_release
        || manifest.assets.len() != 1
        || manifest.assets[0].kind != "condition-ontology"
        || manifest.assets[0].filename != "mondo.json"
        || manifest.assets[0].url != expected_asset
        || manifest.assets[0].bytes == 0
        || manifest.assets[0].sha256.len() != 64
        || !manifest.assets[0]
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("MONDO asset manifest failed validation".into());
    }
    Ok(())
}

fn valid_release_version(value: &str) -> bool {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return false;
    }
    let month = value[5..7].parse::<u8>().ok();
    let day = value[8..10].parse::<u8>().ok();
    month.is_some_and(|month| (1..=12).contains(&month))
        && day.is_some_and(|day| (1..=31).contains(&day))
}

pub(crate) fn knowledge(root: &Path) -> Result<Arc<MondoKnowledge>, String> {
    let path = root.join("raw").join("mondo.json");
    if !path.is_file() {
        return Err(
            "Condition knowledge is not installed. Update phenotype and condition knowledge in Data sources."
                .into(),
        );
    }
    if let Some(cached) = knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&path)
        .cloned()
    {
        return Ok(cached);
    }
    let loaded = Arc::new(load(&path)?);
    knowledge_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(path, loaded.clone());
    Ok(loaded)
}

pub(crate) fn validate_file(path: &Path) -> Result<usize, String> {
    Ok(load(path)?.active_terms.len())
}

fn load(path: &Path) -> Result<MondoKnowledge, String> {
    let reader = BufReader::new(
        File::open(path).map_err(|error| format!("cannot read MONDO ontology: {error}"))?,
    );
    let document: MondoDocument = serde_json::from_reader(reader)
        .map_err(|error| format!("cannot parse MONDO ontology: {error}"))?;
    let graph = document
        .graphs
        .into_iter()
        .find(|graph| {
            graph
                .nodes
                .iter()
                .any(|node| canonical_mondo_id(&node.id).is_some())
        })
        .ok_or("MONDO ontology contains no disease graph")?;
    build_knowledge(graph)
}

fn build_knowledge(graph: MondoGraph) -> Result<MondoKnowledge, String> {
    let mondo_nodes = graph
        .nodes
        .into_iter()
        .filter_map(|node| canonical_mondo_id(&node.id).map(|id| (id, node)))
        .collect::<Vec<_>>();
    if mondo_nodes.is_empty() {
        return Err("MONDO ontology contains no condition terms".into());
    }
    let term_index = mondo_nodes
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut replacement_ids = Vec::with_capacity(mondo_nodes.len());
    let mut terms = Vec::with_capacity(mondo_nodes.len());
    for (id, node) in mondo_nodes {
        let synonyms = node
            .meta
            .synonyms
            .into_iter()
            .filter(|synonym| !synonym.val.trim().is_empty())
            .map(|synonym| (synonym.val, synonym_scope(&synonym.pred).to_string()))
            .collect::<Vec<_>>();
        let mut replacement = None;
        let mut exact_external_ids = Vec::new();
        for property in node.meta.basic_property_values {
            if property.pred == REPLACED_BY {
                replacement = canonical_mondo_id(&property.val);
            } else if property.pred == EXACT_MATCH
                && let Some(external) = canonical_external_id(&property.val)
            {
                exact_external_ids.push(external);
            }
        }
        exact_external_ids.sort();
        exact_external_ids.dedup();
        let mut search = vec![id.to_ascii_lowercase(), node.lbl.to_ascii_lowercase()];
        search.extend(synonyms.iter().map(|(value, _)| value.to_ascii_lowercase()));
        terms.push(MondoTerm {
            id,
            label: node.lbl,
            synonyms,
            exact_external_ids,
            search_text: search.join("\n"),
            ancestors: Vec::new(),
            subtype_count: 0,
            deprecated: node.meta.deprecated,
            replacement: None,
        });
        replacement_ids.push(replacement);
    }
    for (index, replacement) in replacement_ids.into_iter().enumerate() {
        terms[index].replacement = replacement.and_then(|id| term_index.get(&id).copied());
    }
    let mut parents = vec![Vec::new(); terms.len()];
    for edge in graph.edges {
        if edge.pred != "is_a" {
            continue;
        }
        let (Some(child), Some(parent)) = (
            canonical_mondo_id(&edge.sub).and_then(|id| term_index.get(&id).copied()),
            canonical_mondo_id(&edge.obj).and_then(|id| term_index.get(&id).copied()),
        ) else {
            continue;
        };
        parents[child].push(parent);
    }
    populate_ancestors(&parents, &mut terms)?;
    let searchable_roots = ["MONDO:0700096", "MONDO:0042489"]
        .into_iter()
        .filter_map(|id| term_index.get(id).copied())
        .collect::<Vec<_>>();
    if searchable_roots.len() != 2 {
        return Err("MONDO ontology is missing a searchable human-condition root".into());
    }
    let active_terms = terms
        .iter()
        .enumerate()
        .filter_map(|(index, term)| {
            (!term.deprecated
                && !term.label.trim().is_empty()
                && !searchable_roots.contains(&index)
                && searchable_roots
                    .iter()
                    .any(|root| term.ancestors.contains(root)))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut subtype_counts = vec![0usize; terms.len()];
    for &candidate in &active_terms {
        for &ancestor in &terms[candidate].ancestors {
            if ancestor != candidate {
                subtype_counts[ancestor] += 1;
            }
        }
    }
    for (term, subtype_count) in terms.iter_mut().zip(subtype_counts) {
        term.subtype_count = subtype_count;
    }
    let active = active_terms.iter().copied().collect::<HashSet<_>>();
    let mut exact_external_candidates = HashMap::<String, Option<usize>>::new();
    for index in 0..terms.len() {
        let Some(target) = active_replacement(index, &terms, &active) else {
            continue;
        };
        for identifier in &terms[index].exact_external_ids {
            exact_external_candidates
                .entry(identifier.clone())
                .and_modify(|candidate| {
                    if *candidate != Some(target) {
                        *candidate = None;
                    }
                })
                .or_insert(Some(target));
        }
    }
    let exact_external_index = exact_external_candidates
        .into_iter()
        .filter_map(|(identifier, index)| index.map(|index| (identifier, index)))
        .collect();
    Ok(MondoKnowledge {
        terms,
        term_index,
        exact_external_index,
        active_terms,
    })
}

fn active_replacement(
    mut index: usize,
    terms: &[MondoTerm],
    active: &HashSet<usize>,
) -> Option<usize> {
    let mut visited = HashSet::new();
    loop {
        if active.contains(&index) {
            return Some(index);
        }
        if !terms[index].deprecated || !visited.insert(index) {
            return None;
        }
        index = terms[index].replacement?;
    }
}

fn populate_ancestors(parents: &[Vec<usize>], terms: &mut [MondoTerm]) -> Result<(), String> {
    fn visit(
        index: usize,
        parents: &[Vec<usize>],
        memo: &mut [Option<Vec<usize>>],
        visiting: &mut HashSet<usize>,
    ) -> Result<Vec<usize>, String> {
        if let Some(value) = &memo[index] {
            return Ok(value.clone());
        }
        if !visiting.insert(index) {
            return Err("MONDO ontology contains a parent cycle".into());
        }
        let mut ancestors = vec![index];
        for &parent in &parents[index] {
            ancestors.extend(visit(parent, parents, memo, visiting)?);
        }
        ancestors.sort_unstable();
        ancestors.dedup();
        visiting.remove(&index);
        memo[index] = Some(ancestors.clone());
        Ok(ancestors)
    }
    let mut memo = vec![None; terms.len()];
    for (index, term) in terms.iter_mut().enumerate() {
        term.ancestors = visit(index, parents, &mut memo, &mut HashSet::new())?;
    }
    Ok(())
}

impl MondoKnowledge {
    pub(crate) fn subtype_count(&self, id: &str) -> Option<usize> {
        self.term_index
            .get(id)
            .map(|&index| self.terms[index].subtype_count)
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<TermMatch> {
        let query = normalize_query(query);
        if query.len() < 2 {
            return Vec::new();
        }
        let exact_external = self.exact_external_index.get(&query).copied();
        let mut matches = self
            .active_terms
            .iter()
            .filter_map(|&index| {
                let term = &self.terms[index];
                let label = term.label.to_ascii_lowercase();
                let (score, matched_text, match_kind, synonym_scope) =
                    if term.id.to_ascii_lowercase() == query {
                        (0, term.id.clone(), "identifier", None)
                    } else if exact_external == Some(index) {
                        (1, query.to_ascii_uppercase(), "externalIdentifier", None)
                    } else if label == query {
                        (2, term.label.clone(), "label", None)
                    } else if let Some((value, scope)) = term
                        .synonyms
                        .iter()
                        .find(|(value, _)| value.eq_ignore_ascii_case(&query))
                    {
                        (4, value.clone(), "synonym", Some(scope.clone()))
                    } else if label.starts_with(&query) {
                        (3, term.label.clone(), "label", None)
                    } else if label.contains(&query) {
                        (5, term.label.clone(), "label", None)
                    } else if let Some((value, scope)) = term
                        .synonyms
                        .iter()
                        .find(|(value, _)| value.to_ascii_lowercase().contains(&query))
                    {
                        (6, value.clone(), "synonym", Some(scope.clone()))
                    } else if term.search_text.contains(&query) {
                        (7, term.label.clone(), "label", None)
                    } else {
                        return None;
                    };
                Some((
                    score,
                    term.label.len(),
                    TermMatch {
                        score,
                        id: term.id.clone(),
                        label: term.label.clone(),
                        matched_text,
                        match_kind: match_kind.into(),
                        synonym_scope,
                        subtype_count: term.subtype_count,
                    },
                ))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(left.2.label.cmp(&right.2.label))
        });
        matches
            .into_iter()
            .take(limit.clamp(1, 100))
            .map(|(_, _, item)| item)
            .collect()
    }

    pub(crate) fn canonical_conditions(
        &self,
        requested: &[super::phenotype::PhenotypeTerm],
    ) -> Result<Vec<CanonicalCondition>, String> {
        let mut conditions = Vec::with_capacity(requested.len());
        let mut seen = HashSet::new();
        for requested in requested {
            let mut index = self
                .term_index
                .get(&requested.id)
                .copied()
                .ok_or_else(|| format!("MONDO condition {} is unavailable", requested.id))?;
            let mut visited = HashSet::new();
            while self.terms[index].deprecated {
                if !visited.insert(index) {
                    return Err(format!(
                        "MONDO condition {} has a replacement cycle",
                        requested.id
                    ));
                }
                index = self.terms[index].replacement.ok_or_else(|| {
                    format!(
                        "MONDO condition {} is obsolete and has no single replacement",
                        requested.id
                    )
                })?;
            }
            if !self.active_terms.contains(&index) {
                return Err(format!(
                    "MONDO condition {} is not an active human condition",
                    requested.id
                ));
            }
            if seen.insert(index) {
                let term = &self.terms[index];
                conditions.push(CanonicalCondition {
                    id: term.id.clone(),
                    label: term.label.clone(),
                    index,
                });
            }
        }
        conditions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(conditions)
    }

    pub(crate) fn disease_matches(
        &self,
        selected: &[CanonicalCondition],
        disease_id: &str,
    ) -> Vec<DiseaseConditionMatch> {
        let Some(&disease_index) = self
            .exact_external_index
            .get(&normalize_query(disease_id))
            .or_else(|| self.term_index.get(disease_id))
        else {
            return Vec::new();
        };
        let matched = &self.terms[disease_index];
        selected
            .iter()
            .filter_map(|condition| {
                let relation = if disease_index == condition.index {
                    "Exact condition"
                } else if matched.ancestors.contains(&condition.index) {
                    "Condition subtype"
                } else {
                    return None;
                };
                Some(DiseaseConditionMatch {
                    selected_id: condition.id.clone(),
                    selected_label: condition.label.clone(),
                    matched_id: matched.id.clone(),
                    matched_label: matched.label.clone(),
                    relation,
                })
            })
            .collect()
    }
}

fn canonical_mondo_id(value: &str) -> Option<String> {
    let digits = value
        .strip_prefix(MONDO_ID_PREFIX)
        .or_else(|| value.strip_prefix("MONDO:"))?;
    (digits.len() == 7 && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| format!("MONDO:{digits}"))
}

fn canonical_external_id(value: &str) -> Option<String> {
    let pairs = [
        ("https://omim.org/entry/", "omim:"),
        ("http://omim.org/entry/", "omim:"),
        ("https://omim.org/phenotypicSeries/PS", "omimps:"),
        ("http://omim.org/phenotypicSeries/PS", "omimps:"),
        ("http://www.orpha.net/ORDO/Orphanet_", "orpha:"),
        ("https://www.orpha.net/ORDO/Orphanet_", "orpha:"),
        ("https://www.deciphergenomics.org/syndrome/", "decipher:"),
        ("http://www.deciphergenomics.org/syndrome/", "decipher:"),
    ];
    for (prefix, canonical_prefix) in pairs {
        if let Some(identifier) = value.strip_prefix(prefix)
            && !identifier.is_empty()
        {
            return Some(format!("{canonical_prefix}{identifier}").to_ascii_lowercase());
        }
    }
    if let Some(identifier) = value.strip_prefix("http://purl.obolibrary.org/obo/OMIM_") {
        return Some(format!("omim:{identifier}").to_ascii_lowercase());
    }
    None
}

fn synonym_scope(value: &str) -> &'static str {
    match value {
        "hasExactSynonym" => "exact",
        "hasBroadSynonym" => "broad",
        "hasNarrowSynonym" => "narrow",
        _ => "related",
    }
}

fn normalize_query(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_manifest_is_verified() {
        let manifest = embedded_asset_manifest().unwrap();
        assert_eq!(manifest.release(), "2026-07-06");
        assert_eq!(manifest.asset().filename, "mondo.json");
        assert_eq!(manifest.asset().bytes, 107_273_669);
    }

    #[test]
    fn exact_external_identifiers_are_canonicalized_for_hpo_disease_ids() {
        assert_eq!(
            canonical_external_id("https://omim.org/entry/123456").as_deref(),
            Some("omim:123456")
        );
        assert_eq!(
            canonical_external_id("http://www.orpha.net/ORDO/Orphanet_42").as_deref(),
            Some("orpha:42")
        );
        assert!(canonical_external_id("https://example.org/close-match/42").is_none());
    }

    #[test]
    fn exact_and_subtype_condition_links_remain_distinct() {
        let node = |id: &str, label: &str, properties: Vec<MondoProperty>| MondoNode {
            id: format!("{MONDO_ID_PREFIX}{id}"),
            lbl: label.into(),
            meta: MondoMeta {
                basic_property_values: properties,
                ..MondoMeta::default()
            },
        };
        let graph = MondoGraph {
            nodes: vec![
                node("0700096", "human disease", vec![]),
                node("0042489", "disease susceptibility", vec![]),
                node("0000001", "selected condition", vec![]),
                node(
                    "0000002",
                    "condition subtype",
                    vec![MondoProperty {
                        pred: EXACT_MATCH.into(),
                        val: "http://www.orpha.net/ORDO/Orphanet_42".into(),
                    }],
                ),
                MondoNode {
                    id: format!("{MONDO_ID_PREFIX}0000003"),
                    lbl: "obsolete condition name".into(),
                    meta: MondoMeta {
                        deprecated: true,
                        basic_property_values: vec![
                            MondoProperty {
                                pred: EXACT_MATCH.into(),
                                val: "http://www.orpha.net/ORDO/Orphanet_43".into(),
                            },
                            MondoProperty {
                                pred: REPLACED_BY.into(),
                                val: format!("{MONDO_ID_PREFIX}0000002"),
                            },
                        ],
                        ..MondoMeta::default()
                    },
                },
            ],
            edges: vec![
                MondoEdge {
                    sub: format!("{MONDO_ID_PREFIX}0000001"),
                    pred: "is_a".into(),
                    obj: format!("{MONDO_ID_PREFIX}0700096"),
                },
                MondoEdge {
                    sub: format!("{MONDO_ID_PREFIX}0000002"),
                    pred: "is_a".into(),
                    obj: format!("{MONDO_ID_PREFIX}0000001"),
                },
            ],
        };
        let knowledge = build_knowledge(graph).unwrap();
        let selected = knowledge
            .canonical_conditions(&[super::super::phenotype::PhenotypeTerm {
                id: "MONDO:0000001".into(),
                label: "Selected condition".into(),
            }])
            .unwrap();
        assert_eq!(
            knowledge.disease_matches(&selected, "MONDO:0000001")[0].relation,
            "Exact condition"
        );
        assert_eq!(
            knowledge.disease_matches(&selected, "ORPHA:42")[0].relation,
            "Condition subtype"
        );
        assert_eq!(
            knowledge.disease_matches(&selected, "ORPHA:43")[0].matched_id,
            "MONDO:0000002"
        );
        assert_eq!(knowledge.subtype_count("MONDO:0000001"), Some(1));
        assert!(
            knowledge
                .search("human disease", 10)
                .iter()
                .all(|item| item.id != "MONDO:0700096")
        );
    }

    #[test]
    #[ignore = "set ANNOCAT_MONDO_FIXTURE to an official mondo.json release"]
    fn official_release_fixture_is_searchable_and_maps_hpo_disease_ids() {
        let path = std::env::var("ANNOCAT_MONDO_FIXTURE").unwrap();
        let knowledge = load(Path::new(&path)).unwrap();
        assert!(knowledge.active_terms.len() > 20_000);
        let omim_count = knowledge
            .exact_external_index
            .keys()
            .filter(|value| value.starts_with("omim:"))
            .count();
        assert!(
            omim_count > 8_000,
            "official MONDO release exposed only {omim_count} unambiguous active OMIM mappings"
        );
        assert!(
            knowledge
                .exact_external_index
                .keys()
                .filter(|value| value.starts_with("orpha:"))
                .count()
                > 1_000
        );
        assert!(
            knowledge
                .search("migraine", 10)
                .iter()
                .any(|item| item.label.to_ascii_lowercase().contains("migraine"))
        );
    }
}
