//! Events that extensions can listen to.
//!
//! The set is fixed by the plan (§M9.3). Adding to it is a plan
//! change, not a runtime change — the surface stays auditable.

use std::path::PathBuf;

use mlua::{Lua, Table};

/// Discriminator for an event kind. Used as a map key when storing
/// per-event handler slots and as the string the Lua `on(name, fn)`
/// call accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKind {
    EditorOpen,
    EditorSave,
    EditorCursor,
    EditorMode,
    MuxPaneFocus,
    MuxPaneExit,
    PaletteOpen,
    ProjectOpen,
}

impl EventKind {
    /// Token used by Lua `cockpit.events.on(<name>, fn)`.
    pub fn name(self) -> &'static str {
        match self {
            Self::EditorOpen => "editor.open",
            Self::EditorSave => "editor.save",
            Self::EditorCursor => "editor.cursor",
            Self::EditorMode => "editor.mode",
            Self::MuxPaneFocus => "mux.pane_focus",
            Self::MuxPaneExit => "mux.pane_exit",
            Self::PaletteOpen => "palette.open",
            Self::ProjectOpen => "project.open",
        }
    }

    /// Parse a Lua-side event name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "editor.open" => Some(Self::EditorOpen),
            "editor.save" => Some(Self::EditorSave),
            "editor.cursor" => Some(Self::EditorCursor),
            "editor.mode" => Some(Self::EditorMode),
            "mux.pane_focus" => Some(Self::MuxPaneFocus),
            "mux.pane_exit" => Some(Self::MuxPaneExit),
            "palette.open" => Some(Self::PaletteOpen),
            "project.open" => Some(Self::ProjectOpen),
            _ => None,
        }
    }

    /// Every kind in declaration order (used by `Debug: Extensions`).
    pub fn all() -> &'static [EventKind] {
        &[
            Self::EditorOpen,
            Self::EditorSave,
            Self::EditorCursor,
            Self::EditorMode,
            Self::MuxPaneFocus,
            Self::MuxPaneExit,
            Self::PaletteOpen,
            Self::ProjectOpen,
        ]
    }
}

/// Typed event payload. Each variant carries exactly the fields listed
/// in §M9.3.
#[derive(Debug, Clone)]
pub enum Event {
    EditorOpen {
        path: PathBuf,
        language: String,
    },
    EditorSave {
        path: PathBuf,
        language: String,
        bytes: usize,
    },
    EditorCursor {
        path: PathBuf,
        line: u32,
        col: u32,
    },
    EditorMode {
        path: PathBuf,
        mode: String,
    },
    MuxPaneFocus {
        session: String,
        window: u32,
        pane: u32,
        command: String,
    },
    MuxPaneExit {
        session: String,
        pane: u32,
        exit_code: i32,
    },
    PaletteOpen {
        query: String,
    },
    ProjectOpen {
        root: PathBuf,
        name: String,
    },
}

impl Event {
    /// Event discriminator.
    pub fn kind(&self) -> EventKind {
        match self {
            Self::EditorOpen { .. } => EventKind::EditorOpen,
            Self::EditorSave { .. } => EventKind::EditorSave,
            Self::EditorCursor { .. } => EventKind::EditorCursor,
            Self::EditorMode { .. } => EventKind::EditorMode,
            Self::MuxPaneFocus { .. } => EventKind::MuxPaneFocus,
            Self::MuxPaneExit { .. } => EventKind::MuxPaneExit,
            Self::PaletteOpen { .. } => EventKind::PaletteOpen,
            Self::ProjectOpen { .. } => EventKind::ProjectOpen,
        }
    }

