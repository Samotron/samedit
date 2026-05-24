//! Per-frame render preparation.

use std::path::Path;

use crate::atlas_persist::{self, AtlasPersistError};
use crate::geometry::{GeometryError, Mesh, rect_mesh_from_commands};
use crate::glyph_cache::{
    CachedGlyph, GlyphCacheUpdate, GlyphRasterCache, GlyphRasterError, GlyphUpload, RehydrateError,
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

    /// Try to warm the glyph atlas from a previously written disk cache
    /// (v0.6 M6.4).
    ///
    /// Returns:
    /// - `Ok(Some(pixels))` — cache hit, pixel buffer ready to upload
    ///   in a single `tex_sub_image_2d` call.
    /// - `Ok(None)` — no usable cache (file missing, dimensions mismatch,
    ///   config hash drifted, allocator replay didn't match the stored
    ///   layout, …); the caller should proceed with an empty atlas and
    ///   pay the per-glyph rasterization cost as usual.
    /// - `Err(_)` — disk I/O error severe enough to surface; the binary
    ///   logs it and falls back to a fresh atlas regardless.
    ///
    /// All "soft" cache misses are logged at `debug`/`info`/`warn` via
    /// `tracing` so the binary doesn't have to repeat that wiring.
    pub fn warm_from_disk(&mut self, path: &Path) -> Result<Option<Vec<u8>>, AtlasPersistError> {
        let snapshot = match atlas_persist::load_from_disk(path)? {
            Some(snapshot) => snapshot,
            None => {
                tracing::debug!(path = %path.display(), "no glyph atlas cache on disk");
                return Ok(None);
            }
        };

        let live_hash = atlas_persist::font_set_config_hash(
            self.text_layouter.font_system_mut(),
            self.glyph_cache.atlas_width(),
            self.glyph_cache.atlas_height(),
            // padding is owned by GlyphAtlas — match the
            // GlyphRasterCache constructor's default of 1.
            1,
        );
        if snapshot.config_hash != live_hash {
            tracing::info!(
                path = %path.display(),
                snapshot_hash = snapshot.config_hash,
                live_hash,
                "glyph atlas cache hash mismatch — rebuilding from scratch",
            );
            return Ok(None);
        }

        let glyph_count = snapshot.glyphs.len();
        match self
            .glyph_cache
            .rehydrate(snapshot, self.text_layouter.font_system_mut())
        {
            Ok(pixels) => {
                tracing::info!(
                    path = %path.display(),
                    glyphs = glyph_count,
                    "glyph atlas warmed from disk cache",
                );
                Ok(Some(pixels))
            }
            Err(err) => {
                let reason: &dyn std::fmt::Display = match &err {
                    RehydrateError::AtlasSizeMismatch { .. } => &"atlas size mismatch",
                    RehydrateError::Persist(_) => &"persist error",
                    RehydrateError::Atlas(_) => &"atlas allocation error",
                    RehydrateError::UnknownFont { .. } => &"unknown font",
                    RehydrateError::AllocatorReplayDrift { .. } => &"allocator replay drift",
                };
                tracing::warn!(
                    path = %path.display(),
                    %reason,
                    error = %err,
                    "glyph atlas cache rehydrate failed — rebuilding",
                );
                Ok(None)
            }
        }
    }

    /// Persist the warmed glyph atlas to disk (v0.6 M6.4).
    ///
    /// Returns `Ok(true)` when a snapshot was actually written,
    /// `Ok(false)` when there was nothing to persist (cache clean since
    /// the last warm/persist).
    pub fn persist_to_disk(&mut self, path: &Path) -> Result<bool, AtlasPersistError> {
        if !self.glyph_cache.is_dirty() {
            return Ok(false);
        }
        let live_hash = atlas_persist::font_set_config_hash(
            self.text_layouter.font_system_mut(),
            self.glyph_cache.atlas_width(),
            self.glyph_cache.atlas_height(),
            1,
        );
        let Some(snapshot) = self
            .glyph_cache
            .snapshot(self.text_layouter.font_system_mut(), live_hash)
        else {
            tracing::warn!(
                "glyph atlas snapshot skipped: a cached glyph references a font no longer in the db",
            );
            return Ok(false);
        };
        atlas_persist::store_to_disk(path, &snapshot)?;
        self.glyph_cache.mark_persisted();
        tracing::debug!(
            path = %path.display(),
            glyphs = snapshot.glyphs.len(),
            "glyph atlas snapshot written",
        );
        Ok(true)
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

    /// M6.4 end-to-end: a planner that's painted some text writes a
    /// snapshot to disk; a fresh planner warms from that snapshot and
    /// reports zero glyph uploads on the next equivalent frame.
    #[test]
    fn warm_from_disk_round_trips_through_persist_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("atlas.bin");
        let theme = Theme::default();

        // Session 1: paint, then persist.
        let mut first = FramePlanner::new(256, 256).unwrap();
        let mut painter = Painter::new();
        painter.text(0.0, 16.0, "Hi", theme.text, 14.0);
        let first_frame = first.build(&painter, &theme).unwrap();
        assert!(
            !first_frame.glyph_uploads.is_empty(),
            "cold planner must upload glyphs",
        );
        assert!(first.glyph_cache.is_dirty());
        let wrote = first.persist_to_disk(&path).expect("persist");
        assert!(wrote, "dirty planner must persist a snapshot");
        // Persist clears the dirty flag.
        let wrote_again = first.persist_to_disk(&path).expect("persist clean");
        assert!(!wrote_again, "clean planner must not re-write");

        // Session 2: warm from the same file, paint the same string,
        // and assert no fresh uploads come out.
        let mut second = FramePlanner::new(256, 256).unwrap();
        let warmed = second.warm_from_disk(&path).expect("warm");
        assert!(warmed.is_some(), "snapshot must rehydrate");
        let second_frame = second.build(&painter, &theme).unwrap();
        assert!(
            second_frame.glyph_uploads.is_empty(),
            "warmed planner must skip the per-glyph upload",
        );
        assert_eq!(second_frame.text_runs.len(), 1);
    }

    /// M6.4: warming from a missing file is a no-op success, not an error.
    #[test]
    fn warm_from_disk_returns_none_for_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.bin");
        let mut planner = FramePlanner::new(64, 64).unwrap();
        let warmed = planner.warm_from_disk(&path).expect("warm");
        assert!(warmed.is_none());
    }
}
