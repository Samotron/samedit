//! GPU-ready geometry builders for painter commands.

use crate::painter::{DrawCommand, Rect, RectBatch};
use crate::theme::Color;
use thiserror::Error;

/// Per-vertex data consumed by the renderer backend.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 2],
    pub tex_coord: [f32; 2],
    pub color: [f32; 4],
}

impl Vertex {
    /// Create a vertex.
    pub const fn new(position: [f32; 2], tex_coord: [f32; 2], color: Color) -> Self {
        Self {
            position,
            tex_coord,
            color: color.to_array(),
        }
    }
}

/// Indexed triangle mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Create an empty mesh.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when this mesh contains no triangles.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append a solid rectangle as two triangles.
    pub fn push_solid_rect(&mut self, rect: Rect, color: Color) -> Result<(), GeometryError> {
        if rect.is_empty() {
            return Ok(());
        }

        let base =
            u32::try_from(self.vertices.len()).map_err(|_| GeometryError::TooManyVertices)?;
        if base > u32::MAX - 4 {
            return Err(GeometryError::TooManyVertices);
        }

        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        let tex_coord = [0.0, 0.0];

        self.vertices.extend_from_slice(&[
            Vertex::new([rect.x, rect.y], tex_coord, color),
            Vertex::new([right, rect.y], tex_coord, color),
            Vertex::new([right, bottom], tex_coord, color),
            Vertex::new([rect.x, bottom], tex_coord, color),
        ]);
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

        Ok(())
    }
}

/// Build rectangle geometry from draw commands, preserving submission order.
pub fn rect_mesh_from_commands(commands: &[DrawCommand]) -> Result<Mesh, GeometryError> {
    let mut mesh = Mesh::new();

    for command in commands {
        if let DrawCommand::Rect { rect, color } = command {
            mesh.push_solid_rect(*rect, *color)?;
        }
    }

    Ok(mesh)
}

/// Build rectangle geometry from same-color rectangle batches.
pub fn rect_mesh_from_batches(batches: &[RectBatch]) -> Result<Mesh, GeometryError> {
    let mut mesh = Mesh::new();

    for batch in batches {
        for rect in &batch.rects {
            mesh.push_solid_rect(*rect, batch.color)?;
        }
    }

    Ok(mesh)
}

/// Geometry build error.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    #[error("mesh contains too many vertices for u32 indices")]
    TooManyVertices,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::Painter;

    #[test]
    fn builds_two_triangles_for_a_rect() {
        let color = Color::rgb(1.0, 0.5, 0.25);
        let mut mesh = Mesh::new();

        mesh.push_solid_rect(Rect::new(2.0, 3.0, 10.0, 20.0), color)
            .unwrap();

        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(mesh.vertices[0], Vertex::new([2.0, 3.0], [0.0, 0.0], color));
        assert_eq!(
            mesh.vertices[2],
            Vertex::new([12.0, 23.0], [0.0, 0.0], color)
        );
    }

    #[test]
    fn command_mesh_preserves_rect_submission_order_and_skips_text() {
        let mut painter = Painter::new();
        let red = Color::rgb(1.0, 0.0, 0.0);
        let blue = Color::rgb(0.0, 0.0, 1.0);

        painter.rect(Rect::new(0.0, 0.0, 1.0, 1.0), red);
        painter.text(10.0, 10.0, "label", red, 13.0);
        painter.rect(Rect::new(5.0, 0.0, 2.0, 2.0), blue);

        let mesh = rect_mesh_from_commands(painter.commands()).unwrap();

        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.vertices[0].color, red.to_array());
        assert_eq!(mesh.vertices[4].color, blue.to_array());
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn batch_mesh_uses_batch_color() {
        let red = Color::rgb(1.0, 0.0, 0.0);
        let batches = [RectBatch {
            color: red,
            rects: vec![Rect::new(0.0, 0.0, 1.0, 1.0), Rect::new(1.0, 0.0, 1.0, 1.0)],
        }];

        let mesh = rect_mesh_from_batches(&batches).unwrap();

        assert_eq!(mesh.vertices.len(), 8);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.color == red.to_array())
        );
    }

    #[test]
    fn skips_empty_rects() {
        let mut mesh = Mesh::new();

        mesh.push_solid_rect(Rect::new(0.0, 0.0, 0.0, 5.0), Color::rgb(1.0, 1.0, 1.0))
            .unwrap();

        assert!(mesh.is_empty());
    }
}
