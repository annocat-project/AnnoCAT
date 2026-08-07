use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const SOURCE_URL: &str = "https://www.genenames.org/download/";
pub const CONTRACT_VERSION: &str = "hgnc-identity-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGene {
    pub symbol: String,
    pub gene_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Resolved(ResolvedGene),
    Ambiguous,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub gene: ResolvedGene,
    pub matched_text: String,
    pub match_kind: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct ResultKeys {
    pub symbols: HashSet<String>,
    pub gene_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct Gene {
    symbol: String,
    hgnc_id: String,
    ncbi_id: String,
    ensembl_id: String,
}

#[derive(Debug)]
struct Bundle {
    genes: Vec<Gene>,
    direct: HashMap<String, Vec<usize>>,
    historical: HashMap<String, Vec<usize>>,
}

#[derive(Debug)]
pub struct Resolver {
    bundle: Option<Arc<Bundle>>,
    identity_release: Option<String>,
    runtime_direct: HashMap<String, BTreeSet<String>>,
    canonical_ids: HashMap<String, BTreeSet<String>>,
    result_keys: HashMap<String, ResultKeys>,
}

impl Resolver {
    pub fn new(resources: &Path, report: &[(String, String)]) -> Self {
        let installed = installed_bundle(resources).ok();
        let mut resolver = Self {
            bundle: installed.as_ref().map(|(bundle, _)| bundle.clone()),
            identity_release: installed.map(|(_, release)| release),
            runtime_direct: HashMap::new(),
            canonical_ids: HashMap::new(),
            result_keys: HashMap::new(),
        };
        if let Ok(genes) = transcript_genes(resources) {
            for (symbol, gene_id) in genes.iter() {
                resolver.add_runtime(symbol, gene_id);
            }
        }
        for (symbol, gene_id) in report {
            resolver.add_runtime(symbol, gene_id);
        }
        for (symbol, gene_id) in report {
            let resolved = resolver
                .resolve_pair(gene_id, symbol)
                .resolved()
                .unwrap_or_else(|| ResolvedGene {
                    symbol: normalize(symbol),
                    gene_id: strip_ensembl_version(gene_id),
                });
            let keys = resolver.result_keys.entry(resolved.symbol).or_default();
            if !symbol.trim().is_empty() {
                keys.symbols.insert(normalize(symbol));
            }
            if !gene_id.trim().is_empty() {
                keys.gene_ids.insert(normalize(gene_id));
            }
        }
        resolver
    }

    pub fn identity_release(&self) -> Option<&str> {
        self.identity_release.as_deref()
    }

    pub fn resolve_pair(&self, id: &str, label: &str) -> Resolution {
        match self.resolve(id) {
            Resolution::Unknown => self.resolve(label),
            resolution => resolution,
        }
    }

    pub fn resolve(&self, value: &str) -> Resolution {
        let key = normalize_identifier(value);
        if key.is_empty() {
            return Resolution::Unknown;
        }
        if let Some(bundle) = &self.bundle {
            if let Some(candidates) = bundle.direct.get(&key) {
                return self.bundle_resolution(candidates);
            }
            if let Some(candidates) = bundle.historical.get(&key) {
                return self.bundle_resolution(candidates);
            }
        }
        self.runtime_direct
            .get(&key)
            .map_or(Resolution::Unknown, |symbols| {
                self.runtime_resolution(symbols)
            })
    }

    pub fn canonical_symbol(&self, symbol: &str) -> Option<ResolvedGene> {
        self.resolve(symbol).resolved()
    }

    pub fn canonicalize(&self, symbol: &str, gene_id: &str) -> ResolvedGene {
        self.resolve_pair(gene_id, symbol)
            .resolved()
            .unwrap_or_else(|| ResolvedGene {
                symbol: normalize(symbol),
                gene_id: strip_ensembl_version(gene_id),
            })
    }

    pub fn result_keys(&self, included: &std::collections::HashSet<String>) -> ResultKeys {
        let mut keys = ResultKeys::default();
        for symbol in included {
            if let Some(found) = self.result_keys.get(symbol) {
                keys.symbols.extend(found.symbols.iter().cloned());
                keys.gene_ids.extend(found.gene_ids.iter().cloned());
            }
        }
        keys
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchMatch> {
        let query = normalize_identifier(query);
        if query.len() < 2 {
            return Vec::new();
        }
        let mut matches = BTreeMap::<String, (u8, SearchMatch)>::new();
        if let Some(bundle) = &self.bundle {
            for (index, gene) in bundle.genes.iter().enumerate() {
                for (text, kind) in [
                    (gene.symbol.as_str(), "geneSymbol"),
                    (gene.hgnc_id.as_str(), "geneIdentifier"),
                    (gene.ncbi_id.as_str(), "geneIdentifier"),
                    (gene.ensembl_id.as_str(), "geneIdentifier"),
                ] {
                    add_search_match(&mut matches, &query, text, kind, self.bundle_gene(index));
                }
            }
            for (text, candidates) in &bundle.historical {
                if candidates.len() == 1 {
                    add_search_match(
                        &mut matches,
                        &query,
                        text,
                        "geneSymbol",
                        self.bundle_gene(candidates[0]),
                    );
                }
            }
        }
        for (text, symbols) in &self.runtime_direct {
            if symbols.len() == 1 {
                let symbol = symbols.iter().next().unwrap();
                add_search_match(
                    &mut matches,
                    &query,
                    text,
                    if text.starts_with("ENSG") {
                        "geneIdentifier"
                    } else {
                        "geneSymbol"
                    },
                    self.runtime_gene(symbol),
                );
            }
        }
        let mut matches = matches.into_values().collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.gene.symbol.cmp(&right.1.gene.symbol))
                .then(left.1.gene.gene_id.cmp(&right.1.gene.gene_id))
        });
        matches
            .into_iter()
            .take(limit.clamp(1, 100))
            .map(|(_, matched)| matched)
            .collect()
    }

    fn add_runtime(&mut self, symbol: &str, gene_id: &str) {
        let symbol = normalize(symbol);
        if symbol.is_empty() {
            return;
        }
        let gene_id = strip_ensembl_version(gene_id);
        self.runtime_direct
            .entry(symbol.clone())
            .or_default()
            .insert(symbol.clone());
        if !gene_id.is_empty() {
            self.runtime_direct
                .entry(gene_id.clone())
                .or_default()
                .insert(symbol.clone());
            self.canonical_ids
                .entry(symbol)
                .or_default()
                .insert(gene_id);
        }
    }

    fn runtime_resolution(&self, symbols: &BTreeSet<String>) -> Resolution {
        if symbols.len() != 1 {
            return Resolution::Ambiguous;
        }
        Resolution::Resolved(self.runtime_gene(symbols.iter().next().unwrap()))
    }

    fn runtime_gene(&self, symbol: &str) -> ResolvedGene {
        let ids = self.canonical_ids.get(symbol);
        ResolvedGene {
            symbol: symbol.to_owned(),
            gene_id: ids
                .filter(|ids| ids.len() == 1)
                .and_then(|ids| ids.iter().next().cloned())
                .unwrap_or_else(|| symbol.to_owned()),
        }
    }

    fn bundle_resolution(&self, candidates: &[usize]) -> Resolution {
        let unique = candidates.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != 1 {
            return Resolution::Ambiguous;
        }
        Resolution::Resolved(self.bundle_gene(*unique.iter().next().unwrap()))
    }

    fn bundle_gene(&self, index: usize) -> ResolvedGene {
        let gene = &self.bundle.as_ref().unwrap().genes[index];
        let runtime_id = self.canonical_ids.get(&gene.symbol);
        ResolvedGene {
            symbol: gene.symbol.clone(),
            gene_id: runtime_id
                .filter(|ids| ids.len() == 1)
                .and_then(|ids| ids.iter().next().cloned())
                .or_else(|| (!gene.ensembl_id.is_empty()).then(|| gene.ensembl_id.clone()))
                .unwrap_or_else(|| gene.symbol.clone()),
        }
    }
}

