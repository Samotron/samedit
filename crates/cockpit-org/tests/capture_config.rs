//! Prove the `org.toml` schema in the plan (M12.5a) deserialises into the
//! capture types, and that a configured template captures end-to-end.

use cockpit_org::{CaptureContext, NowStamp, OrgConfig, OrgDate, OrgTime, run_capture};
use serde::Deserialize;

/// The whole `org.toml` is `[org]` plus `[[org.capture]]` tables.
#[derive(Deserialize)]
struct OrgToml {
    org: OrgConfig,
}

/// Verbatim from IMPLEMENTATION_PLAN.md M12.5a.
const ORG_TOML: &str = r#"
[org]
root        = "~/org"
default_todo_keywords = ["TODO", "DONE"]

[[org.capture]]
key      = "t"
name     = "Todo"
target   = { file = "inbox.org", under = "Tasks" }
template = """
* TODO %?
  :PROPERTIES:
  :CREATED: %U
  :END:
"""

[[org.capture]]
key      = "n"
name     = "Note"
target   = { file = "notes.org", under = "Inbox" }
template = "* %? :note:\n  Captured %U from %a"

[[org.capture]]
key      = "j"
name     = "Journal"
target   = { file = "journal.org", datetree = true }
template = "* %U %?"
"#;

fn now() -> NowStamp {
    NowStamp::new(OrgDate::new(2026, 5, 29), OrgTime::new(11, 24), "Fri")
}

fn config() -> OrgConfig {
    toml::from_str::<OrgToml>(ORG_TOML)
        .expect("org.toml parses")
        .org
}

#[test]
fn schema_matches_plan() {
    let cfg = config();
    assert_eq!(cfg.root.as_deref(), Some("~/org"));
    assert_eq!(cfg.default_todo_keywords, ["TODO", "DONE"]);
    assert_eq!(cfg.capture.len(), 3);

    let kw = cfg.keywords();
    assert_eq!(kw.done, ["DONE"]);

    let todo = cfg.template("t").expect("todo template");
    assert_eq!(todo.name, "Todo");
    assert_eq!(todo.target.file, "inbox.org");
    assert_eq!(todo.target.under.as_deref(), Some("Tasks"));
    assert!(!todo.target.datetree);

    let journal = cfg.template("j").expect("journal template");
    assert!(journal.target.datetree);
}

#[test]
fn todo_template_files_under_tasks_with_properties() {
    let cfg = config();
    let todo = cfg.template("t").unwrap();

    let inbox = "* Tasks\n* Reference\n";
    let out = run_capture(inbox, todo, &now(), &CaptureContext::default());

    // The entry is demoted to a child of Tasks; %U expands; %? sets the cursor;
    // the unrelated "Reference" heading is untouched.
    assert_eq!(
        out.source,
        "* Tasks\n\
         ** TODO \n  \
         :PROPERTIES:\n  \
         :CREATED: [2026-05-29 Fri 11:24]\n  \
         :END:\n\
         * Reference\n"
    );
    let c = out.cursor.expect("cursor slot");
    assert!(out.source[..c].ends_with("** TODO "));
}

#[test]
fn journal_template_uses_datetree() {
    let cfg = config();
    let journal = cfg.template("j").unwrap();
    let ctx = CaptureContext::default();

    let out = run_capture("", journal, &now(), &ctx);
    assert_eq!(
        out.source,
        "* 2026\n\
         ** 2026-05\n\
         *** 2026-05-29 Fri\n\
         **** [2026-05-29 Fri 11:24] \n"
    );
}

#[test]
fn note_template_annotation_substitutes() {
    let cfg = config();
    let note = cfg.template("n").unwrap();
    let ctx = CaptureContext {
        annotation: Some("src/lib.rs:10".to_string()),
        ..Default::default()
    };
    let out = run_capture("* Inbox\n", note, &now(), &ctx);
    assert_eq!(
        out.source,
        // `* %? :note:` → stars, the empty cursor slot, then ` :note:` (two
        // spaces), demoted to level 2 under Inbox.
        "* Inbox\n**  :note:\n  Captured [2026-05-29 Fri 11:24] from src/lib.rs:10\n"
    );
}
