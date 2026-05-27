//! `cockpit-mux` — headless terminal multiplexer state.
//!
//! This crate owns the pure session/window/pane model for the native
//! multiplexer (v0.7 M7.2). It deliberately has no PTY, GPU, window, or
//! filesystem dependency: UI and terminal wiring map these stable ids to
//! real resources outside this crate.

use std::fmt;

use cockpit_commands::{CommandId, KeyChord, Modifiers};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable command ids for native mux operations.
pub mod command_ids {
    pub const NEW_WINDOW: &str = "mux.window.new";
    pub const RENAME_WINDOW: &str = "mux.window.rename";
    pub const KILL_WINDOW: &str = "mux.window.kill";
    pub const NEXT_WINDOW: &str = "mux.window.next";
    pub const PREVIOUS_WINDOW: &str = "mux.window.previous";
    pub const SELECT_WINDOW_0: &str = "mux.window.select_0";
    pub const SELECT_WINDOW_1: &str = "mux.window.select_1";
    pub const SELECT_WINDOW_2: &str = "mux.window.select_2";
    pub const SELECT_WINDOW_3: &str = "mux.window.select_3";
    pub const SELECT_WINDOW_4: &str = "mux.window.select_4";
    pub const SELECT_WINDOW_5: &str = "mux.window.select_5";
    pub const SELECT_WINDOW_6: &str = "mux.window.select_6";
    pub const SELECT_WINDOW_7: &str = "mux.window.select_7";
    pub const SELECT_WINDOW_8: &str = "mux.window.select_8";
    pub const SELECT_WINDOW_9: &str = "mux.window.select_9";
    pub const SPLIT_HORIZONTAL: &str = "mux.pane.split_horizontal";
    pub const SPLIT_VERTICAL: &str = "mux.pane.split_vertical";
    pub const KILL_PANE: &str = "mux.pane.kill";
    pub const NEXT_PANE: &str = "mux.pane.next";
    pub const LAST_PANE: &str = "mux.pane.last";
    pub const SWAP_PANE_NEXT: &str = "mux.pane.swap_next";
    pub const FOCUS_UP: &str = "mux.pane.focus_up";
    pub const FOCUS_DOWN: &str = "mux.pane.focus_down";
    pub const FOCUS_LEFT: &str = "mux.pane.focus_left";
    pub const FOCUS_RIGHT: &str = "mux.pane.focus_right";
    pub const RESIZE_UP: &str = "mux.pane.resize_up";
    pub const RESIZE_DOWN: &str = "mux.pane.resize_down";
    pub const RESIZE_LEFT: &str = "mux.pane.resize_left";
    pub const RESIZE_RIGHT: &str = "mux.pane.resize_right";
    pub const ZOOM_PANE: &str = "mux.pane.zoom";
    pub const NEXT_LAYOUT: &str = "mux.layout.next";
    pub const COPY_MODE: &str = "mux.copy_mode.enter";
    pub const PASTE: &str = "mux.paste";
    pub const DETACH: &str = "mux.session.detach";
    pub const NEW_SESSION: &str = "mux.session.new";
    pub const NEXT_SESSION: &str = "mux.session.next";
    pub const PREVIOUS_SESSION: &str = "mux.session.previous";

    pub const SELECT_WINDOW: [&str; 10] = [
        SELECT_WINDOW_0,
        SELECT_WINDOW_1,
        SELECT_WINDOW_2,
        SELECT_WINDOW_3,
        SELECT_WINDOW_4,
        SELECT_WINDOW_5,
        SELECT_WINDOW_6,
        SELECT_WINDOW_7,
        SELECT_WINDOW_8,
        SELECT_WINDOW_9,
    ];
}

/// Stable session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(u64);

impl SessionId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable window identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WindowId(u64);

impl WindowId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable pane identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneId(u64);

impl PaneId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Split direction in a window layout tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitDirection {
    /// Left/right split.
    Horizontal,
    /// Top/bottom split.
    Vertical,
}

/// Built-in layout presets matching the tmux subset in the v0.7 plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutPreset {
    EvenHorizontal,
    MainVertical,
    Tiled,
}

impl LayoutPreset {
    pub const ALL: [Self; 3] = [Self::EvenHorizontal, Self::MainVertical, Self::Tiled];

    pub fn next(self) -> Self {
        match self {
            Self::EvenHorizontal => Self::MainVertical,
            Self::MainVertical => Self::Tiled,
            Self::Tiled => Self::EvenHorizontal,
        }
    }
}

/// One mux command emitted by the prefix FSM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxCommand {
    id: CommandId,
}

impl MuxCommand {
    pub fn new(id: impl Into<CommandId>) -> Self {
        Self { id: id.into() }
    }

    pub fn id(&self) -> &CommandId {
        &self.id
    }
}

/// Logical rectangle used when projecting a mux layout into the terminal area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// One projected pane rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRect {
    pub pane: PaneId,
    pub rect: Rect,
    pub active: bool,
}

/// Whether the next key should be interpreted as terminal input or a mux
/// command after the prefix key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixState {
    Passthrough,
    AwaitCommand,
}

/// Pure tmux-style prefix dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixDispatcher {
    prefix: KeyChord,
    state: PrefixState,
}

impl Default for PrefixDispatcher {
    fn default() -> Self {
        Self::new(default_prefix())
    }
}

impl PrefixDispatcher {
    pub fn new(prefix: KeyChord) -> Self {
        Self {
            prefix,
            state: PrefixState::Passthrough,
        }
    }

    pub fn state(&self) -> PrefixState {
        self.state
    }

    pub fn prefix(&self) -> &KeyChord {
        &self.prefix
    }

    /// Feed one key chord. Returns `None` when the key should be forwarded to
    /// the active PTY, and `Some` when the mux consumed it.
    pub fn handle_key(&mut self, chord: &KeyChord) -> Option<Vec<MuxCommand>> {
        match self.state {
            PrefixState::Passthrough if chord == &self.prefix => {
                self.state = PrefixState::AwaitCommand;
                Some(Vec::new())
            }
            PrefixState::Passthrough => None,
            PrefixState::AwaitCommand => {
                self.state = PrefixState::Passthrough;
                Some(
                    command_for_chord(chord)
                        .into_iter()
                        .map(MuxCommand::new)
                        .collect(),
                )
            }
        }
    }
}

/// Default mux prefix: `Ctrl+b`.
pub fn default_prefix() -> KeyChord {
    KeyChord::single("b", Modifiers::CTRL)
}

/// Resolve the key after the prefix into the command spine id.
pub fn command_for_chord(chord: &KeyChord) -> Option<CommandId> {
    let [stroke] = chord.strokes() else {
        return None;
    };
    if !stroke.modifiers().is_none() {
        return match (stroke.key(), stroke.modifiers()) {
            ("ArrowUp", Modifiers::CTRL) => Some(command_ids::RESIZE_UP.into()),
            ("ArrowDown", Modifiers::CTRL) => Some(command_ids::RESIZE_DOWN.into()),
            ("ArrowLeft", Modifiers::CTRL) => Some(command_ids::RESIZE_LEFT.into()),
            ("ArrowRight", Modifiers::CTRL) => Some(command_ids::RESIZE_RIGHT.into()),
            _ => None,
        };
    }

    match stroke.key() {
        "c" => Some(command_ids::NEW_WINDOW.into()),
        "," => Some(command_ids::RENAME_WINDOW.into()),
        "&" => Some(command_ids::KILL_WINDOW.into()),
        "n" => Some(command_ids::NEXT_WINDOW.into()),
        "p" => Some(command_ids::PREVIOUS_WINDOW.into()),
        "%" => Some(command_ids::SPLIT_HORIZONTAL.into()),
        "\"" => Some(command_ids::SPLIT_VERTICAL.into()),
        "x" => Some(command_ids::KILL_PANE.into()),
        "o" => Some(command_ids::NEXT_PANE.into()),
        ";" => Some(command_ids::LAST_PANE.into()),
        "}" => Some(command_ids::SWAP_PANE_NEXT.into()),
        "ArrowUp" => Some(command_ids::FOCUS_UP.into()),
        "ArrowDown" => Some(command_ids::FOCUS_DOWN.into()),
        "ArrowLeft" => Some(command_ids::FOCUS_LEFT.into()),
        "ArrowRight" => Some(command_ids::FOCUS_RIGHT.into()),
        "z" => Some(command_ids::ZOOM_PANE.into()),
        "Space" => Some(command_ids::NEXT_LAYOUT.into()),
        "[" => Some(command_ids::COPY_MODE.into()),
        "]" => Some(command_ids::PASTE.into()),
        "d" => Some(command_ids::DETACH.into()),
        digit @ ("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => {
            let index = digit.parse::<usize>().ok()?;
            Some(command_ids::SELECT_WINDOW[index].into())
        }
        _ => None,
    }
}

/// Pane interaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneMode {
    Live,
    Copy,
}

/// Cursor position inside copy mode's viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyCursor {
    pub row: usize,
    pub col: usize,
}

impl CopyCursor {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Selection anchor inside copy mode's viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopySelection {
    pub anchor: CopyCursor,
}

/// Pending copy-mode forward search state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopySearch {
    pub query: String,
}

/// One terminal pane. Real PTY handles live in `cockpit-terminal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub scrollback_offset: usize,
    pub mode: PaneMode,
    #[serde(default, skip_serializing_if = "CopyCursor::is_default")]
    pub copy_cursor: CopyCursor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_selection: Option<CopySelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_search: Option<CopySearch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_search: Option<String>,
}

impl Pane {
    fn new(id: PaneId) -> Self {
        Self {
            id,
            scrollback_offset: 0,
            mode: PaneMode::Live,
            copy_cursor: CopyCursor::default(),
            copy_selection: None,
            copy_search: None,
            last_search: None,
        }
    }

