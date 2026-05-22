//! Windowing harness — the `winit` event loop and `glutin` GL bootstrap.
//!
//! This is the one genuinely non-headless module in the workspace. It owns the
//! window, the GL context, the [`GlRenderer`], and the [`FramePlanner`], and
//! drives them from a `winit` event loop. Application code never names a
//! `winit` or `glutin` type: it implements [`CockpitApp`] and hands it to
//! [`run_app`], keeping every other crate display-server-free (AGENTS §2).
//!
//! Input translation funnels through the headless [`event_to_chord`] mapping in
//! [`key_event`](crate::key_event); only the thin `winit` → [`KeyEvent`] step
//! lives here.

use std::num::NonZeroU32;
use std::rc::Rc;

use cockpit_commands::KeyChord;
use glutin::config::{Config, ConfigTemplateBuilder};
use glutin::context::ContextAttributesBuilder;
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent as WinitKeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

use crate::frame::{FrameError, FramePlanner};
use crate::key_event::{KeyEvent, KeyModifiers, LogicalKey, NamedKey, event_to_chord};
use crate::painter::Painter;
use crate::renderer::{GlRenderer, RendererError};
use crate::theme::Theme;

/// Square glyph-atlas edge length, in texels, shared by the renderer and the
/// frame planner.
const ATLAS_SIZE: i32 = 1024;

/// Viewport handed to [`CockpitApp::paint`] each frame.
///
/// Sizes are in physical pixels; `scale` is the display scale factor so the
/// application can size fonts and panes for HiDPI screens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// The application driven by the windowing harness.
///
/// Implementors hold the headless view-model and turn it into draw commands.
/// The harness owns the window and GL state and never inspects app state
/// beyond this trait.
pub trait CockpitApp {
    /// Paint the current view-model into `painter` for a frame of `viewport`.
    fn paint(&mut self, painter: &mut Painter, viewport: Viewport);

    /// Theme used to clear the frame and resolve colors.
    fn theme(&self) -> &Theme;

    /// Handle one resolved key chord (key-down only).
    fn on_key(&mut self, chord: KeyChord);

    /// Handle committed text input (insert-mode typing). Default: ignored.
    fn on_text(&mut self, _text: &str) {}

    /// Notified after the window is resized. Default: ignored.
    fn on_resize(&mut self, _viewport: Viewport) {}

    /// Receive a [`RedrawHandle`] once, before the event loop starts. Lets the
    /// app wake the loop for redraws driven by background threads (PTY output).
    /// Default: ignored.
    fn set_redraw_handle(&mut self, _handle: RedrawHandle) {}

    /// True once the application wants the event loop to exit.
    fn wants_exit(&self) -> bool {
        false
    }
}

/// A thread-safe handle the application uses to wake the event loop for a
/// redraw — e.g. from a terminal reader thread when PTY output arrives.
#[derive(Debug, Clone)]
pub struct RedrawHandle {
    proxy: EventLoopProxy<()>,
}

impl RedrawHandle {
    /// Request a redraw. A no-op once the event loop has exited.
    pub fn request(&self) {
        let _ = self.proxy.send_event(());
    }
}

