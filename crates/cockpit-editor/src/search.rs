//! Substring search over [`Buffer`](crate::Buffer).

use std::ops::Range;

use crate::Buffer;

/// Find every non-overlapping occurrence of `needle` in `buffer`.
pub fn find_all(buffer: &Buffer, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }

    let haystack = buffer.text();
    haystack
        .match_indices(needle)
        .map(|(start, matched)| start..start + matched.len())
        .collect()
}

/// Find the first occurrence at or after `from`.
pub fn find_next(buffer: &Buffer, needle: &str, from: usize) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }

    let haystack = buffer.text();
    let mut start = from.min(haystack.len());
    // `from` may land inside a multi-byte character; round up to a boundary so
    // the slice below cannot panic. A match can only start on a boundary.
    while start < haystack.len() && !haystack.is_char_boundary(start) {
        start += 1;
    }
    haystack[start..]
        .find(needle)
        .map(|relative| start + relative..start + relative + needle.len())
}

/// Find the last occurrence whose start is before or at `from`.
pub fn find_previous(buffer: &Buffer, needle: &str, from: usize) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }

    let haystack = buffer.text();
    let mut end = from.min(haystack.len());
    // Round `from` down to a character boundary so the slice cannot panic.
    while end > 0 && !haystack.is_char_boundary(end) {
        end -= 1;
    }
    haystack[..end]
        .rfind(needle)
        .map(|start| start..start + needle.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_non_overlapping_matches() {
        let buffer = Buffer::from("one two one");
        assert_eq!(find_all(&buffer, "one"), vec![0..3, 8..11]);
    }

    #[test]
    fn empty_needle_has_no_matches() {
        let buffer = Buffer::from("abc");
        assert!(find_all(&buffer, "").is_empty());
        assert_eq!(find_next(&buffer, "", 0), None);
        assert_eq!(find_previous(&buffer, "", 3), None);
    }

    #[test]
    fn finds_next_from_offset() {
        let buffer = Buffer::from("abc abc abc");
        assert_eq!(find_next(&buffer, "abc", 1), Some(4..7));
    }

    #[test]
    fn finds_previous_before_offset() {
        let buffer = Buffer::from("abc abc abc");
        assert_eq!(find_previous(&buffer, "abc", 8), Some(4..7));
    }

    #[test]
    fn reports_utf8_byte_ranges() {
        let buffer = Buffer::from("aé aé");
        assert_eq!(find_all(&buffer, "é"), vec![1..3, 5..7]);
    }

    #[test]
    fn tolerates_offsets_inside_multibyte_chars() {
        let buffer = Buffer::from("aé bé cé");
        // Byte 2 is inside the first 'é' (bytes 1..3): round to a boundary
        // rather than panic when slicing the haystack.
        assert_eq!(find_next(&buffer, "é", 2), Some(5..7));
        let _ = find_previous(&buffer, "é", 2);
    }
}
