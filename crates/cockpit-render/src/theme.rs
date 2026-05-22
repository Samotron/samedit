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

    /// Return this color as an array suitable for vertex attributes.
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
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
}
