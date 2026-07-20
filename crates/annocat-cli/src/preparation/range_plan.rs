use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const RANGE_PLAN_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct IndexedByteRange {
    pub chromosome: String,
    pub start: u64,
    pub end: u64,
    pub uncompressed_skip: u16,
}

impl IndexedByteRange {
    pub(super) fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CachedRangePlan {
    schema_version: u16,
    identity: String,
    ranges: Vec<IndexedByteRange>,
}

/// Reuse verified transport framing across restarts. The identity contains the
/// source and index artifact identities, so changing a release or mirror cannot
/// accidentally reuse offsets from different bytes.
pub(super) fn load_or_build(
    resource_root: &Path,
    name: &str,
    identity: &str,
    build: impl FnOnce() -> Result<Vec<IndexedByteRange>, String>,
) -> Result<Vec<IndexedByteRange>, String> {
    let directory = resource_root.join("range-plans");
    let path = directory.join(format!("{name}.json"));
    if let Ok(bytes) = fs::read(&path)
        && let Ok(cached) = serde_json::from_slice::<CachedRangePlan>(&bytes)
        && cached.schema_version == RANGE_PLAN_SCHEMA_VERSION
        && cached.identity == identity
        && valid_ranges(&cached.ranges)
    {
        return Ok(cached.ranges);
    }

    let ranges = build()?;
    if !valid_ranges(&ranges) {
        return Err("indexed source produced an invalid byte-range plan".into());
    }
    fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create range-plan directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(&CachedRangePlan {
        schema_version: RANGE_PLAN_SCHEMA_VERSION,
        identity: identity.into(),
        ranges: ranges.clone(),
    })
    .map_err(|error| format!("cannot encode indexed range plan: {error}"))?;
    fs::write(&temporary, encoded)
        .map_err(|error| format!("cannot write indexed range plan: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot publish indexed range plan: {error}"))?;
    Ok(ranges)
}

fn valid_ranges(ranges: &[IndexedByteRange]) -> bool {
    !ranges.is_empty()
        && ranges.iter().all(|range| {
            !range.chromosome.is_empty() && range.start <= range.end && range.len() > 0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_plan_is_reused_and_changed_identity_is_rebuilt() {
        let root = std::env::temp_dir().join(format!(
            "annocat-range-plan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        let first = IndexedByteRange {
            chromosome: "1".into(),
            start: 10,
            end: 20,
            uncompressed_skip: 3,
        };
        assert_eq!(
            load_or_build(&root, "source", "v1", || Ok(vec![first.clone()])).unwrap(),
            vec![first.clone()]
        );
        assert_eq!(
            load_or_build(&root, "source", "v1", || {
                Err("matching cache should have been reused".into())
            })
            .unwrap(),
            vec![first]
        );
        let replacement = IndexedByteRange {
            chromosome: "2".into(),
            start: 30,
            end: 40,
            uncompressed_skip: 0,
        };
        assert_eq!(
            load_or_build(&root, "source", "v2", || Ok(vec![replacement.clone()])).unwrap(),
            vec![replacement]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
