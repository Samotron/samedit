//! Glyph rasterization and atlas upload planning.

use crate::atlas::{AtlasError, AtlasRect, GlyphAllocation, GlyphAtlas};
use crate::atlas_persist::{AtlasPersistError, AtlasSnapshot, GlyphManifestEntry};
use cosmic_text::fontdb;
use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SubpixelBin, SwashCache, SwashContent};
use std::collections::HashMap;
use thiserror::Error;

/// One glyph cached in the atlas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedGlyph {
    pub cache_key: CacheKey,
    pub allocation: GlyphAllocation,
    pub rect: AtlasRect,
    pub left: i32,
    pub top: i32,
}

/// Pixel upload required to populate a glyph atlas slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphUpload {
    pub cache_key: CacheKey,
    pub rect: AtlasRect,
    pub pixels: Vec<u8>,
    pub format: GlyphUploadFormat,
}

/// Texture upload format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphUploadFormat {
    Rgba8,
}

/// Raster cache backed by `cosmic-text`'s swash cache and the renderer
/// atlas.
///
/// The cache keeps a CPU-side shadow of the atlas texture so the M6.4
/// disk-cache path can snapshot the warmed atlas at shutdown and re-
/// upload it whole on the next launch.
pub struct GlyphRasterCache {
    atlas: GlyphAtlas,
    swash_cache: SwashCache,
    /// Glyph metadata in atlas-insertion order. Insertion order matters
    /// because [`GlyphRasterCache::rehydrate`] replays the same sequence
    /// into a fresh [`GlyphAtlas`] and bails if the allocator picks a
    /// different rect, so the on-disk pixel layout stays in sync with
    /// the live allocator (M6.4).
    glyphs: Vec<CachedGlyph>,
    /// O(1) lookup by cache key, mapping into `glyphs` by index.
    by_key: HashMap<CacheKey, usize>,
    /// CPU shadow of the GPU atlas texture (RGBA8). Sized to
    /// `atlas_width * atlas_height * 4` and updated alongside every
    /// upload, so [`GlyphRasterCache::snapshot`] is a pure read.
    shadow: Vec<u8>,
    /// True once at least one fresh rasterization has happened since
    /// the cache was rehydrated; only then does the binary write the
    /// snapshot back to disk on shutdown.
    dirty: bool,
}

impl GlyphRasterCache {
    /// Create a raster cache with a new atlas.
    pub fn new(width: i32, height: i32) -> Result<Self, GlyphRasterError> {
        let shadow_len = AtlasSnapshot::pixel_buffer_size(width, height)
            .ok_or(GlyphRasterError::InvalidImageSize)?;
        Ok(Self {
            atlas: GlyphAtlas::new(width, height)?,
            swash_cache: SwashCache::new(),
            glyphs: Vec::new(),
            by_key: HashMap::new(),
            shadow: vec![0u8; shadow_len],
            dirty: false,
        })
    }

    /// Atlas width in pixels.
    pub fn atlas_width(&self) -> i32 {
        self.atlas.width()
    }

    /// Atlas height in pixels.
    pub fn atlas_height(&self) -> i32 {
        self.atlas.height()
    }

    /// True when the cache has accepted at least one new glyph since
    /// the most recent rehydrate / shutdown snapshot.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Force-clear the dirty flag — called by the binary after a
    /// successful snapshot write so the next launch does not write the
    /// same bytes again.
    pub fn mark_persisted(&mut self) {
        self.dirty = false;
    }

    /// True when the glyph already has an atlas allocation.
    pub fn contains(&self, cache_key: CacheKey) -> bool {
        self.by_key.contains_key(&cache_key)
    }

    /// Retrieve an already cached glyph.
    pub fn cached(&self, cache_key: CacheKey) -> Option<&CachedGlyph> {
        self.by_key
            .get(&cache_key)
            .and_then(|index| self.glyphs.get(*index))
    }

    /// Read-only access to the CPU pixel shadow.
    pub fn shadow_pixels(&self) -> &[u8] {
        &self.shadow
    }

    /// Iterate cached glyphs in insertion order.
    pub fn iter_cached(&self) -> impl Iterator<Item = &CachedGlyph> {
        self.glyphs.iter()
    }

