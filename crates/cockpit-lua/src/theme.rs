//! Extension-registered themes.
//!
//! Themes are plain data here — the binary maps an [`ExtensionTheme`]
//! into a `cockpit_render::theme::Theme` once it accepts the
//! registration. Keeping this crate free of the render dependency
//! preserves the "only `cockpit-render` may touch `winit`/`glow`"
//! invariant (AGENTS §2 hard rule #1).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A theme an extension declared via `cockpit.themes.register{…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionTheme {
    /// Theme name as it will appear in the `Theme: Switch <name>` palette.
    pub name: String,
    /// Named colour palette. Recognised top-level keys: `background`,
    /// `pane_background`, `pane_border`, `text`, `muted_text`, `accent`,
    /// `selection`, `cursor`, `diagnostic_error`, `diagnostic_warning`,
    /// `diagnostic_info`, `diagnostic_hint`. Unknown keys are kept but
    /// ignored by the renderer.
    pub colors: BTreeMap<String, String>,
}

impl ExtensionTheme {
    /// Look up a colour by key, returning the raw hex string.
    pub fn color(&self, key: &str) -> Option<&str> {
        self.colors.get(key).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_theme_round_trips_colors() {
        let mut colors = BTreeMap::new();
        colors.insert("background".to_string(), "#1e1e2e".to_string());
        colors.insert("text".to_string(), "#cdd6f4".to_string());
        let theme = ExtensionTheme {
            name: "user.mocha".to_string(),
            colors,
        };
        assert_eq!(theme.color("background"), Some("#1e1e2e"));
        assert_eq!(theme.color("text"), Some("#cdd6f4"));
        assert!(theme.color("missing").is_none());
    }
}
