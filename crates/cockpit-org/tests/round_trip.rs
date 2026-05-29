//! Round-trip and structural tests against an Emacs-authored fixture.
//!
//! The hard rule: edits replace line ranges in the original buffer, so every
//! line we did not touch stays byte-identical. These tests guard that on a
//! corpus that exercises headlines, tags, priority, TODO/DONE keywords,
//! SCHEDULED/DEADLINE/CLOSED, active/inactive timestamps, ranges, and a
//! repeater.

use cockpit_org::{Keywords, OrgDate, cycle_todo, parse_file};

const SAMPLE: &str = include_str!("../../../tests/fixtures/org/sample.org");

#[test]
fn source_is_stored_verbatim() {
    let file = parse_file("sample.org", SAMPLE);
    assert_eq!(
        file.source, SAMPLE,
        "source buffer must be preserved verbatim"
    );
}

#[test]
fn structure_matches_fixture() {
    let file = parse_file("sample.org", SAMPLE);

    // Top-level headings: Inbox, Ship, Weekly review, Notes, Meeting block.
    let tops: Vec<_> = file.headings.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(
        tops,
        [
            "Inbox",
            "Ship v0.12 jot surface",
            "Weekly review",
            "Notes",
            "Meeting block"
        ]
    );

    let ship = &file.headings[1];
    assert_eq!(ship.todo_keyword.as_deref(), Some("TODO"));
    assert_eq!(ship.priority, Some('A'));
    assert_eq!(ship.tags, ["work", "urgent"]);
    assert_eq!(
        ship.scheduled.as_ref().map(|t| t.date),
        Some(OrgDate::new(2026, 6, 1))
    );
    assert!(ship.scheduled.as_ref().unwrap().is_active);

    // Children of "Ship": the hotkey task and the DONE decision.
    assert_eq!(ship.children.len(), 2);
    let hotkey = &ship.children[0];
    assert_eq!(hotkey.todo_keyword.as_deref(), Some("TODO"));
    let deadline = hotkey.deadline.as_ref().expect("deadline");
    assert_eq!(deadline.time.map(|t| (t.hour, t.minute)), Some((17, 0)));

    let decision = &ship.children[1];
    assert_eq!(decision.todo_keyword.as_deref(), Some("DONE"));
    let closed = decision.closed.as_ref().expect("closed");
    assert!(!closed.is_active, "CLOSED uses an inactive timestamp");

    // Repeater on the weekly review.
    let review = &file.headings[2];
    assert_eq!(
        review.scheduled.as_ref().and_then(|t| t.repeater.clone()),
        Some("+1w".to_string())
    );
}

#[test]
fn edit_leaves_other_lines_byte_identical() {
    let file = parse_file("sample.org", SAMPLE);
    let review = &file.headings[2]; // "* TODO Weekly review"
    let kw = Keywords::default();

    let edited = cycle_todo(SAMPLE, review, &kw); // TODO -> DONE

    let before: Vec<&str> = SAMPLE.lines().collect();
    let after: Vec<&str> = edited.lines().collect();
    assert_eq!(before.len(), after.len(), "no lines added or removed");

    let changed_line = review.line_range.start;
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if i == changed_line {
            assert_eq!(*b, "* TODO Weekly review                          :review:");
            assert_eq!(*a, "* DONE Weekly review                          :review:");
        } else {
            assert_eq!(b, a, "line {i} must be byte-identical");
        }
    }
}

#[test]
fn no_op_keyword_set_is_identity() {
    let file = parse_file("sample.org", SAMPLE);
    let inbox = &file.headings[0]; // no keyword
    let kw = Keywords::default();
    // Setting the (absent) keyword to None must not change a single byte.
    let out = cockpit_org::set_todo(SAMPLE, inbox, &kw, None);
    assert_eq!(out, SAMPLE);
}
