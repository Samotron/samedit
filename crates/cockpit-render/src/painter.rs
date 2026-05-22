//! Immediate-mode painter command buffer.

use crate::theme::Color;

/// Pixel rectangle in logical coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// Create a rectangle.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// True when this rectangle has no drawable area.
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// A shaped-text request. Glyph placement is resolved by the text renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub color: Color,
    pub font_size: f32,
}

/// One immediate draw command.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Rect { rect: Rect, color: Color },
    Text(TextRun),
}

/// A batch of same-color rectangles.
#[derive(Debug, Clone, PartialEq)]
pub struct RectBatch {
    pub color: Color,
    pub rects: Vec<Rect>,
}

/// Immediate-mode painter. Callers rebuild this every frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Painter {
    commands: Vec<DrawCommand>,
}

impl Painter {
    /// Create an empty painter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a rectangle draw.
    pub fn rect(&mut self, rect: Rect, color: Color) {
        if !rect.is_empty() {
            self.commands.push(DrawCommand::Rect { rect, color });
        }
    }

    /// Queue a text draw.
    pub fn text(&mut self, x: f32, y: f32, text: impl Into<String>, color: Color, font_size: f32) {
        let text = text.into();
        if !text.is_empty() && font_size > 0.0 {
            self.commands.push(DrawCommand::Text(TextRun {
                x,
                y,
                text,
                color,
                font_size,
            }));
        }
    }

    /// Draw commands in submission order.
    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// Remove all queued commands.
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    /// Build same-color rectangle batches while preserving first-use color
    /// order. Text commands are not included in rectangle batches.
    pub fn rect_batches(&self) -> Vec<RectBatch> {
        let mut batches: Vec<RectBatch> = Vec::new();

        for command in &self.commands {
            let DrawCommand::Rect { rect, color } = command else {
                continue;
            };
            match batches.iter_mut().find(|batch| batch.color == *color) {
                Some(batch) => batch.rects.push(*rect),
                None => batches.push(RectBatch {
                    color: *color,
                    rects: vec![*rect],
                }),
            }
        }

        batches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_empty_rects_and_text() {
        let mut painter = Painter::new();
        painter.rect(Rect::new(0.0, 0.0, 0.0, 10.0), Color::rgb(1.0, 0.0, 0.0));
        painter.text(0.0, 0.0, "", Color::rgb(1.0, 1.0, 1.0), 13.0);

        assert!(painter.commands().is_empty());
    }

    #[test]
    fn preserves_command_order() {
        let mut painter = Painter::new();
        let red = Color::rgb(1.0, 0.0, 0.0);
        let white = Color::rgb(1.0, 1.0, 1.0);

        painter.rect(Rect::new(0.0, 0.0, 10.0, 10.0), red);
        painter.text(2.0, 8.0, "hi", white, 13.0);

        assert_eq!(painter.commands().len(), 2);
        assert!(matches!(painter.commands()[0], DrawCommand::Rect { .. }));
        assert!(matches!(painter.commands()[1], DrawCommand::Text(_)));
    }

    #[test]
    fn batches_rectangles_by_color() {
        let mut painter = Painter::new();
        let red = Color::rgb(1.0, 0.0, 0.0);
        let blue = Color::rgb(0.0, 0.0, 1.0);

        painter.rect(Rect::new(0.0, 0.0, 10.0, 10.0), red);
        painter.rect(Rect::new(10.0, 0.0, 10.0, 10.0), blue);
        painter.rect(Rect::new(20.0, 0.0, 10.0, 10.0), red);

        let batches = painter.rect_batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].color, red);
        assert_eq!(batches[0].rects.len(), 2);
        assert_eq!(batches[1].color, blue);
        assert_eq!(batches[1].rects.len(), 1);
    }
}
