//! Chart-conversion plan — M5.5.
//!
//! ggsql cells emit Vega-Lite v6 JSON in their result. The notebook UI
//! turns that JSON into a PNG (or SVG) by piping it through
//! `mise exec -- vl-convert vl2png`. The conversion itself is a
//! subprocess call; this module is the headless planner that builds the
//! command spec the caller hands to a `ProcessRunner` — so the chart
//! renderer is testable without `vl-convert` on the test machine.
//!
//! AGENTS rule #6 applies here too: when `vl-convert` is absent, the
//! caller surfaces the "detect, surface, prompt" flow ("Add `vl-convert`
//! to mise.toml [tools]?"). Cockpit never auto-installs.

use std::path::Path;

use cockpit_project::ProcessSpec;

/// Target format the caller wants — drives the `vl-convert` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartFormat {
    /// Raster PNG, the M5.5 default — feeds the existing texture path
    /// in `cockpit-render`.
    Png,
    /// Vector SVG — handy for high-DPI displays once the painter has
    /// SVG support; off the v0.5 critical path.
    Svg,
}

impl ChartFormat {
    /// The `vl-convert` subcommand for this format.
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::Png => "vl2png",
            Self::Svg => "vl2svg",
        }
    }
}

/// Build the `mise exec -- vl-convert vlNN <subcommand>` spawn that
/// converts `input_path` (a Vega-Lite JSON file) into `output_path`.
/// The caller writes the JSON to disk first (typically under
/// `.cockpit/results/<file>.<cell>.json`) and reads the result back
/// from `output_path` after the subprocess returns.
pub fn vl_convert_spec(
    format: ChartFormat,
    input_path: &Path,
    output_path: &Path,
    root: &Path,
) -> ProcessSpec {
    ProcessSpec::new("mise")
        .arg("exec")
        .arg("--")
        .arg("vl-convert")
        .arg(format.subcommand())
        .arg("--input")
        .arg(input_path)
        .arg("--output")
        .arg(output_path)
        .current_dir(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn vl_convert_spec_uses_mise_exec_wrapper() {
        let spec = vl_convert_spec(
            ChartFormat::Png,
            Path::new("/proj/.cockpit/cell0.json"),
            Path::new("/proj/.cockpit/cell0.png"),
            Path::new("/proj"),
        );
        assert_eq!(spec.program, "mise");
        assert_eq!(spec.args[0], "exec");
        assert_eq!(spec.args[1], "--");
        assert_eq!(spec.args[2], "vl-convert");
        assert_eq!(spec.args[3], "vl2png");
        assert_eq!(spec.current_dir, Some(PathBuf::from("/proj")));
    }

    #[test]
    fn vl_convert_spec_supports_svg_target() {
        let spec = vl_convert_spec(
            ChartFormat::Svg,
            Path::new("/in.json"),
            Path::new("/out.svg"),
            Path::new("/proj"),
        );
        assert_eq!(spec.args[3], "vl2svg");
    }
}
