//! Agenda performance gate (v0.12 M12.5b).
//!
//! The plan's budget: for ~10k headlines across 100 files (a reasonable upper
//! bound), the agenda must build in < 50 ms. Opt-in via `--features bench` so
//! the timing assertion never runs in the fast/default test leg.

#![cfg(feature = "bench")]

use std::time::Instant;

use cockpit_org::{Filter, OrgDate, OrgRoot, next_7_days, today, todo_list};

/// Build a root of `files` files, each with `per_file` scheduled headlines.
fn big_root(files: usize, per_file: usize) -> OrgRoot {
    let mut root = OrgRoot::new("/org");
    for f in 0..files {
        let mut src = String::new();
        for h in 0..per_file {
            let day = (h % 28) + 1;
            src.push_str(&format!(
                "* TODO item {f}-{h} :work:\nSCHEDULED: <2026-05-{day:02} Fri +1w>\n"
            ));
        }
        root.insert(format!("/org/file{f}.org"), src);
    }
    root
}

#[test]
fn agenda_perf_under_50ms_for_10k_headlines() {
    let root = big_root(100, 100); // 10_000 headlines
    let today_date = OrgDate::new(2026, 5, 15);
    let filter = Filter::default();

    let start = Instant::now();
    let _t = today(&root, today_date, &filter);
    let _w = next_7_days(&root, today_date, &filter);
    let _l = todo_list(&root, &filter);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "agenda build took {} ms (budget: 50 ms)",
        elapsed.as_millis()
    );
}
