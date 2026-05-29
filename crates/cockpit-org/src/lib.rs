//! `cockpit-org` — Org-mode subset parser, domain model, and line-range edit
//! primitives for the Coding Cockpit jot surface (v0.12, milestone M12.5).
//!
//! The `.org` files on disk are the source of truth — the same files open
//! unchanged in Emacs, Logseq, Orgzly, or any other Org tool. This crate is the
//! in-memory index over them, plus the editing primitives that keep untouched
//! bytes byte-identical.
//!
//! # Scope (v0.12 subset)
//!
//! In scope: hierarchical headlines with title, tags, priority cookie, and TODO
//! keyword; the default `TODO | DONE` workflow (and the general N-state shape);
//! `SCHEDULED:` / `DEADLINE:` / `CLOSED:` planning timestamps with active
//! (`<...>`) / inactive (`[...]`) forms, date-only and date-with-time, and
//! repeater / delay cookies. Out of scope (per the plan): property drawers,
//! Babel code blocks, org tables, clocking, org-roam links, and Emacs-style
//! internal link resolution.
//!
//! # Architecture
//!
//! - Parsing leans on [`orgize`] (0.9, stable) for the fiddly inline grammar,
//!   but **positions are ours**: orgize 0.9 exposes no source ranges, so we
//!   scan headline lines and zip them against orgize's pre-order headlines.
//! - **Round-trip is non-negotiable.** We never re-emit an AST. Every edit is a
//!   line-range replacement on the original [`OrgFile::source`] buffer (see
//!   [`edit`]), so blank lines, comments, and unrelated headings survive
//!   byte-for-byte.
//! - The store ([`OrgRoot`]) is pure data: it parses `(path, source)` pairs.
//!   The directory walk, `notify` watcher, and disk writes live in the jot
//!   binary / cockpit integration behind `cockpit-project::env`.
//!
//! Stays headless and unit-tested per the AGENTS.md hard rules — no window, no
//! GPU, no PTY, no real filesystem, no network.
//!
//! # Plan deviation
//!
//! The plan (M12.5) named `orgize 0.10+`, but the only published 0.10 is an
//! alpha rewrite with an unstable API. We pin the stable **0.9** instead;
//! recorded in `IMPLEMENTATION_PLAN.md`. Round-trip is unaffected because we
//! never use orgize's serialiser — only its parser.

pub mod agenda;
pub mod capture;
pub mod date;
pub mod edit;
pub mod keywords;
pub mod model;
pub mod parse;
pub mod store;
pub mod timestamp;

pub use agenda::{
    AgendaDay, AgendaFileGroup, AgendaItem, AgendaKind, Filter, complete, next_7_days, today,
    todo_list,
};
pub use capture::{
    CaptureContext, CaptureOutcome, CaptureTarget, CaptureTemplate, Expansion, NowStamp, OrgConfig,
    apply_capture, expand, expand_with, run_capture,
};
pub use edit::{
    byte_offset_of_line, cycle_todo, insert_at_line, replace_line_content, replace_line_range,
    set_todo,
};
pub use keywords::Keywords;
pub use model::{Heading, OrgFile};
pub use parse::{headline_line_indices, parse_file, parse_file_with, parse_headings};
pub use store::{DEFAULT_LAYOUT, OrgRoot};
pub use timestamp::{OrgDate, OrgTime, Timestamp};
