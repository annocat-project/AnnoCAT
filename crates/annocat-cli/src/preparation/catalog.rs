use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbnsfpArchiveShard {
    pub chromosome: String,
    pub member_name: String,
    /// Size of the gzip member payload after ZIP decompression. These are the
    /// exact bytes sent to fastVEP stdin.
    pub source_bytes: u64,
    pub compressed_bytes: u64,
    pub data_offset: u64,
    pub compression_method: u16,
    pub crc32: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbnsfpPinnedManifest {
    pub schema_version: u16,
    pub resource_id: String,
    pub release: String,
    pub archive_url: String,
    pub archive_bytes: u64,
    pub archive_md5: String,
    pub members: Vec<DbnsfpArchiveShard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedStreamShard {
    pub chromosome: String,
    pub url: String,
    pub compressed_bytes: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedShardedSource {
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub source_type: String,
    pub selected_schema: String,
    pub shards: Vec<PinnedStreamShard>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedStreamCatalog {
    schema_version: u16,
    sources: Vec<PinnedShardedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevelArchive {
    pub chromosome: String,
    pub filename: String,
    pub bytes: u64,
    pub md5: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevelArchiveManifest {
    schema_version: u16,
    pub resource_id: String,
    pub release: String,
    pub assembly: String,
    pub record_url: String,
    pub archives: Vec<RevelArchive>,
}

pub fn pinned_revel_manifest() -> Result<RevelArchiveManifest, String> {
    let manifest: RevelArchiveManifest =
        serde_json::from_str(include_str!("../../../../config/revel-1.3-archives.json"))
            .map_err(|error| format!("invalid pinned REVEL archive manifest: {error}"))?;
    let expected = canonical_chromosomes(false);
    if manifest.schema_version != 1
        || manifest.resource_id != "revel"
        || manifest.release != "1.3"
        || manifest.assembly != "GRCh38"
        || manifest.record_url != "https://zenodo.org/records/7072866"
        || manifest.archives.len() != expected.len()
        || manifest
            .archives
            .iter()
            .map(|item| item.chromosome.as_str())
            .ne(expected)
        || manifest.archives.iter().map(|item| item.bytes).sum::<u64>() != 667_188_638
        || manifest.archives.iter().any(|item| {
            item.bytes == 0
                || item.md5.len() != 32
                || !item.md5.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !item.filename.starts_with("revel-v1.3_segments_chrom_")
                || !item.filename.ends_with(".zip")
        })
    {
        return Err("pinned REVEL archive manifest identity is invalid".into());
    }
    Ok(manifest)
}

pub fn pinned_sharded_source(resource_id: &str) -> Result<PinnedShardedSource, String> {
    let catalog: PinnedStreamCatalog =
        serde_json::from_str(include_str!("../../../../config/wgs-streams.json"))
            .map_err(|error| format!("invalid pinned WGS stream catalog: {error}"))?;
    if catalog.schema_version != 1 || catalog.sources.len() != 3 {
        return Err("pinned WGS stream catalog identity is invalid".into());
    }
    let source = catalog
        .sources
        .into_iter()
        .find(|source| source.resource_id == resource_id)
        .ok_or_else(|| format!("resource '{resource_id}' has no pinned shard stream"))?;
    let expected = (1..=22)
        .map(|number| number.to_string())
        .chain(["X", "Y"].into_iter().map(str::to_string))
        .chain((resource_id == "phylop").then(|| "M".to_string()))
        .collect::<Vec<_>>();
    if source.assembly != "GRCh38"
        || source.shards.len() != expected.len()
        || source
            .shards
            .iter()
            .map(|shard| &shard.chromosome)
            .ne(expected.iter())
        || source.shards.iter().any(|shard| {
            shard.compressed_bytes == 0
                || !shard.url.starts_with("https://")
                || (shard.etag.is_none() && shard.last_modified.is_none())
        })
    {
        return Err(format!(
            "pinned {resource_id} chromosome stream metadata is invalid"
        ));
    }
    match resource_id {
        "gnomad"
            if source.release == "4.1.1-exomes"
                && source.source_type == "gnomad"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 199_241_266_182
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                    "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/",
                )
                }) => {}
        "gnomad-genomes"
            if source.release == "4.1.1-genomes"
                && source.source_type == "gnomad"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 565_643_483_329
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                    "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/genomes/",
                )
                }) => {}
        "phylop"
            if source.release == "hg38-100way-2015-05-08"
                && source.source_type == "phylop"
                && source
                    .shards
                    .iter()
                    .map(|shard| shard.compressed_bytes)
                    .sum::<u64>()
                    == 5_452_453_066
                && source.shards.iter().all(|shard| {
                    shard.url.starts_with(
                        "https://hgdownload.soe.ucsc.edu/goldenPath/hg38/phyloP100way/",
                    )
                }) => {}
        _ => return Err(format!("pinned {resource_id} stream identity is invalid")),
    }
    Ok(source)
}

pub fn pinned_dbnsfp_manifest() -> Result<DbnsfpPinnedManifest, String> {
    let manifest: DbnsfpPinnedManifest =
        serde_json::from_str(include_str!("../../../../config/dbnsfp-4.9a-members.json"))
            .map_err(|error| format!("invalid pinned dbNSFP member manifest: {error}"))?;
    if manifest.schema_version != 1
        || manifest.resource_id != "dbnsfp"
        || manifest.release != "4.9a"
        || manifest.archive_url
            != "https://usf.box.com/shared/static/0tq7q3b8ucaxxkmfyvnb0ss7g58ptgcl"
        || manifest.archive_bytes != 38_969_753_349
        || !manifest
            .archive_md5
            .eq_ignore_ascii_case("be89346ab3dc5c14a8a7b602f50c66fb")
        || manifest.members.len() != 25
    {
        return Err("pinned dbNSFP member manifest identity is invalid".into());
    }
    let expected = (1..=22)
        .map(|number| number.to_string())
        .chain(["X", "Y", "M"].into_iter().map(str::to_string));
    for (member, chromosome) in manifest.members.iter().zip(expected) {
        if member.chromosome != chromosome
            || member.compression_method != 0
            || member.source_bytes != member.compressed_bytes
            || member.source_bytes == 0
            || member.data_offset.saturating_add(member.compressed_bytes) > manifest.archive_bytes
        {
            return Err(format!(
                "pinned dbNSFP member metadata is invalid for chromosome {chromosome}"
            ));
        }
    }
    Ok(manifest)
}

pub(super) fn canonical_chromosomes(include_mitochondrial: bool) -> Vec<&'static str> {
    let mut chromosomes = vec![
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
        "17", "18", "19", "20", "21", "22", "X", "Y",
    ];
    if include_mitochondrial {
        chromosomes.push("M");
    }
    chromosomes
}
