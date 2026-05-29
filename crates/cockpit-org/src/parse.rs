//! Parse `.org` source into an [`OrgFile`].
//!
//! Strategy: orgize owns the fiddly inline grammar (title vs. keyword vs.
//! priority vs. tags, and the planning timestamps). We own the *positions* —
//! orgize 0.9 does not expose source ranges, so we scan the raw source for
//! headline lines (`^\*+(space|eol)`) and zip them, in document order, against
//! orgize's pre-order headline list. The two enumerations agree 1:1 for the
//! v0.12 subset (no code/example blocks, where a leading `*` could otherwise be
//! body text).

use std::path::Path;

use orgize::{Org, ParseConfig};

use crate::keywords::Keywords;
use crate::model::{Heading, OrgFile};
use crate::timestamp::{Timestamp, parse_timestamp};

/// Parse `source` (loaded from `path`) using the default `TODO | DONE` workflow.
pub fn parse_file(path: impl AsRef<Path>, source: impl Into<String>) -> OrgFile {
    parse_file_with(path, source, &Keywords::default())
}

/// Parse `source` using a caller-supplied TODO-keyword workflow.
pub fn parse_file_with(
    path: impl AsRef<Path>,
    source: impl Into<String>,
    keywords: &Keywords,
) -> OrgFile {
    let source = source.into();
    let content_hash = OrgFile::hash_source(&source);
    let headings = parse_headings(&source, keywords);
    OrgFile {
        path: path.as_ref().to_path_buf(),
        source,
        content_hash,
        headings,
    }
}

/// Parse just the heading tree out of a source string.
pub fn parse_headings(source: &str, keywords: &Keywords) -> Vec<Heading> {
    let (todo, done) = keywords.as_orgize();
    let config = ParseConfig {
        todo_keywords: (todo, done),
    };
    let org = Org::parse_custom(source, &config);

    // orgize handles the inline grammar (title vs. keyword vs. priority vs.
    // tags). Planning timestamps are parsed separately below, because orgize
    // 0.9 drops any timestamp carrying a repeater/delay cookie.
    let parsed: Vec<ParsedHeadline> = org
        .headlines()
        .map(|hl| {
            let title = hl.title(&org);
            ParsedHeadline {
                level: title.level,
                title: title.raw.trim().to_string(),
                todo_keyword: title.keyword.as_ref().map(|k| k.to_string()),
                priority: title.priority,
                tags: title.tags.iter().map(|t| t.to_string()).collect(),
            }
        })
        .collect();

    // Line indices (0-based) of every headline line, in document order.
    let headline_lines: Vec<usize> = headline_line_indices(source);

    // For the subset, both enumerations agree. If they ever diverge (a stray
    // `*` we mis-scan, or an orgize quirk), fall back to the shorter length so
    // we never index out of bounds — the worst case is a missing tail heading,
    // not a panic.
    let n = parsed.len().min(headline_lines.len());
    let total_lines = source.lines().count();

    let lines: Vec<&str> = source.lines().collect();
    let mut flat: Vec<Heading> = Vec::with_capacity(n);
    for i in 0..n {
        let start = headline_lines[i];
        // Own extent ends at the next headline line of any level, or EOF.
        let end = headline_lines.get(i + 1).copied().unwrap_or(total_lines);
        let p = &parsed[i];

        // Planning, if any, sits on the line immediately after the headline.
        let planning = lines
            .get(start + 1)
            .filter(|l| is_planning_line(l))
            .map(|l| parse_planning(l))
            .unwrap_or_default();

        let body = section_body(source, start, end);
        flat.push(Heading {
            level: p.level,
            title: p.title.clone(),
            todo_keyword: p.todo_keyword.clone(),
            priority: p.priority,
            tags: p.tags.clone(),
            scheduled: planning.scheduled,
            deadline: planning.deadline,
            closed: planning.closed,
            body,
            line_range: start..end,
            children: Vec::new(),
        });
    }

    nest(flat)
}

struct ParsedHeadline {
    level: usize,
    title: String,
    todo_keyword: Option<String>,
    priority: Option<char>,
    tags: Vec<String>,
}

/// Planning timestamps extracted from a `SCHEDULED:/DEADLINE:/CLOSED:` line.
#[derive(Default)]
struct Planning {
    scheduled: Option<Timestamp>,
    deadline: Option<Timestamp>,
    closed: Option<Timestamp>,
}

fn parse_planning(line: &str) -> Planning {
    Planning {
        scheduled: extract_stamp(line, "SCHEDULED:"),
        deadline: extract_stamp(line, "DEADLINE:"),
        closed: extract_stamp(line, "CLOSED:"),
    }
}

/// Find `keyword` in `line` and parse the timestamp that follows it.
fn extract_stamp(line: &str, keyword: &str) -> Option<Timestamp> {
    let pos = line.find(keyword)?;
    let after = &line[pos + keyword.len()..];
    let bracket = after.find(['<', '['])?;
    parse_timestamp(&after[bracket..])
}