    /// Normalized copy selection endpoints, if this pane has an active anchor.
    pub fn copy_selection_range(&self) -> Option<(CopyCursor, CopyCursor)> {
        let selection = self.copy_selection?;
        Some(normalize_selection(selection.anchor, self.copy_cursor))
    }
}

/// Recursive layout tree for a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LayoutNode {
    Leaf {
        pane: PaneId,
    },
    Split {
        dir: SplitDirection,
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// Walk leaves left-to-right / top-to-bottom.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.push_leaves(&mut panes);
        panes
    }

    /// Project this layout into logical rectangles for rendering.
    pub fn pane_rects(&self, bounds: Rect, active: PaneId, border_px: u32) -> Vec<PaneRect> {
        let mut rects = Vec::new();
        self.push_pane_rects(bounds, active, border_px, &mut rects);
        rects
    }

    fn push_leaves(&self, panes: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane } => panes.push(*pane),
            Self::Split { a, b, .. } => {
                a.push_leaves(panes);
                b.push_leaves(panes);
            }
        }
    }

    fn push_pane_rects(
        &self,
        bounds: Rect,
        active: PaneId,
        border_px: u32,
        rects: &mut Vec<PaneRect>,
    ) {
        match self {
            Self::Leaf { pane } => rects.push(PaneRect {
                pane: *pane,
                rect: bounds,
                active: *pane == active,
            }),
            Self::Split {
                dir, ratio, a, b, ..
            } => {
                let (first, second) = split_rect(bounds, *dir, *ratio, border_px);
                a.push_pane_rects(first, active, border_px, rects);
                b.push_pane_rects(second, active, border_px, rects);
            }
        }
    }

    fn split_leaf(&mut self, target: PaneId, new_pane: PaneId, dir: SplitDirection) -> bool {
        match self {
            Self::Leaf { pane } if *pane == target => {
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Self::Leaf { pane: target }),
                    b: Box::new(Self::Leaf { pane: new_pane }),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { a, b, .. } => {
                a.split_leaf(target, new_pane, dir) || b.split_leaf(target, new_pane, dir)
            }
        }
    }

    fn remove_leaf(&mut self, target: PaneId) -> bool {
        match self {
            Self::Leaf { pane } => *pane == target,
            Self::Split { a, b, .. } => {
                if a.remove_leaf(target) {
                    *self = (**b).clone();
                    true
                } else if b.remove_leaf(target) {
                    *self = (**a).clone();
                    true
                } else {
                    false
                }
            }
        }
    }

    fn resize_parent_of(&mut self, target: PaneId, delta: f32) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split { ratio, a, b, .. } => {
                if a.contains(target) {
                    *ratio = clamp_ratio(*ratio + delta);
                    true
                } else if b.contains(target) {
                    *ratio = clamp_ratio(*ratio - delta);
                    true
                } else {
                    a.resize_parent_of(target, delta) || b.resize_parent_of(target, delta)
                }
            }
        }
    }

    fn contains(&self, target: PaneId) -> bool {
        match self {
            Self::Leaf { pane } => *pane == target,
            Self::Split { a, b, .. } => a.contains(target) || b.contains(target),
        }
    }

    fn swap(&mut self, left: PaneId, right: PaneId) {
        match self {
            Self::Leaf { pane } if *pane == left => *pane = right,
            Self::Leaf { pane } if *pane == right => *pane = left,
            Self::Leaf { .. } => {}
            Self::Split { a, b, .. } => {
                a.swap(left, right);
                b.swap(left, right);
            }
        }
    }
}

/// One mux window: a named layout tree plus active pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub name: String,
    pub layout: LayoutNode,
    pub active: PaneId,
    pub layout_preset: LayoutPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoomed: Option<PaneId>,
    pub panes: Vec<Pane>,
}

impl Window {
    fn new(id: WindowId, name: impl Into<String>, pane: Pane) -> Self {
        let active = pane.id;
        Self {
            id,
            name: name.into(),
            layout: LayoutNode::Leaf { pane: active },
            active,
            layout_preset: LayoutPreset::EvenHorizontal,
            zoomed: None,
            panes: vec![pane],
        }
    }

    /// Project the window layout into pane rectangles for the terminal area.
    pub fn pane_rects(&self, bounds: Rect, border_px: u32) -> Vec<PaneRect> {
        if let Some(pane) = self.zoomed.filter(|pane| self.layout.contains(*pane)) {
            return vec![PaneRect {
                pane,
                rect: bounds,
                active: pane == self.active,
            }];
        }
        self.layout.pane_rects(bounds, self.active, border_px)
    }
}

/// Complete in-process mux session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub windows: Vec<Window>,
    pub active: WindowId,
    next_window: u64,
    next_pane: u64,
}

