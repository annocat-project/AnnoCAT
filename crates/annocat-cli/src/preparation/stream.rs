use std::io::BufRead;

/// Reads the next complete, non-header record from an indexed text range.
///
/// Indexed BGZF ranges end on compressed block boundaries rather than record
/// boundaries, so the final decoded bytes can be the beginning of a record
/// owned by the next range. That incomplete tail is intentionally ignored.
pub(super) fn next_complete_data_line<R: BufRead>(
    input: &mut R,
    decode_context: &str,
) -> Result<Option<String>, String> {
    loop {
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|error| format!("{decode_context}: {error}"))?;
        if read == 0 || !line.ends_with('\n') {
            return Ok(None);
        }
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        return Ok(Some(line));
    }
}
