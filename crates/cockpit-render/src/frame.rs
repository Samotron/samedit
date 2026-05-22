//! Per-frame render preparation.

use crate::geometry::{GeometryError, Mesh, rect_mesh_from_commands};
use crate::glyph_cache::{
    CachedGlyph, GlyphCacheUpdate, GlyphRasterCache, GlyphRasterError, GlyphUpload,
};
use crate::painter::{DrawCommand, Painter};
use crate::text::{GlyphPlacement, TextLayouter};
use crate::theme::{Color, Theme};
use thiserror::Error;

/// A frame prepared for renderer backend upload/draw calls.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderFrame {
    pub clear_color: Color,
    pub rect_mesh: Mesh,
    pub text_runs: Vec<PreparedTextRun>,
    pub glyph_uploads: Vec<GlyphUpload>,
}

/// One shaped text run with atlas locations resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreparedTextRun {
    pub glyphs: Vec<PreparedGlyph>,
    pub width: f32,
    pub height: f32,
}

/// One glyph instance ready to turn into a textured quad.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedGlyph {
    pub placement: GlyphPlacement,
    pub cached: CachedGlyph,
}

/// Stateful frame planner. Owns text and glyph caches reused across frames.
pub struct FramePlanner {
    text_layouter: TextLayouter,
    glyph_cache: GlyphRasterCache,
}

impl FramePlanner {
    /// Create a planner with a glyph atlas of the provided size.
    pub fn new(atlas_width: i32, atlas_height: i32) -> Result<Self, FrameError> {
        Ok(Self {
            text_layouter: TextLayouter::new(),
            glyph_cache: GlyphRasterCache::new(atlas_width, atlas_height)?,
        })
    }

    /// Build one render frame from a painter command buffer.
    pub fn build(&mut self, painter: &Painter, theme: &Theme) -> Result<RenderFrame, FrameError> {
        let rect_mesh = rect_mesh_from_commands(painter.commands())?;
        let mut text_runs = Vec::new();
        let mut glyph_uploads = Vec::new();

        for command in painter.commands() {
            let DrawCommand::Text(text_run) = command else {
                continue;
            };

            let shaped = self.text_layouter.shape_run(text_run, None);
            let mut prepared = PreparedTextRun {
                glyphs: Vec::with_capacity(shaped.glyphs.len()),
                width: shaped.width,
                height: shaped.height,
            };

            for placement in shaped.glyphs {
                match self
                    .glyph_cache
                    .ensure_glyph(self.text_layouter.font_system_mut(), placement.cache_key)?
                {
                    GlyphCacheUpdate::Cached(cached) => {
                        prepared.glyphs.push(PreparedGlyph { placement, cached });
                    }
                    GlyphCacheUpdate::Uploaded { glyph, upload } => {
                        glyph_uploads.push(upload);
                        prepared.glyphs.push(PreparedGlyph {
                            placement,
                            cached: glyph,
                        });
                    }
                    GlyphCacheUpdate::Missing => {}
                }
            }

            text_runs.push(prepared);
        }

        Ok(RenderFrame {
            clear_color: theme.background,
            rect_mesh,
            text_runs,
            glyph_uploads,
        })
    }

    /// Access the glyph cache for renderer atlas metadata.
    pub fn glyph_cache(&self) -> &GlyphRasterCache {
        &self.glyph_cache
    }
}

/// Frame preparation error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameError {
    #[error(transparent)]
    Geometry(#[from] GeometryError),
    #[error(transparent)]
    GlyphRaster(#[from] GlyphRasterError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::Rect;

    #[test]
    fn builds_rect_mesh_and_prepared_text() {
        let mut painter = Painter::new();
        let theme = Theme::default();
        painter.rect(Rect::new(0.0, 0.0, 10.0, 10.0), theme.accent);
        painter.text(4.0, 18.0, "Hi", theme.text, 14.0);

        let mut planner = FramePlanner::new(256, 256).unwrap();
        let frame = planner.build(&painter, &theme).unwrap();

        assert_eq!(frame.clear_color, theme.background);
        assert_eq!(frame.rect_mesh.vertices.len(), 4);
        assert_eq!(frame.text_runs.len(), 1);
        assert!(!frame.text_runs[0].glyphs.is_empty());
        assert!(!frame.glyph_uploads.is_empty());
    }

    #[test]
    fn reuses_glyph_uploads_on_later_frames() {
        let mut painter = Painter::new();
        let theme = Theme::default();
        painter.text(0.0, 16.0, "A", theme.text, 14.0);

        let mut planner = FramePlanner::new(256, 256).unwrap();
        let first = planner.build(&painter, &theme).unwrap();
        let second = planner.build(&painter, &theme).unwrap();

        assert!(!first.glyph_uploads.is_empty());
        assert!(second.glyph_uploads.is_empty());
        assert_eq!(second.text_runs.len(), 1);
        assert!(!second.text_runs[0].glyphs.is_empty());
    }

    #[test]
    fn propagates_glyph_atlas_errors() {
        let mut painter = Painter::new();
        painter.text(0.0, 16.0, "W", Color::rgb(1.0, 1.0, 1.0), 16.0);

        let mut planner = FramePlanner::new(4, 4).unwrap();
        let err = planner.build(&painter, &Theme::default()).unwrap_err();

        assert!(matches!(err, FrameError::GlyphRaster(_)));
    }
}