impl Session {
    /// Create a session with one window and one live pane.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_id_base(name, 0)
    }

    /// Create a session with one window and one live pane whose window /
    /// pane id counters start from `id_base`. Used by [`SessionRegistry`]
    /// so concurrent sessions hand out non-overlapping ids (v0.7 M7.5).
    pub fn with_id_base(name: impl Into<String>, id_base: u64) -> Self {
        let mut ids = Ids::starting_at(id_base);
        let first_window = ids.window();
        let first_pane = Pane::new(ids.pane());
        Self {
            id: SessionId(1),
            name: name.into(),
            windows: vec![Window::new(first_window, "0", first_pane)],
            active: first_window,
            next_window: ids.next_window,
            next_pane: ids.next_pane,
        }
    }

    pub fn active_window(&self) -> &Window {
        self.windows
            .iter()
            .find(|window| window.id == self.active)
            .expect("active window id is maintained by Session")
    }

    pub fn active_window_mut(&mut self) -> &mut Window {
        self.windows
            .iter_mut()
            .find(|window| window.id == self.active)
            .expect("active window id is maintained by Session")
    }

    /// Borrow the active pane in the active window.
    pub fn active_pane(&self) -> &Pane {
        let window = self.active_window();
        window
            .panes
            .iter()
            .find(|pane| pane.id == window.active)
            .expect("active pane id is maintained by Window")
    }

    fn active_pane_mut(&mut self) -> &mut Pane {
        let window = self.active_window_mut();
        let pane_id = window.active;
        window
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .expect("active pane id is maintained by Window")
    }

    /// Create and select a new window with a single pane.
    pub fn new_window(&mut self, name: impl Into<String>) -> WindowId {
        let window_id = self.alloc_window();
        let pane = Pane::new(self.alloc_pane());
        self.windows.push(Window::new(window_id, name, pane));
        self.active = window_id;
        window_id
    }

    /// Select the next window, wrapping at the end.
    pub fn next_window(&mut self) {
        self.select_relative_window(1);
    }

    /// Select the previous window, wrapping at the start.
    pub fn previous_window(&mut self) {
        self.select_relative_window(-1);
    }

    /// Select the window at a zero-based index.
    pub fn select_window(&mut self, index: usize) -> Result<(), MuxError> {
        let window = self
            .windows
            .get(index)
            .ok_or(MuxError::WindowIndexOutOfRange(index))?;
        self.active = window.id;
        Ok(())
    }

    /// Kill the active window. The last window in a session is preserved.
    pub fn kill_window(&mut self) -> Result<WindowId, MuxError> {
        if self.windows.len() == 1 {
            return Err(MuxError::CannotKillLastWindow);
        }
        let killed = self.active;
        let index = self
            .windows
            .iter()
            .position(|window| window.id == killed)
            .ok_or(MuxError::BrokenLayout)?;
        self.windows.remove(index);
        let next = index.min(self.windows.len() - 1);
        self.active = self.windows[next].id;
        Ok(killed)
    }

    /// Split the active pane and focus the newly-created pane.
    pub fn split_active(&mut self, dir: SplitDirection) -> PaneId {
        let new_pane_id = self.alloc_pane();
        let window = self.active_window_mut();
        let active = window.active;
        window.layout.split_leaf(active, new_pane_id, dir);
        window.panes.push(Pane::new(new_pane_id));
        window.active = new_pane_id;
        window.zoomed = None;
        new_pane_id
    }

    /// Kill the active pane. The last pane in a window is preserved.
    pub fn kill_pane(&mut self) -> Result<PaneId, MuxError> {
        let window = self.active_window_mut();
        if window.panes.len() == 1 {
            return Err(MuxError::CannotKillLastPane);
        }
        let killed = window.active;
        window.layout.remove_leaf(killed);
        window.panes.retain(|pane| pane.id != killed);
        if window.zoomed == Some(killed) {
            window.zoomed = None;
        }
        window.active = window
            .layout
            .leaves()
            .into_iter()
            .next()
            .ok_or(MuxError::BrokenLayout)?;
        Ok(killed)
    }

    /// Focus the next pane in layout order.
    pub fn next_pane(&mut self) {
        self.select_relative_pane(1);
    }

    /// Focus the previous pane in layout order.
    pub fn previous_pane(&mut self) {
        self.select_relative_pane(-1);
    }

    /// Focus a pane in the active window.
    pub fn select_pane(&mut self, pane: PaneId) -> Result<(), MuxError> {
        let window = self.active_window_mut();
        if !window.layout.contains(pane) {
            return Err(MuxError::UnknownPane(pane));
        }
        window.active = pane;
        if window.zoomed.is_some() {
            window.zoomed = Some(pane);
        }
        Ok(())
    }

    /// Swap the active pane with the next pane in layout order.
    pub fn swap_panes(&mut self) -> Result<(), MuxError> {
        let window = self.active_window_mut();
        let leaves = window.layout.leaves();
        if leaves.len() < 2 {
            return Err(MuxError::CannotSwapSinglePane);
        }
        let current = leaves
            .iter()
            .position(|pane| *pane == window.active)
            .ok_or(MuxError::BrokenLayout)?;
        let next = leaves[(current + 1) % leaves.len()];
        window.layout.swap(window.active, next);
        Ok(())
    }

    /// Toggle zoom for the active pane in the active window.
    pub fn toggle_zoom(&mut self) -> Option<PaneId> {
        let window = self.active_window_mut();
        if window.zoomed == Some(window.active) {
            window.zoomed = None;
            None
        } else {
            window.zoomed = Some(window.active);
            window.zoomed
        }
    }

    /// Enter copy mode on the active pane and reset its viewport to live edge.
    pub fn enter_copy_mode(&mut self) -> PaneId {
        let pane = self.active_pane_mut();
        let pane_id = pane.id;
        pane.mode = PaneMode::Copy;
        pane.scrollback_offset = 0;
        pane.copy_cursor = CopyCursor::default();
        pane.copy_selection = None;
        pane.copy_search = None;
        pane.last_search = None;
        pane_id
    }

    /// Return the active pane to live terminal mode.
    pub fn exit_copy_mode(&mut self) -> PaneId {
        let pane = self.active_pane_mut();
        let pane_id = pane.id;
        pane.mode = PaneMode::Live;
        pane.scrollback_offset = 0;
        pane.copy_cursor = CopyCursor::default();
        pane.copy_selection = None;
        pane.copy_search = None;
        pane.last_search = None;
        pane_id
    }

    /// Move the active copy-mode viewport. Positive deltas move away from
    /// the live edge; negative deltas move back toward it.
    pub fn scroll_copy_mode(&mut self, delta: isize, max_offset: usize) -> Option<usize> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        let next = if delta.is_negative() {
            pane.scrollback_offset.saturating_sub(delta.unsigned_abs())
        } else {
            pane.scrollback_offset.saturating_add(delta as usize)
        };
        pane.scrollback_offset = next.min(max_offset);
        Some(pane.scrollback_offset)
    }

    /// Move the active copy-mode cursor inside the visible viewport.
    pub fn move_copy_cursor(
        &mut self,
        row_delta: isize,
        col_delta: isize,
        max_row: usize,
        max_col: usize,
    ) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.copy_cursor.row = apply_bounded_delta(pane.copy_cursor.row, row_delta, max_row);
        pane.copy_cursor.col = apply_bounded_delta(pane.copy_cursor.col, col_delta, max_col);
        Some(pane.copy_cursor)
    }

    /// Move the active copy-mode cursor to an absolute column.
    pub fn set_copy_cursor_col(&mut self, col: usize, max_col: usize) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.copy_cursor.col = col.min(max_col);
        Some(pane.copy_cursor)
    }

    /// Toggle copy-mode selection anchored at the current cursor.
    pub fn toggle_copy_selection(&mut self) -> Option<Option<CopySelection>> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.copy_selection = match pane.copy_selection {
            Some(_) => None,
            None => Some(CopySelection {
                anchor: pane.copy_cursor,
            }),
        };
        Some(pane.copy_selection)
    }

    /// Jump the copy-mode viewport to the top of the scrollback (vim `gg`).
    pub fn copy_top_of_scrollback(&mut self, max_offset: usize) -> Option<usize> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.scrollback_offset = max_offset;
        Some(pane.scrollback_offset)
    }

    /// Jump the copy-mode viewport to the live edge (vim `G` in tmux copy-mode-vi).
    pub fn copy_bottom_of_scrollback(&mut self) -> Option<usize> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.scrollback_offset = 0;
        Some(pane.scrollback_offset)
    }

    /// Move the active copy-mode cursor to the next word start on `row_text`
    /// (vim `w`).
    pub fn copy_word_forward(&mut self, row_text: &str, max_col: usize) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.copy_cursor.col = next_word_col(row_text, pane.copy_cursor.col).min(max_col);
        Some(pane.copy_cursor)
    }

    /// Move the active copy-mode cursor to the previous word start on
    /// `row_text` (vim `b`).
    pub fn copy_word_backward(&mut self, row_text: &str) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        pane.copy_cursor.col = prev_word_col(row_text, pane.copy_cursor.col);
        Some(pane.copy_cursor)
    }

    /// Extract the active pane's currently selected text from `rows`. Each
    /// entry in `rows` is one display line, top-to-bottom in the viewport.
    /// Selection is inclusive of both endpoints (matches tmux copy-mode-vi
    /// yank semantics).
    pub fn copy_selection_text(&self, rows: &[&str]) -> Option<String> {
        let pane = self.active_pane();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        let (start, end) = pane.copy_selection_range()?;
        Some(extract_selection_text(rows, start, end))
    }

    /// Begin a copy-mode forward search. Returns `false` if the active pane
    /// is not in copy mode.
    pub fn begin_copy_search(&mut self) -> bool {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return false;
        }
        pane.copy_search = Some(CopySearch::default());
        true
    }

    /// Append `ch` to the active pane's pending search query. No-op when no
    /// search is in progress.
    pub fn push_copy_search_char(&mut self, ch: char) -> Option<String> {
        let pane = self.active_pane_mut();
        let search = pane.copy_search.as_mut()?;
        search.query.push(ch);
        Some(search.query.clone())
    }

    /// Drop the last char from the pending search query. Cancels the search
    /// once the query becomes empty.
    pub fn pop_copy_search_char(&mut self) -> Option<String> {
        let pane = self.active_pane_mut();
        let search = pane.copy_search.as_mut()?;
        search.query.pop();
        if search.query.is_empty() {
            pane.copy_search = None;
            return Some(String::new());
        }
        Some(search.query.clone())
    }

    /// Abandon the pending copy-mode search.
    pub fn cancel_copy_search(&mut self) -> bool {
        let pane = self.active_pane_mut();
        if pane.copy_search.is_none() {
            return false;
        }
        pane.copy_search = None;
        true
    }

    /// Snapshot of the pending copy-mode search query, if any.
    pub fn copy_search_query(&self) -> Option<&str> {
        let pane = self.active_pane();
        pane.copy_search
            .as_ref()
            .map(|search| search.query.as_str())
    }

    /// Run the pending copy-mode search forward across `rows`. On match,
    /// moves the cursor to the first hit at or after the current cursor
    /// position and stores the query as the pane's last completed search.
    /// On no-match leaves the cursor where it was. Returns the resulting
    /// match position when found.
    pub fn finish_copy_search(&mut self, rows: &[&str]) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        let query = pane
            .copy_search
            .as_ref()
            .map(|search| search.query.clone())?;
        pane.copy_search = None;
        if query.is_empty() {
            return None;
        }
        let start = pane.copy_cursor;
        let hit = find_match_forward(rows, &query, start)?;
        pane.copy_cursor = hit;
        pane.last_search = Some(query);
        Some(hit)
    }

    /// Jump to the next match of the last completed search (`n` in tmux
    /// copy-mode-vi).
    pub fn repeat_copy_search_forward(&mut self, rows: &[&str]) -> Option<CopyCursor> {
        let pane = self.active_pane_mut();
        if pane.mode != PaneMode::Copy {
            return None;
        }
        let query = pane.last_search.clone()?;
        let after = next_position(pane.copy_cursor, rows);
        let hit = find_match_forward(rows, &query, after)?;
        pane.copy_cursor = hit;
        Some(hit)
    }

    /// Resize the split that directly contains the active pane.
    pub fn resize_pane(&mut self, delta: f32) -> Result<(), MuxError> {
        let window = self.active_window_mut();
        if window.layout.resize_parent_of(window.active, delta) {
            Ok(())
        } else {
            Err(MuxError::CannotResizeSinglePane)
        }
    }

    /// Rewrite the active window into a built-in preset while preserving
    /// pane ids and focus.
    pub fn select_layout(&mut self, preset: LayoutPreset) {
        let window = self.active_window_mut();
        let panes = window.layout.leaves();
        window.layout = layout_for_preset(&panes, preset);
        window.layout_preset = preset;
        window.zoomed = None;
    }

    /// Cycle the active window to the next built-in layout preset.
    pub fn next_layout(&mut self) -> LayoutPreset {
        let preset = self.active_window().layout_preset.next();
        self.select_layout(preset);
        preset
    }

    fn select_relative_window(&mut self, delta: isize) {
        if self.windows.is_empty() {
            return;
        }
        let index = self
            .windows
            .iter()
            .position(|window| window.id == self.active)
            .unwrap_or(0);
        let next = wrap_index(index, delta, self.windows.len());
        self.active = self.windows[next].id;
    }

    fn select_relative_pane(&mut self, delta: isize) {
        let window = self.active_window_mut();
        let leaves = window.layout.leaves();
        if leaves.is_empty() {
            return;
        }
        let index = leaves
            .iter()
            .position(|pane| *pane == window.active)
            .unwrap_or(0);
        window.active = leaves[wrap_index(index, delta, leaves.len())];
        if window.zoomed.is_some() {
            window.zoomed = Some(window.active);
        }
    }

    /// Build a session from a [`LayoutDescription`] tree, allocating fresh
    /// pane ids in left-to-right / top-to-bottom order. The returned vector
    /// pairs each allocated pane id with its first-attach command (M7.8)
    /// so the caller can spawn the PTYs without re-walking the tree.
    pub fn from_layout(
        name: impl Into<String>,
        description: &LayoutDescription,
    ) -> (Self, Vec<(PaneId, Option<String>)>) {
        let mut ids = Ids::default();
        let first_window_id = ids.window();
        let mut pane_commands: Vec<(PaneId, Option<String>)> = Vec::new();
        let layout = build_layout_node(description, &mut ids, &mut pane_commands);
        let active = pane_commands
            .first()
            .map(|(pane, _)| *pane)
            .unwrap_or(PaneId(0));
        let panes = pane_commands
            .iter()
            .map(|(id, _)| Pane::new(*id))
            .collect::<Vec<_>>();
        let window = Window {
            id: first_window_id,
            name: "0".to_string(),
            layout,
            active,
            layout_preset: LayoutPreset::EvenHorizontal,
            zoomed: None,
            panes,
        };
        let session = Self {
            id: SessionId(1),
            name: name.into(),
            windows: vec![window],
            active: first_window_id,
            next_window: ids.next_window,
            next_pane: ids.next_pane,
        };
        (session, pane_commands)
    }

    /// Headless snapshot of the data a status line / mode-line (M7.7)
    /// needs to draw: session name plus per-window index, name, and
    /// active marker. Time and task fields are external inputs the
    /// caller adds when formatting.
    pub fn status_summary(&self) -> StatusSummary {
        let active_pane_id = self.active_pane().id;
        let windows = self
            .windows
            .iter()
            .enumerate()
            .map(|(index, window)| {
                let pane_count = window.panes.len();
                let active_pane_in_window = window
                    .panes
                    .iter()
                    .position(|pane| pane.id == window.active)
                    .unwrap_or(0);
                WindowStatus {
                    index,
                    name: window.name.clone(),
                    active: window.id == self.active,
                    pane_count,
                    active_pane_in_window,
                    contains_active_pane: window.id == self.active
                        && window.active == active_pane_id,
                }
            })
            .collect();
        StatusSummary {
            session_name: self.name.clone(),
            windows,
        }
    }

    /// Rename this session.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    fn alloc_window(&mut self) -> WindowId {
        let id = WindowId(self.next_window);
        self.next_window += 1;
        id
    }

    fn alloc_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }
}

