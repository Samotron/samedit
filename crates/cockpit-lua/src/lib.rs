//! `cockpit-lua` — sandboxed Lua extension runtime (v0.9).
//!
//! Power users author extensions in Lua. Each extension lives in its own
//! [`mlua::Lua`] VM (`Lua::sandbox(true)` plus the extra restrictions in
//! [`api`]). Extensions register commands, keybindings, themes, tool-pane
//! recipes, and event handlers via a single `cockpit` global. They cannot
//! spawn processes, touch the filesystem, render pixels, or escape the
//! sandbox unless the user explicitly grants a capability (M9.4).
//!
//! This crate is **headless** — no `winit`, `glow`, or PTY dependency.
//! The cockpit binary wires the runtime into the existing
//! `cockpit-commands` registry and `AppModel` so extensions land as plain
//! command ids, keybindings, themes, and recipes (AGENTS §2 hard rule #5:
//! commands are the single spine).

pub mod api;
pub mod capability;
pub mod event;
pub mod http_scripts;
pub mod theme;
pub mod watcher;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_commands::{CommandId, KeyChord};
use cockpit_project::env::Clock;
use mlua::Lua;
use thiserror::Error;

pub use capability::{Capability, CapabilityError, CapabilitySet};
pub use event::{Event, EventContext, EventKind};
pub use http_scripts::{HttpScriptError, ScriptResponseView, run_post_response, run_pre_request};
pub use theme::ExtensionTheme;
pub use watcher::ExtensionWatcher;

/// Per-event handler budget — overruns disable the handler until reload.
/// Spec value from §M9.3.
pub const EVENT_BUDGET: Duration = Duration::from_millis(5);

/// Hard wall on per-handler-overruns. Three strikes and the handler is
/// disabled until the extension is reloaded.
pub const EVENT_BUDGET_STRIKES: u32 = 3;

/// Top-level errors raised by the runtime.
#[derive(Debug, Error)]
pub enum LuaError {
    /// Failed to read the extension file from disk.
    #[error("failed to read extension `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The Lua VM rejected the script (syntax error, runtime panic, etc.).
    #[error("extension `{name}` failed: {source}")]
    Script {
        name: String,
        #[source]
        source: mlua::Error,
    },
    /// Sandbox setup failed before user code ran.
    #[error("sandbox setup failed for `{name}`: {source}")]
    Sandbox {
        name: String,
        #[source]
        source: mlua::Error,
    },
    /// An extension declared a capability the user has not granted.
    #[error("extension `{name}` is missing required capability `{capability}`")]
    MissingCapability { name: String, capability: String },
}

/// Side-effects an extension callback can emit back into the cockpit.
/// The runtime returns these instead of touching real state directly so
/// the binary stays in control of dispatch ordering.
#[derive(Debug, Clone, Default)]
pub struct LuaEffects {
    /// Status-line toasts surfaced by `cockpit.toast` or sandbox errors.
    pub toasts: Vec<String>,
    /// Commands an extension asked to dispatch via `cockpit.commands.dispatch`.
    pub dispatch: Vec<DispatchRequest>,
}

impl LuaEffects {
    /// Merge `other` into `self` in place — order is preserved.
    pub fn extend(&mut self, other: LuaEffects) {
        self.toasts.extend(other.toasts);
        self.dispatch.extend(other.dispatch);
    }
}

/// One `cockpit.commands.dispatch(id, args?)` request from Lua.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    pub command: CommandId,
}

/// One palette command an extension asked the runtime to register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCommand {
    pub id: CommandId,
    pub title: String,
}

/// One keybinding registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredKeybind {
    pub chord: KeyChord,
    pub command: CommandId,
}

/// One tool-pane recipe registration (mirrors v0.8's TOML schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredRecipe {
    pub name: String,
    pub recipe: cockpit_config::ToolPaneRecipe,
}

