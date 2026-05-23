//! `cockpit-render` — windowing and rendering.
//!
//! The *only* crate that depends on `winit` and `glow`. Owns the window,
//! the GL context, the glyph atlas, and the immediate-mode painter that turns
//! the [`cockpit-ui`](../cockpit_ui/index.html) view-model into draw calls.

pub mod app;
pub mod atlas;
pub mod frame;
pub mod geometry;
pub mod glyph_cache;
pub mod key_event;
pub mod painter;
pub mod renderer;
pub mod text;
pub mod text_mesh;
pub mod theme;

pub use app::{
    AppError, CockpitApp, MouseButton, PointerPosition, RedrawHandle, Viewport, run_app,
};
pub use atlas::{AtlasError, AtlasRect, GlyphAllocation, GlyphAtlas};
pub use frame::{FrameError, FramePlanner, PreparedGlyph, PreparedTextRun, RenderFrame};
pub use geometry::{GeometryError, Mesh, Vertex, rect_mesh_from_batches, rect_mesh_from_commands};
pub use glyph_cache::{
    CachedGlyph, GlyphCacheUpdate, GlyphRasterCache, GlyphRasterError, GlyphUpload,
    GlyphUploadFormat,
};
pub use key_event::{KeyEvent, KeyModifiers, LogicalKey, NamedKey, event_to_chord};
pub use painter::{DrawCommand, Painter, Rect, RectBatch, TextRun};
pub use renderer::{GlRenderer, RendererError};
pub use text::{DEFAULT_LINE_HEIGHT, GlyphPlacement, ShapedText, TextLayouter};
pub use text_mesh::{TextMeshError, TexturedMesh, TexturedVertex, text_mesh_from_runs};
pub use theme::{Color, Theme};
