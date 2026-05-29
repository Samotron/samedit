//! Theme and color primitives for render commands.

/// Linear RGBA color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Create an RGBA color.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create an opaque RGB color.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    /// Opaque color from a 0xRRGGBB sRGB value. Convenient for pasting
    /// palette hex codes (e.g. Catppuccin) verbatim.
    pub const fn hex(rgb: u32) -> Self {
        let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
        let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
        let b = (rgb & 0xFF) as f32 / 255.0;
        Self::rgba(r, g, b, 1.0)
    }

    /// Same as [`hex`] but with an explicit alpha channel in `[0, 1]`.
    pub const fn hex_with_alpha(rgb: u32, alpha: f32) -> Self {
        let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
        let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
        let b = (rgb & 0xFF) as f32 / 255.0;
        Self::rgba(r, g, b, alpha)
    }

    /// Return this color as an array suitable for vertex attributes.
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Rec. 709 perceptual luminance approximation used by Catppuccin
    /// flavour sanity checks.
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

/// Syntax-highlighting colours, one per token category.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxTheme {
    pub keyword: Color,
    pub function: Color,
    pub type_name: Color,
    pub string: Color,
    pub comment: Color,
    pub constant: Color,
    pub variable: Color,
    pub operator: Color,
    pub attribute: Color,
    pub punctuation: Color,
}

impl Default for SyntaxTheme {
    fn default() -> Self {
        Self {
            keyword: Color::rgb(0.780, 0.470, 0.870),
            function: Color::rgb(0.400, 0.680, 0.950),
            type_name: Color::rgb(0.900, 0.800, 0.450),
            string: Color::rgb(0.550, 0.780, 0.500),
            comment: Color::rgb(0.450, 0.500, 0.560),
            constant: Color::rgb(0.900, 0.620, 0.400),
            variable: Color::rgb(0.820, 0.860, 0.920),
            operator: Color::rgb(0.700, 0.780, 0.850),
            attribute: Color::rgb(0.450, 0.780, 0.720),
            punctuation: Color::rgb(0.620, 0.670, 0.740),
        }
    }
}

/// Default renderer theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub background: Color,
    pub pane_background: Color,
    pub pane_border: Color,
    pub text: Color,
    pub muted_text: Color,
    pub accent: Color,
    pub selection: Color,
    pub cursor: Color,
    pub syntax: SyntaxTheme,
    pub diagnostic_error: Color,
    pub diagnostic_warning: Color,
    pub diagnostic_info: Color,
    pub diagnostic_hint: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color::rgb(0.075, 0.082, 0.095),
            pane_background: Color::rgb(0.105, 0.113, 0.130),
            pane_border: Color::rgb(0.210, 0.225, 0.250),
            text: Color::rgb(0.900, 0.920, 0.940),
            muted_text: Color::rgb(0.560, 0.600, 0.650),
            accent: Color::rgb(0.270, 0.520, 0.900),
            selection: Color::rgba(0.270, 0.520, 0.900, 0.35),
            cursor: Color::rgb(0.960, 0.960, 0.900),
            syntax: SyntaxTheme::default(),
            diagnostic_error: Color::rgb(0.920, 0.380, 0.380),
            diagnostic_warning: Color::rgb(0.950, 0.760, 0.380),
            diagnostic_info: Color::rgb(0.470, 0.700, 0.940),
            diagnostic_hint: Color::rgb(0.560, 0.600, 0.650),
        }
    }
}

impl Theme {
    /// Resolve a theme by name. Returns `None` so callers can fall back
    /// without panicking on a typo'd config (v0.8 M8.1).
    ///
    /// Recognises (case-insensitive): `dark`, `latte`, `frappe`,
    /// `macchiato`, `mocha`. The `catppuccin-` alias prefix is accepted
    /// (so `catppuccin-mocha` and `mocha` resolve to the same flavour).
    pub fn from_name(name: &str) -> Option<Self> {
        let canonical = name
            .trim()
            .to_ascii_lowercase()
            .trim_start_matches("catppuccin-")
            .to_string();
        match canonical.as_str() {
            "dark" => Some(Self::default()),
            "latte" => Some(Self::catppuccin_latte()),
            "frappe" => Some(Self::catppuccin_frappe()),
            "macchiato" => Some(Self::catppuccin_macchiato()),
            "mocha" => Some(Self::catppuccin_mocha()),
            _ => None,
        }
    }

