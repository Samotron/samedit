//! Editor↔terminal bridge (spec §17).
//!
//! Two directions:
//!
//! * **Terminal → editor** — [`detect_paths_in_grid`] surfaces file references
//!   printed in terminal output so the UI can jump to them (M2.6). The
//!   detection itself lives in [`path_detect`](crate::path_detect).
//! * **Editor → terminal** — [`paste_to_terminal`] wraps editor content in
//!   bracketed-paste markers so the receiving shell treats it as an inserted
//!   block (no accidental execution for multi-line selections, and the cursor
//!   ends after the text). [`render_document_path`] formats a document path
//!   project-relative when possible, matching the §17 `path:line:col` form.

use std::path::Path;

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

/// Bracketed-paste start sequence (DEC `\e[200~`).
const PASTE_START: &[u8] = b"\x1b[200~";
/// Bracketed-paste end sequence (DEC `\e[201~`).
const PASTE_END: &[u8] = b"\x1b[201~";

/// Wrap `text` in bracketed-paste markers so the receiving shell's line editor
/// treats it as a pasted block. Without this, multi-line selections would run
/// each line as a command the moment it hit the shell.
pub fn paste_to_terminal(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PASTE_START.len() + text.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// Render a document path for the terminal: project-relative when `path` lives
/// under `project_root`, otherwise the absolute path. The relative form
/// matches the §17 `path:line:col` references printed by tools.
pub fn render_document_path(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
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
    fn paste_wraps_text_in_bracketed_paste_markers() {
        let bytes = paste_to_terminal("ls -la");
        assert_eq!(bytes, b"\x1b[200~ls -la\x1b[201~");
    }

    #[test]
    fn paste_preserves_newlines_inside_the_block() {
        let bytes = paste_to_terminal("fn main() {\n    println!(\"hi\");\n}");
        assert!(bytes.starts_with(b"\x1b[200~"));
        assert!(bytes.ends_with(b"\x1b[201~"));
        // The body sits verbatim between the markers, newlines included.
        assert_eq!(
            &bytes[PASTE_START.len()..bytes.len() - PASTE_END.len()],
            b"fn main() {\n    println!(\"hi\");\n}",
        );
    }

    #[test]
    fn paste_handles_empty_text() {
        assert_eq!(paste_to_terminal(""), b"\x1b[200~\x1b[201~");
    }

    #[test]
    fn renders_document_paths_project_relative() {
        let root = Path::new("/code/geotech");
        let path = Path::new("/code/geotech/src/main.rs");
        assert_eq!(render_document_path(path, root), "src/main.rs");
    }

    #[test]
    fn renders_document_paths_absolute_when_outside_the_project() {
        let root = Path::new("/code/geotech");
        let path = Path::new("/etc/hosts");
        assert_eq!(render_document_path(path, root), "/etc/hosts");
    }
}