/// 0-based line indices of every Org headline line in `source`.
///
/// A headline line is one or more `*` at column 0 followed by a space or the
/// end of the line — the same rule orgize applies for the v0.12 subset.
pub fn headline_line_indices(source: &str) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| is_headline_line(line))
        .map(|(i, _)| i)
        .collect()
}

fn is_headline_line(line: &str) -> bool {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    if stars == 0 {
        return false;
    }
    matches!(line.as_bytes().get(stars), None | Some(b' '))
}

/// Body text of a heading's own section: lines after the headline line and any
/// leading planning line, joined with `\n`. Trailing blank lines are trimmed.
fn section_body(source: &str, start: usize, end: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut idx = start + 1;
    // Skip a leading planning line (SCHEDULED/DEADLINE/CLOSED), if present.
    if idx < end && idx < lines.len() && is_planning_line(lines[idx]) {
        idx += 1;
    }
    let body_lines = &lines[idx..end.min(lines.len())];
    body_lines.join("\n").trim_end().to_string()
}

fn is_planning_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("SCHEDULED:") || t.starts_with("DEADLINE:") || t.starts_with("CLOSED:")
}

/// Turn a flat, document-ordered heading list into a nested tree using levels.
fn nest(flat: Vec<Heading>) -> Vec<Heading> {
    let mut roots: Vec<Heading> = Vec::new();
    // Stack of indices into a growing tree is awkward; instead recurse via a
    // simple owned stack of in-progress headings.
    let mut stack: Vec<Heading> = Vec::new();

    for mut h in flat {
        // Pop everything at this level or deeper, attaching each to its parent.
        while let Some(top) = stack.last() {
            if top.level >= h.level {
                let finished = stack.pop().unwrap();
                attach(&mut stack, &mut roots, finished);
            } else {
                break;
            }
        }
        // `h` becomes the new open node.
        h.children.clear();
        stack.push(h);
    }
    while let Some(finished) = stack.pop() {
        attach(&mut stack, &mut roots, finished);
    }
    roots
}

/// Attach a finished heading to the current parent (top of stack) or to roots.
fn attach(stack: &mut [Heading], roots: &mut Vec<Heading>, finished: Heading) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(finished);
    } else {
        roots.push(finished);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timestamp::OrgDate;

    #[test]
    fn flat_headlines_and_ranges() {
        let src = "* First\nbody one\n* Second\nbody two\nmore\n";
        let file = parse_file("t.org", src);
        assert_eq!(file.headings.len(), 2);
        assert_eq!(file.headings[0].title, "First");
        assert_eq!(file.headings[0].line_range, 0..2);
        assert_eq!(file.headings[1].title, "Second");
        assert_eq!(file.headings[1].line_range, 2..5);
        assert_eq!(file.headings[0].body, "body one");
    }

    #[test]
    fn keyword_priority_tags() {
        let src = "* TODO [#A] Ship it :work:urgent:\n";
        let file = parse_file("t.org", src);
        let h = &file.headings[0];
        assert_eq!(h.todo_keyword.as_deref(), Some("TODO"));
        assert_eq!(h.priority, Some('A'));
        assert_eq!(h.title, "Ship it");
        assert_eq!(h.tags, ["work", "urgent"]);
    }

    #[test]
    fn nesting_by_level() {
        let src = "* Parent\n** Child A\n*** Grandchild\n** Child B\n* Sibling\n";
        let file = parse_file("t.org", src);
        assert_eq!(file.headings.len(), 2); // Parent, Sibling
        let parent = &file.headings[0];
        assert_eq!(parent.children.len(), 2); // Child A, Child B
        assert_eq!(parent.children[0].children.len(), 1); // Grandchild
        assert_eq!(parent.children[0].children[0].title, "Grandchild");

        // Pre-order over the whole file.
        let titles: Vec<_> = file.iter_headings().map(|h| h.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Parent", "Child A", "Grandchild", "Child B", "Sibling"]
        );
    }

    #[test]
    fn planning_extracted_and_excluded_from_body() {
        let src = "* TODO Task\nSCHEDULED: <2026-06-01 Mon>\nthe body\n";
        let file = parse_file("t.org", src);
        let h = &file.headings[0];
        assert_eq!(
            h.scheduled.as_ref().map(|t| t.date),
            Some(OrgDate::new(2026, 6, 1))
        );
        assert_eq!(h.body, "the body");
    }

    #[test]
    fn subtree_range_covers_children() {
        let src = "* Parent\n** Child\nbody\n* Next\n";
        let file = parse_file("t.org", src);
        assert_eq!(file.headings[0].line_range, 0..1);
        assert_eq!(file.headings[0].subtree_line_range(), 0..3);
    }
}
