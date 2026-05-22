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
}