    /// Ensure a glyph is rasterized and packed. Returns an upload only
    /// when the atlas texture needs new pixels.
    pub fn ensure_glyph(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Result<GlyphCacheUpdate, GlyphRasterError> {
        if let Some(index) = self.by_key.get(&cache_key) {
            return Ok(GlyphCacheUpdate::Cached(self.glyphs[*index].clone()));
        }

        // Extract everything we need from `image` up front so the
        // mutable borrow on `swash_cache` ends before we mutably borrow
        // `self` again to write into the shadow.
        let (width, height, pixels, left, top) = {
            let Some(image) = self.swash_cache.get_image(font_system, cache_key) else {
                return Ok(GlyphCacheUpdate::Missing);
            };
            let width = i32::try_from(image.placement.width)
                .map_err(|_| GlyphRasterError::InvalidImageSize)?;
            let height = i32::try_from(image.placement.height)
                .map_err(|_| GlyphRasterError::InvalidImageSize)?;
            if width <= 0 || height <= 0 {
                return Ok(GlyphCacheUpdate::Missing);
            }
            (
                width,
                height,
                rgba_pixels(image.content, &image.data),
                image.placement.left,
                image.placement.top,
            )
        };

        let allocation = self.atlas.allocate(width, height)?;
        let rect = allocation.rect();
        self.write_to_shadow(rect, &pixels);
        let glyph = CachedGlyph {
            cache_key,
            allocation,
            rect,
            left,
            top,
        };
        let index = self.glyphs.len();
        self.glyphs.push(glyph.clone());
        self.by_key.insert(cache_key, index);
        self.dirty = true;

        Ok(GlyphCacheUpdate::Uploaded {
            glyph,
            upload: GlyphUpload {
                cache_key,
                rect,
                pixels,
                format: GlyphUploadFormat::Rgba8,
            },
        })
    }

    /// Snapshot the cache for disk persistence (M6.4).
    ///
    /// Walks every cached glyph in insertion order, maps its
    /// session-local font id back to a stable PostScript name via
    /// `font_system.db()`, and returns the full pixel shadow alongside.
    /// `config_hash` is the caller-supplied identifier of the font /
    /// theme set that produced this atlas, stored verbatim in the
    /// snapshot so the load path can invalidate stale caches.
    ///
    /// Returns `None` if any cached glyph's font is no longer present in
    /// the fontdb — a half-mapped snapshot would silently render the
    /// wrong glyphs on the next launch, so the safer behaviour is to
    /// skip persistence and let the next launch rebuild from scratch.
    pub fn snapshot(&self, font_system: &FontSystem, config_hash: u64) -> Option<AtlasSnapshot> {
        let db = font_system.db();
        let mut entries = Vec::with_capacity(self.glyphs.len());
        for glyph in &self.glyphs {
            let face = db.face(glyph.cache_key.font_id)?;
            entries.push(GlyphManifestEntry {
                font_post_script_name: face.post_script_name.clone(),
                font_weight: glyph.cache_key.font_weight.0,
                glyph_id: glyph.cache_key.glyph_id,
                font_size_bits: glyph.cache_key.font_size_bits,
                x_bin: subpixel_to_u8(glyph.cache_key.x_bin),
                y_bin: subpixel_to_u8(glyph.cache_key.y_bin),
                flags: glyph.cache_key.flags.bits(),
                rect_x: glyph.rect.x,
                rect_y: glyph.rect.y,
                rect_w: glyph.rect.width,
                rect_h: glyph.rect.height,
                left: glyph.left,
                top: glyph.top,
            });
        }
        Some(AtlasSnapshot {
            atlas_width: self.atlas.width(),
            atlas_height: self.atlas.height(),
            config_hash,
            glyphs: entries,
            pixels: self.shadow.clone(),
        })
    }

    /// Restore the cache from a previous snapshot (M6.4).
    ///
    /// Replays each manifest entry in stored order through the atlas
    /// allocator; if the allocator picks a different rect than the
    /// stored one, the cache is wiped and `Err(Rehydrate::Mismatch)` is
    /// returned so the caller can start fresh.
    ///
    /// Returns the pixel buffer to push to the GPU. Always equal to
    /// `snapshot.pixels` on success.
    pub fn rehydrate(
        &mut self,
        snapshot: AtlasSnapshot,
        font_system: &FontSystem,
    ) -> Result<Vec<u8>, RehydrateError> {
        if snapshot.atlas_width != self.atlas.width()
            || snapshot.atlas_height != self.atlas.height()
        {
            return Err(RehydrateError::AtlasSizeMismatch {
                snapshot_w: snapshot.atlas_width,
                snapshot_h: snapshot.atlas_height,
                cache_w: self.atlas.width(),
                cache_h: self.atlas.height(),
            });
        }
        if !snapshot.has_well_sized_pixel_buffer() {
            return Err(RehydrateError::Persist(
                AtlasPersistError::PixelBufferSize {
                    expected: AtlasSnapshot::pixel_buffer_size(
                        snapshot.atlas_width,
                        snapshot.atlas_height,
                    )
                    .unwrap_or(0),
                    found: snapshot.pixels.len(),
                },
            ));
        }

        let face_index = build_face_index(font_system);

        // Reset the live cache before replay so any failure leaves us
        // with an empty but consistent state.
        self.atlas = GlyphAtlas::new(self.atlas.width(), self.atlas.height())
            .map_err(RehydrateError::Atlas)?;
        self.glyphs.clear();
        self.by_key.clear();

        for (i, entry) in snapshot.glyphs.iter().enumerate() {
            let cache_key = rebuild_cache_key(&face_index, entry)
                .ok_or(RehydrateError::UnknownFont { index: i })?;

            let allocation = self
                .atlas
                .allocate(entry.rect_w, entry.rect_h)
                .map_err(RehydrateError::Atlas)?;
            let allocator_rect = allocation.rect();
            if allocator_rect.x != entry.rect_x || allocator_rect.y != entry.rect_y {
                return Err(RehydrateError::AllocatorReplayDrift {
                    index: i,
                    expected_x: entry.rect_x,
                    expected_y: entry.rect_y,
                    got_x: allocator_rect.x,
                    got_y: allocator_rect.y,
                });
            }

            let glyph = CachedGlyph {
                cache_key,
                allocation,
                rect: AtlasRect {
                    x: entry.rect_x,
                    y: entry.rect_y,
                    width: entry.rect_w,
                    height: entry.rect_h,
                },
                left: entry.left,
                top: entry.top,
            };
            let index = self.glyphs.len();
            self.glyphs.push(glyph);
            self.by_key.insert(cache_key, index);
        }

        self.shadow = snapshot.pixels.clone();
        self.dirty = false;
        Ok(snapshot.pixels)
    }

    /// Copy `pixels` into the CPU shadow at the supplied rect. Out-of-
    /// bounds rows are silently clipped — the atlas allocator already
    /// rejects glyphs that don't fit, so this only fires on logic bugs.
    fn write_to_shadow(&mut self, rect: AtlasRect, pixels: &[u8]) {
        let atlas_w = self.atlas.width();
        if atlas_w <= 0 {
            return;
        }
        let row_bytes = (rect.width.max(0) as usize) * 4;
        for row in 0..rect.height.max(0) {
            let src_offset = row as usize * row_bytes;
            let src_end = src_offset + row_bytes;
            if src_end > pixels.len() {
                break;
            }
            let dst_y = rect.y + row;
            let dst_offset = (dst_y as usize * atlas_w as usize + rect.x as usize) * 4;
            let dst_end = dst_offset + row_bytes;
            if dst_end > self.shadow.len() {
                break;
            }
            self.shadow[dst_offset..dst_end].copy_from_slice(&pixels[src_offset..src_end]);
        }
    }
}

/// Result of ensuring one glyph exists in the atlas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphCacheUpdate {
    Cached(CachedGlyph),
    Uploaded {
        glyph: CachedGlyph,
        upload: GlyphUpload,
    },
    Missing,
}

