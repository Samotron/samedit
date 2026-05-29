//! Agenda views and repeater-bump over an Emacs-style fixture root that
//! contains every scheduling permutation (M12.5b).

use cockpit_org::{AgendaKind, Filter, OrgDate, OrgRoot, complete, next_7_days, today, todo_list};

const AGENDA: &str = include_str!("../../../tests/fixtures/org/agenda.org");

const TODAY: OrgDate = OrgDate {
    year: 2026,
    month: 5,
    day: 29,
};

fn root() -> OrgRoot {
    OrgRoot::from_files("/org", [("/org/agenda.org", AGENDA)])
}

#[test]
fn today_view_buckets_and_orders() {
    let items = today(&root(), TODAY, &Filter::default());
    let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Overdue deadline",  // 05-20
            "Overdue scheduled", // 05-25
            "Timed meeting",     // 05-29 09:30 (timed sorts first)
            "Deadline today",    // 05-29
            "Scheduled today",   // 05-29
            "Weekly repeat",     // 05-29
        ]
    );
    assert!(items[0].overdue && items[1].overdue);
    assert!(items.iter().skip(2).all(|i| !i.overdue));
    assert_eq!(items[0].kind, AgendaKind::Deadline);
    assert_eq!(items[2].time.map(|t| (t.hour, t.minute)), Some((9, 30)));
}

#[test]
fn next_7_days_window() {
    let week = next_7_days(&root(), TODAY, &Filter::default());
    assert_eq!(week.len(), 7);
    assert_eq!(week[0].date, OrgDate::new(2026, 5, 29));

    let day0: Vec<_> = week[0].items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        day0,
        [
            "Timed meeting",
            "Deadline today",
            "Scheduled today",
            "Weekly repeat"
        ]
    );
    assert_eq!(week[2].items[0].title, "Later this week"); // 05-31
    assert_eq!(week[6].date, OrgDate::new(2026, 6, 4));
    assert_eq!(week[6].items[0].title, "Next week edge");

    // Far future (07-15) is outside the window.
    assert!(
        week.iter()
            .all(|d| d.items.iter().all(|i| i.title != "Far future"))
    );
}

#[test]
fn todo_list_collects_open_keywords() {
    let groups = todo_list(&root(), &Filter::default());
    assert_eq!(groups.len(), 1);
    let titles: Vec<_> = groups[0].items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles.len(), 10);
    assert!(titles.contains(&"No date but a keyword"));
    assert!(!titles.contains(&"Done in the past"));
    assert!(!titles.contains(&"Plain heading without a keyword"));
}

#[test]
fn tag_filter_narrows_today() {
    let items = today(&root(), TODAY, &Filter::parse("+home"));
    let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Overdue scheduled", "Weekly repeat"]);
}

#[test]
fn complete_bumps_repeater_byte_identically() {
    let file = root();
    let f = file.file("/org/agenda.org").unwrap();
    let weekly = f
        .iter_headings()
        .find(|h| h.title == "Weekly repeat")
        .expect("weekly repeat heading");

    let out = complete(AGENDA, weekly, file.keywords(), TODAY);

    let before: Vec<&str> = AGENDA.lines().collect();
    let after: Vec<&str> = out.lines().collect();
    assert_eq!(before.len(), after.len());

    let changed = weekly.line_range.start + 1; // the SCHEDULED line
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if i == changed {
            assert_eq!(b.trim(), "SCHEDULED: <2026-05-29 Fri +1w>");
            assert_eq!(a.trim(), "SCHEDULED: <2026-06-05 Fri +1w>");
        } else {
            assert_eq!(b, a, "line {i} must be byte-identical");
        }
    }
}
