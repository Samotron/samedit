//! Capture templates (M12.5a).
//!
//! A capture template is a typed slot the user fills in once and commits to a
//! specific destination file + heading. Templates are declared in
//! `~/.config/cockpit/org.toml` and deserialise straight into [`CaptureTemplate`]
//! (the file read + path resolution is wired by the jot binary later).
//!
//! Two halves:
//! 1. [`expand`] — substitute the Emacs-style `%`-tokens in a template body,
//!    recording where `%?` leaves the cursor.
//! 2. [`apply_capture`] — splice the expanded entry into the right file under
//!    the right heading (or date-tree), creating missing structure, via the
//!    line-range edit primitives so unrelated headings stay byte-identical.
//!
//! Non-determinism (the current date/time) is injected as a [`NowStamp`], never
//! read from a global clock — the caller passes `clock.now()` converted to a
//! calendar value, and tests pass frozen values.

use serde::Deserialize;

use crate::Keywords;
use crate::edit::{byte_offset_of_line, insert_at_line};
use crate::parse::parse_headings;
use crate::timestamp::{OrgDate, OrgTime};

/// The current calendar moment, injected into capture (no global clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowStamp {
    pub date: OrgDate,
    pub time: OrgTime,
    /// Abbreviated day name as Org writes it, e.g. `"Fri"`.
    pub dayname: String,
}

impl NowStamp {
    pub fn new(date: OrgDate, time: OrgTime, dayname: impl Into<String>) -> Self {
        NowStamp {
            date,
            time,
            dayname: dayname.into(),
        }
    }

    fn format(&self, active: bool, with_time: bool) -> String {
        let (open, close) = if active { ('<', '>') } else { ('[', ']') };
        let OrgDate { year, month, day } = self.date;
        let mut s = format!(
            "{open}{year:04}-{month:02}-{day:02} {dn}",
            dn = self.dayname
        );
        if with_time {
            s.push_str(&format!(" {:02}:{:02}", self.time.hour, self.time.minute));
        }
        s.push(close);
        s
    }
}

/// Where a captured entry lands: a file plus an optional parent heading or a
/// date-tree. Deserialised from `target = { file = "...", under = "..." }` or
/// `target = { file = "...", datetree = true }`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CaptureTarget {
    /// Destination file, relative to the org root.
    pub file: String,
    /// Parent heading title to file the entry under, if any.
    #[serde(default)]
    pub under: Option<String>,
    /// File under a `year → month → day` date-tree when `true`.
    #[serde(default)]
    pub datetree: bool,
}

/// A single capture template, as declared in `org.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CaptureTemplate {
    /// The key the user presses in the template picker (e.g. `"t"`).
    pub key: String,
    /// Human-readable name shown in the picker.
    pub name: String,
    /// Destination.
    pub target: CaptureTarget,
    /// Template body with `%`-tokens.
    pub template: String,
}

/// The `[org]` section of `org.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrgConfig {
    /// Org root folder (may contain a leading `~`; resolved by the binary).
    #[serde(default)]
    pub root: Option<String>,
    /// TODO-keyword workflow.
    #[serde(default = "default_keywords")]
    pub default_todo_keywords: Vec<String>,
    /// Capture templates.
    #[serde(default, rename = "capture")]
    pub capture: Vec<CaptureTemplate>,
}

fn default_keywords() -> Vec<String> {
    vec!["TODO".to_string(), "DONE".to_string()]
}

impl OrgConfig {
    /// The TODO workflow these keywords describe.
    pub fn keywords(&self) -> Keywords {
        Keywords::from_sequence(self.default_todo_keywords.clone())
    }

    /// Look up a template by its picker key.
    pub fn template(&self, key: &str) -> Option<&CaptureTemplate> {
        self.capture.iter().find(|t| t.key == key)
    }
}

/// Context for token substitution.
#[derive(Debug, Clone, Default)]
pub struct CaptureContext {
    /// `%a` — annotation (e.g. the editor's `path:line`), if any.
    pub annotation: Option<String>,
    /// `%i` — initial content (e.g. the current selection), if any.
    pub initial: Option<String>,
}

/// The result of expanding a template body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// Expanded text with all tokens resolved and `%?` removed.
    pub text: String,
    /// Byte offset of the `%?` cursor slot within `text`, if the template had
    /// one (the first `%?` wins, as in Emacs).
    pub cursor: Option<usize>,
}

/// Expand a template body, with no `%(lua)` evaluator (such tokens resolve to
/// empty). See [`expand_with`] to wire one in.
pub fn expand(template: &str, now: &NowStamp, ctx: &CaptureContext) -> Expansion {
    expand_with(template, now, ctx, |_| None)
}