    /// Catppuccin Latte — light flavour. Palette: https://catppuccin.com/palette
    pub fn catppuccin_latte() -> Self {
        Self {
            background: Color::hex(0xeff1f5),      // Base
            pane_background: Color::hex(0xe6e9ef), // Mantle
            pane_border: Color::hex(0xccd0da),     // Surface0
            text: Color::hex(0x4c4f69),            // Text
            muted_text: Color::hex(0x6c6f85),      // Subtext0
            accent: Color::hex(0x1e66f5),          // Blue
            selection: Color::hex_with_alpha(0x1e66f5, 0.25),
            cursor: Color::hex(0xdc8a78), // Rosewater
            syntax: SyntaxTheme {
                keyword: Color::hex(0x8839ef),     // Mauve
                function: Color::hex(0x1e66f5),    // Blue
                type_name: Color::hex(0xdf8e1d),   // Yellow
                string: Color::hex(0x40a02b),      // Green
                comment: Color::hex(0x8c8fa1),     // Overlay1
                constant: Color::hex(0xfe640b),    // Peach
                variable: Color::hex(0x4c4f69),    // Text
                operator: Color::hex(0x04a5e5),    // Sky
                attribute: Color::hex(0x179299),   // Teal
                punctuation: Color::hex(0x7c7f93), // Overlay2
            },
            diagnostic_error: Color::hex(0xd20f39),   // Red
            diagnostic_warning: Color::hex(0xdf8e1d), // Yellow
            diagnostic_info: Color::hex(0x209fb5),    // Sapphire
            diagnostic_hint: Color::hex(0x6c6f85),    // Subtext0
        }
    }

    /// Catppuccin Frappé — medium-dark flavour.
    pub fn catppuccin_frappe() -> Self {
        Self {
            background: Color::hex(0x303446),      // Base
            pane_background: Color::hex(0x292c3c), // Mantle
            pane_border: Color::hex(0x414559),     // Surface0
            text: Color::hex(0xc6d0f5),            // Text
            muted_text: Color::hex(0xb5bfe2),      // Subtext1
            accent: Color::hex(0x8caaee),          // Blue
            selection: Color::hex_with_alpha(0x8caaee, 0.30),
            cursor: Color::hex(0xf2d5cf), // Rosewater
            syntax: SyntaxTheme {
                keyword: Color::hex(0xca9ee6),     // Mauve
                function: Color::hex(0x8caaee),    // Blue
                type_name: Color::hex(0xe5c890),   // Yellow
                string: Color::hex(0xa6d189),      // Green
                comment: Color::hex(0x838ba7),     // Overlay1
                constant: Color::hex(0xef9f76),    // Peach
                variable: Color::hex(0xc6d0f5),    // Text
                operator: Color::hex(0x99d1db),    // Sky
                attribute: Color::hex(0x81c8be),   // Teal
                punctuation: Color::hex(0x949cbb), // Overlay2
            },
            diagnostic_error: Color::hex(0xe78284),   // Red
            diagnostic_warning: Color::hex(0xe5c890), // Yellow
            diagnostic_info: Color::hex(0x85c1dc),    // Sapphire
            diagnostic_hint: Color::hex(0xa5adce),    // Subtext0
        }
    }

    /// Catppuccin Macchiato — darker flavour.
    pub fn catppuccin_macchiato() -> Self {
        Self {
            background: Color::hex(0x24273a),      // Base
            pane_background: Color::hex(0x1e2030), // Mantle
            pane_border: Color::hex(0x363a4f),     // Surface0
            text: Color::hex(0xcad3f5),            // Text
            muted_text: Color::hex(0xb8c0e0),      // Subtext1
            accent: Color::hex(0x8aadf4),          // Blue
            selection: Color::hex_with_alpha(0x8aadf4, 0.30),
            cursor: Color::hex(0xf4dbd6), // Rosewater
            syntax: SyntaxTheme {
                keyword: Color::hex(0xc6a0f6),     // Mauve
                function: Color::hex(0x8aadf4),    // Blue
                type_name: Color::hex(0xeed49f),   // Yellow
                string: Color::hex(0xa6da95),      // Green
                comment: Color::hex(0x8087a2),     // Overlay1
                constant: Color::hex(0xf5a97f),    // Peach
                variable: Color::hex(0xcad3f5),    // Text
                operator: Color::hex(0x91d7e3),    // Sky
                attribute: Color::hex(0x8bd5ca),   // Teal
                punctuation: Color::hex(0x939ab7), // Overlay2
            },
            diagnostic_error: Color::hex(0xed8796),   // Red
            diagnostic_warning: Color::hex(0xeed49f), // Yellow
            diagnostic_info: Color::hex(0x7dc4e4),    // Sapphire
            diagnostic_hint: Color::hex(0xa5adcb),    // Subtext0
        }
    }