/// Per-session id stride for pane/window allocation. Keeps sessions from
/// colliding when the registry hands out ids across multiple in-flight
/// sessions (v0.7 M7.5). One million ids per session is more than any real
/// user-driven workflow will hit.
pub const SESSION_ID_STRIDE: u64 = 1_000_000;

/// Multi-session registry for the native multiplexer (v0.7 M7.5).
///
/// Cockpit keeps every detached session running in-process: a detach flips
/// the registry into an "unattached" state so the workspace can paint a
/// session-list overlay, and an attach makes a chosen session visible
/// again with its layout, scrollback, and pane state intact. Session
/// persistence across cockpit restarts is M7.5a (deferred).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRegistry {
    sessions: Vec<Session>,
    active: SessionId,
    attached: bool,
    next_session_id: u64,
}

impl SessionRegistry {
    /// Build a registry around an initial active session. The session's id
    /// is kept as-is; subsequent [`Self::create`] / [`Self::add`] calls
    /// allocate fresh ids from there.
    pub fn new(initial: Session) -> Self {
        let active = initial.id;
        let next = active.0.saturating_add(1);
        Self {
            sessions: vec![initial],
            active,
            attached: true,
            next_session_id: next,
        }
    }

    /// Currently active session id (the one the workspace is showing when
    /// attached, or the one most recently detached from).
    pub fn active_id(&self) -> SessionId {
        self.active
    }

    /// True when the workspace is showing the active session. False after a
    /// [`Self::detach`], until [`Self::attach`] re-activates one.
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Borrow every session in registration order.
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// Borrow the active session.
    pub fn active(&self) -> &Session {
        self.find(self.active)
            .expect("active session id is maintained by SessionRegistry")
    }

    /// Mutably borrow the active session.
    pub fn active_mut(&mut self) -> &mut Session {
        let id = self.active;
        self.sessions
            .iter_mut()
            .find(|session| session.id == id)
            .expect("active session id is maintained by SessionRegistry")
    }

    /// Find a session by id.
    pub fn find(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    /// Create a new session, register it, and activate it. The session
    /// inherits a single live pane. Window/pane ids start from a fresh
    /// stride so they never collide with already-registered sessions.
    pub fn create(&mut self, name: impl Into<String>) -> SessionId {
        let id = self.alloc_session_id();
        let mut session = Session::with_id_base(name, id.0 * SESSION_ID_STRIDE);
        session.id = id;
        self.sessions.push(session);
        self.active = id;
        self.attached = true;
        id
    }

    /// Register an externally-built session (e.g. one produced by
    /// `Session::from_layout`) and activate it. The session's id is
    /// rewritten so it stays unique inside the registry.
    pub fn add(&mut self, mut session: Session) -> SessionId {
        let id = self.alloc_session_id();
        session.id = id;
        self.sessions.push(session);
        self.active = id;
        self.attached = true;
        id
    }

    /// Detach from the active session. The session itself keeps running;
    /// the workspace's terminal area should switch to the session-list
    /// overlay until [`Self::attach`] re-activates one.
    pub fn detach(&mut self) -> SessionId {
        self.attached = false;
        self.active
    }

    /// Attach to a registered session, making it the visible one.
    pub fn attach(&mut self, id: SessionId) -> Result<SessionId, MuxError> {
        if self.find(id).is_none() {
            return Err(MuxError::UnknownSession(id));
        }
        self.active = id;
        self.attached = true;
        Ok(id)
    }

    /// Kill a session by id. Fails if it would leave the registry empty.
    /// Activates the next session in registration order when the killed
    /// session was active.
    pub fn kill(&mut self, id: SessionId) -> Result<SessionId, MuxError> {
        if self.sessions.len() == 1 {
            return Err(MuxError::CannotKillLastSession);
        }
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == id)
            .ok_or(MuxError::UnknownSession(id))?;
        self.sessions.remove(index);
        if self.active == id {
            let next = index.min(self.sessions.len() - 1);
            self.active = self.sessions[next].id;
            self.attached = true;
        }
        Ok(id)
    }

    fn alloc_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id);
        self.next_session_id = self.next_session_id.saturating_add(1);
        id
    }
}

/// Recursive description of a layout to build from config (v0.7 M7.8).
/// Independent of [`PaneId`] allocation — `Session::from_layout` walks the
/// description and allocates fresh ids.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutDescription {
    /// Leaf pane with an optional command to run on first attach.
    Pane { command: Option<String> },
    /// Recursive split between two child descriptions.
    Split {
        direction: SplitDirection,
        ratio: f32,
        a: Box<LayoutDescription>,
        b: Box<LayoutDescription>,
    },
}

impl LayoutDescription {
    /// Total leaf-pane count under this description.
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane { .. } => 1,
            Self::Split { a, b, .. } => a.pane_count() + b.pane_count(),
        }
    }
}

fn build_layout_node(
    description: &LayoutDescription,
    ids: &mut Ids,
    pane_commands: &mut Vec<(PaneId, Option<String>)>,
) -> LayoutNode {
    match description {
        LayoutDescription::Pane { command } => {
            let pane = ids.pane();
            pane_commands.push((pane, command.clone()));
            LayoutNode::Leaf { pane }
        }
        LayoutDescription::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            let a = build_layout_node(a, ids, pane_commands);
            let b = build_layout_node(b, ids, pane_commands);
            LayoutNode::Split {
                dir: *direction,
                ratio: clamp_ratio(*ratio),
                a: Box::new(a),
                b: Box::new(b),
            }
        }
    }
}

/// Per-window state surfaced to a status / mode-line painter (M7.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowStatus {
    pub index: usize,
    pub name: String,
    pub active: bool,
    pub pane_count: usize,
    pub active_pane_in_window: usize,
    pub contains_active_pane: bool,
}

/// Headless snapshot used by a mode-line painter. Time / mise task come from
/// outside this crate — pure session state lives here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSummary {
    pub session_name: String,
    pub windows: Vec<WindowStatus>,
}