    /// Materialise this event as a Lua table the handler receives as
    /// `ctx`. Caller is responsible for passing the table into the
    /// callback.
    pub fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        table.set("event", self.kind().name())?;
        match self {
            Self::EditorOpen { path, language } => {
                table.set("path", path.to_string_lossy().to_string())?;
                table.set("language", language.clone())?;
            }
            Self::EditorSave {
                path,
                language,
                bytes,
            } => {
                table.set("path", path.to_string_lossy().to_string())?;
                table.set("language", language.clone())?;
                table.set("bytes", *bytes)?;
            }
            Self::EditorCursor { path, line, col } => {
                table.set("path", path.to_string_lossy().to_string())?;
                table.set("line", *line)?;
                table.set("col", *col)?;
            }
            Self::EditorMode { path, mode } => {
                table.set("path", path.to_string_lossy().to_string())?;
                table.set("mode", mode.clone())?;
            }
            Self::MuxPaneFocus {
                session,
                window,
                pane,
                command,
            } => {
                table.set("session", session.clone())?;
                table.set("window", *window)?;
                table.set("pane", *pane)?;
                table.set("command", command.clone())?;
            }
            Self::MuxPaneExit {
                session,
                pane,
                exit_code,
            } => {
                table.set("session", session.clone())?;
                table.set("pane", *pane)?;
                table.set("exit_code", *exit_code)?;
            }
            Self::PaletteOpen { query } => {
                table.set("query", query.clone())?;
            }
            Self::ProjectOpen { root, name } => {
                table.set("root", root.to_string_lossy().to_string())?;
                table.set("name", name.clone())?;
            }
        }
        Ok(table)
    }
}

/// Context passed to a registered command callback.
///
/// Currently carries the command id plus a `toast` method binding back
/// to the [`SharedState`](crate::SharedState). The struct stays plain
/// so the binary can populate it without touching Lua types.
#[derive(Debug, Clone, Default)]
pub struct EventContext {
    pub command: Option<String>,
    pub active_path: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub project_name: Option<String>,
}

impl EventContext {
    /// Context for a synthetic command dispatch.
    pub fn for_command(command: impl Into<String>) -> Self {
        Self {
            command: Some(command.into()),
            ..Self::default()
        }
    }

    /// Builder: attach the currently active editor path.
    pub fn with_active_path(mut self, path: PathBuf) -> Self {
        self.active_path = Some(path);
        self
    }

    /// Builder: attach project metadata.
    pub fn with_project(mut self, root: PathBuf, name: String) -> Self {
        self.project_root = Some(root);
        self.project_name = Some(name);
        self
    }

    /// Render the context as the Lua table a command callback receives.
    pub fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        if let Some(cmd) = &self.command {
            table.set("command", cmd.clone())?;
        }
        if let Some(path) = &self.active_path {
            table.set("path", path.to_string_lossy().to_string())?;
        }
        if let Some(root) = &self.project_root {
            table.set("project_root", root.to_string_lossy().to_string())?;
        }
        if let Some(name) = &self.project_name {
            table.set("project_name", name.clone())?;
        }
        Ok(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_round_trip() {
        for kind in EventKind::all() {
            let name = kind.name();
            assert_eq!(EventKind::parse(name), Some(*kind), "round-trip {name}");
        }
    }

    #[test]
    fn unknown_event_name_is_none() {
        assert_eq!(EventKind::parse("editor.exploded"), None);
    }

    #[test]
    fn editor_save_event_carries_bytes_field() {
        let lua = Lua::new();
        let event = Event::EditorSave {
            path: "/p/main.rs".into(),
            language: "rust".into(),
            bytes: 42,
        };
        let table = event.to_lua_table(&lua).unwrap();
        let bytes: usize = table.get("bytes").unwrap();
        let lang: String = table.get("language").unwrap();
        assert_eq!(bytes, 42);
        assert_eq!(lang, "rust");
    }

    #[test]
    fn event_context_renders_active_path() {
        let lua = Lua::new();
        let ctx = EventContext::for_command("user.save").with_active_path("/p/x.rs".into());
        let table = ctx.to_lua_table(&lua).unwrap();
        let cmd: String = table.get("command").unwrap();
        let path: String = table.get("path").unwrap();
        assert_eq!(cmd, "user.save");
        assert_eq!(path, "/p/x.rs");
    }
}
