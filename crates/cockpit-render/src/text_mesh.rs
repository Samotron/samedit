//! Textured mesh generation for prepared glyphs.

use crate::frame::{PreparedGlyph, PreparedTextRun};
use thiserror::Error;

/// Per-vertex data for textured glyph quads.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexturedVertex {
    pub position: [f32; 2],
    pub tex_coord: [f32; 2],
    pub color: [f32; 4],
}

/// Indexed mesh for glyph atlas rendering.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TexturedMesh {
    pub vertices: Vec<TexturedVertex>,
    pub indices: Vec<u32>,
}

impl TexturedMesh {
    /// Create an empty mesh.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when this mesh contains no triangles.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append one prepared glyph as a textured quad.
    pub fn push_glyph(
        &mut self,
        glyph: &PreparedGlyph,
        atlas_width: i32,
        atlas_height: i32,
    ) -> Result<(), TextMeshError> {
        if atlas_width <= 0 || atlas_height <= 0 {
            return Err(TextMeshError::InvalidAtlasSize {
                width: atlas_width,
                height: atlas_height,
            });
        }

        let rect = glyph.cached.rect;
        if rect.width <= 0 || rect.height <= 0 {
            return Ok(());
        }

        let base =
            u32::try_from(self.vertices.len()).map_err(|_| TextMeshError::TooManyVertices)?;
        if base > u32::MAX - 4 {
            return Err(TextMeshError::TooManyVertices);
        }

        let left = (glyph.placement.physical_x + glyph.cached.left) as f32;
        let top = (glyph.placement.physical_y - glyph.cached.top) as f32;
        let right = left + rect.width as f32;
        let bottom = top + rect.height as f32;

        let u0 = rect.x as f32 / atlas_width as f32;
        let v0 = rect.y as f32 / atlas_height as f32;
        let u1 = (rect.x + rect.width) as f32 / atlas_width as f32;
        let v1 = (rect.y + rect.height) as f32 / atlas_height as f32;
        let color = glyph.placement.color;

        self.vertices.extend_from_slice(&[
            TexturedVertex {
                position: [left, top],
                tex_coord: [u0, v0],
                color,
            },
            TexturedVertex {
                position: [right, top],
                tex_coord: [u1, v0],
                color,
            },
            TexturedVertex {
                position: [right, bottom],
                tex_coord: [u1, v1],
                color,
            },
            TexturedVertex {
                position: [left, bottom],
                tex_coord: [u0, v1],
                color,
            },
        ]);
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

        Ok(())
    }
}

/// Build a text mesh from prepared text runs.
pub fn text_mesh_from_runs(
    runs: &[PreparedTextRun],
    atlas_width: i32,
    atlas_height: i32,
) -> Result<TexturedMesh, TextMeshError> {
    if atlas_width <= 0 || atlas_height <= 0 {
        return Err(TextMeshError::InvalidAtlasSize {
            width: atlas_width,
            height: atlas_height,
        });
    }

    let mut mesh = TexturedMesh::new();

    for run in runs {
        for glyph in &run.glyphs {
            mesh.push_glyph(glyph, atlas_width, atlas_height)?;
        }
    }

    Ok(mesh)
}

/// Text mesh build error.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TextMeshError {
    #[error("invalid atlas size {width}x{height}")]
    InvalidAtlasSize { width: i32, height: i32 },
    #[error("mesh contains too many vertices for u32 indices")]
    TooManyVertices,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FramePlanner;
    use crate::painter::Painter;
    use crate::theme::Theme;

    #[test]
    fn builds_textured_quads_for_prepared_text() {
        let theme = Theme::default();
        let mut painter = Painter::new();
        painter.text(10.0, 20.0, "Hi", theme.text, 14.0);

        let mut planner = FramePlanner::new(256, 256).unwrap();
        let frame = planner.build(&painter, &theme).unwrap();
        let mesh = text_mesh_from_runs(&frame.text_runs, 256, 256).unwrap();

        assert!(!mesh.is_empty());
        assert_eq!(mesh.vertices.len() % 4, 0);
        assert_eq!(mesh.indices.len() % 6, 0);
        assert!(mesh.vertices.iter().all(|vertex| {
            (0.0..=1.0).contains(&vertex.tex_coord[0]) && (0.0..=1.0).contains(&vertex.tex_coord[1])
        }));
    }

    #[test]
    fn rejects_invalid_atlas_size() {
        assert_eq!(
            text_mesh_from_runs(&[], 0, 256).unwrap_err(),
            TextMeshError::InvalidAtlasSize {
                width: 0,
                height: 256
            }
        );
    }
}
