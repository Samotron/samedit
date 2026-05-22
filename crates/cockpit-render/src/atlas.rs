//! Glyph atlas allocation.

use etagere::{AllocId, AtlasAllocator, size2};
use std::fmt;
use thiserror::Error;

/// Integer atlas rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// One allocated glyph slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphAllocation {
    id: AllocId,
    rect: AtlasRect,
}

impl GlyphAllocation {
    /// Pixel rectangle occupied by this allocation, excluding atlas padding.
    pub fn rect(self) -> AtlasRect {
        self.rect
    }
}

/// Dynamic glyph atlas allocator.
pub struct GlyphAtlas {
    allocator: AtlasAllocator,
    width: i32,
    height: i32,
    padding: i32,
}

impl fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("padding", &self.padding)
            .field("allocated_space", &self.allocated_space())
            .finish()
    }
}

impl GlyphAtlas {
    /// Create an atlas with one-pixel glyph padding.
    pub fn new(width: i32, height: i32) -> Result<Self, AtlasError> {
        Self::with_padding(width, height, 1)
    }

    /// Create an atlas with explicit glyph padding.
    pub fn with_padding(width: i32, height: i32, padding: i32) -> Result<Self, AtlasError> {
        if width <= 0 || height <= 0 {
            return Err(AtlasError::InvalidSize { width, height });
        }
        if padding < 0 {
            return Err(AtlasError::InvalidPadding(padding));
        }

        Ok(Self {
            allocator: AtlasAllocator::new(size2(width, height)),
            width,
            height,
            padding,
        })
    }

    /// Atlas width in pixels.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// Atlas height in pixels.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// Allocate a glyph slot. Returned rectangle excludes padding.
    pub fn allocate(&mut self, width: i32, height: i32) -> Result<GlyphAllocation, AtlasError> {
        if width <= 0 || height <= 0 {
            return Err(AtlasError::InvalidSize { width, height });
        }

        let padded_width = width + self.padding * 2;
        let padded_height = height + self.padding * 2;
        let allocation = self
            .allocator
            .allocate(size2(padded_width, padded_height))
            .ok_or(AtlasError::Full {
                width,
                height,
                atlas_width: self.width,
                atlas_height: self.height,
            })?;

        Ok(GlyphAllocation {
            id: allocation.id,
            rect: AtlasRect {
                x: allocation.rectangle.min.x + self.padding,
                y: allocation.rectangle.min.y + self.padding,
                width,
                height,
            },
        })
    }

    /// Free a glyph allocation.
    pub fn deallocate(&mut self, allocation: GlyphAllocation) {
        self.allocator.deallocate(allocation.id);
    }

    /// Number of pixels reserved by allocated padded rectangles.
    pub fn allocated_space(&self) -> i32 {
        self.allocator.allocated_space()
    }

    /// True when the atlas has no active allocations.
    pub fn is_empty(&self) -> bool {
        self.allocator.is_empty()
    }
}

/// Glyph atlas allocation error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AtlasError {
    #[error("invalid atlas or glyph size {width}x{height}")]
    InvalidSize { width: i32, height: i32 },
    #[error("invalid glyph padding {0}")]
    InvalidPadding(i32),
    #[error("atlas {atlas_width}x{atlas_height} is full for glyph {width}x{height}")]
    Full {
        width: i32,
        height: i32,
        atlas_width: i32,
        atlas_height: i32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_glyph_rect_with_padding() {
        let mut atlas = GlyphAtlas::with_padding(64, 64, 1).unwrap();
        let allocation = atlas.allocate(10, 12).unwrap();

        assert_eq!(
            allocation.rect(),
            AtlasRect {
                x: 1,
                y: 1,
                width: 10,
                height: 12
            }
        );
        assert!(atlas.allocated_space() >= 12 * 14);
    }

    #[test]
    fn rejects_invalid_sizes() {
        assert_eq!(
            GlyphAtlas::new(0, 64).unwrap_err(),
            AtlasError::InvalidSize {
                width: 0,
                height: 64
            }
        );
        let mut atlas = GlyphAtlas::new(64, 64).unwrap();
        assert_eq!(
            atlas.allocate(-1, 8).unwrap_err(),
            AtlasError::InvalidSize {
                width: -1,
                height: 8
            }
        );
    }

    #[test]
    fn reports_full_atlas() {
        let mut atlas = GlyphAtlas::with_padding(8, 8, 1).unwrap();

        assert_eq!(
            atlas.allocate(16, 16).unwrap_err(),
            AtlasError::Full {
                width: 16,
                height: 16,
                atlas_width: 8,
                atlas_height: 8,
            }
        );
    }

    #[test]
    fn deallocates_glyph_slots() {
        let mut atlas = GlyphAtlas::with_padding(32, 32, 1).unwrap();
        let allocation = atlas.allocate(8, 8).unwrap();
        assert!(!atlas.is_empty());

        atlas.deallocate(allocation);

        assert!(atlas.is_empty());
    }
}