impl Resolution {
    pub fn resolved(self) -> Option<ResolvedGene> {
        match self {
            Self::Resolved(gene) => Some(gene),
            Self::Ambiguous | Self::Unknown => None,
        }
    }
}

fn add_search_match(
    matches: &mut BTreeMap<String, (u8, SearchMatch)>,
    query: &str,
    text: &str,
    kind: &'static str,
    gene: ResolvedGene,
) {
    if text.is_empty() {
        return;
    }
    let text = normalize_identifier(text);
    let score = if text == query {
        0
    } else if text.starts_with(query) {
        1
    } else if text.contains(query) {
        2
    } else {
        return;
    };
    let candidate = SearchMatch {
        gene: gene.clone(),
        matched_text: text,
        match_kind: kind,
    };
    matches
        .entry(gene.symbol)
        .and_modify(|current| {
            if score < current.0 {
                *current = (score, candidate.clone());
            }
        })
        .or_insert((score, candidate));
}

fn installed_bundle(resources: &Path) -> Result<(Arc<Bundle>, String), String> {
    type Cache = HashMap<(PathBuf, PathBuf, String), Arc<Bundle>>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let (complete, withdrawn, release) = super::phenotype::gene_identity_files(resources)
        .ok_or("HGNC gene identities are not installed")?;
    let key = (complete.clone(), withdrawn.clone(), release.clone());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(bundle) = cache
        .lock()
        .map_err(|_| "the HGNC identity cache is unavailable")?
        .get(&key)
        .cloned()
    {
        return Ok((bundle, release));
    }
    let bundle = Arc::new(load_bundle(&complete, &withdrawn)?);
    cache
        .lock()
        .map_err(|_| "the HGNC identity cache is unavailable")?
        .insert(key, bundle.clone());
    Ok((bundle, release))
}