/// What an extension's top-level register code produced. The binary
/// drains this after a successful [`Extension::load`] and applies the
/// individual registrations to the relevant subsystems.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Registrations {
    pub commands: Vec<RegisteredCommand>,
    pub keybinds: Vec<RegisteredKeybind>,
    pub themes: Vec<ExtensionTheme>,
    pub recipes: Vec<RegisteredRecipe>,
    /// Number of event handlers the extension installed, keyed by event.
    pub handler_counts: BTreeMap<EventKind, u32>,
}

/// Shared state between Rust and Lua. The Lua bridge mutates this inside
/// callbacks; the runtime drains it back into [`LuaEffects`] /
/// [`Registrations`] each turn.
#[derive(Debug, Default)]
pub(crate) struct SharedState {
    pub registrations: Registrations,
    pub effects: LuaEffects,
    /// Numeric ids returned to Lua handlers — index into `event_handlers`.
    pub event_handlers: BTreeMap<EventKind, Vec<EventHandlerSlot>>,
    pub next_handler_id: u64,
    /// Capability grant set for the active extension. Reserved for the
    /// upcoming capability-gated APIs (M9.4 `fs.read.project`, `process`,
    /// `clipboard.*`); populated at load time so the gated namespaces
    /// can read it without re-parsing the header.
    #[allow(dead_code)]
    pub capabilities: CapabilitySet,
    /// `print(…)` output captured during script execution (tests, debug).
    pub print_capture: String,
}

#[derive(Debug)]
pub(crate) struct EventHandlerSlot {
    pub id: u64,
    pub registry_key: mlua::RegistryKey,
    pub strikes: u32,
    pub disabled: bool,
}

/// One loaded extension — one Lua VM, one declared capability set, the
/// registrations it produced, and an event-handler table.
pub struct Extension {
    name: String,
    source_path: PathBuf,
    lua: Lua,
    declared_capabilities: CapabilitySet,
    state: Arc<Mutex<SharedState>>,
    last_error: Option<String>,
}

impl Extension {
    /// Extension name (used as the registration key + capability namespace).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Source path on disk.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Snapshot the registrations produced at load time.
    pub fn registrations(&self) -> Registrations {
        self.state
            .lock()
            .map(|s| s.registrations.clone())
            .unwrap_or_default()
    }

    /// Drain any side-effects accumulated since the last call. Always
    /// returns the buffered toasts and dispatch requests, in registration
    /// order.
    pub fn take_effects(&mut self) -> LuaEffects {
        let Ok(mut state) = self.state.lock() else {
            return LuaEffects::default();
        };
        std::mem::take(&mut state.effects)
    }

    /// Most recent error, if any. Cleared on every successful tick.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// True when every event handler the extension installed has been
    /// disabled by repeated budget overruns.
    pub fn all_handlers_disabled(&self) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        if state.event_handlers.is_empty() {
            return false;
        }
        state
            .event_handlers
            .values()
            .all(|slots| slots.iter().all(|slot| slot.disabled))
    }

    /// Granted capability set.
    pub fn capabilities(&self) -> CapabilitySet {
        self.declared_capabilities.clone()
    }
}

/// Header parser for `--[[ @cockpit:requires X, Y ]]--` declarations.
fn parse_required_capabilities(source: &str) -> Result<CapabilitySet, CapabilityError> {
    capability::parse_requires_header(source)
}

/// Decorate a context table with the per-invocation helpers — `toast`
/// and `dispatch` both route through the shared state, so handlers can
/// write `ctx.toast("hi")` exactly like the plan's worked examples.
fn attach_ctx_helpers(
    lua: &mlua::Lua,
    table: &mlua::Table,
    state: &Arc<Mutex<SharedState>>,
) -> mlua::Result<()> {
    let toast_state = Arc::clone(state);
    let toast = lua.create_function(move |_, msg: String| {
        if let Ok(mut s) = toast_state.lock() {
            s.effects.toasts.push(msg);
        }
        Ok(())
    })?;
    table.set("toast", toast)?;

    let dispatch_state = Arc::clone(state);
    let dispatch = lua.create_function(move |_, id: String| {
        if let Ok(mut s) = dispatch_state.lock() {
            s.effects.dispatch.push(DispatchRequest {
                command: CommandId::from(id.as_str()),
            });
        }
        Ok(())
    })?;
    table.set("dispatch", dispatch)?;
    Ok(())
}