impl StatusSummary {
    /// Default tmux-style mode-line render. Format:
    ///
    /// `[<session>] 0:name 1:name* 2:name`
    ///
    /// where `*` marks the active window. `extras` (time, mise task) appear
    /// right-padded after a `│` separator if any are supplied.
    pub fn render(&self, extras: &[&str]) -> String {
        let mut out = format!("[{}]", self.session_name);
        for window in &self.windows {
            out.push(' ');
            out.push_str(&window.index.to_string());
            out.push(':');
            out.push_str(&window.name);
            if window.active {
                out.push('*');
            }
        }
        let mut first_extra = true;
        for extra in extras.iter().filter(|extra| !extra.is_empty()) {
            if first_extra {
                out.push_str(" │ ");
                first_extra = false;
            } else {
                out.push_str(" · ");
            }
            out.push_str(extra);
        }
        out
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MuxError {
    #[error("cannot kill the last window in a session")]
    CannotKillLastWindow,
    #[error("cannot kill the last pane in a window")]
    CannotKillLastPane,
    #[error("cannot swap panes in a single-pane window")]
    CannotSwapSinglePane,
    #[error("cannot resize a single-pane window")]
    CannotResizeSinglePane,
    #[error("window index {0} is out of range")]
    WindowIndexOutOfRange(usize),
    #[error("unknown pane `{0}`")]
    UnknownPane(PaneId),
    #[error("unknown session `{0}`")]
    UnknownSession(SessionId),
    #[error("cannot kill the last session in a registry")]
    CannotKillLastSession,
    #[error("layout tree no longer references its pane set")]
    BrokenLayout,
}

#[derive(Default)]
struct Ids {
    next_window: u64,
    next_pane: u64,
}

impl Ids {
    fn starting_at(base: u64) -> Self {
        Self {
            next_window: base,
            next_pane: base,
        }
    }

    fn window(&mut self) -> WindowId {
        let id = WindowId(self.next_window);
        self.next_window += 1;
        id
    }

    fn pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }
}

fn layout_for_preset(panes: &[PaneId], preset: LayoutPreset) -> LayoutNode {
    match panes {
        [] => LayoutNode::Leaf { pane: PaneId(0) },
        [pane] => LayoutNode::Leaf { pane: *pane },
        _ => match preset {
            LayoutPreset::EvenHorizontal => balanced_split(panes, SplitDirection::Horizontal),
            LayoutPreset::Tiled => balanced_split(panes, SplitDirection::Vertical),
            LayoutPreset::MainVertical => LayoutNode::Split {
                dir: SplitDirection::Horizontal,
                ratio: 0.6,
                a: Box::new(LayoutNode::Leaf { pane: panes[0] }),
                b: Box::new(balanced_split(&panes[1..], SplitDirection::Vertical)),
            },
        },
    }
}

fn balanced_split(panes: &[PaneId], dir: SplitDirection) -> LayoutNode {
    match panes {
        [] => LayoutNode::Leaf { pane: PaneId(0) },
        [pane] => LayoutNode::Leaf { pane: *pane },
        _ => {
            let mid = panes.len().div_ceil(2);
            LayoutNode::Split {
                dir,
                ratio: mid as f32 / panes.len() as f32,
                a: Box::new(balanced_split(&panes[..mid], flip(dir))),
                b: Box::new(balanced_split(&panes[mid..], flip(dir))),
            }
        }
    }
}

fn flip(dir: SplitDirection) -> SplitDirection {
    match dir {
        SplitDirection::Horizontal => SplitDirection::Vertical,
        SplitDirection::Vertical => SplitDirection::Horizontal,
    }
}

fn clamp_ratio(value: f32) -> f32 {
    value.clamp(0.1, 0.9)
}

fn split_rect(bounds: Rect, dir: SplitDirection, ratio: f32, border_px: u32) -> (Rect, Rect) {
    let ratio = clamp_ratio(ratio);
    match dir {
        SplitDirection::Horizontal => {
            if bounds.width <= border_px {
                return (
                    Rect::new(bounds.x, bounds.y, 0, bounds.height),
                    Rect::new(bounds.x + bounds.width, bounds.y, 0, bounds.height),
                );
            }
            let content = bounds.width - border_px;
            let first_width = ((content as f32) * ratio).round() as u32;
            let second_width = content.saturating_sub(first_width);
            (
                Rect::new(bounds.x, bounds.y, first_width, bounds.height),
                Rect::new(
                    bounds.x + first_width + border_px,
                    bounds.y,
                    second_width,
                    bounds.height,
                ),
            )
        }
        SplitDirection::Vertical => {
            if bounds.height <= border_px {
                return (
                    Rect::new(bounds.x, bounds.y, bounds.width, 0),
                    Rect::new(bounds.x, bounds.y + bounds.height, bounds.width, 0),
                );
            }
            let content = bounds.height - border_px;
            let first_height = ((content as f32) * ratio).round() as u32;
            let second_height = content.saturating_sub(first_height);
            (
                Rect::new(bounds.x, bounds.y, bounds.width, first_height),
                Rect::new(
                    bounds.x,
                    bounds.y + first_height + border_px,
                    bounds.width,
                    second_height,
                ),
            )
        }
    }
}

fn wrap_index(index: usize, delta: isize, len: usize) -> usize {
    let len = len as isize;
    (index as isize + delta).rem_euclid(len) as usize
}

fn apply_bounded_delta(value: usize, delta: isize, max: usize) -> usize {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as usize).min(max)
    }
}

fn normalize_selection(left: CopyCursor, right: CopyCursor) -> (CopyCursor, CopyCursor) {
    if (left.row, left.col) <= (right.row, right.col) {
        (left, right)
    } else {
        (right, left)
    }
}