pub(crate) fn validate_files(complete: &Path, withdrawn: &Path) -> Result<usize, String> {
    Ok(load_bundle(complete, withdrawn)?.genes.len())
}

fn load_bundle(complete: &Path, withdrawn: &Path) -> Result<Bundle, String> {
    parse_bundle(
        BufReader::new(
            File::open(complete)
                .map_err(|error| format!("cannot read installed HGNC identities: {error}"))?,
        ),
        BufReader::new(File::open(withdrawn).map_err(|error| {
            format!("cannot read installed withdrawn HGNC identities: {error}")
        })?),
    )
}

fn parse_bundle(mut complete: impl BufRead, withdrawn: impl BufRead) -> Result<Bundle, String> {
    let mut header = String::new();
    complete
        .read_line(&mut header)
        .map_err(|error| format!("cannot read installed HGNC identities: {error}"))?;
    if header.is_empty() {
        return Err("the installed HGNC identities are empty".into());
    }
    let columns = header
        .trim_end_matches(['\r', '\n'])
        .split('\t')
        .enumerate()
        .map(|(index, name)| (name, index))
        .collect::<HashMap<_, _>>();
    let column = |name| {
        columns
            .get(name)
            .copied()
            .ok_or_else(|| format!("the installed HGNC identities have no {name} column"))
    };
    let hgnc = column("hgnc_id")?;
    let symbol = column("symbol")?;
    let status = column("status")?;
    let alias = column("alias_symbol")?;
    let previous = column("prev_symbol")?;
    let ncbi = column("entrez_id")?;
    let ensembl = column("ensembl_gene_id")?;
    let mut genes = Vec::new();
    let mut historical_fields = Vec::new();
    for line in complete.lines() {
        let line =
            line.map_err(|error| format!("cannot read installed HGNC identities: {error}"))?;
        let fields = line.split('\t').map(unquote).collect::<Vec<_>>();
        if fields.get(status).copied() != Some("Approved") {
            continue;
        }
        let gene = Gene {
            symbol: normalize(field(&fields, symbol)),
            hgnc_id: normalize_identifier(field(&fields, hgnc)),
            ncbi_id: normalize_identifier(field(&fields, ncbi)),
            ensembl_id: strip_ensembl_version(field(&fields, ensembl)),
        };
        historical_fields.push((
            field(&fields, alias).to_owned(),
            field(&fields, previous).to_owned(),
        ));
        genes.push(gene);
    }
    if genes.is_empty() {
        return Err("the installed HGNC identities contain no approved genes".into());
    }
    let mut direct = HashMap::<String, Vec<usize>>::new();
    let mut historical = HashMap::<String, Vec<usize>>::new();
    let mut by_hgnc = HashMap::new();
    for (index, gene) in genes.iter().enumerate() {
        by_hgnc.insert(gene.hgnc_id.clone(), index);
        for key in [&gene.symbol, &gene.hgnc_id, &gene.ncbi_id, &gene.ensembl_id] {
            insert_candidate(&mut direct, key, index);
        }
        for value in [&historical_fields[index].0, &historical_fields[index].1] {
            for key in value.split('|') {
                insert_candidate(&mut historical, &normalize(key), index);
            }
        }
    }
    let mut lines = withdrawn.lines();
    let header = lines
        .next()
        .ok_or("the installed withdrawn HGNC identities are empty")?
        .map_err(|error| format!("cannot read installed withdrawn HGNC identities: {error}"))?;
    let columns = header
        .split('\t')
        .enumerate()
        .map(|(index, name)| (name, index))
        .collect::<HashMap<_, _>>();
    let withdrawn_symbol = *columns
        .get("WITHDRAWN_SYMBOL")
        .ok_or("the installed withdrawn HGNC identities have no symbol column")?;
    let merged = *columns
        .get("MERGED_INTO_REPORT(S) (i.e HGNC_ID|SYMBOL|STATUS)")
        .ok_or("the installed withdrawn HGNC identities have no merged-target column")?;
    for line in lines {
        let line = line
            .map_err(|error| format!("cannot read installed withdrawn HGNC identities: {error}"))?;
        let fields = line.split('\t').map(unquote).collect::<Vec<_>>();
        let key = normalize(field(&fields, withdrawn_symbol));
        for target in field(&fields, merged).split(',') {
            let hgnc_id = normalize_identifier(target.trim().split('|').next().unwrap_or(""));
            if let Some(index) = by_hgnc.get(&hgnc_id) {
                insert_candidate(&mut historical, &key, *index);
            }
        }
    }
    Ok(Bundle {
        genes,
        direct,
        historical,
    })
}