/// Driver that owns every loaded extension. Loaded once per cockpit
/// process; the binary calls into it via the methods on this type.
pub struct LuaRuntime {
    extensions: BTreeMap<String, Extension>,
    /// User-granted capabilities, keyed by extension name. Loaded from
    /// `extensions.toml` (M9.4); only entries that match a declared
    /// capability actually take effect.
    grants: BTreeMap<String, CapabilitySet>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for LuaRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaRuntime")
            .field("extensions", &self.extensions.keys().collect::<Vec<_>>())
            .field("grants", &self.grants)
            .finish()
    }
}

impl LuaRuntime {
    /// Empty runtime with the std clock.
    pub fn new() -> Self {
        Self::with_clock(Arc::new(cockpit_project::env::StdClock))
    }

    /// Empty runtime with an injected clock (M9.3 budget enforcement, tests).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            extensions: BTreeMap::new(),
            grants: BTreeMap::new(),
            clock,
        }
    }

    /// Record a user grant. Extensions that declare matching capabilities
    /// get them at load time. Repeated calls overwrite the prior grant
    /// for the same extension.
    pub fn grant(&mut self, extension: impl Into<String>, capabilities: CapabilitySet) {
        self.grants.insert(extension.into(), capabilities);
    }

    /// Load an extension from a file on disk. The extension name is the
    /// file stem (`hello.lua` → `hello`).
    pub fn load_path(&mut self, path: impl AsRef<Path>) -> Result<(), LuaError> {
        let path = path.as_ref().to_path_buf();
        let source = std::fs::read_to_string(&path).map_err(|source| LuaError::Read {
            path: path.clone(),
            source,
        })?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("extension")
            .to_string();
        self.load_source(name, path, source)
    }

    /// Load an extension from in-memory source (the embedded defaults in
    /// M9.8 use this).
    pub fn load_source(
        &mut self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        source: impl Into<String>,
    ) -> Result<(), LuaError> {
        let name = name.into();
        let path = path.into();
        let source = source.into();

        let declared = parse_required_capabilities(&source).map_err(|err| LuaError::Script {
            name: name.clone(),
            source: mlua::Error::external(err),
        })?;
        let granted = self.grants.get(&name).cloned().unwrap_or_default();
        let effective = declared.intersect(&granted);

        let lua = Lua::new();
        api::apply_sandbox(&lua).map_err(|source| LuaError::Sandbox {
            name: name.clone(),
            source,
        })?;

        let state = Arc::new(Mutex::new(SharedState {
            capabilities: effective.clone(),
            ..SharedState::default()
        }));

        api::install_cockpit_global(&lua, &name, &state).map_err(|source| LuaError::Sandbox {
            name: name.clone(),
            source,
        })?;

        // Execute the top-level register block. Errors carry the original
        // script trace from `mlua`.
        lua.load(&source)
            .set_name(name.clone())
            .exec()
            .map_err(|source| LuaError::Script {
                name: name.clone(),
                source,
            })?;

        let extension = Extension {
            name: name.clone(),
            source_path: path,
            lua,
            declared_capabilities: declared,
            state,
            last_error: None,
        };
        self.extensions.insert(name, extension);
        Ok(())
    }

    /// Reload an extension from its source path, tearing down its VM
    /// first (M9.5).
    pub fn reload(&mut self, name: &str) -> Result<(), LuaError> {
        let Some(path) = self.extensions.get(name).map(|e| e.source_path.clone()) else {
            return Err(LuaError::Read {
                path: PathBuf::from(name),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("extension `{name}` is not loaded"),
                ),
            });
        };
        self.extensions.remove(name);
        self.load_path(path)
    }

    /// Remove an extension, freeing its VM.
    pub fn unload(&mut self, name: &str) -> bool {
        self.extensions.remove(name).is_some()
    }

    /// Drain side-effects for every extension. The binary calls this
    /// once per tick / dispatch to surface toasts and process Lua-issued
    /// dispatch requests.
    pub fn take_effects(&mut self) -> LuaEffects {
        let mut combined = LuaEffects::default();
        for ext in self.extensions.values_mut() {
            combined.extend(ext.take_effects());
        }
        combined
    }

    /// Sum the registrations across every extension.
    pub fn registrations(&self) -> Registrations {
        let mut sum = Registrations::default();
        for ext in self.extensions.values() {
            let reg = ext.registrations();
            sum.commands.extend(reg.commands);
            sum.keybinds.extend(reg.keybinds);
            sum.themes.extend(reg.themes);
            sum.recipes.extend(reg.recipes);
            for (k, v) in reg.handler_counts {
                *sum.handler_counts.entry(k).or_default() += v;
            }
        }
        sum
    }

    /// Fire `event` across every extension that registered a handler for
    /// it. Handlers are dispatched in registration order. Each handler
    /// runs synchronously with the [`EVENT_BUDGET`] cap — overruns push
    /// a strike, [`EVENT_BUDGET_STRIKES`] strikes disable the handler.
    pub fn fire_event(&mut self, event: &Event) {
        let kind = event.kind();
        for ext in self.extensions.values_mut() {
            ext.fire_event(kind, event, &*self.clock);
        }
    }

    /// Dispatch a Lua-registered command by id. Returns `true` if some
    /// extension owns the id and the handler ran (possibly with error,
    /// surfaced via toasts), `false` if no extension owns it.
    pub fn dispatch_command(&mut self, id: &CommandId, ctx: EventContext) -> bool {
        let mut handled = false;
        for ext in self.extensions.values_mut() {
            if ext.dispatch_command(id, &ctx) {
                handled = true;
            }
        }
        handled
    }

    /// Snapshot for the `Debug: Extensions` palette command (M9.5).
    pub fn debug_summary(&self) -> Vec<ExtensionSummary> {
        self.extensions
            .values()
            .map(|ext| ExtensionSummary {
                name: ext.name.clone(),
                path: ext.source_path.clone(),
                disabled: ext.all_handlers_disabled(),
                last_error: ext.last_error.clone(),
                registrations: ext.registrations(),
                declared_capabilities: ext.declared_capabilities.clone(),
            })
            .collect()
    }

    /// Iterate loaded extension names in alphabetical order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.extensions.keys().map(String::as_str)
    }
}