    /// Catppuccin Mocha — darkest flavour.
    pub fn catppuccin_mocha() -> Self {
        Self {
            background: Color::hex(0x1e1e2e),      // Base
            pane_background: Color::hex(0x181825), // Mantle
            pane_border: Color::hex(0x313244),     // Surface0
            text: Color::hex(0xcdd6f4),            // Text
            muted_text: Color::hex(0xbac2de),      // Subtext1
            accent: Color::hex(0x89b4fa),          // Blue
            selection: Color::hex_with_alpha(0x89b4fa, 0.30),
            cursor: Color::hex(0xf5e0dc), // Rosewater
            syntax: SyntaxTheme {
                keyword: Color::hex(0xcba6f7),     // Mauve
                function: Color::hex(0x89b4fa),    // Blue
                type_name: Color::hex(0xf9e2af),   // Yellow
                string: Color::hex(0xa6e3a1),      // Green
                comment: Color::hex(0x7f849c),     // Overlay1
                constant: Color::hex(0xfab387),    // Peach
                variable: Color::hex(0xcdd6f4),    // Text
                operator: Color::hex(0x89dceb),    // Sky
                attribute: Color::hex(0x94e2d5),   // Teal
                punctuation: Color::hex(0x9399b2), // Overlay2
            },
            diagnostic_error: Color::hex(0xf38ba8),   // Red
            diagnostic_warning: Color::hex(0xf9e2af), // Yellow
            diagnostic_info: Color::hex(0x74c7ec),    // Sapphire
            diagnostic_hint: Color::hex(0xa6adc8),    // Subtext0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_opaque_except_translucent_selection() {
        let theme = Theme::default();
        assert_eq!(theme.background.a, 1.0);
        assert_eq!(theme.text.a, 1.0);
        assert!(theme.selection.a < 1.0);
    }

    #[test]
    fn default_syntax_colors_are_opaque() {
        let syntax = SyntaxTheme::default();
        for color in [
            syntax.keyword,
            syntax.function,
            syntax.type_name,
            syntax.string,
            syntax.comment,
            syntax.constant,
            syntax.variable,
            syntax.operator,
            syntax.attribute,
            syntax.punctuation,
        ] {
            assert_eq!(color.a, 1.0);
        }
    }

    #[test]
    fn color_converts_to_vertex_array() {
        assert_eq!(
            Color::rgba(0.1, 0.2, 0.3, 0.4).to_array(),
            [0.1, 0.2, 0.3, 0.4]
        );
    }

    #[test]
    fn color_hex_decodes_24_bit_values() {
        let teal = Color::hex(0x179299);
        assert!((teal.r - 0x17 as f32 / 255.0).abs() < 1e-6);
        assert!((teal.g - 0x92 as f32 / 255.0).abs() < 1e-6);
        assert!((teal.b - 0x99 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(teal.a, 1.0);

        let translucent = Color::hex_with_alpha(0xff8800, 0.5);
        assert_eq!(translucent.a, 0.5);
    }

    #[test]
    fn theme_from_name_resolves_every_known_flavour() {
        assert_eq!(Theme::from_name("dark"), Some(Theme::default()));
        assert_eq!(Theme::from_name("Latte"), Some(Theme::catppuccin_latte()));
        assert_eq!(Theme::from_name("FRAPPE"), Some(Theme::catppuccin_frappe()));
        assert_eq!(
            Theme::from_name("catppuccin-macchiato"),
            Some(Theme::catppuccin_macchiato())
        );
        assert_eq!(
            Theme::from_name("CATPPUCCIN-MOCHA"),
            Some(Theme::catppuccin_mocha())
        );

        assert_eq!(Theme::from_name("solarized"), None);
        assert_eq!(Theme::from_name(""), None);
    }

    #[test]
    fn every_catppuccin_flavour_is_opaque_except_selection() {
        for theme in [
            Theme::catppuccin_latte(),
            Theme::catppuccin_frappe(),
            Theme::catppuccin_macchiato(),
            Theme::catppuccin_mocha(),
        ] {
            assert_eq!(theme.background.a, 1.0);
            assert_eq!(theme.text.a, 1.0);
            assert_eq!(theme.accent.a, 1.0);
            assert_eq!(theme.cursor.a, 1.0);
            for color in [
                theme.syntax.keyword,
                theme.syntax.function,
                theme.syntax.type_name,
                theme.syntax.string,
                theme.syntax.comment,
                theme.syntax.constant,
                theme.syntax.variable,
                theme.syntax.operator,
                theme.syntax.attribute,
                theme.syntax.punctuation,
            ] {
                assert_eq!(color.a, 1.0);
            }
            assert!(theme.selection.a < 1.0);
        }
    }

    #[test]
    fn latte_luminance_is_greater_than_mocha_luminance() {
        let latte = Theme::catppuccin_latte().background.luminance();
        let mocha = Theme::catppuccin_mocha().background.luminance();
        assert!(
            latte > mocha,
            "expected Latte to be brighter than Mocha (latte={latte}, mocha={mocha})"
        );
        // Sanity-check the ordering of every flavour from darkest to brightest.
        let frappe = Theme::catppuccin_frappe().background.luminance();
        let macchiato = Theme::catppuccin_macchiato().background.luminance();
        assert!(mocha < macchiato);
        assert!(macchiato < frappe);
        assert!(frappe < latte);
    }
}
