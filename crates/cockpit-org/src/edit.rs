//! Line-range edit primitives.
//!
//! AGENTS.md's hard round-trip rule: we mutate `.org` files by replacing line
//! ranges in the **original source buffer**, never by re-emitting an AST. Every
//! editing operation in this crate funnels through [`replace_line_range`] /
//! [`replace_line_content`] so untouched bytes stay byte-identical.

use std::ops::Range;

use crate::keywords::Keywords;
use crate::model::Heading;
use crate::timestamp::Timestamp;

/// Byte offset of the start of each `lines()` line, with a final sentinel at
/// `source.len()`. Indexing past the end yields `source.len()`.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn offset_at(starts: &[usize], source: &str, line: usize) -> usize {
    starts.get(line).copied().unwrap_or(source.len())
}

/// Byte offset of the start of line `line` (0-based). A `line` at or past the
/// end of the buffer returns `source.len()`, so this doubles as the insertion
/// point for appending at EOF.
pub fn byte_offset_of_line(source: &str, line: usize) -> usize {
    offset_at(&line_starts(source), source, line)
}

/// Insert `text` verbatim at the start of line `line`. Equivalent to replacing
/// the empty range `line..line`; everything else stays byte-identical.
pub fn insert_at_line(source: &str, line: usize, text: &str) -> String {
    replace_line_range(source, line..line, text)
}

/// Replace the lines in `range` (half-open, 0-based) with `replacement`,
/// verbatim. The replaced span runs from the first byte of line `range.start`
/// up to the first byte of line `range.end` — i.e. it includes the trailing
/// newline of every replaced line. `replacement` must therefore carry its own
/// terminators. Everything outside the span is left byte-for-byte unchanged.
pub fn replace_line_range(source: &str, range: Range<usize>, replacement: &str) -> String {
    let starts = line_starts(source);
    let from = offset_at(&starts, source, range.start);
    let to = offset_at(&starts, source, range.end).max(from);
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.push_str(&source[..from]);
    out.push_str(replacement);
    out.push_str(&source[to..]);
    out
}

/// Replace the *content* of a single line (its terminator is preserved).
pub fn replace_line_content(source: &str, line: usize, new_content: &str) -> String {
    let starts = line_starts(source);
    let start = offset_at(&starts, source, line);
    let next = offset_at(&starts, source, line + 1);
    // Content end excludes the line terminator.
    let mut end = next;
    if end > start && source.as_bytes().get(end - 1) == Some(&b'\n') {
        end -= 1;
        if end > start && source.as_bytes().get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
    }
    let mut out = String::with_capacity(source.len() + new_content.len());
    out.push_str(&source[..start]);
    out.push_str(new_content);
    out.push_str(&source[end..]);
    out
}

/// Rewrite a headline line so its TODO keyword becomes `keyword` (or none),
/// preserving the stars, leading spacing, priority cookie, title, and tags.
fn rewrite_keyword_line(line: &str, keywords: &Keywords, keyword: Option<&str>) -> String {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    let after_stars = &line[stars..];
    let trimmed = after_stars.trim_start_matches(' ');
    let lead = &after_stars[..after_stars.len() - trimmed.len()];

    // Strip an existing keyword if the first word is one.
    let first_word = trimmed.split(' ').next().unwrap_or("");
    let body = if !first_word.is_empty() && keywords.contains(first_word) {
        trimmed[first_word.len()..].trim_start_matches(' ')
    } else {
        trimmed
    };

    let kw = keyword.map(|k| format!("{k} ")).unwrap_or_default();
    format!("{}{}{}{}", &line[..stars], lead, kw, body)
}

/// Set the TODO keyword on `heading` to `keyword` (`None` clears it), returning
/// the new full source. Only the headline line changes.
pub fn set_todo(
    source: &str,
    heading: &Heading,
    keywords: &Keywords,
    keyword: Option<&str>,
) -> String {
    let line_idx = heading.line_range.start;
    let line = source.lines().nth(line_idx).unwrap_or("");
    let new_line = rewrite_keyword_line(line, keywords, keyword);
    replace_line_content(source, line_idx, &new_line)
}

/// Cycle `heading`'s TODO keyword to the next workflow state
/// (`None → TODO → DONE → None` on the default workflow), returning the new
/// full source.
pub fn cycle_todo(source: &str, heading: &Heading, keywords: &Keywords) -> String {
    let next = keywords.cycle(heading.todo_keyword.as_deref());
    set_todo(source, heading, keywords, next.as_deref())
}

/// `true` if `line` is a planning line (`SCHEDULED:` / `DEADLINE:` / `CLOSED:`,
/// possibly indented).
pub(crate) fn is_planning_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("SCHEDULED:") || t.starts_with("DEADLINE:") || t.starts_with("CLOSED:")
}

/// Replace the first `<...>` / `[...]` timestamp following `keyword` in `line`
/// with `new_stamp`, leaving everything else byte-identical. Returns `line`
/// unchanged if the keyword or a bracket isn't found.
pub(crate) fn replace_stamp_after(line: &str, keyword: &str, new_stamp: &str) -> String {
    let Some(kw_pos) = line.find(keyword) else {
        return line.to_string();
    };
    let after = &line[kw_pos + keyword.len()..];
    let Some(rel_open) = after.find(['<', '[']) else {
        return line.to_string();
    };
    let open_byte = kw_pos + keyword.len() + rel_open;
    let close_char = if line.as_bytes()[open_byte] == b'<' {
        '>'
    } else {
        ']'
    };
    let Some(rel_close) = line[open_byte..].find(close_char) else {
        return line.to_string();
    };
    let close_byte = open_byte + rel_close + 1; // inclusive of the closing bracket
    format!("{}{}{}", &line[..open_byte], new_stamp, &line[close_byte..])
}

