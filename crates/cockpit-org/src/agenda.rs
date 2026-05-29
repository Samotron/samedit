//! Agenda views and repeater handling (M12.5b).
//!
//! Pure functions over an in-memory [`OrgRoot`] — no disk access, no clock.
//! "Today" is passed in by the caller (a frozen [`OrgDate`] in tests). Three
//! views:
//! - [`today`] — items scheduled/deadlined for the day, plus overdue open items.
//! - [`next_7_days`] — one block per day for the coming week.
//! - [`todo_list`] — every open TODO headline, grouped by file, ignoring dates.
//!
//! Plus [`complete`], which marks a headline done — and, when it carries a
//! repeating timestamp, bumps that timestamp forward one period (Emacs
//! semantics) instead, via the line-range edit primitives so the rest of the
//! file stays byte-identical.

use std::path::PathBuf;

use crate::date;
use crate::edit::{replace_line_content, set_todo};
use crate::keywords::Keywords;
use crate::model::{Heading, OrgFile};
use crate::store::OrgRoot;
use crate::timestamp::{OrgDate, OrgTime, Timestamp};

/// What kind of agenda line an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaKind {
    Scheduled,
    Deadline,
    /// A bare TODO headline (no date), used by the TODO-list view.
    Todo,
}

/// One agenda line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaItem {
    pub file: PathBuf,
    pub title: String,
    pub todo_keyword: Option<String>,
    pub tags: Vec<String>,
    pub kind: AgendaKind,
    /// The relevant date (Scheduled/Deadline). `None` for bare TODOs.
    pub date: Option<OrgDate>,
    pub time: Option<OrgTime>,
    /// `true` if this is a past-due open item surfaced in the Today view.
    pub overdue: bool,
    /// Headline line (0-based) for jump-to.
    pub line: usize,
    pub level: usize,
}

/// One day's block in the next-7-days view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaDay {
    pub date: OrgDate,
    pub items: Vec<AgendaItem>,
}

/// One file's group in the TODO-list view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaFileGroup {
    pub file: PathBuf,
    pub items: Vec<AgendaItem>,
}

/// A small subset of Org's agenda filter: required/excluded tags, plus optional
/// TODO-keyword and file restrictions. Parse `+work-personal`-style strings via
/// [`Filter::parse`]; add keyword/file axes with the builder methods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub keywords: Vec<String>,
    pub files: Vec<String>,
}

impl Filter {
    /// Parse a tag query like `+work-personal` (require `work`, exclude
    /// `personal`). Whitespace between tokens is allowed. Tokens without a
    /// leading sign are treated as required tags.
    pub fn parse(query: &str) -> Self {
        let mut filter = Filter::default();
        let mut sign = '+';
        let mut buf = String::new();
        let flush = |filter: &mut Filter, sign: char, buf: &mut String| {
            if !buf.is_empty() {
                if sign == '-' {
                    filter.exclude_tags.push(std::mem::take(buf));
                } else {
                    filter.include_tags.push(std::mem::take(buf));
                }
            }
        };
        for c in query.chars() {
            match c {
                '+' | '-' => {
                    flush(&mut filter, sign, &mut buf);
                    sign = c;
                }
                c if c.is_whitespace() => {
                    flush(&mut filter, sign, &mut buf);
                    sign = '+';
                }
                c => buf.push(c),
            }
        }
        flush(&mut filter, sign, &mut buf);
        filter
    }

    /// Require this TODO keyword (repeatable).
    pub fn with_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords.push(keyword.into());
        self
    }

    /// Restrict to this file name, e.g. `tasks.org` (repeatable).
    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.files.push(file.into());
        self
    }

    fn matches(&self, file: &OrgFile, heading: &Heading) -> bool {
        if !self.include_tags.iter().all(|t| heading.has_tag(t)) {
            return false;
        }
        if self.exclude_tags.iter().any(|t| heading.has_tag(t)) {
            return false;
        }
        if !self.keywords.is_empty() {
            match &heading.todo_keyword {
                Some(k) if self.keywords.iter().any(|w| w == k) => {}
                _ => return false,
            }
        }
        if !self.files.is_empty() && !file_matches(&file.path, &self.files) {
            return false;
        }
        true
    }
}

fn file_matches(path: &std::path::Path, files: &[String]) -> bool {
    let name = path.file_name().and_then(|n| n.to_str());
    let full = path.to_str();
    files
        .iter()
        .any(|f| name == Some(f.as_str()) || full == Some(f.as_str()))
}

/// `true` if the heading is in an open TODO state (has a keyword that is not a
/// "done" keyword).
fn is_open(heading: &Heading, keywords: &Keywords) -> bool {
    match &heading.todo_keyword {
        Some(k) => !keywords.is_done(k),
        None => false,
    }
}

