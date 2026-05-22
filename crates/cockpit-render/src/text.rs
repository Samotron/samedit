//! Text shaping primitives built on `cosmic-text`.

use crate::painter::TextRun;
use crate::theme::Color;
use cosmic_text::{Attrs, Buffer, CacheKey, FontSystem, Metrics, Shaping};

/// Default line-height multiplier used for UI text.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.35;

/// One shaped glyph ready for raster-cache lookup and quad generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPlacement {
    pub cache_key: CacheKey,
    pub physical_x: i32,
    pub physical_y: i32,
    pub logical_x: f32,
    pub logical_y: f32,
    pub width: f32,
    pub font_size: f32,
    pub color: [f32; 4],
}

/// Shaped output for one text run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapedText {
    pub glyphs: Vec<GlyphPlacement>,
    pub width: f32,
    pub height: f32,
}

/// Application text shaper. Create one per renderer and reuse it across frames.
pub struct TextLayouter {
    font_system: FontSystem,
    attrs: Attrs<'static>,
    line_height_scale: f32,
}

impl TextLayouter {
    /// Create a layouter using the system font database.
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            attrs: Attrs::new(),
            line_height_scale: DEFAULT_LINE_HEIGHT,
        }
    }

    /// Shape a painter text run with optional wrapping width.
    pub fn shape_run(&mut self, run: &TextRun, max_width: Option<f32>) -> ShapedText {
        self.shape(run.x, run.y, &run.text, run.color, run.font_size, max_width)
    }

    /// Access the underlying font system for glyph rasterization.
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Shape text at a logical origin with optional wrapping width.
    pub fn shape(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        color: Color,
        font_size: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        if text.is_empty() || font_size <= 0.0 {
            return ShapedText::default();
        }

        let metrics = Metrics::relative(font_size, self.line_height_scale);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let mut buffer = buffer.borrow_with(&mut self.font_system);
        buffer.set_size(max_width, None);
        buffer.set_text(text, &self.attrs, Shaping::Advanced, None);

        let mut shaped = ShapedText::default();
        let color = color.to_array();

        for layout_run in buffer.layout_runs() {
            shaped.width = shaped.width.max(layout_run.line_w);
            shaped.height = shaped
                .height
                .max(layout_run.line_top + layout_run.line_height);

            for glyph in layout_run.glyphs {
                let physical = glyph.physical((x, y + layout_run.line_y), 1.0);
                shaped.glyphs.push(GlyphPlacement {
                    cache_key: physical.cache_key,
                    physical_x: physical.x,
                    physical_y: physical.y,
                    logical_x: x + glyph.x,
                    logical_y: y + layout_run.line_top,
                    width: glyph.w,
                    font_size: glyph.font_size,
                    color,
                });
            }
        }

        shaped
    }
}

impl Default for TextLayouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_ascii_text_into_glyphs() {
        let mut layouter = TextLayouter::new();

        let shaped = layouter.shape(10.0, 20.0, "Hello", Color::rgb(1.0, 1.0, 1.0), 14.0, None);

        assert!(!shaped.glyphs.is_empty());
        assert!(shaped.width > 0.0);
        assert!(shaped.height >= 14.0);
        assert_eq!(shaped.glyphs[0].color, [1.0, 1.0, 1.0, 1.0]);
        assert!(shaped.glyphs[0].logical_x >= 10.0);
    }

    #[test]
    fn skips_empty_or_invalid_text() {
        let mut layouter = TextLayouter::new();

        assert!(
            layouter
                .shape(0.0, 0.0, "", Color::rgb(1.0, 1.0, 1.0), 14.0, None)
                .glyphs
                .is_empty()
        );
        assert!(
            layouter
                .shape(0.0, 0.0, "x", Color::rgb(1.0, 1.0, 1.0), 0.0, None)
                .glyphs
                .is_empty()
        );
    }

    #[test]
    fn shape_run_uses_painter_text_properties() {
        let mut layouter = TextLayouter::new();
        let run = TextRun {
            x: 4.0,
            y: 8.0,
            text: "ok".to_string(),
            color: Color::rgb(0.25, 0.5, 0.75),
            font_size: 16.0,
        };

        let shaped = layouter.shape_run(&run, None);

        assert!(!shaped.glyphs.is_empty());
        assert_eq!(shaped.glyphs[0].color, run.color.to_array());
        assert!(shaped.glyphs.iter().all(|glyph| glyph.font_size == 16.0));
    }
}