/// Set `heading`'s `SCHEDULED:` timestamp, returning the new full source.
pub fn set_scheduled(source: &str, heading: &Heading, timestamp: &Timestamp) -> String {
    set_planning(source, heading, "SCHEDULED", timestamp)
}

/// Set `heading`'s `DEADLINE:` timestamp, returning the new full source.
pub fn set_deadline(source: &str, heading: &Heading, timestamp: &Timestamp) -> String {
    set_planning(source, heading, "DEADLINE", timestamp)
}

/// Insert or update a planning timestamp on `heading`.
///
/// - If the heading already has a planning line carrying `keyword`, only that
///   timestamp is rewritten (the line's other planning entries and indentation
///   are preserved).
/// - If a planning line exists without `keyword`, the `keyword: <stamp>` pair
///   is appended to it.
/// - Otherwise a new, un-indented planning line is inserted right after the
///   headline (matching modern Emacs, `org-adapt-indentation` nil).
///
/// Everything outside the touched line stays byte-identical.
fn set_planning(source: &str, heading: &Heading, keyword: &str, timestamp: &Timestamp) -> String {
    let stamp = timestamp.format();
    let headline = heading.line_range.start;
    let plan_idx = headline + 1;
    let lines: Vec<&str> = source.lines().collect();

    let has_planning = plan_idx < heading.line_range.end
        && lines.get(plan_idx).is_some_and(|l| is_planning_line(l));

    if has_planning {
        let line = lines[plan_idx];
        let kw = format!("{keyword}:");
        let new_line = if line.contains(&kw) {
            replace_stamp_after(line, &kw, &stamp)
        } else {
            format!("{line} {keyword}: {stamp}")
        };
        replace_line_content(source, plan_idx, &new_line)
    } else {
        insert_at_line(source, plan_idx, &format!("{keyword}: {stamp}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_file;

    #[test]
    fn replace_line_range_preserves_surroundings() {
        let src = "a\nb\nc\nd\n";
        let out = replace_line_range(src, 1..3, "X\nY\n");
        assert_eq!(out, "a\nX\nY\nd\n");
    }

    #[test]
    fn replace_line_range_at_eof_without_trailing_newline() {
        let src = "a\nb";
        let out = replace_line_range(src, 1..2, "B");
        assert_eq!(out, "a\nB");
    }

    #[test]
    fn replace_line_content_keeps_terminator() {
        let src = "one\ntwo\nthree\n";
        let out = replace_line_content(src, 1, "TWO");
        assert_eq!(out, "one\nTWO\nthree\n");
    }

    #[test]
    fn cycle_todo_adds_then_advances_then_clears() {
        let src = "* [#A] Task :work:\nbody stays\n";
        let kw = Keywords::default();

        let file = parse_file("t.org", src);
        let s1 = cycle_todo(src, &file.headings[0], &kw);
        assert_eq!(s1, "* TODO [#A] Task :work:\nbody stays\n");

        let f1 = parse_file("t.org", &s1);
        let s2 = cycle_todo(&s1, &f1.headings[0], &kw);
        assert_eq!(s2, "* DONE [#A] Task :work:\nbody stays\n");

        let f2 = parse_file("t.org", &s2);
        let s3 = cycle_todo(&s2, &f2.headings[0], &kw);
        assert_eq!(s3, "* [#A] Task :work:\nbody stays\n");
    }

    #[test]
    fn set_todo_only_touches_headline_line() {
        let src = "* one\n* TODO two\nbody\n* three\n";
        let kw = Keywords::default();
        let file = parse_file("t.org", src);
        // Clear the keyword on the middle heading.
        let out = set_todo(src, &file.headings[1], &kw, None);
        assert_eq!(out, "* one\n* two\nbody\n* three\n");
    }

    fn date(y: i32, m: u32, d: u32) -> crate::timestamp::OrgDate {
        crate::timestamp::OrgDate::new(y, m, d)
    }

    #[test]
    fn set_scheduled_inserts_planning_line_when_absent() {
        let src = "* TODO task\nbody\n* next\n";
        let file = parse_file("t.org", src);
        let ts = Timestamp::active_date(date(2026, 6, 1));
        let out = set_scheduled(src, &file.headings[0], &ts);
        assert_eq!(
            out,
            "* TODO task\nSCHEDULED: <2026-06-01 Mon>\nbody\n* next\n"
        );
    }

    #[test]
    fn set_scheduled_updates_existing_stamp_in_place() {
        let src = "* TODO task\n  SCHEDULED: <2026-06-01 Mon>\nbody\n";
        let file = parse_file("t.org", src);
        let ts = Timestamp::active_date(date(2026, 6, 8));
        let out = set_scheduled(src, &file.headings[0], &ts);
        // Only the date changes; the original indentation is preserved.
        assert_eq!(out, "* TODO task\n  SCHEDULED: <2026-06-08 Mon>\nbody\n");
    }

    #[test]
    fn set_deadline_appends_to_existing_planning_line() {
        let src = "* TODO task\nSCHEDULED: <2026-06-01 Mon>\nbody\n";
        let file = parse_file("t.org", src);
        let ts =
            Timestamp::active_datetime(date(2026, 6, 5), crate::timestamp::OrgTime::new(17, 0));
        let out = set_deadline(src, &file.headings[0], &ts);
        assert_eq!(
            out,
            "* TODO task\nSCHEDULED: <2026-06-01 Mon> DEADLINE: <2026-06-05 Fri 17:00>\nbody\n"
        );
    }
}