/// Today view: scheduled/deadline items for `today`, plus past-due open items.
pub fn today(root: &OrgRoot, today: OrgDate, filter: &Filter) -> Vec<AgendaItem> {
    let mut items = Vec::new();
    for (file, heading) in root.iter_headings() {
        if !filter.matches(file, heading) {
            continue;
        }
        let open = is_open(heading, root.keywords());
        for (ts, kind) in planning_stamps(heading) {
            if let Some(item) = today_item(file, heading, ts, kind, today, open) {
                items.push(item);
            }
        }
    }
    sort_dated(&mut items);
    items
}

fn today_item(
    file: &OrgFile,
    heading: &Heading,
    ts: &Timestamp,
    kind: AgendaKind,
    today: OrgDate,
    open: bool,
) -> Option<AgendaItem> {
    let diff = date::diff_days(ts.date, today);
    let overdue = if diff == 0 {
        false
    } else if diff < 0 && open {
        true
    } else {
        return None; // future, or past-but-closed
    };
    Some(item(file, heading, kind, Some(ts.date), ts.time, overdue))
}

/// Next-7-days view: seven day-blocks starting at `start`.
pub fn next_7_days(root: &OrgRoot, start: OrgDate, filter: &Filter) -> Vec<AgendaDay> {
    let mut days: Vec<AgendaDay> = (0..7)
        .map(|n| AgendaDay {
            date: date::add_days(start, n),
            items: Vec::new(),
        })
        .collect();

    for (file, heading) in root.iter_headings() {
        if !filter.matches(file, heading) {
            continue;
        }
        for (ts, kind) in planning_stamps(heading) {
            let offset = date::diff_days(ts.date, start);
            if (0..7).contains(&offset) {
                let it = item(file, heading, kind, Some(ts.date), ts.time, false);
                days[offset as usize].items.push(it);
            }
        }
    }
    for day in &mut days {
        sort_dated(&mut day.items);
    }
    days
}

/// TODO-list view: every open TODO headline, grouped by file (path order),
/// ignoring dates.
pub fn todo_list(root: &OrgRoot, filter: &Filter) -> Vec<AgendaFileGroup> {
    let mut groups: Vec<AgendaFileGroup> = Vec::new();
    for file in root.files.values() {
        let mut items = Vec::new();
        for heading in file.iter_headings() {
            if is_open(heading, root.keywords()) && filter.matches(file, heading) {
                items.push(item(file, heading, AgendaKind::Todo, None, None, false));
            }
        }
        if !items.is_empty() {
            groups.push(AgendaFileGroup {
                file: file.path.clone(),
                items,
            });
        }
    }
    groups
}

fn item(
    file: &OrgFile,
    heading: &Heading,
    kind: AgendaKind,
    date: Option<OrgDate>,
    time: Option<OrgTime>,
    overdue: bool,
) -> AgendaItem {
    AgendaItem {
        file: file.path.clone(),
        title: heading.title.clone(),
        todo_keyword: heading.todo_keyword.clone(),
        tags: heading.tags.clone(),
        kind,
        date,
        time,
        overdue,
        line: heading.line_range.start,
        level: heading.level,
    }
}

/// The scheduled and deadline timestamps of a heading, in a stable order.
fn planning_stamps(heading: &Heading) -> Vec<(&Timestamp, AgendaKind)> {
    let mut out = Vec::new();
    if let Some(s) = &heading.scheduled {
        out.push((s, AgendaKind::Scheduled));
    }
    if let Some(d) = &heading.deadline {
        out.push((d, AgendaKind::Deadline));
    }
    out
}

fn sort_dated(items: &mut [AgendaItem]) {
    items.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            // Timed items before untimed within a day.
            .then(a.time.is_none().cmp(&b.time.is_none()))
            .then(a.time.cmp(&b.time))
            .then(a.title.cmp(&b.title))
    });
}

// ---- Repeater handling -------------------------------------------------------

/// How a repeater shifts: `+` (one period), `++` (catch up past today), `.+`
/// (relative to today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepeatMark {
    Plus,
    PlusPlus,
    DotPlus,
}

struct Repeater {
    mark: RepeatMark,
    count: i64,
    unit: char,
}

fn parse_repeater(cookie: &str) -> Option<Repeater> {
    let (mark, rest) = if let Some(r) = cookie.strip_prefix(".+") {
        (RepeatMark::DotPlus, r)
    } else if let Some(r) = cookie.strip_prefix("++") {
        (RepeatMark::PlusPlus, r)
    } else if let Some(r) = cookie.strip_prefix('+') {
        (RepeatMark::Plus, r)
    } else {
        return None;
    };
    let unit = rest.chars().last()?;
    if !matches!(unit, 'd' | 'w' | 'm' | 'y' | 'h') {
        return None;
    }
    let count: i64 = rest[..rest.len() - unit.len_utf8()].parse().ok()?;
    Some(Repeater { mark, count, unit })
}

