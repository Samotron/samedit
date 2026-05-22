//! Golden tests for command palette filtering (spec §18.3 / M1.19).

use std::fmt::Write;

use cockpit_ui::palette::{Palette, PaletteEntry};

fn fixture() -> Palette {
    // Curated subset of spec §16 v0.1 commands.
    Palette::new(vec![
        PaletteEntry::new("project.open", "Project: Open Project"),
        PaletteEntry::new("project.recent", "Project: Recent Projects"),
        PaletteEntry::new("project.close", "Project: Close Project"),
        PaletteEntry::new("file.open", "File: Open"),
        PaletteEntry::new("file.save", "File: Save"),
        PaletteEntry::new("file.reveal", "File: Reveal in Tree"),
        PaletteEntry::new(
            "editor.toggle_relative_line_numbers",
            "Editor: Toggle Relative Line Numbers",
        ),
        PaletteEntry::new("terminal.focus", "Terminal: Focus"),
        PaletteEntry::new("terminal.restart_zellij", "Terminal: Restart Zellij"),
        PaletteEntry::new(
            "terminal.new_zellij_session",
            "Terminal: New Zellij Session",
        ),
        PaletteEntry::new("mise.run_task", "Mise: Run Task"),
        PaletteEntry::new("mise.install_tools", "Mise: Install Tools"),
        PaletteEntry::new("mise.open_config", "Mise: Open Config"),
        PaletteEntry::new("mise.show_tools", "Mise: Show Tools"),
        PaletteEntry::new("test.run_all", "Test: Run All"),
        PaletteEntry::new("test.run_current_file", "Test: Run Current File"),
        PaletteEntry::new("test.run_nearest", "Test: Run Nearest"),
    ])
}

fn snapshot(query: &str) -> String {
    let mut palette = fixture();
    palette.set_query(query);

    let mut out = String::new();
    writeln!(&mut out, "query: {query:?}").unwrap();
    writeln!(&mut out, "matches: {}", palette.matches().len()).unwrap();
    for m in palette.matches() {
        let entry = &palette.entries()[m.entry_index];
        writeln!(
            &mut out,
            "  - [{:>3}] {} ({})",
            m.score, entry.title, entry.id
        )
        .unwrap();
    }
    out
}

#[test]
fn golden_empty_query_lists_everything_in_order() {
    insta::assert_snapshot!(snapshot(""));
}

#[test]
fn golden_prefix_query_terminal() {
    insta::assert_snapshot!(snapshot("term"));
}

#[test]
fn golden_word_start_acronym_rt() {
    insta::assert_snapshot!(snapshot("rt"));
}

#[test]
fn golden_subsequence_query_mst() {
    insta::assert_snapshot!(snapshot("mst"));
}

#[test]
fn golden_no_matches() {
    insta::assert_snapshot!(snapshot("xyzzy"));
}