/// Column of the next word start on or after `col` in `text`. Whitespace-only
/// tails saturate at the line length so the caller can clamp against its own
/// max column.
fn next_word_col(text: &str, col: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return 0;
    }
    let mut i = col.min(chars.len());
    if i < chars.len() && !chars[i].is_whitespace() {
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
    }
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Column of the previous word start before `col` in `text`.
fn prev_word_col(text: &str, col: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    if col == 0 || chars.is_empty() {
        return 0;
    }
    let mut i = col.saturating_sub(1).min(chars.len().saturating_sub(1));
    while i > 0 && chars[i].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// Extract selection text from `rows`, inclusive of both endpoints.
fn extract_selection_text(rows: &[&str], start: CopyCursor, end: CopyCursor) -> String {
    if rows.is_empty() {
        return String::new();
    }
    if start.row == end.row {
        return slice_row(
            rows.get(start.row).copied().unwrap_or(""),
            start.col,
            end.col + 1,
        );
    }
    let mut out = String::new();
    for row_idx in start.row..=end.row {
        let line = rows.get(row_idx).copied().unwrap_or("");
        if row_idx == start.row {
            let chars: Vec<char> = line.chars().collect();
            let from = start.col.min(chars.len());
            out.extend(&chars[from..]);
        } else if row_idx == end.row {
            let chars: Vec<char> = line.chars().collect();
            let to = (end.col + 1).min(chars.len());
            out.extend(&chars[..to]);
        } else {
            out.push_str(line);
        }
        if row_idx < end.row {
            out.push('\n');
        }
    }
    out
}

/// Substring of `line` between char-index columns `start..end`, clamped.
fn slice_row(line: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let from = start.min(chars.len());
    let to = end.min(chars.len()).max(from);
    chars[from..to].iter().collect()
}

/// Find the first match of `needle` in `rows` at or after `from`. Returns the
/// matching cursor position (row + char column).
fn find_match_forward(rows: &[&str], needle: &str, from: CopyCursor) -> Option<CopyCursor> {
    if needle.is_empty() {
        return None;
    }
    for (row_idx, line) in rows.iter().enumerate().skip(from.row) {
        let chars: Vec<char> = line.chars().collect();
        let start_col = if row_idx == from.row { from.col } else { 0 };
        if start_col > chars.len() {
            continue;
        }
        let haystack: String = chars[start_col..].iter().collect();
        if let Some(byte_idx) = haystack.find(needle) {
            let char_offset = haystack[..byte_idx].chars().count();
            return Some(CopyCursor {
                row: row_idx,
                col: start_col + char_offset,
            });
        }
    }
    None
}

/// One position past `cursor` so search-next doesn't re-match the current hit.
fn next_position(cursor: CopyCursor, rows: &[&str]) -> CopyCursor {
    let line_len = rows
        .get(cursor.row)
        .map(|line| line.chars().count())
        .unwrap_or(0);
    if cursor.col < line_len {
        CopyCursor {
            row: cursor.row,
            col: cursor.col + 1,
        }
    } else {
        CopyCursor {
            row: cursor.row + 1,
            col: 0,
        }
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pane-{}", self.0)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(session: &Session) -> String {
        serde_json::to_string_pretty(session).expect("session serializes")
    }

    fn chord(input: &str) -> KeyChord {
        input.parse().expect("test chord parses")
    }

    fn mapped(input: &str) -> Option<String> {
        command_for_chord(&chord(input)).map(|id| id.to_string())
    }

    #[test]
    fn default_prefix_is_ctrl_b() {
        assert_eq!(default_prefix(), chord("Ctrl+b"));
    }

    #[test]
    fn prefix_dispatcher_consumes_prefix_then_emits_command_ids() {
        let mut dispatcher = PrefixDispatcher::default();

        assert_eq!(dispatcher.handle_key(&chord("a")), None);
        assert_eq!(dispatcher.state(), PrefixState::Passthrough);

        assert_eq!(dispatcher.handle_key(&chord("Ctrl+b")), Some(Vec::new()));
        assert_eq!(dispatcher.state(), PrefixState::AwaitCommand);

        let commands = dispatcher
            .handle_key(&chord("%"))
            .expect("prefix command is consumed");
        assert_eq!(
            commands,
            vec![MuxCommand::new(command_ids::SPLIT_HORIZONTAL)]
        );
        assert_eq!(dispatcher.state(), PrefixState::Passthrough);
    }

    #[test]
    fn unknown_key_after_prefix_is_consumed_without_a_command() {
        let mut dispatcher = PrefixDispatcher::default();

        assert_eq!(dispatcher.handle_key(&chord("Ctrl+b")), Some(Vec::new()));
        assert_eq!(dispatcher.handle_key(&chord("q")), Some(Vec::new()));
        assert_eq!(dispatcher.state(), PrefixState::Passthrough);
    }

    #[test]
    fn default_prefix_bindings_resolve_to_command_ids() {
        let cases = [
            ("c", command_ids::NEW_WINDOW),
            (",", command_ids::RENAME_WINDOW),
            ("&", command_ids::KILL_WINDOW),
            ("n", command_ids::NEXT_WINDOW),
            ("p", command_ids::PREVIOUS_WINDOW),
            ("0", command_ids::SELECT_WINDOW_0),
            ("1", command_ids::SELECT_WINDOW_1),
            ("2", command_ids::SELECT_WINDOW_2),
            ("3", command_ids::SELECT_WINDOW_3),
            ("4", command_ids::SELECT_WINDOW_4),
            ("5", command_ids::SELECT_WINDOW_5),
            ("6", command_ids::SELECT_WINDOW_6),
            ("7", command_ids::SELECT_WINDOW_7),
            ("8", command_ids::SELECT_WINDOW_8),
            ("9", command_ids::SELECT_WINDOW_9),
            ("%", command_ids::SPLIT_HORIZONTAL),
            ("\"", command_ids::SPLIT_VERTICAL),
            ("x", command_ids::KILL_PANE),
            ("o", command_ids::NEXT_PANE),
            (";", command_ids::LAST_PANE),
            ("}", command_ids::SWAP_PANE_NEXT),
            ("ArrowUp", command_ids::FOCUS_UP),
            ("ArrowDown", command_ids::FOCUS_DOWN),
            ("ArrowLeft", command_ids::FOCUS_LEFT),
            ("ArrowRight", command_ids::FOCUS_RIGHT),
            ("Ctrl+ArrowUp", command_ids::RESIZE_UP),
            ("Ctrl+ArrowDown", command_ids::RESIZE_DOWN),
            ("Ctrl+ArrowLeft", command_ids::RESIZE_LEFT),
            ("Ctrl+ArrowRight", command_ids::RESIZE_RIGHT),
            ("z", command_ids::ZOOM_PANE),
            ("Space", command_ids::NEXT_LAYOUT),
            ("[", command_ids::COPY_MODE),
            ("]", command_ids::PASTE),
            ("d", command_ids::DETACH),
        ];

        for (input, expected) in cases {
            assert_eq!(mapped(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn modified_non_arrow_keys_do_not_resolve() {
        assert_eq!(command_for_chord(&chord("Ctrl+c")), None);
        assert_eq!(command_for_chord(&chord("Shift+ArrowUp")), None);
    }

    #[test]
    fn recorded_keystream_emits_only_prefixed_mux_commands() {
        let mut dispatcher = PrefixDispatcher::default();
        let stream = [
            "l", "s", "Ctrl+b", "%", "echo", "Ctrl+b", "\"", "Ctrl+b", "n", "Ctrl+b", "0",
            "Ctrl+b", "[",
        ];

        let emitted: Vec<String> = stream
            .into_iter()
            .flat_map(|input| dispatcher.handle_key(&chord(input)).unwrap_or_default())
            .map(|command| command.id().to_string())
            .collect();

        assert_eq!(
            emitted,
            vec![
                command_ids::SPLIT_HORIZONTAL,
                command_ids::SPLIT_VERTICAL,
                command_ids::NEXT_WINDOW,
                command_ids::SELECT_WINDOW_0,
                command_ids::COPY_MODE,
            ]
        );
    }

    #[test]
    fn pane_rects_project_horizontal_splits_with_border_gap() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);

        let rects = session
            .active_window()
            .pane_rects(Rect::new(10, 20, 101, 50), 1);

        assert_eq!(
            rects,
            vec![
                PaneRect {
                    pane: PaneId(0),
                    rect: Rect::new(10, 20, 50, 50),
                    active: false,
                },
                PaneRect {
                    pane: PaneId(1),
                    rect: Rect::new(61, 20, 50, 50),
                    active: true,
                },
            ]
        );
    }

    #[test]
    fn pane_rects_project_vertical_splits_with_border_gap() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Vertical);

        let rects = session
            .active_window()
            .pane_rects(Rect::new(0, 0, 80, 25), 1);

        assert_eq!(
            rects,
            vec![
                PaneRect {
                    pane: PaneId(0),
                    rect: Rect::new(0, 0, 80, 12),
                    active: false,
                },
                PaneRect {
                    pane: PaneId(1),
                    rect: Rect::new(0, 13, 80, 12),
                    active: true,
                },
            ]
        );
    }

    #[test]
    fn pane_rects_project_nested_layouts_in_leaf_order() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);

        let rects = session
            .active_window()
            .pane_rects(Rect::new(0, 0, 121, 61), 1);

        assert_eq!(
            rects,
            vec![
                PaneRect {
                    pane: PaneId(0),
                    rect: Rect::new(0, 0, 60, 61),
                    active: false,
                },
                PaneRect {
                    pane: PaneId(1),
                    rect: Rect::new(61, 0, 60, 30),
                    active: false,
                },
                PaneRect {
                    pane: PaneId(2),
                    rect: Rect::new(61, 31, 60, 30),
                    active: true,
                },
            ]
        );
    }

    #[test]
    fn pane_rects_tolerate_bounds_smaller_than_the_border() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);

        let rects = session
            .active_window()
            .pane_rects(Rect::new(5, 5, 1, 10), 2);

        assert_eq!(
            rects,
            vec![
                PaneRect {
                    pane: PaneId(0),
                    rect: Rect::new(5, 5, 0, 10),
                    active: false,
                },
                PaneRect {
                    pane: PaneId(1),
                    rect: Rect::new(6, 5, 0, 10),
                    active: true,
                },
            ]
        );
    }

    #[test]
    fn new_session_starts_with_one_window_and_one_pane() {
        let session = Session::new("dev");

        assert_eq!(session.name, "dev");
        assert_eq!(session.active_window().name, "0");
        assert_eq!(session.active_window().layout.leaves(), vec![PaneId(0)]);
        insta::assert_snapshot!(json(&session), @r#"
        {
          "id": 1,
          "name": "dev",
          "windows": [
            {
              "id": 0,
              "name": "0",
              "layout": {
                "type": "leaf",
                "pane": 0
              },
              "active": 0,
              "layout_preset": "even-horizontal",
              "panes": [
                {
                  "id": 0,
                  "scrollback_offset": 0,
                  "mode": "live"
                }
              ]
            }
          ],
          "active": 0,
          "next_window": 1,
          "next_pane": 1
        }
        "#);
    }

    #[test]
    fn split_active_builds_a_recursive_layout_tree() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);

        assert_eq!(
            session.active_window().layout.leaves(),
            vec![PaneId(0), PaneId(1), PaneId(2)]
        );
        assert_eq!(session.active_window().active, PaneId(2));
        insta::assert_snapshot!(json(&session), @r#"
        {
          "id": 1,
          "name": "dev",
          "windows": [
            {
              "id": 0,
              "name": "0",
              "layout": {
                "type": "split",
                "dir": "horizontal",
                "ratio": 0.5,
                "a": {
                  "type": "leaf",
                  "pane": 0
                },
                "b": {
                  "type": "split",
                  "dir": "vertical",
                  "ratio": 0.5,
                  "a": {
                    "type": "leaf",
                    "pane": 1
                  },
                  "b": {
                    "type": "leaf",
                    "pane": 2
                  }
                }
              },
              "active": 2,
              "layout_preset": "even-horizontal",
              "panes": [
                {
                  "id": 0,
                  "scrollback_offset": 0,
                  "mode": "live"
                },
                {
                  "id": 1,
                  "scrollback_offset": 0,
                  "mode": "live"
                },
                {
                  "id": 2,
                  "scrollback_offset": 0,
                  "mode": "live"
                }
              ]
            }
          ],
          "active": 0,
          "next_window": 1,
          "next_pane": 3
        }
        "#);
    }

    #[test]
    fn kill_pane_collapses_the_parent_split() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        assert_eq!(session.kill_pane(), Ok(PaneId(1)));

        assert_eq!(
            session.active_window().layout,
            LayoutNode::Leaf { pane: PaneId(0) }
        );
        assert_eq!(session.kill_pane(), Err(MuxError::CannotKillLastPane));
    }

    #[test]
    fn pane_focus_cycles_in_layout_order() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Horizontal);

        session.next_pane();
        assert_eq!(session.active_window().active, PaneId(0));
        session.previous_pane();
        assert_eq!(session.active_window().active, PaneId(2));
    }

    #[test]
    fn select_pane_focuses_an_existing_leaf() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);

        assert_eq!(session.select_pane(PaneId(0)), Ok(()));
        assert_eq!(session.active_window().active, PaneId(0));
        assert_eq!(
            session.select_pane(PaneId(99)),
            Err(MuxError::UnknownPane(PaneId(99)))
        );
    }

    #[test]
    fn windows_can_be_created_and_selected() {
        let mut session = Session::new("dev");
        let second = session.new_window("logs");

        assert_eq!(session.active, second);
        session.previous_window();
        assert_eq!(session.active, WindowId(0));
        session.next_window();
        assert_eq!(session.active, second);
        assert_eq!(session.select_window(0), Ok(()));
        assert_eq!(
            session.select_window(9),
            Err(MuxError::WindowIndexOutOfRange(9))
        );
    }

    #[test]
    fn kill_window_removes_the_active_window_and_preserves_one_window() {
        let mut session = Session::new("dev");
        let logs = session.new_window("logs");
        session.new_window("shell");

        assert_eq!(session.kill_window(), Ok(WindowId(2)));
        assert_eq!(session.active, logs);
        assert_eq!(
            session
                .windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            vec!["0", "logs"]
        );

        assert_eq!(session.kill_window(), Ok(logs));
        assert_eq!(session.active, WindowId(0));
        assert_eq!(session.kill_window(), Err(MuxError::CannotKillLastWindow));
    }

    #[test]
    fn swap_panes_rewrites_leaves_without_moving_focus() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Horizontal);

        session.swap_panes().expect("multi-pane swap succeeds");
        assert_eq!(session.active_window().active, PaneId(2));
        assert_eq!(
            session.active_window().layout.leaves(),
            vec![PaneId(2), PaneId(1), PaneId(0)]
        );
    }

    #[test]
    fn zoom_projects_only_the_active_pane_without_rewriting_layout() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);
        let leaves = session.active_window().layout.leaves();

        assert_eq!(session.toggle_zoom(), Some(PaneId(2)));
        assert_eq!(session.active_window().zoomed, Some(PaneId(2)));
        assert_eq!(session.active_window().layout.leaves(), leaves);
        assert_eq!(
            session
                .active_window()
                .pane_rects(Rect::new(10, 20, 300, 200), 1),
            vec![PaneRect {
                pane: PaneId(2),
                rect: Rect::new(10, 20, 300, 200),
                active: true,
            }]
        );

        assert_eq!(session.toggle_zoom(), None);
        assert_eq!(session.active_window().zoomed, None);
        assert_eq!(
            session
                .active_window()
                .pane_rects(Rect::new(0, 0, 90, 60), 1)
                .len(),
            3
        );
    }

    #[test]
    fn zoom_tracks_focus_changes_so_the_active_pane_stays_visible() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Horizontal);

        assert_eq!(session.toggle_zoom(), Some(PaneId(2)));
        session.previous_pane();
        assert_eq!(session.active_window().active, PaneId(1));
        assert_eq!(session.active_window().zoomed, Some(PaneId(1)));
        assert_eq!(
            session
                .active_window()
                .pane_rects(Rect::new(0, 0, 120, 80), 1),
            vec![PaneRect {
                pane: PaneId(1),
                rect: Rect::new(0, 0, 120, 80),
                active: true,
            }]
        );

        session.select_pane(PaneId(0)).expect("pane 0 exists");
        assert_eq!(session.active_window().zoomed, Some(PaneId(0)));
    }

    #[test]
    fn copy_mode_is_recorded_on_the_active_pane() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);

        assert_eq!(session.enter_copy_mode(), PaneId(1));
        assert_eq!(session.active_pane().mode, PaneMode::Copy);
        assert_eq!(session.active_pane().scrollback_offset, 0);

        assert_eq!(session.exit_copy_mode(), PaneId(1));
        assert_eq!(session.active_pane().mode, PaneMode::Live);
        assert_eq!(session.active_pane().scrollback_offset, 0);
    }

    #[test]
    fn copy_mode_scroll_offset_moves_with_saturation() {
        let mut session = Session::new("dev");

        assert_eq!(session.scroll_copy_mode(1, 10), None);
        session.enter_copy_mode();

        assert_eq!(session.scroll_copy_mode(3, 10), Some(3));
        assert_eq!(session.active_pane().scrollback_offset, 3);
        assert_eq!(session.scroll_copy_mode(99, 10), Some(10));
        assert_eq!(session.active_pane().scrollback_offset, 10);
        assert_eq!(session.scroll_copy_mode(-4, 10), Some(6));
        assert_eq!(session.scroll_copy_mode(-99, 10), Some(0));
        assert_eq!(session.active_pane().scrollback_offset, 0);
    }

    #[test]
    fn copy_mode_cursor_moves_inside_the_visible_viewport() {
        let mut session = Session::new("dev");

        assert_eq!(session.move_copy_cursor(0, 1, 10, 10), None);
        session.enter_copy_mode();

        assert_eq!(
            session.move_copy_cursor(2, 3, 10, 10),
            Some(CopyCursor { row: 2, col: 3 })
        );
        assert_eq!(
            session.move_copy_cursor(99, 99, 10, 10),
            Some(CopyCursor { row: 10, col: 10 })
        );
        assert_eq!(
            session.move_copy_cursor(-99, -4, 10, 10),
            Some(CopyCursor { row: 0, col: 6 })
        );

        session.exit_copy_mode();
        assert_eq!(session.active_pane().copy_cursor, CopyCursor::default());
    }

    #[test]
    fn copy_mode_cursor_jumps_to_line_edges() {
        let mut session = Session::new("dev");

        assert_eq!(session.set_copy_cursor_col(5, 10), None);
        session.enter_copy_mode();
        session.move_copy_cursor(0, 4, 10, 10);

        assert_eq!(
            session.set_copy_cursor_col(0, 10),
            Some(CopyCursor { row: 0, col: 0 })
        );
        assert_eq!(
            session.set_copy_cursor_col(usize::MAX, 12),
            Some(CopyCursor { row: 0, col: 12 })
        );
    }

    #[test]
    fn copy_mode_selection_toggles_from_the_current_cursor() {
        let mut session = Session::new("dev");

        assert_eq!(session.toggle_copy_selection(), None);
        session.enter_copy_mode();
        session.move_copy_cursor(2, 4, 10, 10);

        assert_eq!(
            session.toggle_copy_selection(),
            Some(Some(CopySelection {
                anchor: CopyCursor { row: 2, col: 4 },
            }))
        );
        session.move_copy_cursor(-1, -3, 10, 10);
        assert_eq!(
            session.active_pane().copy_selection_range(),
            Some((CopyCursor { row: 1, col: 1 }, CopyCursor { row: 2, col: 4 },))
        );

        assert_eq!(session.toggle_copy_selection(), Some(None));
        assert_eq!(session.active_pane().copy_selection_range(), None);
    }

    #[test]
    fn copy_mode_selection_clears_when_leaving_copy_mode() {
        let mut session = Session::new("dev");

        session.enter_copy_mode();
        session.toggle_copy_selection();
        assert!(session.active_pane().copy_selection.is_some());

        session.exit_copy_mode();
        assert_eq!(session.active_pane().mode, PaneMode::Live);
        assert_eq!(session.active_pane().copy_selection, None);
    }

    #[test]
    fn copy_mode_gg_and_g_capital_jump_to_scrollback_extents() {
        let mut session = Session::new("dev");
        assert_eq!(session.copy_top_of_scrollback(40), None);
        session.enter_copy_mode();

        assert_eq!(session.copy_top_of_scrollback(40), Some(40));
        assert_eq!(session.active_pane().scrollback_offset, 40);
        assert_eq!(session.copy_bottom_of_scrollback(), Some(0));
        assert_eq!(session.active_pane().scrollback_offset, 0);
    }

    #[test]
    fn copy_mode_word_motions_walk_to_the_next_and_previous_word() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();

        // "hello world cockpit"
        //  0     6     12
        assert_eq!(
            session.copy_word_forward("hello world cockpit", 80),
            Some(CopyCursor { row: 0, col: 6 })
        );
        assert_eq!(
            session.copy_word_forward("hello world cockpit", 80),
            Some(CopyCursor { row: 0, col: 12 })
        );
        // Saturates at line end (whitespace-tail clamps).
        assert_eq!(
            session.copy_word_forward("hello world cockpit", 80),
            Some(CopyCursor { row: 0, col: 19 })
        );

        assert_eq!(
            session.copy_word_backward("hello world cockpit"),
            Some(CopyCursor { row: 0, col: 12 })
        );
        assert_eq!(
            session.copy_word_backward("hello world cockpit"),
            Some(CopyCursor { row: 0, col: 6 })
        );
        assert_eq!(
            session.copy_word_backward("hello world cockpit"),
            Some(CopyCursor { row: 0, col: 0 })
        );
    }

    #[test]
    fn copy_mode_word_motions_clamp_to_max_col() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();

        assert_eq!(
            session.copy_word_forward("alpha beta gamma", 7),
            Some(CopyCursor { row: 0, col: 6 })
        );
        assert_eq!(
            session.copy_word_forward("alpha beta gamma", 7),
            Some(CopyCursor { row: 0, col: 7 })
        );
    }

    #[test]
    fn copy_mode_word_motions_no_op_outside_copy_mode() {
        let mut session = Session::new("dev");
        assert_eq!(session.copy_word_forward("hello world", 80), None);
        assert_eq!(session.copy_word_backward("hello world"), None);
    }

    #[test]
    fn copy_mode_selection_text_extracts_a_single_line() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.move_copy_cursor(0, 6, 5, 80);
        session.toggle_copy_selection();
        session.move_copy_cursor(0, 4, 5, 80);

        assert_eq!(
            session.copy_selection_text(&["hello world cockpit"]),
            Some("world".to_string())
        );
    }

    #[test]
    fn copy_mode_selection_text_spans_multiple_rows() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        // Anchor at (0, 2); selection cursor at (2, 1) — inclusive of both ends.
        session.move_copy_cursor(0, 2, 5, 80);
        session.toggle_copy_selection();
        session.move_copy_cursor(2, -1, 5, 80);

        let rows = ["abcdef", "ghijkl", "mnopqr", "stuvwx"];
        assert_eq!(
            session.copy_selection_text(&rows),
            Some("cdef\nghijkl\nmn".to_string())
        );
    }

    #[test]
    fn copy_mode_selection_text_returns_none_without_a_selection() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        assert_eq!(session.copy_selection_text(&["a", "b"]), None);
    }

    #[test]
    fn copy_mode_search_input_records_query_and_supports_backspace() {
        let mut session = Session::new("dev");
        assert!(!session.begin_copy_search());

        session.enter_copy_mode();
        assert!(session.begin_copy_search());
        assert_eq!(session.copy_search_query(), Some(""));

        assert_eq!(session.push_copy_search_char('f'), Some("f".to_string()));
        assert_eq!(session.push_copy_search_char('o'), Some("fo".to_string()));
        assert_eq!(session.push_copy_search_char('o'), Some("foo".to_string()));
        assert_eq!(session.copy_search_query(), Some("foo"));

        assert_eq!(session.pop_copy_search_char(), Some("fo".to_string()));
        assert_eq!(session.copy_search_query(), Some("fo"));
    }

    #[test]
    fn copy_mode_search_pop_clears_the_query_when_empty() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.begin_copy_search();
        session.push_copy_search_char('a');

        assert_eq!(session.pop_copy_search_char(), Some(String::new()));
        assert_eq!(session.copy_search_query(), None);
    }

    #[test]
    fn copy_mode_search_cancel_drops_the_pending_query() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.begin_copy_search();
        session.push_copy_search_char('x');

        assert!(session.cancel_copy_search());
        assert_eq!(session.copy_search_query(), None);
        assert!(!session.cancel_copy_search());
    }

    #[test]
    fn copy_mode_search_finishes_by_jumping_to_the_first_match() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.begin_copy_search();
        for ch in "world".chars() {
            session.push_copy_search_char(ch);
        }

        let rows = ["hello world", "cockpit world"];
        let hit = session.finish_copy_search(&rows);
        assert_eq!(hit, Some(CopyCursor { row: 0, col: 6 }));
        assert_eq!(
            session.active_pane().copy_cursor,
            CopyCursor { row: 0, col: 6 }
        );
        assert_eq!(session.copy_search_query(), None);
        assert_eq!(session.active_pane().last_search.as_deref(), Some("world"));
    }

    #[test]
    fn copy_mode_search_finish_leaves_cursor_when_no_match() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.move_copy_cursor(1, 2, 5, 10);
        session.begin_copy_search();
        for ch in "nope".chars() {
            session.push_copy_search_char(ch);
        }

        let rows = ["hello world", "cockpit world"];
        assert_eq!(session.finish_copy_search(&rows), None);
        assert_eq!(
            session.active_pane().copy_cursor,
            CopyCursor { row: 1, col: 2 }
        );
        assert_eq!(session.copy_search_query(), None);
        assert!(session.active_pane().last_search.is_none());
    }

    #[test]
    fn copy_mode_repeat_search_walks_to_the_next_match() {
        let mut session = Session::new("dev");
        session.enter_copy_mode();
        session.begin_copy_search();
        for ch in "world".chars() {
            session.push_copy_search_char(ch);
        }

        let rows = ["hello world", "cockpit world", "world again"];
        session.finish_copy_search(&rows);
        assert_eq!(
            session.active_pane().copy_cursor,
            CopyCursor { row: 0, col: 6 }
        );

        assert_eq!(
            session.repeat_copy_search_forward(&rows),
            Some(CopyCursor { row: 1, col: 8 })
        );
        assert_eq!(
            session.repeat_copy_search_forward(&rows),
            Some(CopyCursor { row: 2, col: 0 })
        );
        assert_eq!(session.repeat_copy_search_forward(&rows), None);
    }

    #[test]
    fn resize_pane_adjusts_the_nearest_parent_split() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);

        session
            .resize_pane(0.2)
            .expect("multi-pane resize succeeds");
        match &session.active_window().layout {
            LayoutNode::Split { ratio, .. } => assert_eq!(*ratio, 0.3),
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn select_layout_rewrites_the_tree_for_builtin_presets() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);
        session.select_layout(LayoutPreset::MainVertical);

        assert_eq!(
            session.active_window().layout.leaves(),
            vec![PaneId(0), PaneId(1), PaneId(2)]
        );
        insta::assert_snapshot!(json(&session), @r#"
        {
          "id": 1,
          "name": "dev",
          "windows": [
            {
              "id": 0,
              "name": "0",
              "layout": {
                "type": "split",
                "dir": "horizontal",
                "ratio": 0.6,
                "a": {
                  "type": "leaf",
                  "pane": 0
                },
                "b": {
                  "type": "split",
                  "dir": "vertical",
                  "ratio": 0.5,
                  "a": {
                    "type": "leaf",
                    "pane": 1
                  },
                  "b": {
                    "type": "leaf",
                    "pane": 2
                  }
                }
              },
              "active": 2,
              "layout_preset": "main-vertical",
              "panes": [
                {
                  "id": 0,
                  "scrollback_offset": 0,
                  "mode": "live"
                },
                {
                  "id": 1,
                  "scrollback_offset": 0,
                  "mode": "live"
                },
                {
                  "id": 2,
                  "scrollback_offset": 0,
                  "mode": "live"
                }
              ]
            }
          ],
          "active": 0,
          "next_window": 1,
          "next_pane": 3
        }
        "#);
    }

    #[test]
    fn from_layout_builds_a_single_pane_session() {
        let description = LayoutDescription::Pane {
            command: Some("cargo watch -x test".to_string()),
        };
        let (session, pane_commands) = Session::from_layout("dev", &description);

        assert_eq!(session.windows.len(), 1);
        assert_eq!(session.active_window().panes.len(), 1);
        assert_eq!(pane_commands.len(), 1);
        assert_eq!(pane_commands[0].1.as_deref(), Some("cargo watch -x test"));
        assert_eq!(session.active_pane().id, pane_commands[0].0);
    }

    #[test]
    fn from_layout_builds_nested_splits_in_leaf_order() {
        let description = LayoutDescription::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.6,
            a: Box::new(LayoutDescription::Pane {
                command: Some("editor".to_string()),
            }),
            b: Box::new(LayoutDescription::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                a: Box::new(LayoutDescription::Pane { command: None }),
                b: Box::new(LayoutDescription::Pane {
                    command: Some("lazygit".to_string()),
                }),
            }),
        };
        let (session, pane_commands) = Session::from_layout("dev", &description);

        assert_eq!(pane_commands.len(), 3);
        assert_eq!(
            pane_commands
                .iter()
                .map(|(_, cmd)| cmd.clone())
                .collect::<Vec<_>>(),
            vec![
                Some("editor".to_string()),
                None,
                Some("lazygit".to_string())
            ]
        );
        match &session.active_window().layout {
            LayoutNode::Split { dir, ratio, .. } => {
                assert_eq!(*dir, SplitDirection::Horizontal);
                assert!((ratio - 0.6).abs() < f32::EPSILON);
            }
            other => panic!("expected split root, got {other:?}"),
        }
        assert_eq!(session.active_pane().id, pane_commands[0].0);
    }

    #[test]
    fn status_summary_lists_every_window_with_active_marker() {
        let mut session = Session::new("dev");
        session.new_window("logs");
        session.new_window("vim");
        // Select the middle window.
        session.select_window(1).expect("window exists");

        let summary = session.status_summary();
        assert_eq!(summary.session_name, "dev");
        assert_eq!(summary.windows.len(), 3);
        assert_eq!(summary.windows[0].name, "0");
        assert!(!summary.windows[0].active);
        assert_eq!(summary.windows[1].name, "logs");
        assert!(summary.windows[1].active);
        assert_eq!(summary.windows[2].name, "vim");
        assert!(!summary.windows[2].active);
    }

    #[test]
    fn status_summary_tracks_panes_per_window() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);

        let summary = session.status_summary();
        assert_eq!(summary.windows.len(), 1);
        assert_eq!(summary.windows[0].pane_count, 3);
        assert_eq!(summary.windows[0].active_pane_in_window, 2);
        assert!(summary.windows[0].contains_active_pane);
    }

    #[test]
    fn status_summary_default_render_marks_active_window() {
        let mut session = Session::new("dev");
        session.new_window("logs");
        session.select_window(1).unwrap();

        let summary = session.status_summary();
        assert_eq!(summary.render(&[]), "[dev] 0:0 1:logs*");
    }

    #[test]
    fn status_summary_render_appends_extras_with_separators() {
        let session = Session::new("dev");
        let summary = session.status_summary();
        assert_eq!(
            summary.render(&["12:34", "build"]),
            "[dev] 0:0* │ 12:34 · build"
        );
        assert_eq!(summary.render(&["", "build", ""]), "[dev] 0:0* │ build");
    }

    #[test]
    fn session_registry_starts_attached_to_initial_session() {
        let registry = SessionRegistry::new(Session::new("dev"));
        assert!(registry.is_attached());
        assert_eq!(registry.sessions().len(), 1);
        assert_eq!(registry.active().name, "dev");
    }

    #[test]
    fn session_registry_create_activates_a_fresh_session() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        let first = registry.active_id();
        let second = registry.create("logs");

        assert_ne!(first, second);
        assert_eq!(registry.sessions().len(), 2);
        assert_eq!(registry.active().name, "logs");
        assert!(registry.is_attached());
    }

    #[test]
    fn session_registry_detach_keeps_session_running_and_flips_attached() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        registry.create("logs");

        let active = registry.active_id();
        assert_eq!(registry.detach(), active);
        assert!(!registry.is_attached());
        // Detached registry still holds every session.
        assert_eq!(registry.sessions().len(), 2);
    }

    #[test]
    fn session_registry_attach_reactivates_a_known_session() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        let dev = registry.active_id();
        let logs = registry.create("logs");
        registry.detach();

        assert_eq!(registry.attach(dev).unwrap(), dev);
        assert_eq!(registry.active_id(), dev);
        assert!(registry.is_attached());

        assert_eq!(registry.attach(logs).unwrap(), logs);
        assert_eq!(registry.active().name, "logs");

        let missing = SessionId(999);
        assert_eq!(
            registry.attach(missing).unwrap_err(),
            MuxError::UnknownSession(missing)
        );
    }

    #[test]
    fn session_registry_kill_refuses_to_empty_the_registry() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        let dev = registry.active_id();

        assert_eq!(
            registry.kill(dev).unwrap_err(),
            MuxError::CannotKillLastSession
        );
        assert_eq!(registry.sessions().len(), 1);
    }

    #[test]
    fn session_registry_create_allocates_pane_ids_in_a_disjoint_range() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        // First session was created with base 0 — pane and window both
        // start there.
        let first_pane = registry.active().active_pane().id;
        assert_eq!(first_pane, PaneId(0));

        registry.create("scratch");
        // Newly created session owns ids >= SESSION_ID_STRIDE.
        let scratch_pane = registry.active().active_pane().id;
        assert!(
            scratch_pane.get() >= SESSION_ID_STRIDE,
            "expected non-overlapping pane id, got {scratch_pane}"
        );

        // Splitting the new session continues to allocate inside its own
        // stride window.
        let split = registry
            .active_mut()
            .split_active(SplitDirection::Horizontal);
        assert!(split.get() >= SESSION_ID_STRIDE);
        assert!(split.get() < SESSION_ID_STRIDE * 3);
    }

    #[test]
    fn session_registry_kill_advances_active_to_the_next_session() {
        let mut registry = SessionRegistry::new(Session::new("dev"));
        let dev = registry.active_id();
        let logs = registry.create("logs");
        registry.create("vim");

        // Killing the middle session activates the next in order.
        assert_eq!(registry.kill(logs).unwrap(), logs);
        assert_eq!(registry.sessions().len(), 2);
        assert_eq!(registry.active().name, "vim");

        // Killing a non-active session leaves the active alone.
        registry.kill(dev).unwrap();
        assert_eq!(registry.sessions().len(), 1);
        assert_eq!(registry.active().name, "vim");
    }

    #[test]
    fn next_layout_cycles_builtin_presets() {
        let mut session = Session::new("dev");
        session.split_active(SplitDirection::Horizontal);
        session.split_active(SplitDirection::Vertical);

        assert_eq!(
            session.active_window().layout_preset,
            LayoutPreset::EvenHorizontal
        );
        assert_eq!(session.next_layout(), LayoutPreset::MainVertical);
        assert_eq!(
            session.active_window().layout_preset,
            LayoutPreset::MainVertical
        );
        assert_eq!(session.next_layout(), LayoutPreset::Tiled);
        assert_eq!(session.next_layout(), LayoutPreset::EvenHorizontal);
    }
}