fn shift_once(date: OrgDate, count: i64, unit: char) -> OrgDate {
    match unit {
        'd' | 'h' => date::add_days(date, count),
        'w' => date::add_days(date, count * 7),
        'm' => date::add_months(date, count),
        'y' => date::add_years(date, count),
        _ => date,
    }
}

/// Compute the next date for a repeating timestamp, given `today` as the
/// reference for `++` / `.+`.
fn bump_date(date: OrgDate, rep: &Repeater, today: OrgDate) -> OrgDate {
    match rep.mark {
        RepeatMark::Plus => shift_once(date, rep.count, rep.unit),
        RepeatMark::DotPlus => shift_once(today, rep.count, rep.unit),
        RepeatMark::PlusPlus => {
            let mut d = shift_once(date, rep.count, rep.unit);
            while date::diff_days(d, today) <= 0 {
                d = shift_once(d, rep.count, rep.unit);
            }
            d
        }
    }
}

/// Mark `heading` complete. If it carries a repeating (non-range) SCHEDULED or
/// DEADLINE timestamp, bump that timestamp forward one period and keep the
/// keyword in its first open state (Emacs repeat semantics). Otherwise mark it
/// DONE normally. Returns the new file source; unrelated lines stay identical.
pub fn complete(source: &str, heading: &Heading, keywords: &Keywords, today: OrgDate) -> String {
    let repeating = planning_stamps(heading)
        .into_iter()
        .find(|(ts, _)| ts.is_repeating() && !ts.is_range());

    let Some((ts, kind)) = repeating else {
        // Plain completion: set the last (done) keyword.
        let done = keywords.done.last().map(String::as_str);
        return set_todo(source, heading, keywords, done);
    };

    let rep = match ts.repeater.as_deref().and_then(parse_repeater) {
        Some(r) => r,
        None => {
            let done = keywords.done.last().map(String::as_str);
            return set_todo(source, heading, keywords, done);
        }
    };

    let mut bumped = ts.clone();
    bumped.date = bump_date(ts.date, &rep, today);

    // Rewrite just the stamp on the planning line (the line after the headline).
    let plan_idx = heading.line_range.start + 1;
    let plan_line = source.lines().nth(plan_idx).unwrap_or("");
    let keyword = match kind {
        AgendaKind::Scheduled => "SCHEDULED:",
        AgendaKind::Deadline => "DEADLINE:",
        AgendaKind::Todo => unreachable!("planning stamps are dated"),
    };
    let new_line = replace_stamp_after(plan_line, keyword, &bumped.format());
    let mut out = replace_line_content(source, plan_idx, &new_line);

    // Reset the keyword to the first open state so the task recurs.
    let reset = keywords.todo.first().map(String::as_str);
    if heading.todo_keyword.as_deref() != reset {
        out = set_todo(&out, heading, keywords, reset);
    }
    out
}