/// Errors raised while bootstrapping or running the windowing harness.
#[derive(Debug, Error)]
pub enum AppError {
    /// Windowing or GL bootstrap failed (no display server, driver, …).
    #[error("windowing/GL setup failed: {0}")]
    Setup(String),
    /// The GL renderer failed.
    #[error(transparent)]
    Renderer(#[from] RendererError),
    /// Per-frame preparation failed.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// Run `app` in a window titled `title` until it exits.
///
/// Blocks until the window closes. Returns the first fatal error, if any.
pub fn run_app<A: CockpitApp>(title: impl Into<String>, mut app: A) -> Result<(), AppError> {
    let event_loop = EventLoop::<()>::with_user_event()
        .build()
        .map_err(|err| AppError::Setup(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    app.set_redraw_handle(RedrawHandle {
        proxy: event_loop.create_proxy(),
    });

    let mut harness = Harness {
        app,
        title: title.into(),
        modifiers: ModifiersState::empty(),
        gl: None,
        result: Ok(()),
    };
    event_loop
        .run_app(&mut harness)
        .map_err(|err| AppError::Setup(err.to_string()))?;
    harness.result
}

/// Live GL state — created on the first `resumed` event.
struct GlState {
    window: Window,
    surface: Surface<WindowSurface>,
    context: glutin::context::PossiblyCurrentContext,
    renderer: GlRenderer,
    planner: FramePlanner,
}

/// `winit` application handler wrapping a [`CockpitApp`].
struct Harness<A: CockpitApp> {
    app: A,
    title: String,
    modifiers: ModifiersState,
    gl: Option<GlState>,
    result: Result<(), AppError>,
}

impl<A: CockpitApp> Harness<A> {
    /// Bootstrap the window, GL context, renderer, and frame planner.
    fn create_gl(&self, event_loop: &ActiveEventLoop) -> Result<GlState, AppError> {
        let window_attributes = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let display_builder = DisplayBuilder::new().with_window_attributes(Some(window_attributes));

        let (window, gl_config) = display_builder
            .build(event_loop, ConfigTemplateBuilder::new(), pick_config)
            .map_err(|err| AppError::Setup(format!("GL config selection failed: {err}")))?;
        let window =
            window.ok_or_else(|| AppError::Setup("winit produced no window".to_string()))?;

        let raw_window_handle = window.window_handle().ok().map(|handle| handle.as_raw());
        let gl_display = gl_config.display();

        let context_attributes = ContextAttributesBuilder::new().build(raw_window_handle);
        // SAFETY: `gl_config` and the handle both come from the window created above.
        let not_current = unsafe { gl_display.create_context(&gl_config, &context_attributes) }
            .map_err(|err| AppError::Setup(format!("GL context creation failed: {err}")))?;

        let surface_attributes = window
            .build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())
            .map_err(|err| AppError::Setup(format!("window handle unavailable: {err}")))?;
        // SAFETY: the surface attributes were built from this window's handle.
        let surface = unsafe { gl_display.create_window_surface(&gl_config, &surface_attributes) }
            .map_err(|err| AppError::Setup(format!("GL surface creation failed: {err}")))?;

        let context = not_current
            .make_current(&surface)
            .map_err(|err| AppError::Setup(format!("GL make-current failed: {err}")))?;
        // Best effort: vsync is not fatal if the platform refuses it.
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));

        // SAFETY: the context was just made current on this thread.
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|symbol| gl_display.get_proc_address(symbol))
        };
        let gl = Rc::new(gl);

        // SAFETY: the GL context is current on the calling thread.
        let renderer = unsafe { GlRenderer::new(Rc::clone(&gl), ATLAS_SIZE, ATLAS_SIZE) }?;
        let planner = FramePlanner::new(ATLAS_SIZE, ATLAS_SIZE)?;

        Ok(GlState {
            window,
            surface,
            context,
            renderer,
            planner,
        })
    }

    /// Translate, then dispatch, one keyboard event to the application.
    fn handle_key(&mut self, event: &WinitKeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if let Some(key_event) = translate_key(event, self.modifiers)
            && let Some(chord) = event_to_chord(&key_event)
        {
            self.app.on_key(chord);
        }

        // Forward plain typed text (no command modifiers, no control glyphs) so
        // a future editor insert mode can consume it.
        let command_modifier =
            self.modifiers.control_key() || self.modifiers.alt_key() || self.modifiers.super_key();
        if !command_modifier
            && let Some(text) = &event.text
            && !text.is_empty()
            && !text.chars().any(char::is_control)
        {
            self.app.on_text(text);
        }
    }

    /// Build and present one frame.
    fn redraw(&mut self) -> Result<(), AppError> {
        let Some(gl) = self.gl.as_mut() else {
            return Ok(());
        };
        let size = gl.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let viewport = Viewport {
            width: size.width,
            height: size.height,
            scale: gl.window.scale_factor() as f32,
        };

        let mut painter = Painter::new();
        self.app.paint(&mut painter, viewport);
        let theme = self.app.theme().clone();

        let frame = gl.planner.build(&painter, &theme)?;
        // SAFETY: the context is current on this thread for the loop's lifetime.
        unsafe {
            gl.renderer
                .draw_frame(&frame, viewport.width, viewport.height)?
        };
        gl.surface
            .swap_buffers(&gl.context)
            .map_err(|err| AppError::Setup(format!("buffer swap failed: {err}")))?;
        Ok(())
    }
}

