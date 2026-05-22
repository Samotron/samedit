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
    let start = from.min(haystack.len());
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
    let end = from.min(haystack.len());
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
}
