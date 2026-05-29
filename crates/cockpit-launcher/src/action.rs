//! The action model — what a launcher row *is* and what hitting Enter *does*.
//!
//! An [`Action`] is the unit a provider emits and the launcher ranks. Its
//! [`ActionRun`] is the typed dispatch payload. Crucially every dispatch path
//! either *is* a [`CommandId`] or is mapped onto one by the binary
//! (AGENTS §2 #5: commands are the single execution spine), so the launcher
//! and the in-cockpit palette never grow divergent ways to "do a thing".

use std::path::PathBuf;

use cockpit_commands::CommandId;
use cockpit_project::env::ProcessSpec;

/// A single launcher entry. Providers emit these; the launcher ranks and
/// merges them. Pure data — no behaviour, so it stays trivially testable and
/// (later) serialisable for the IPC hop to `cockpit-quick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Stable id, unique within a provider (e.g. `mise:my-app:test`).
    pub id: String,
    /// Primary label shown in the row and matched against the query.
    pub title: String,
    /// Optional secondary line (the command, a hint, a path).
    pub subtitle: Option<String>,
    /// Semantic icon tag. The headless core never renders; the shell maps
    /// this onto a glyph.
    pub icon: ActionIcon,
    /// What pressing Enter does.
    pub run: ActionRun,
}

impl Action {
    /// Convenience constructor for the common `(id, title, run)` shape.
    pub fn new(id: impl Into<String>, title: impl Into<String>, run: ActionRun) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            icon: ActionIcon::Generic,
            run,
        }
    }

    /// Builder: attach a subtitle.
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Builder: set the icon tag.
    pub fn with_icon(mut self, icon: ActionIcon) -> Self {
        self.icon = icon;
        self
    }

    /// The text the matcher scores the query against. Today that's the title;
    /// kept as a method so subtitle/keyword scoring can be folded in without
    /// touching the launcher.
    pub fn haystack(&self) -> &str {
        &self.title
    }
}

/// Semantic icon tag. Deliberately a small closed set — new tags are a plan
/// change, mirroring the no-marketplace discipline on the provider trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionIcon {
    /// No specific icon.
    Generic,
    /// A runnable task (mise).
    Task,
    /// A project / workspace.
    Project,
    /// A web URL.
    Url,
    /// A calculator result.
    Calculator,
    /// A theme switch.
    Theme,
    /// An Org capture / agenda action.
    Org,
    /// A file or path.
    File,
    /// A user-authored Lua action.
    Lua,
}

/// One argument to a [`CommandId`] dispatch. Kept tiny on purpose — the
/// launcher carries values, the binary interprets them per command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionArg {
    /// A free-text argument.
    Str(String),
    /// A filesystem path argument.
    Path(PathBuf),
}

impl ActionArg {
    /// Build a string argument.
    pub fn str(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }
}

/// The typed dispatch payload behind an [`Action`].
///
/// Every variant resolves to a `cockpit-commands` dispatch in the end: the
/// open/process variants are conveniences the binary lowers onto the matching
/// built-in command, so there is exactly one execution spine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRun {
    /// Dispatch a registered command with arguments (the canonical path).
    Command(CommandId, Vec<ActionArg>),
    /// Spawn a process (mise tasks). Carries the fully-built spec, including
    /// the project root as the working directory.
    Process(ProcessSpec),
    /// Open a URL in the user's browser.
    OpenUrl(String),
    /// Open a path (file or project) in the cockpit.
    OpenPath(PathBuf),
    /// Invoke a registered Lua launcher action.
    Lua(LuaActionHandle),
}

/// Opaque handle to a Lua-registered launcher action (M13.3). The launcher
/// core only needs to route to it; the Lua runtime owns the closure keyed by
/// `(extension, id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaActionHandle {
    /// Extension file stem that registered the action.
    pub extension: String,
    /// Action id as declared in `cockpit.launcher.action { id = … }`.
    pub id: String,
}

impl LuaActionHandle {
    /// Construct a handle.
    pub fn new(extension: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            extension: extension.into(),
            id: id.into(),
        }
    }
}
