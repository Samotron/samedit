//! Glyph rasterization and atlas upload planning.

use crate::atlas::{AtlasError, AtlasRect, GlyphAllocation, GlyphAtlas};
use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent};
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

/// Raster cache backed by `cosmic-text`'s swash cache and the renderer atlas.
pub struct GlyphRasterCache {
    atlas: GlyphAtlas,
    swash_cache: SwashCache,
    glyphs: HashMap<CacheKey, CachedGlyph>,
}

impl GlyphRasterCache {
    /// Create a raster cache with a new atlas.
    pub fn new(width: i32, height: i32) -> Result<Self, GlyphRasterError> {
        Ok(Self {
            atlas: GlyphAtlas::new(width, height)?,
            swash_cache: SwashCache::new(),
            glyphs: HashMap::new(),
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

    /// True when the glyph already has an atlas allocation.
    pub fn contains(&self, cache_key: CacheKey) -> bool {
        self.glyphs.contains_key(&cache_key)
    }

    /// Retrieve an already cached glyph.
    pub fn cached(&self, cache_key: CacheKey) -> Option<&CachedGlyph> {
        self.glyphs.get(&cache_key)
    }

    /// Ensure a glyph is rasterized and packed. Returns an upload only when the
    /// atlas texture needs new pixels.
    pub fn ensure_glyph(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Result<GlyphCacheUpdate, GlyphRasterError> {
        if let Some(glyph) = self.glyphs.get(&cache_key) {
            return Ok(GlyphCacheUpdate::Cached(glyph.clone()));
        }

        let Some(image) = self.swash_cache.get_image(font_system, cache_key) else {
            return Ok(GlyphCacheUpdate::Missing);
        };

        let width =
            i32::try_from(image.placement.width).map_err(|_| GlyphRasterError::InvalidImageSize)?;
        let height = i32::try_from(image.placement.height)
            .map_err(|_| GlyphRasterError::InvalidImageSize)?;
        if width <= 0 || height <= 0 {
            return Ok(GlyphCacheUpdate::Missing);
        }

        let allocation = self.atlas.allocate(width, height)?;
        let rect = allocation.rect();
        let pixels = rgba_pixels(image.content, &image.data);
        let glyph = CachedGlyph {
            cache_key,
            allocation,
            rect,
            left: image.placement.left,
            top: image.placement.top,
        };
        self.glyphs.insert(cache_key, glyph.clone());

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
}
