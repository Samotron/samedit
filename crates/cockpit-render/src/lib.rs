//! `cockpit-render` — windowing and rendering.
//!
//! The *only* crate that depends on `winit` and `glow`. Owns the window,
//! the GL context, the glyph atlas, and the immediate-mode painter that turns
//! the [`cockpit-ui`](../cockpit_ui/index.html) view-model into draw calls.

pub mod app;
pub mod atlas;
pub mod atlas_persist;
pub mod frame;
pub mod geometry;
pub mod glyph_cache;
pub mod key_event;
pub mod renderer;
pub mod text;
pub mod text_mesh;

// The immediate-mode painter and theme palette were extracted into the
// headless `cockpit-paint` crate (v0.12 M12.4) so the sibling popover binaries
// can paint without depending on `winit`/`glow`. Re-exported here under their
// original module paths (`cockpit_render::painter`, `::theme`) so existing
// callers — and this crate's own `crate::painter`/`crate::theme` uses — keep
// compiling unchanged.
pub use cockpit_paint::{painter, theme};

pub use app::{
    AppError, CockpitApp, MouseButton, PointerPosition, RedrawHandle, Viewport, run_app,
};
pub use atlas::{AtlasError, AtlasRect, GlyphAllocation, GlyphAtlas};
pub use atlas_persist::{
    ATLAS_FORMAT_VERSION, ATLAS_MAGIC, AtlasPersistError, AtlasSnapshot, GlyphManifestEntry,
};
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
