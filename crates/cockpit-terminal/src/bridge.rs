//! Editor↔terminal bridge.
//!
//! Surfaces file references printed in terminal output so the UI can jump to
//! them in the editor (spec §17 / M2.6). The detection itself lives in
//! [`path_detect`](crate::path_detect); this module scans a parsed
//! [`ScreenGrid`](crate::engine::ScreenGrid) row by row.

use crate::engine::ScreenGrid;
use crate::path_detect::{PathMatch, detect_paths};

/// Collect every file reference visible in `grid`, scanning rows top-to-bottom
/// and left-to-right. Later rows are the most recently printed output.
pub fn detect_paths_in_grid(grid: &ScreenGrid) -> Vec<PathMatch> {
    (0..grid.height())
        .filter_map(|row| grid.row_text(row))
        .flat_map(|line| detect_paths(&line))
        .collect()
}

/// Bytes to send when bridging a file path from the editor into the terminal.
/// The path is shell-quoted and terminated with Enter so it is ready for
/// commands that expect one path argument.
pub fn terminal_input_for_path(path: &str) -> Vec<u8> {
    let mut input = shell_quote_arg(path).into_bytes();
    input.push(b'\r');
    input
}

/// Bytes to send when bridging selected editor text into the terminal.
/// Newlines become carriage returns because PTY input uses CR for Enter.
pub fn terminal_input_for_selection(text: &str) -> Vec<u8> {
    let mut input = text.replace("\r\n", "\n").replace('\n', "\r").into_bytes();
    if !input.ends_with(b"\r") {
        input.push(b'\r');
    }
    input
}

/// Quote one shell argument using POSIX single-quote escaping.
pub fn shell_quote_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TerminalEngine;
    use crate::termwiz_engine::TermwizEngine;

    fn engine_with(lines: &[&str]) -> TermwizEngine {
        let mut engine = TermwizEngine::new(80, 24);
        for line in lines {
            engine.feed(line.as_bytes());
            engine.feed(b"\r\n");
        }
        engine
    }

    #[test]
    fn collects_paths_from_multiple_rows() {
        let engine = engine_with(&[
            "Compiling cockpit",
            "error --> src/main.rs:42:13",
            "note: also tests/api.rs:7",
        ]);
        let refs: Vec<String> = detect_paths_in_grid(engine.grid())
            .iter()
            .map(PathMatch::reference)
            .collect();
        assert_eq!(refs, ["src/main.rs:42:13", "tests/api.rs:7"]);
    }

    #[test]
    fn empty_grid_yields_no_paths() {
        let engine = engine_with(&[]);
        assert!(detect_paths_in_grid(engine.grid()).is_empty());
    }

    #[test]
    fn formats_current_file_path_for_terminal_input() {
        assert_eq!(
            terminal_input_for_path("src/main.rs"),
            b"'src/main.rs'\r".to_vec()
        );
        assert_eq!(
            terminal_input_for_path("src/it's.rs"),
            b"'src/it'\\''s.rs'\r".to_vec()
        );
    }

    #[test]
    fn formats_selected_text_for_terminal_input() {
        assert_eq!(
            terminal_input_for_selection("cargo test"),
            b"cargo test\r".to_vec()
        );
        assert_eq!(
            terminal_input_for_selection("echo one\necho two\n"),
            b"echo one\recho two\r".to_vec()
        );
    }
}