fn insert_candidate(map: &mut HashMap<String, Vec<usize>>, key: &str, index: usize) {
    if key.is_empty() {
        return;
    }
    let values = map.entry(key.to_owned()).or_default();
    if !values.contains(&index) {
        values.push(index);
    }
}

fn field<'a>(fields: &'a [&str], index: usize) -> &'a str {
    fields.get(index).copied().unwrap_or("")
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn normalize_identifier(value: &str) -> String {
    let value = normalize(value);
    strip_ensembl_version(&value)
}

fn strip_ensembl_version(value: &str) -> String {
    let value = normalize(value);
    let Some((gene_id, version)) = value.split_once('.') else {
        return value;
    };
    let suffix = gene_id.strip_prefix("ENSG").unwrap_or("");
    if !suffix.is_empty()
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
    {
        gene_id.to_owned()
    } else {
        value
    }
}

fn transcript_genes(resources: &Path) -> Result<Arc<Vec<(String, String)>>, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Vec<(String, String)>>>>> = OnceLock::new();
    let path = resources
        .canonicalize()
        .unwrap_or_else(|_| resources.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(genes) = cache
        .lock()
        .map_err(|_| "the transcript gene cache is unavailable")?
        .get(&path)
        .cloned()
    {
        return Ok(genes);
    }
    let genes: Arc<Vec<(String, String)>> = Arc::new(
        super::transcript::gene_dictionary(resources)?
            .into_iter()
            .map(|gene| (gene.symbol, gene.gene_id))
            .collect(),
    );
    cache
        .lock()
        .map_err(|_| "the transcript gene cache is unavailable")?
        .insert(path, genes.clone());
    Ok(genes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_resolver() -> Resolver {
        let complete = concat!(
            "hgnc_id\tsymbol\tstatus\talias_symbol\tprev_symbol\tentrez_id\tensembl_gene_id\n",
            "HGNC:11998\tTP53\tApproved\tACS3\t\t7157\tENSG00000141510\n",
            "HGNC:391\tUBA1\tApproved\tA1S9T\t\t7317\tENSG00000130985\n",
            "HGNC:2\tOTHER\tApproved\tACS3\t\t2\tENSG00000000002\n",
        );
        let withdrawn = concat!(
            "WITHDRAWN_SYMBOL\tMERGED_INTO_REPORT(S) (i.e HGNC_ID|SYMBOL|STATUS)\n",
            "CBBM\tHGNC:11998|TP53|Approved,HGNC:391|UBA1|Approved\n",
        );
        Resolver {
            bundle: Some(Arc::new(
                parse_bundle(Cursor::new(complete), Cursor::new(withdrawn)).unwrap(),
            )),
            identity_release: Some("2026-08-07".into()),
            runtime_direct: HashMap::new(),
            canonical_ids: HashMap::new(),
            result_keys: HashMap::new(),
        }
    }

    #[test]
    fn resolves_current_and_historical_hgnc_identities() {
        let resolver = test_resolver();
        for value in ["TP53", "HGNC:11998", "7157", "ENSG00000141510.18"] {
            assert_eq!(resolver.resolve(value).resolved().unwrap().symbol, "TP53");
        }
        assert_eq!(resolver.resolve("ENSG00000141510.bad"), Resolution::Unknown);
        assert_eq!(resolver.resolve("A1S9T").resolved().unwrap().symbol, "UBA1");
        assert_eq!(resolver.resolve("ACS3"), Resolution::Ambiguous);
        assert_eq!(resolver.resolve("CBBM"), Resolution::Ambiguous);
    }

    #[test]
    fn preserves_non_hgnc_result_identities() {
        let resolver = Resolver::new(
            Path::new("missing"),
            &[("TESTGENE".into(), "ENSG99999999999".into())],
        );
        assert_eq!(
            resolver.resolve("TESTGENE"),
            Resolution::Resolved(ResolvedGene {
                symbol: "TESTGENE".into(),
                gene_id: "ENSG99999999999".into(),
            })
        );
    }

    #[test]
    fn approved_symbols_take_priority_over_historical_names() {
        let bundle = Arc::new(Bundle {
            genes: vec![
                Gene {
                    symbol: "CURRENT".into(),
                    hgnc_id: "HGNC:1".into(),
                    ncbi_id: "1".into(),
                    ensembl_id: "ENSG1".into(),
                },
                Gene {
                    symbol: "OTHER".into(),
                    hgnc_id: "HGNC:2".into(),
                    ncbi_id: "2".into(),
                    ensembl_id: "ENSG2".into(),
                },
            ],
            direct: HashMap::from([("CURRENT".into(), vec![0])]),
            historical: HashMap::from([("CURRENT".into(), vec![1])]),
        });
        let resolver = Resolver {
            bundle: Some(bundle),
            identity_release: Some("test".into()),
            runtime_direct: HashMap::new(),
            canonical_ids: HashMap::new(),
            result_keys: HashMap::new(),
        };

        assert_eq!(
            resolver.resolve("CURRENT").resolved().unwrap().symbol,
            "CURRENT"
        );
    }
}