impl<A: CockpitApp> ApplicationHandler for Harness<A> {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        // A background thread asked for a redraw (e.g. fresh PTY output).
        if let Some(gl) = self.gl.as_ref() {
            gl.window.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl.is_some() {
            return;
        }
        match self.create_gl(event_loop) {
            Ok(gl) => {
                gl.window.request_redraw();
                self.gl = Some(gl);
            }
            Err(err) => {
                self.result = Err(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Resized(size) => {
                if let Some(gl) = self.gl.as_ref() {
                    if let (Some(width), Some(height)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        gl.surface.resize(&gl.context, width, height);
                    }
                    let viewport = Viewport {
                        width: size.width,
                        height: size.height,
                        scale: gl.window.scale_factor() as f32,
                    };
                    self.app.on_resize(viewport);
                    gl.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(gl) = self.gl.as_ref() {
                    gl.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(&event);
                if self.app.wants_exit() {
                    event_loop.exit();
                    return;
                }
                if let Some(gl) = self.gl.as_ref() {
                    gl.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.redraw() {
                    self.result = Err(err);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

/// Pick the GL config with the most MSAA samples.
///
/// `DisplayBuilder::build` requires the picker to return a `Config` directly —
/// there is no fallible path because glutin only invokes it once at least one
/// config matched the template.
fn pick_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|best, config| {
            if config.num_samples() > best.num_samples() {
                config
            } else {
                best
            }
        })
        .expect("glutin yields at least one matching GL config")
}

/// Translate a `winit` keyboard event into a headless [`KeyEvent`].
fn translate_key(event: &WinitKeyEvent, modifiers: ModifiersState) -> Option<KeyEvent> {
    let key = match &event.logical_key {
        Key::Character(text) => {
            let mut chars = text.chars();
            let first = chars.next()?;
            // A logical key is a single character; ignore multi-char commits.
            if chars.next().is_some() {
                return None;
            }
            LogicalKey::Char(first)
        }
        Key::Named(named) => LogicalKey::Named(translate_named(*named)?),
        Key::Dead(_) | Key::Unidentified(_) => return None,
    };

    Some(KeyEvent {
        key,
        modifiers: KeyModifiers {
            ctrl: modifiers.control_key(),
            alt: modifiers.alt_key(),
            shift: modifiers.shift_key(),
            meta: modifiers.super_key(),
        },
        pressed: event.state == ElementState::Pressed,
    })
}

/// Map a `winit` named key to the headless [`NamedKey`], if recognized.
fn translate_named(named: WinitNamedKey) -> Option<NamedKey> {
    Some(match named {
        WinitNamedKey::Escape => NamedKey::Escape,
        WinitNamedKey::Enter => NamedKey::Enter,
        WinitNamedKey::Tab => NamedKey::Tab,
        WinitNamedKey::Backspace => NamedKey::Backspace,
        WinitNamedKey::Delete => NamedKey::Delete,
        WinitNamedKey::Insert => NamedKey::Insert,
        WinitNamedKey::Home => NamedKey::Home,
        WinitNamedKey::End => NamedKey::End,
        WinitNamedKey::PageUp => NamedKey::PageUp,
        WinitNamedKey::PageDown => NamedKey::PageDown,
        WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
        WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
        WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
        WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
        WinitNamedKey::Space => NamedKey::Space,
        WinitNamedKey::F1 => NamedKey::F(1),
        WinitNamedKey::F2 => NamedKey::F(2),
        WinitNamedKey::F3 => NamedKey::F(3),
        WinitNamedKey::F4 => NamedKey::F(4),
        WinitNamedKey::F5 => NamedKey::F(5),
        WinitNamedKey::F6 => NamedKey::F(6),
        WinitNamedKey::F7 => NamedKey::F(7),
        WinitNamedKey::F8 => NamedKey::F(8),
        WinitNamedKey::F9 => NamedKey::F(9),
        WinitNamedKey::F10 => NamedKey::F(10),
        WinitNamedKey::F11 => NamedKey::F(11),
        WinitNamedKey::F12 => NamedKey::F(12),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_common_named_keys() {
        assert_eq!(
            translate_named(WinitNamedKey::Escape),
            Some(NamedKey::Escape)
        );
        assert_eq!(translate_named(WinitNamedKey::Enter), Some(NamedKey::Enter));
        assert_eq!(
            translate_named(WinitNamedKey::ArrowDown),
            Some(NamedKey::ArrowDown)
        );
        assert_eq!(translate_named(WinitNamedKey::Space), Some(NamedKey::Space));
    }

    #[test]
    fn translates_function_keys_to_numbered_variant() {
        assert_eq!(translate_named(WinitNamedKey::F1), Some(NamedKey::F(1)));
        assert_eq!(translate_named(WinitNamedKey::F12), Some(NamedKey::F(12)));
    }

    #[test]
    fn unmapped_named_keys_are_dropped() {
        assert_eq!(translate_named(WinitNamedKey::PrintScreen), None);
    }
}
