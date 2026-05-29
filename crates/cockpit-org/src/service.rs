//! The Org IPC service contract (M12.6 / M12.7).
//!
//! When `cockpit-jot` is running it owns the canonical in-memory [`OrgRoot`];
//! the main cockpit drives it over IPC instead of touching the files directly,
//! so the tray app's index stays live. This module defines the request/response
//! messages exchanged over that channel and a pure [`handle`] function that
//! applies a request to an `OrgRoot`.
//!
//! The messages are plain `serde` types — they ride as the payload of
//! `cockpit_ipc::Envelope` (which is generic over the payload), so this module
//! has no dependency on the transport crate. Mutating requests return the new
//! file source; the caller (jot) writes it to disk, keeping the
//! direct-write and IPC paths byte-identical (they share the same edit
//! primitives).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agenda::{self, AgendaItem, AgendaKind};
use crate::store::OrgRoot;
use crate::timestamp::OrgDate;

/// A request from the cockpit (client) to the jot org service (server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgRequest {
    /// Re-parse `source` for `path` (the cockpit edited the buffer).
    Reload { path: PathBuf, source: String },
    /// The Today agenda for `today`.
    Today { today: OrgDate },
    /// Every open TODO headline (the TODO-list view).
    TodoList,
    /// Mark the heading at `path`:`line` complete, bumping a repeater if any.
    /// Returns the new file source for the caller to persist.
    Complete {
        path: PathBuf,
        line: usize,
        today: OrgDate,
    },
}

/// A response from the org service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgResponse {
    /// A `Reload` was applied.
    Reloaded,
    /// Agenda rows (for `Today` / `TodoList`).
    Agenda(Vec<AgendaItemDto>),
    /// A mutating request produced new file content to persist.
    Updated { path: PathBuf, source: String },
    /// The request could not be served.
    Error(String),
}

/// Wire-friendly agenda item — primitive fields only, so the protocol stays
/// decoupled from the in-crate domain types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgendaItemDto {
    pub file: PathBuf,
    pub title: String,
    pub todo_keyword: Option<String>,
    pub tags: Vec<String>,
    pub kind: AgendaItemKind,
    /// `(year, month, day)` for dated items.
    pub date: Option<(i32, u32, u32)>,
    /// `(hour, minute)` for timed items.
    pub time: Option<(u8, u8)>,
    pub overdue: bool,
    pub line: usize,
}

/// Wire mirror of [`AgendaKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgendaItemKind {
    Scheduled,
    Deadline,
    Todo,
}

impl From<&AgendaItem> for AgendaItemDto {
    fn from(it: &AgendaItem) -> Self {
        AgendaItemDto {
            file: it.file.clone(),
            title: it.title.clone(),
            todo_keyword: it.todo_keyword.clone(),
            tags: it.tags.clone(),
            kind: match it.kind {
                AgendaKind::Scheduled => AgendaItemKind::Scheduled,
                AgendaKind::Deadline => AgendaItemKind::Deadline,
                AgendaKind::Todo => AgendaItemKind::Todo,
            },
            date: it.date.map(|d| (d.year, d.month, d.day)),
            time: it.time.map(|t| (t.hour, t.minute)),
            overdue: it.overdue,
            line: it.line,
        }
    }
}

/// Apply a request to `root`, returning the response. Pure except for the
/// in-memory mutation of `root` (no disk, no clock — `today` is in the request).
pub fn handle(root: &mut OrgRoot, request: OrgRequest) -> OrgResponse {
    match request {
        OrgRequest::Reload { path, source } => {
            root.insert(path, source);
            OrgResponse::Reloaded
        }
        OrgRequest::Today { today } => {
            let items = agenda::today(root, today, &agenda::Filter::default());
            OrgResponse::Agenda(items.iter().map(AgendaItemDto::from).collect())
        }
        OrgRequest::TodoList => {
            let groups = agenda::todo_list(root, &agenda::Filter::default());
            let items = groups
                .iter()
                .flat_map(|g| g.items.iter())
                .map(AgendaItemDto::from)
                .collect();
            OrgResponse::Agenda(items)
        }
        OrgRequest::Complete { path, line, today } => complete_at(root, &path, line, today),
    }
}

fn complete_at(root: &mut OrgRoot, path: &PathBuf, line: usize, today: OrgDate) -> OrgResponse {
    let Some(file) = root.file(path) else {
        return OrgResponse::Error(format!("unknown file: {}", path.display()));
    };
    let Some(heading) = file
        .iter_headings()
        .find(|h| h.line_range.start == line)
        .cloned()
    else {
        return OrgResponse::Error(format!("no heading at {}:{line}", path.display()));
    };

    let new_source = agenda::complete(&file.source, &heading, root.keywords(), today);
    root.insert(path.clone(), new_source.clone());
    OrgResponse::Updated {
        path: path.clone(),
        source: new_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: OrgDate = OrgDate {
        year: 2026,
        month: 5,
        day: 29,
    };

    fn root() -> OrgRoot {
        OrgRoot::from_files(
            "/org",
            [(
                "/org/a.org",
                "* TODO due\nSCHEDULED: <2026-05-29 Fri>\n* TODO repeat\nSCHEDULED: <2026-05-29 Fri +1w>\n",
            )],
        )
    }

    #[test]
    fn today_request_returns_agenda() {
        let mut root = root();
        let resp = handle(&mut root, OrgRequest::Today { today: TODAY });
        match resp {
            OrgResponse::Agenda(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].date, Some((2026, 5, 29)));
                assert_eq!(items[0].kind, AgendaItemKind::Scheduled);
            }
            other => panic!("expected agenda, got {other:?}"),
        }
    }

    #[test]
    fn reload_replaces_file() {
        let mut root = root();
        let resp = handle(
            &mut root,
            OrgRequest::Reload {
                path: "/org/a.org".into(),
                source: "* TODO only one\n".to_string(),
            },
        );
        assert_eq!(resp, OrgResponse::Reloaded);
        assert_eq!(root.file("/org/a.org").unwrap().headings.len(), 1);
    }

    #[test]
    fn complete_bumps_repeater_and_returns_source() {
        let mut root = root();
        // "repeat" headline starts at line 2.
        let resp = handle(
            &mut root,
            OrgRequest::Complete {
                path: "/org/a.org".into(),
                line: 2,
                today: TODAY,
            },
        );
        match resp {
            OrgResponse::Updated { source, .. } => {
                assert!(source.contains("SCHEDULED: <2026-06-05 Fri +1w>"));
                // The mutation is reflected in the live root too.
                assert_eq!(root.file("/org/a.org").unwrap().source, source);
            }
            other => panic!("expected updated, got {other:?}"),
        }
    }

    #[test]
    fn complete_unknown_file_errors() {
        let mut root = root();
        let resp = handle(
            &mut root,
            OrgRequest::Complete {
                path: "/org/missing.org".into(),
                line: 0,
                today: TODAY,
            },
        );
        assert!(matches!(resp, OrgResponse::Error(_)));
    }
}
