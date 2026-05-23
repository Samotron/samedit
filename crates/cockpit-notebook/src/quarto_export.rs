//! Quarto export plan — M5.Q3.
//!
//! `Quarto: Render` shells out to `mise exec -- quarto render <file>`
//! and reports the output path in a status toast. Spec is explicit
//! about *not* embedding a WebView: that would pull CEF/GTK and break
//! the v0.6 instant-load target. Live preview is out of scope for v0.5
//! — the in-editor Markdown rendering (M5.Q2) *is* the preview.

use std::path::Path;

use cockpit_project::ProcessSpec;

/// Build the `mise exec -- quarto render <file>` spec. The caller
/// runs it through a `ProcessRunner` and opens the output via the OS
/// handler when the process exits 0.
pub fn quarto_render_spec(file: &Path, root: &Path) -> ProcessSpec {
    ProcessSpec::new("mise")
        .arg("exec")
        .arg("--")
        .arg("quarto")
        .arg("render")
        .arg(file)
        .current_dir(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_spec_wraps_quarto_in_mise_exec() {
        let spec = quarto_render_spec(Path::new("/proj/report.qmd"), Path::new("/proj"));
        assert_eq!(spec.program, "mise");
        assert_eq!(spec.args[..4], ["exec", "--", "quarto", "render"]);
        assert_eq!(spec.args[4], "/proj/report.qmd");
    }
}