impl Default for LuaRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// One row in the `Debug: Extensions` view (M9.5).
#[derive(Debug, Clone)]
pub struct ExtensionSummary {
    pub name: String,
    pub path: PathBuf,
    pub disabled: bool,
    pub last_error: Option<String>,
    pub registrations: Registrations,
    pub declared_capabilities: CapabilitySet,
}

impl Extension {
    fn fire_event(&mut self, kind: EventKind, event: &Event, clock: &dyn Clock) {
        let slot_ids: Vec<(usize, u64)> = {
            let Ok(state) = self.state.lock() else {
                return;
            };
            let Some(slots) = state.event_handlers.get(&kind) else {
                return;
            };
            slots
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.disabled)
                .map(|(i, s)| (i, s.id))
                .collect()
        };

        for (index, _id) in slot_ids {
            let key = {
                let Ok(state) = self.state.lock() else {
                    return;
                };
                let Some(slots) = state.event_handlers.get(&kind) else {
                    return;
                };
                let Some(slot) = slots.get(index) else {
                    continue;
                };
                // Borrow only the registry key — `mlua::RegistryKey` is the
                // canonical handle to a Lua-side callable. We can't move
                // it out of the slot (the slot owns it), so look it up by
                // reference each iteration.
                self.lua
                    .registry_value::<mlua::Function>(&slot.registry_key)
            };

            let func = match key {
                Ok(func) => func,
                Err(err) => {
                    self.last_error = Some(format!("event {kind:?}: {err}"));
                    continue;
                }
            };

            let started = clock.now();
            let table_result = event.to_lua_table(&self.lua).and_then(|table| {
                attach_ctx_helpers(&self.lua, &table, &self.state).map(|_| table)
            });
            let call_result = match table_result {
                Ok(table) => func.call::<()>(table),
                Err(err) => Err(err),
            };
            let elapsed = clock.now().saturating_duration_since(started);

            match call_result {
                Ok(()) => {
                    self.last_error = None;
                }
                Err(err) => {
                    let msg = format!("event {kind:?}: {err}");
                    self.push_toast(format!("Lua: {msg}"));
                    self.last_error = Some(msg);
                }
            }

            if elapsed > EVENT_BUDGET {
                self.record_strike(kind, index, elapsed);
            }
        }
    }

    fn dispatch_command(&mut self, id: &CommandId, ctx: &EventContext) -> bool {
        let key_name = format!("__cockpit_command__{}", id.as_str());
        let func: Option<mlua::Function> = self
            .lua
            .named_registry_value(&key_name)
            .ok()
            .filter(|v: &mlua::Value| v.is_function())
            .and_then(|v: mlua::Value| v.as_function().cloned());
        let Some(func) = func else {
            return false;
        };
        let table_result = ctx
            .to_lua_table(&self.lua)
            .and_then(|table| attach_ctx_helpers(&self.lua, &table, &self.state).map(|_| table));
        let result = match table_result {
            Ok(table) => func.call::<()>(table),
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            let msg = format!("command {id}: {err}");
            self.push_toast(format!("Lua: {msg}"));
            self.last_error = Some(msg);
        } else {
            self.last_error = None;
        }
        true
    }

    fn push_toast(&self, msg: String) {
        if let Ok(mut state) = self.state.lock() {
            state.effects.toasts.push(msg);
        }
    }

    fn record_strike(&mut self, kind: EventKind, index: usize, elapsed: Duration) {
        let mut disabled_now = false;
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if let Some(slots) = state.event_handlers.get_mut(&kind)
                && let Some(slot) = slots.get_mut(index)
            {
                slot.strikes += 1;
                if slot.strikes >= EVENT_BUDGET_STRIKES {
                    slot.disabled = true;
                    disabled_now = true;
                }
            }
        }
        let ms = elapsed.as_secs_f32() * 1000.0;
        if disabled_now {
            self.push_toast(format!(
                "Lua: {} {:?} handler exceeded {} ms budget ({ms:.1} ms) — disabled.",
                self.name,
                kind,
                EVENT_BUDGET.as_millis()
            ));
            self.last_error = Some(format!("{kind:?} budget exceeded ({ms:.1} ms)"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::env::FakeClock;

    fn runtime() -> LuaRuntime {
        LuaRuntime::with_clock(Arc::new(FakeClock::new()))
    }

    #[test]
    fn empty_runtime_starts_with_no_extensions() {
        let rt = runtime();
        assert!(rt.names().next().is_none());
        assert_eq!(rt.registrations(), Registrations::default());
    }

    #[test]
    fn load_source_runs_top_level_register_block() {
        let mut rt = runtime();
        rt.load_source(
            "hello",
            "<test>",
            r#"
            cockpit.commands.register{
              id = "user.hello",
              title = "User: Hello",
              run = function(ctx) ctx.toast("hi") end,
            }
            "#,
        )
        .unwrap();
        let reg = rt.registrations();
        assert_eq!(reg.commands.len(), 1);
        assert_eq!(reg.commands[0].id.as_str(), "user.hello");
        assert_eq!(reg.commands[0].title, "User: Hello");
    }

    #[test]
    fn sandbox_blocks_os_execute() {
        let mut rt = runtime();
        let err = rt
            .load_source("bad", "<test>", r#"os.execute("rm -rf /")"#)
            .unwrap_err();
        assert!(
            matches!(err, LuaError::Script { .. }),
            "expected script err, got: {err:?}",
        );
    }

    #[test]
    fn capability_without_grant_is_inert() {
        let mut rt = runtime();
        rt.load_source(
            "needs-process",
            "<test>",
            r#"
            --[[ @cockpit:requires process ]]--
            cockpit.commands.register{
              id = "user.needs",
              title = "Needs process",
              run = function(ctx) end,
            }
            "#,
        )
        .unwrap();
        let summary = rt.debug_summary();
        assert_eq!(summary.len(), 1);
        assert!(
            summary[0]
                .declared_capabilities
                .contains(&Capability::Process),
            "expected declared process capability",
        );
    }

    #[test]
    fn dispatch_user_command_runs_handler() {
        let mut rt = runtime();
        rt.load_source(
            "echo",
            "<test>",
            r#"
            cockpit.commands.register{
              id = "user.echo",
              title = "User: Echo",
              run = function(ctx) ctx.toast("called " .. ctx.command) end,
            }
            "#,
        )
        .unwrap();
        let handled = rt.dispatch_command(
            &CommandId::from("user.echo"),
            EventContext::for_command("user.echo"),
        );
        assert!(handled);
        let effects = rt.take_effects();
        assert_eq!(effects.toasts, vec!["called user.echo".to_string()]);
    }

    #[test]
    fn dispatch_unknown_command_returns_false() {
        let mut rt = runtime();
        let handled = rt.dispatch_command(
            &CommandId::from("user.unknown"),
            EventContext::for_command("user.unknown"),
        );
        assert!(!handled);
    }

    #[test]
    fn event_fires_registered_handler() {
        let mut rt = runtime();
        rt.load_source(
            "log-saves",
            "<test>",
            r#"
            cockpit.events.on("editor.save", function(ctx)
              cockpit.toast("saved " .. ctx.path)
            end)
            "#,
        )
        .unwrap();
        rt.fire_event(&Event::EditorSave {
            path: "/p/foo.rs".into(),
            language: "rust".into(),
            bytes: 12,
        });
        let effects = rt.take_effects();
        assert_eq!(effects.toasts, vec!["saved /p/foo.rs".to_string()]);
    }

    /// Clock that auto-advances by `step` every time `now()` is read.
    /// Used to simulate "the handler took N ms" without relying on real
    /// wall-clock timing inside fast tests.
    #[derive(Debug)]
    struct StepClock {
        cursor: Mutex<std::time::Instant>,
        step: Duration,
    }

    impl StepClock {
        fn new(step: Duration) -> Self {
            Self {
                cursor: Mutex::new(std::time::Instant::now()),
                step,
            }
        }
    }

    impl Clock for StepClock {
        fn now(&self) -> std::time::Instant {
            let mut cursor = self.cursor.lock().unwrap();
            let value = *cursor;
            *cursor = value + self.step;
            value
        }
    }

    #[test]
    fn event_handler_overrun_eventually_disables() {
        let clock = Arc::new(StepClock::new(Duration::from_millis(10)));
        let mut rt = LuaRuntime::with_clock(clock);
        rt.load_source(
            "slow",
            "<test>",
            r#"
            cockpit.events.on("editor.save", function(ctx) end)
            "#,
        )
        .unwrap();
        for _ in 0..EVENT_BUDGET_STRIKES {
            rt.fire_event(&Event::EditorSave {
                path: "/p/foo.rs".into(),
                language: "rust".into(),
                bytes: 1,
            });
        }
        let summary = rt.debug_summary();
        assert_eq!(summary.len(), 1);
        assert!(summary[0].disabled, "handler should be disabled");
    }

    #[test]
    fn reload_swaps_extension_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.lua");
        std::fs::write(
            &path,
            r#"
            cockpit.commands.register{
              id = "user.hello",
              title = "Hello v1",
              run = function() end,
            }
            "#,
        )
        .unwrap();
        let mut rt = runtime();
        rt.load_path(&path).unwrap();
        assert_eq!(rt.registrations().commands[0].title, "Hello v1");

        std::fs::write(
            &path,
            r#"
            cockpit.commands.register{
              id = "user.hello",
              title = "Hello v2",
              run = function() end,
            }
            "#,
        )
        .unwrap();
        rt.reload("hello").unwrap();
        assert_eq!(rt.registrations().commands[0].title, "Hello v2");
    }
}