/// Expand a template body, resolving `%(expr)` via `lua`. `lua` returning
/// `None` drops the token (empty string).
///
/// Tokens (mirroring Emacs `org-capture-templates`):
/// - `%?` — cursor slot (first one wins; removed from output).
/// - `%t` / `%T` — active date / active date-time.
/// - `%u` / `%U` — inactive date / inactive date-time.
/// - `%a` — annotation, `%i` — initial content.
/// - `%(expr)` — evaluated by `lua`.
/// - `%%` — a literal `%`.
pub fn expand_with(
    template: &str,
    now: &NowStamp,
    ctx: &CaptureContext,
    mut lua: impl FnMut(&str) -> Option<String>,
) -> Expansion {
    let mut text = String::with_capacity(template.len());
    let mut cursor = None;
    let mut chars = template.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c != '%' {
            text.push(c);
            continue;
        }
        match chars.peek().map(|&(_, c)| c) {
            Some('?') => {
                chars.next();
                if cursor.is_none() {
                    cursor = Some(text.len());
                }
            }
            Some('t') => {
                chars.next();
                text.push_str(&now.format(true, false));
            }
            Some('T') => {
                chars.next();
                text.push_str(&now.format(true, true));
            }
            Some('u') => {
                chars.next();
                text.push_str(&now.format(false, false));
            }
            Some('U') => {
                chars.next();
                text.push_str(&now.format(false, true));
            }
            Some('a') => {
                chars.next();
                text.push_str(ctx.annotation.as_deref().unwrap_or(""));
            }
            Some('i') => {
                chars.next();
                text.push_str(ctx.initial.as_deref().unwrap_or(""));
            }
            Some('%') => {
                chars.next();
                text.push('%');
            }
            Some('(') => {
                chars.next(); // consume '('
                let mut expr = String::new();
                for (_, ec) in chars.by_ref() {
                    if ec == ')' {
                        break;
                    }
                    expr.push(ec);
                }
                if let Some(val) = lua(&expr) {
                    text.push_str(&val);
                }
            }
            // Unknown token: keep the `%` verbatim.
            _ => text.push('%'),
        }
    }

    Expansion { text, cursor }
}

/// Outcome of a capture: the new file source plus the cursor's absolute byte
/// offset in it (where `%?` landed), if the template had a cursor slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOutcome {
    pub source: String,
    pub cursor: Option<usize>,
}

/// Expand `template` and splice it into `file_source` at the template's target.
/// `file_source` is the destination file's current contents (empty string for a
/// fresh file).
pub fn run_capture(
    file_source: &str,
    template: &CaptureTemplate,
    now: &NowStamp,
    ctx: &CaptureContext,
) -> CaptureOutcome {
    let expansion = expand(&template.template, now, ctx);
    apply_capture(file_source, &template.target, &expansion, now)
}

/// Splice an already-expanded `entry` into `file_source` at `target`, adjusting
/// the entry's heading levels to nest correctly and creating any missing
/// date-tree structure. Unrelated lines stay byte-identical.
pub fn apply_capture(
    file_source: &str,
    target: &CaptureTarget,
    entry: &Expansion,
    now: &NowStamp,
) -> CaptureOutcome {
    // Resolve the destination: a base source, the line to insert before, and
    // the level the entry's top heading should take.
    let (mut source, insert_line, target_level) = if target.datetree {
        let (src, day_end) = ensure_datetree(file_source, now);
        (src, day_end, 4)
    } else if let Some(name) = &target.under {
        resolve_under(file_source, name)
    } else {
        // No parent: append at EOF, keep the template's own levels.
        let lines = file_source.lines().count();
        (file_source.to_string(), lines, 0)
    };

    // Shift the entry's heading levels and recompute the cursor offset.
    let (entry_text, cursor_in_entry) = if target_level == 0 {
        (ensure_trailing_newline(&entry.text), entry.cursor)
    } else {
        shift_levels(&entry.text, entry.cursor, target_level)
    };

    // Separate from preceding content when appending at EOF without a newline.
    let insert_offset = byte_offset_of_line(&source, insert_line);
    let needs_leading_nl =
        insert_offset == source.len() && !source.is_empty() && !source.ends_with('\n');
    let lead = if needs_leading_nl { "\n" } else { "" };

    let final_entry = format!("{lead}{entry_text}");
    let cursor = cursor_in_entry.map(|c| insert_offset + lead.len() + c);

    source = insert_at_line(&source, insert_line, &final_entry);
    CaptureOutcome { source, cursor }
}

