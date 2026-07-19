use std::io::{Cursor, Read};

#[derive(Debug, Clone)]
pub(super) struct TabixReferenceOffset {
    pub(super) name: String,
    pub(super) virtual_offset: u64,
}

pub(super) fn parse_reference_offsets(
    compressed: &[u8],
) -> Result<Vec<TabixReferenceOffset>, String> {
    let mut decoded = Vec::new();
    flate2::read::MultiGzDecoder::new(Cursor::new(compressed))
        .read_to_end(&mut decoded)
        .map_err(|error| format!("cannot decompress CADD tabix index: {error}"))?;
    let mut input = Cursor::new(decoded);
    let mut magic = [0_u8; 4];
    input
        .read_exact(&mut magic)
        .map_err(|error| format!("truncated CADD tabix index: {error}"))?;
    if &magic != b"TBI\x01" {
        return Err("CADD index is not a tabix TBI file".into());
    }
    let reference_count = read_i32_le(&mut input)?;
    if !(1..=10_000).contains(&reference_count) {
        return Err("CADD tabix reference count is invalid".into());
    }
    for _ in 0..6 {
        read_i32_le(&mut input)?;
    }
    let name_bytes = read_i32_le(&mut input)?;
    if !(1..=1_000_000).contains(&name_bytes) {
        return Err("CADD tabix name table size is invalid".into());
    }
    let mut names = vec![0_u8; name_bytes as usize];
    input
        .read_exact(&mut names)
        .map_err(|error| format!("truncated CADD tabix name table: {error}"))?;
    let names = names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            std::str::from_utf8(name)
                .map(str::to_string)
                .map_err(|_| "CADD tabix reference name is not UTF-8".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.len() != reference_count as usize {
        return Err("CADD tabix reference name count is inconsistent".into());
    }
    let mut offsets = Vec::with_capacity(names.len());
    for name in names {
        let bin_count = read_i32_le(&mut input)?;
        if !(0..=1_000_000).contains(&bin_count) {
            return Err("CADD tabix bin count is invalid".into());
        }
        let mut first = None;
        for _ in 0..bin_count {
            let bin = read_u32_le(&mut input)?;
            let chunk_count = read_i32_le(&mut input)?;
            if !(0..=10_000_000).contains(&chunk_count) {
                return Err("CADD tabix chunk count is invalid".into());
            }
            for _ in 0..chunk_count {
                let begin = read_u64_le(&mut input)?;
                let _end = read_u64_le(&mut input)?;
                if bin < 37_450 && begin != 0 {
                    first = Some(first.map_or(begin, |current: u64| current.min(begin)));
                }
            }
        }
        let interval_count = read_i32_le(&mut input)?;
        if !(0..=100_000_000).contains(&interval_count) {
            return Err("CADD tabix linear index size is invalid".into());
        }
        for _ in 0..interval_count {
            let offset = read_u64_le(&mut input)?;
            if offset != 0 {
                first = Some(first.map_or(offset, |current: u64| current.min(offset)));
            }
        }
        offsets.push(TabixReferenceOffset {
            name,
            virtual_offset: first.ok_or("CADD tabix reference has no data chunks")?,
        });
    }
    Ok(offsets)
}

fn read_i32_le(input: &mut Cursor<Vec<u8>>) -> Result<i32, String> {
    let mut bytes = [0_u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|error| format!("truncated CADD tabix index: {error}"))?;
    Ok(i32::from_le_bytes(bytes))
}

fn read_u32_le(input: &mut Cursor<Vec<u8>>) -> Result<u32, String> {
    let mut bytes = [0_u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|error| format!("truncated CADD tabix index: {error}"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64_le(input: &mut Cursor<Vec<u8>>) -> Result<u64, String> {
    let mut bytes = [0_u8; 8];
    input
        .read_exact(&mut bytes)
        .map_err(|error| format!("truncated CADD tabix index: {error}"))?;
    Ok(u64::from_le_bytes(bytes))
}