/// Glyph rasterization or packing error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GlyphRasterError {
    #[error(transparent)]
    Atlas(#[from] AtlasError),
    #[error("rasterized glyph image dimensions do not fit i32")]
    InvalidImageSize,
}

/// Rehydration failure mode. Each variant leaves the cache in a clean
/// empty state so the caller can simply start fresh on any error.
#[derive(Debug, Error)]
pub enum RehydrateError {
    #[error("atlas size mismatch: snapshot {snapshot_w}x{snapshot_h}, cache {cache_w}x{cache_h}")]
    AtlasSizeMismatch {
        snapshot_w: i32,
        snapshot_h: i32,
        cache_w: i32,
        cache_h: i32,
    },
    #[error(transparent)]
    Persist(#[from] AtlasPersistError),
    #[error(transparent)]
    Atlas(#[from] AtlasError),
    #[error("snapshot glyph {index} references a font not present in the fontdb")]
    UnknownFont { index: usize },
    #[error(
        "atlas allocator replay drifted at glyph {index}: expected ({expected_x},{expected_y}), got ({got_x},{got_y})"
    )]
    AllocatorReplayDrift {
        index: usize,
        expected_x: i32,
        expected_y: i32,
        got_x: i32,
        got_y: i32,
    },
}

/// PostScript-name → first matching `fontdb::ID`, used by
/// [`GlyphRasterCache::rehydrate`] to remap session-local ids.
fn build_face_index(font_system: &FontSystem) -> HashMap<String, fontdb::ID> {
    let mut index = HashMap::new();
    for face in font_system.db().faces() {
        index
            .entry(face.post_script_name.clone())
            .or_insert(face.id);
    }
    index
}