fn ensure_trailing_newline(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Shift every heading line in `entry` so its shallowest heading sits at
/// `target_level`, tracking the cursor's new offset.
fn shift_levels(
    entry: &str,
    cursor: Option<usize>,
    target_level: usize,
) -> (String, Option<usize>) {
    let base = entry
        .lines()
        .filter_map(heading_stars)
        .min()
        .unwrap_or(target_level);
    let shift = target_level as isize - base as isize;

    let mut out = String::with_capacity(entry.len());
    let mut new_cursor = cursor;
    let mut consumed = 0usize; // bytes of `entry` walked so far

    for line in entry.split_inclusive('\n') {
        let line_start = consumed;
        consumed += line.len();

        if let Some(stars) = heading_stars(line) {
            let new_stars = (stars as isize + shift).max(1) as usize;
            let added = new_stars as isize - stars as isize;
            // The added/removed stars sit at the very start of the line; shift
            // the cursor if it lies at or past this point.
            if let Some(c) = new_cursor.as_mut()
                && *c >= line_start
            {
                *c = (*c as isize + added) as usize;
            }
            out.push_str(&"*".repeat(new_stars));
            out.push_str(&line[stars..]);
        } else {
            out.push_str(line);
        }
    }

    (ensure_trailing_newline(&out), new_cursor)
}

/// Number of leading `*` if `line` is a headline line, else `None`.
fn heading_stars(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    if stars == 0 {
        return None;
    }
    match line.as_bytes().get(stars) {
        None | Some(b' ') | Some(b'\n') | Some(b'\r') => Some(stars),
        _ => None,
    }
}

/// Resolve an `under = "Name"` target: returns the (possibly amended) source,
/// the line to insert before, and the level for the entry's top heading.
fn resolve_under(file_source: &str, name: &str) -> (String, usize, usize) {
    let headings = parse_headings(file_source, &Keywords::default());
    let found = headings
        .iter()
        .flat_map(|h| h.iter())
        .find(|h| h.title == name);

    match found {
        Some(h) => (
            file_source.to_string(),
            h.subtree_line_range().end,
            h.level + 1,
        ),
        None => {
            // Create the parent heading at EOF, then file beneath it.
            let mut src = ensure_trailing_newline_or_empty(file_source);
            let lines_before = src.lines().count();
            src.push_str(&format!("* {name}\n"));
            (src, lines_before + 1, 2)
        }
    }
}

fn ensure_trailing_newline_or_empty(s: &str) -> String {
    if s.is_empty() || s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Ensure the `year → month → day` heading path for `now` exists, creating any
/// missing levels. Returns the amended source and the line index at the end of
/// the day heading's subtree (where the entry should be inserted).
fn ensure_datetree(file_source: &str, now: &NowStamp) -> (String, usize) {
    let OrgDate { year, month, day } = now.date;
    let year_t = format!("{year:04}");
    let month_t = format!("{year:04}-{month:02}");
    let day_t = format!("{year:04}-{month:02}-{day:02} {dn}", dn = now.dayname);

    let mut source = ensure_trailing_newline_or_empty(file_source);

    // Year.
    if find_heading_line(&source, 1, &year_t).is_none() {
        let at = source.lines().count();
        source = insert_at_line(&source, at, &format!("* {year_t}\n"));
    }
    let year_h = find_heading(&source, 1, &year_t).expect("year heading present");
    let year_end = year_h.subtree_line_range().end;

    // Month (under year).
    if find_heading_line(&source, 2, &month_t).is_none() {
        source = insert_at_line(&source, year_end, &format!("** {month_t}\n"));
    }
    let month_h = find_heading(&source, 2, &month_t).expect("month heading present");
    let month_end = month_h.subtree_line_range().end;

    // Day (under month).
    if find_heading_line(&source, 3, &day_t).is_none() {
        source = insert_at_line(&source, month_end, &format!("*** {day_t}\n"));
    }
    let day_h = find_heading(&source, 3, &day_t).expect("day heading present");
    let day_end = day_h.subtree_line_range().end;

    (source, day_end)
}

fn find_heading(source: &str, level: usize, title: &str) -> Option<crate::Heading> {
    parse_headings(source, &Keywords::default())
        .iter()
        .flat_map(|h| h.iter())
        .find(|h| h.level == level && h.title == title)
        .cloned()
}

fn find_heading_line(source: &str, level: usize, title: &str) -> Option<usize> {
    find_heading(source, level, title).map(|h| h.line_range.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NowStamp {
        NowStamp::new(OrgDate::new(2026, 5, 29), OrgTime::new(11, 24), "Fri")
    }

    #[test]
    fn tokens_substitute_against_frozen_clock() {
        let ctx = CaptureContext {
            annotation: Some("src/main.rs:42".to_string()),
            initial: Some("selected".to_string()),
        };
        let e = expand("t=%t T=%T u=%u U=%U a=%a i=%i pct=%%", &now(), &ctx);
        assert_eq!(
            e.text,
            "t=<2026-05-29 Fri> T=<2026-05-29 Fri 11:24> \
             u=[2026-05-29 Fri] U=[2026-05-29 Fri 11:24] \
             a=src/main.rs:42 i=selected pct=%"
        );
        assert_eq!(e.cursor, None);
    }

    #[test]
    fn cursor_slot_recorded_and_removed() {
        let e = expand("* TODO %?", &now(), &CaptureContext::default());
        assert_eq!(e.text, "* TODO ");
        assert_eq!(e.cursor, Some("* TODO ".len()));
    }

    #[test]
    fn lua_token_uses_evaluator_else_empty() {
        let ctx = CaptureContext::default();
        let none = expand("x=%(foo)", &now(), &ctx);
        assert_eq!(none.text, "x=");
        let some = expand_with("x=%(foo)", &now(), &ctx, |expr| {
            (expr == "foo").then(|| "BAR".to_string())
        });
        assert_eq!(some.text, "x=BAR");
    }

    #[test]
    fn capture_under_existing_heading_nests_and_preserves_rest() {
        let src = "* Tasks\n** Existing\n* Other\n";
        let tmpl = CaptureTemplate {
            key: "t".into(),
            name: "Todo".into(),
            target: CaptureTarget {
                file: "inbox.org".into(),
                under: Some("Tasks".into()),
                datetree: false,
            },
            template: "* TODO %?".into(),
        };
        let out = run_capture(src, &tmpl, &now(), &CaptureContext::default());
        // Entry demoted to level 2 and filed at the end of the Tasks subtree.
        assert_eq!(out.source, "* Tasks\n** Existing\n** TODO \n* Other\n");
        // Cursor sits right after "** TODO ".
        let c = out.cursor.unwrap();
        assert_eq!(&out.source[c..c], "");
        assert!(out.source[..c].ends_with("** TODO "));
    }

    #[test]
    fn capture_under_missing_heading_creates_it() {
        let src = "* Other\n";
        let target = CaptureTarget {
            file: "inbox.org".into(),
            under: Some("Tasks".into()),
            datetree: false,
        };
        let e = expand("* TODO buy milk", &now(), &CaptureContext::default());
        let out = apply_capture(src, &target, &e, &now());
        assert_eq!(out.source, "* Other\n* Tasks\n** TODO buy milk\n");
    }

    #[test]
    fn capture_append_at_eof_keeps_levels() {
        let src = "* one\n";
        let target = CaptureTarget {
            file: "notes.org".into(),
            under: None,
            datetree: false,
        };
        let e = expand("* %? :note:", &now(), &CaptureContext::default());
        let out = apply_capture(src, &target, &e, &now());
        assert_eq!(out.source, "* one\n*  :note:\n");
    }

    #[test]
    fn append_at_eof_without_trailing_newline_inserts_separator() {
        let target = CaptureTarget {
            file: "notes.org".into(),
            under: None,
            datetree: false,
        };
        let e = expand("* tail", &now(), &CaptureContext::default());
        let out = apply_capture("* head", &target, &e, &now());
        assert_eq!(out.source, "* head\n* tail\n");
    }

    #[test]
    fn datetree_into_empty_file_builds_full_path() {
        let target = CaptureTarget {
            file: "journal.org".into(),
            under: None,
            datetree: true,
        };
        let e = expand("* %U entry", &now(), &CaptureContext::default());
        let out = apply_capture("", &target, &e, &now());
        assert_eq!(
            out.source,
            "* 2026\n** 2026-05\n*** 2026-05-29 Fri\n**** [2026-05-29 Fri 11:24] entry\n"
        );
    }

    #[test]
    fn datetree_reuses_existing_year_and_month() {
        let src = "* 2026\n** 2026-05\n*** 2026-05-01 Fri\n**** old\n";
        let target = CaptureTarget {
            file: "journal.org".into(),
            under: None,
            datetree: true,
        };
        let e = expand("* new", &now(), &CaptureContext::default());
        let out = apply_capture(src, &target, &e, &now());
        // A new day heading is appended under the existing month; the old day
        // and its child are untouched.
        assert_eq!(
            out.source,
            "* 2026\n** 2026-05\n*** 2026-05-01 Fri\n**** old\n*** 2026-05-29 Fri\n**** new\n"
        );
    }

    #[test]
    fn datetree_appends_under_existing_day() {
        let src = "* 2026\n** 2026-05\n*** 2026-05-29 Fri\n**** earlier\n";
        let target = CaptureTarget {
            file: "journal.org".into(),
            under: None,
            datetree: true,
        };
        let e = expand("* later", &now(), &CaptureContext::default());
        let out = apply_capture(src, &target, &e, &now());
        assert_eq!(
            out.source,
            "* 2026\n** 2026-05\n*** 2026-05-29 Fri\n**** earlier\n**** later\n"
        );
    }
}
