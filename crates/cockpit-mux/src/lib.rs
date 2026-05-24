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

/// One terminal pane. Real PTY handles live in `cockpit-terminal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub scrollback_offset: usize,
    pub mode: PaneMode,
}

impl Pane {
    fn new(id: PaneId) -> Self {
        Self {
            id,
            scrollback_offset: 0,
            mode: PaneMode::Live,
        }
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
            panes: vec![pane],
        }
    }

    /// Project the window layout into pane rectangles for the terminal area.
    pub fn pane_rects(&self, bounds: Rect, border_px: u32) -> Vec<PaneRect> {
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
        let mut ids = Ids::default();
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
    #[error("layout tree no longer references its pane set")]
    BrokenLayout,
}

#[derive(Default)]
struct Ids {
    next_window: u64,
    next_pane: u64,
}

impl Ids {
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

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pane-{}", self.0)
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