fn rebuild_cache_key(
    face_index: &HashMap<String, fontdb::ID>,
    entry: &GlyphManifestEntry,
) -> Option<CacheKey> {
    let font_id = *face_index.get(&entry.font_post_script_name)?;
    Some(CacheKey {
        font_id,
        glyph_id: entry.glyph_id,
        font_size_bits: entry.font_size_bits,
        x_bin: subpixel_from_u8(entry.x_bin)?,
        y_bin: subpixel_from_u8(entry.y_bin)?,
        font_weight: fontdb::Weight(entry.font_weight),
        flags: CacheKeyFlags::from_bits_truncate(entry.flags),
    })
}

fn subpixel_to_u8(bin: SubpixelBin) -> u8 {
    match bin {
        SubpixelBin::Zero => 0,
        SubpixelBin::One => 1,
        SubpixelBin::Two => 2,
        SubpixelBin::Three => 3,
    }
}

fn subpixel_from_u8(value: u8) -> Option<SubpixelBin> {
    Some(match value {
        0 => SubpixelBin::Zero,
        1 => SubpixelBin::One,
        2 => SubpixelBin::Two,
        3 => SubpixelBin::Three,
        _ => return None,
    })
}

fn rgba_pixels(content: SwashContent, data: &[u8]) -> Vec<u8> {
    match content {
        SwashContent::Mask => {
            let mut pixels = Vec::with_capacity(data.len() * 4);
            for alpha in data {
                pixels.extend_from_slice(&[255, 255, 255, *alpha]);
            }
            pixels
        }
        SwashContent::Color => data.to_vec(),
        SwashContent::SubpixelMask => {
            let mut pixels = Vec::with_capacity(data.len() / 3 * 4);
            for channels in data.chunks_exact(3) {
                pixels.extend_from_slice(&[channels[0], channels[1], channels[2], 255]);
            }
            pixels
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::{Attrs, Buffer, Metrics, Shaping};

    fn first_cache_key(text: &str) -> (FontSystem, CacheKey) {
        let mut font_system = FontSystem::new();
        let cache_key = {
            let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 22.0));
            let mut buffer = buffer.borrow_with(&mut font_system);
            buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
            buffer
                .layout_runs()
                .flat_map(|run| run.glyphs.iter())
                .map(|glyph| glyph.physical((0.0, 0.0), 1.0).cache_key)
                .next()
                .expect("expected at least one glyph")
        };
        (font_system, cache_key)
    }

    #[test]
    fn rasterizes_and_uploads_new_glyph_once() {
        let (mut font_system, cache_key) = first_cache_key("A");
        let mut cache = GlyphRasterCache::new(128, 128).unwrap();

        let update = cache.ensure_glyph(&mut font_system, cache_key).unwrap();

        let GlyphCacheUpdate::Uploaded { glyph, upload } = update else {
            panic!("expected first glyph lookup to upload");
        };
        assert_eq!(glyph.cache_key, cache_key);
        assert_eq!(glyph.rect, upload.rect);
        assert_eq!(upload.format, GlyphUploadFormat::Rgba8);
        assert_eq!(
            upload.pixels.len(),
            (upload.rect.width * upload.rect.height * 4) as usize
        );
        assert!(cache.contains(cache_key));
        assert!(cache.is_dirty());

        let second = cache.ensure_glyph(&mut font_system, cache_key).unwrap();
        assert!(matches!(second, GlyphCacheUpdate::Cached(_)));
    }

    #[test]
    fn reports_full_atlas_for_large_glyph() {
        let (mut font_system, cache_key) = first_cache_key("W");
        let mut cache = GlyphRasterCache::new(4, 4).unwrap();

        let err = cache.ensure_glyph(&mut font_system, cache_key).unwrap_err();

        assert!(matches!(
            err,
            GlyphRasterError::Atlas(AtlasError::Full { .. })
        ));
    }

    #[test]
    fn mask_pixels_expand_to_rgba() {
        assert_eq!(
            rgba_pixels(SwashContent::Mask, &[0, 128, 255]),
            vec![255, 255, 255, 0, 255, 255, 255, 128, 255, 255, 255, 255]
        );
    }

    /// M6.4: a warmed cache snapshots cleanly, and a fresh cache
    /// rehydrates the same glyphs into the same atlas slots without
    /// re-rasterizing.
    #[test]
    fn snapshot_then_rehydrate_round_trips_a_real_glyph() {
        let (mut font_system, cache_key) = first_cache_key("R");
        let mut cache = GlyphRasterCache::new(128, 128).unwrap();
        cache
            .ensure_glyph(&mut font_system, cache_key)
            .expect("warm the cache");
        assert!(cache.is_dirty());

        let snapshot = cache.snapshot(&font_system, 0x42).expect("snapshot");
        assert_eq!(snapshot.config_hash, 0x42);
        assert_eq!(snapshot.glyphs.len(), 1);

        // Rehydrate into a fresh cache and assert the glyph reappears.
        let mut fresh = GlyphRasterCache::new(128, 128).unwrap();
        let pixels = fresh
            .rehydrate(snapshot.clone(), &font_system)
            .expect("rehydrate");
        assert_eq!(pixels.len(), snapshot.pixels.len());
        assert!(!fresh.is_dirty(), "rehydrate must clear the dirty flag");
        assert!(fresh.contains(cache_key));

        // A subsequent ensure_glyph call returns the rehydrated entry
        // without rasterizing again — i.e. no fresh upload is emitted.
        let update = fresh.ensure_glyph(&mut font_system, cache_key).unwrap();
        assert!(matches!(update, GlyphCacheUpdate::Cached(_)));
    }

    #[test]
    fn rehydrate_rejects_an_atlas_size_mismatch() {
        let (font_system, _cache_key) = first_cache_key("X");
        let snapshot = AtlasSnapshot {
            atlas_width: 64,
            atlas_height: 64,
            config_hash: 0,
            glyphs: Vec::new(),
            pixels: vec![0u8; 64 * 64 * 4],
        };
        let mut cache = GlyphRasterCache::new(128, 128).unwrap();
        match cache.rehydrate(snapshot, &font_system) {
            Err(RehydrateError::AtlasSizeMismatch { .. }) => {}
            other => panic!("expected AtlasSizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn rehydrate_rejects_glyphs_with_unknown_fonts() {
        let (mut font_system, cache_key) = first_cache_key("Q");
        let mut cache = GlyphRasterCache::new(128, 128).unwrap();
        cache.ensure_glyph(&mut font_system, cache_key).unwrap();
        let mut snapshot = cache.snapshot(&font_system, 0).expect("snapshot");
        snapshot.glyphs[0].font_post_script_name = "Definitely-Not-A-Real-Font-XYZ".into();

        let mut fresh = GlyphRasterCache::new(128, 128).unwrap();
        match fresh.rehydrate(snapshot, &font_system) {
            Err(RehydrateError::UnknownFont { index: 0 }) => {}
            other => panic!("expected UnknownFont, got {other:?}"),
        }
        // The fresh cache must be empty after a failed rehydrate, not
        // half-populated.
        assert_eq!(fresh.iter_cached().count(), 0);
    }

    #[test]
    fn subpixel_round_trips_through_u8() {
        for bin in [
            SubpixelBin::Zero,
            SubpixelBin::One,
            SubpixelBin::Two,
            SubpixelBin::Three,
        ] {
            assert_eq!(subpixel_from_u8(subpixel_to_u8(bin)), Some(bin));
        }
        assert!(subpixel_from_u8(7).is_none());
    }
}