/// Replace the first `<...>` / `[...]` timestamp following `keyword` in `line`
/// with `new_stamp`, leaving everything else byte-identical.
fn replace_stamp_after(line: &str, keyword: &str, new_stamp: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(files: &[(&str, &str)]) -> OrgRoot {
        OrgRoot::from_files("/org", files.iter().map(|(p, s)| (*p, *s)))
    }

    const TODAY: OrgDate = OrgDate {
        year: 2026,
        month: 5,
        day: 29,
    };

    #[test]
    fn today_buckets_scheduled_deadline_and_overdue() {
        let root = root_with(&[(
            "/org/a.org",
            "* TODO due today\nSCHEDULED: <2026-05-29 Fri>\n\
             * TODO overdue\nDEADLINE: <2026-05-20 Wed>\n\
             * DONE old done\nSCHEDULED: <2026-05-01 Fri>\n\
             * TODO future\nSCHEDULED: <2026-06-10 Wed>\n",
        )]);
        let items = today(&root, TODAY, &Filter::default());
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        // Overdue (earlier date) sorts before today's item; future and the
        // closed past item are excluded.
        assert_eq!(titles, ["overdue", "due today"]);
        assert!(items[0].overdue);
        assert!(!items[1].overdue);
    }

    #[test]
    fn next_7_days_has_seven_blocks_in_order() {
        let root = root_with(&[(
            "/org/a.org",
            "* TODO a\nSCHEDULED: <2026-05-30 Sat>\n\
             * TODO b\nDEADLINE: <2026-06-04 Thu>\n\
             * TODO far\nSCHEDULED: <2026-07-01 Wed>\n",
        )]);
        let week = next_7_days(&root, TODAY, &Filter::default());
        assert_eq!(week.len(), 7);
        assert_eq!(week[0].date, OrgDate::new(2026, 5, 29));
        assert_eq!(week[1].items.len(), 1); // the 30th
        assert_eq!(week[1].items[0].title, "a");
        assert_eq!(week[6].date, OrgDate::new(2026, 6, 4));
        assert_eq!(week[6].items[0].title, "b");
        // The July item is outside the window.
        assert!(
            week.iter()
                .all(|d| d.items.iter().all(|i| i.title != "far"))
        );
    }

    #[test]
    fn todo_list_groups_by_file_ignoring_dates() {
        let root = root_with(&[
            ("/org/a.org", "* TODO one\n* DONE two\n* notkw\n"),
            ("/org/b.org", "* TODO three\n"),
        ]);
        let groups = todo_list(&root, &Filter::default());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].file, PathBuf::from("/org/a.org"));
        let a: Vec<_> = groups[0].items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(a, ["one"]); // DONE and keyword-less excluded
        assert_eq!(groups[1].items[0].title, "three");
    }

    #[test]
    fn filter_parse_include_exclude() {
        let f = Filter::parse("+work-personal");
        assert_eq!(f.include_tags, ["work"]);
        assert_eq!(f.exclude_tags, ["personal"]);
    }

    #[test]
    fn filter_applies_to_today() {
        let root = root_with(&[(
            "/org/a.org",
            "* TODO w :work:\nSCHEDULED: <2026-05-29 Fri>\n\
             * TODO p :personal:\nSCHEDULED: <2026-05-29 Fri>\n",
        )]);
        let items = today(&root, TODAY, &Filter::parse("+work"));
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["w"]);
    }

    #[test]
    fn filter_by_file_name() {
        let root = root_with(&[
            ("/org/a.org", "* TODO t\n"),
            ("/org/b.org", "* TODO other\n"),
        ]);
        let groups = todo_list(&root, &Filter::default().in_file("a.org"));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].file, PathBuf::from("/org/a.org"));
    }

    #[test]
    fn filter_by_keyword() {
        let kw = Keywords::from_sequence(["TODO", "NEXT", "DONE"]);
        let root = OrgRoot::with_keywords("/org", kw);
        let mut root = root;
        root.insert("/org/a.org", "* TODO t\n* NEXT n\n");
        let groups = todo_list(&root, &Filter::default().with_keyword("NEXT"));
        assert_eq!(groups.len(), 1);
        let titles: Vec<_> = groups[0].items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["n"]);
    }

    #[test]
    fn complete_non_repeating_marks_done() {
        let src = "* TODO finish\nSCHEDULED: <2026-05-29 Fri>\n";
        let root = root_with(&[("/org/a.org", src)]);
        let h = &root.file("/org/a.org").unwrap().headings[0];
        let out = complete(src, h, root.keywords(), TODAY);
        assert_eq!(out, "* DONE finish\nSCHEDULED: <2026-05-29 Fri>\n");
    }

    #[test]
    fn complete_repeating_bumps_and_keeps_todo() {
        let src = "* TODO water plants\nSCHEDULED: <2026-05-29 Fri +1w>\n  notes\n";
        let root = root_with(&[("/org/a.org", src)]);
        let h = &root.file("/org/a.org").unwrap().headings[0];
        let out = complete(src, h, root.keywords(), TODAY);
        // +1w → 2026-06-05 (Friday); keyword stays TODO; body untouched.
        assert_eq!(
            out,
            "* TODO water plants\nSCHEDULED: <2026-06-05 Fri +1w>\n  notes\n"
        );
    }

    #[test]
    fn repeater_forms_bump_correctly() {
        let cases = [
            ("+1d", OrgDate::new(2026, 5, 30)),
            ("+1w", OrgDate::new(2026, 6, 5)),
            (".+2d", OrgDate::new(2026, 5, 31)), // relative to today
        ];
        for (cookie, expected) in cases {
            let rep = parse_repeater(cookie).unwrap();
            assert_eq!(bump_date(OrgDate::new(2026, 5, 29), &rep, TODAY), expected);
        }
    }

    #[test]
    fn plusplus_catches_up_past_today() {
        // Stored well in the past; ++1m must land strictly after today.
        let rep = parse_repeater("++1m").unwrap();
        let bumped = bump_date(OrgDate::new(2026, 1, 15), &rep, TODAY);
        assert_eq!(bumped, OrgDate::new(2026, 6, 15));
        assert!(date::diff_days(bumped, TODAY) > 0);
    }

    #[test]
    fn complete_deadline_repeater() {
        let src = "* TODO rent\nDEADLINE: <2026-05-29 Fri ++1m>\n";
        let root = root_with(&[("/org/a.org", src)]);
        let h = &root.file("/org/a.org").unwrap().headings[0];
        let out = complete(src, h, root.keywords(), TODAY);
        assert_eq!(out, "* TODO rent\nDEADLINE: <2026-06-29 Mon ++1m>\n");
    }
}
