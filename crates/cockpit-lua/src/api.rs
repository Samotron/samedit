//! Bridge that exposes the `cockpit.*` Lua surface on top of a fresh
//! sandboxed VM.
//!
//! The bridge is a *registrar*, not an executor. Extensions call
//! `cockpit.commands.register{…}` / `cockpit.keys.bind(…)` / etc.; we
//! record those registrations in [`SharedState`] and the cockpit binary
//! drains them after the script runs (M9.2).
//!
//! Capability gating lives next to each protected call: if the
//! extension didn't declare and the user didn't grant the corresponding
//! capability, the call raises a Lua-side error the script can `pcall`
//! around.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use cockpit_commands::{CommandId, KeyChord};
use mlua::{Function, Lua, Table, Value};

use crate::capability::Capability;
use crate::event::EventKind;
use crate::theme::ExtensionTheme;
use crate::{
    EventHandlerSlot, RegisteredCommand, RegisteredKeybind, RegisteredRecipe, SharedState,
};

/// Strip the Lua 5.4 standard library of escape hatches. Lua 5.4 has no
/// equivalent of Luau's `Lua::sandbox`, so the only way to deny
/// `os.execute`, `io.popen`, `loadfile`, `dofile`, `package.loadlib`,
/// and the `require` resolver is to remove them explicitly.
///
/// What stays: pure-data libraries (`math`, `string`, `table`, `utf8`)
/// and a trimmed `os` / `io` exposing only time/date and string-write
/// helpers. What goes: anything that can spawn a process, touch the
/// filesystem, or load shared libraries.
pub(crate) fn apply_sandbox(lua: &Lua) -> mlua::Result<()> {
    // Drop the whole `io` library — extensions get capability-gated
    // filesystem access via dedicated namespaces (M9.4), not raw
    // file handles.
    lua.globals().set("io", Value::Nil)?;
    lua.globals().set("dofile", Value::Nil)?;
    lua.globals().set("loadfile", Value::Nil)?;
    lua.globals().set("load", Value::Nil)?;
    lua.globals().set("loadstring", Value::Nil)?;
    lua.globals().set("require", Value::Nil)?;
    lua.globals().set("collectgarbage", Value::Nil)?;
    lua.globals().set("debug", Value::Nil)?;
    lua.globals().set("package", Value::Nil)?;

    // Strip `os` of process spawn / filesystem / env / shell exit.
    let globals = lua.globals();
    if let Ok(os_table) = globals.get::<Table>("os") {
        for forbidden in [
            "execute",
            "exit",
            "getenv",
            "remove",
            "rename",
            "setlocale",
            "tmpname",
        ] {
            os_table.set(forbidden, Value::Nil)?;
        }
    }
    Ok(())
}

/// Install the `cockpit` global table and its sub-tables onto `lua`.
pub(crate) fn install_cockpit_global(
    lua: &Lua,
    extension_name: &str,
    state: &Arc<Mutex<SharedState>>,
) -> mlua::Result<()> {
    let cockpit = lua.create_table()?;

    install_commands(lua, &cockpit, extension_name, state)?;
    install_keys(lua, &cockpit, state)?;
    install_themes(lua, &cockpit, state)?;
    install_panes(lua, &cockpit, state)?;
    install_events(lua, &cockpit, state)?;
    install_toast_and_log(lua, &cockpit, extension_name, state)?;

    lua.globals().set("cockpit", cockpit)?;
    Ok(())
}

fn install_commands(
    lua: &Lua,
    cockpit: &Table,
    extension_name: &str,
    state: &Arc<Mutex<SharedState>>,
) -> mlua::Result<()> {
    let commands = lua.create_table()?;
    let ext_name = extension_name.to_string();

    let register_state = Arc::clone(state);
    let register = lua.create_function(move |lua, spec: Table| {
        let id: String = spec.get("id")?;
        let title: String = spec.get("title").unwrap_or_else(|_| id.clone());
        let run: Function = spec.get("run").map_err(|_| {
            mlua::Error::external("cockpit.commands.register requires a `run = function(ctx)`")
        })?;
        let id = id.trim();
        if id.is_empty() {
            return Err(mlua::Error::external("command id must not be empty"));
        }
        // Stash the function in the Lua registry under a stable key so
        // the runtime can fish it out on dispatch without holding a
        // borrowed reference.
        let key = format!("__cockpit_command__{id}");
        lua.set_named_registry_value(&key, run)?;
        let cmd_id = CommandId::from(id);

        let mut state = register_state.lock().map_err(lock_poisoned)?;
        state
            .registrations
            .commands
            .push(RegisteredCommand { id: cmd_id, title });
        Ok(())
    })?;
    commands.set("register", register)?;

    let dispatch_state = Arc::clone(state);
    let dispatch = lua.create_function(move |_lua, args: mlua::Variadic<Value>| {
        let id_value = args
            .iter()
            .next()
            .ok_or_else(|| mlua::Error::external("cockpit.commands.dispatch needs an id"))?;
        let id: String = match id_value {
            Value::String(s) => s.to_str()?.to_string(),
            _ => {
                return Err(mlua::Error::external(
                    "cockpit.commands.dispatch id must be a string",
                ));
            }
        };
        let mut state = dispatch_state.lock().map_err(lock_poisoned)?;
        state.effects.dispatch.push(crate::DispatchRequest {
            command: CommandId::from(id),
        });
        Ok(())
    })?;
    commands.set("dispatch", dispatch)?;
    let _ = ext_name; // reserved for future per-extension command namespacing
    cockpit.set("commands", commands)?;
    Ok(())
}

fn install_keys(lua: &Lua, cockpit: &Table, state: &Arc<Mutex<SharedState>>) -> mlua::Result<()> {
    let keys = lua.create_table()?;
    let bind_state = Arc::clone(state);
    let bind = lua.create_function(move |_lua, (chord, id): (String, String)| {
        let parsed = KeyChord::from_str(&chord).map_err(|err| {
            mlua::Error::external(format!("cockpit.keys.bind: invalid chord `{chord}`: {err}"))
        })?;
        let id = id.trim();
        if id.is_empty() {
            return Err(mlua::Error::external("cockpit.keys.bind: empty command id"));
        }
        let mut state = bind_state.lock().map_err(lock_poisoned)?;
        state.registrations.keybinds.push(RegisteredKeybind {
            chord: parsed,
            command: CommandId::from(id),
        });
        Ok(())
    })?;
    keys.set("bind", bind)?;
    cockpit.set("keys", keys)?;
    Ok(())
}

fn install_themes(lua: &Lua, cockpit: &Table, state: &Arc<Mutex<SharedState>>) -> mlua::Result<()> {
    let themes = lua.create_table()?;
    let register_state = Arc::clone(state);
    let register = lua.create_function(move |_lua, spec: Table| {
        let name: String = spec.get("name")?;
        let colors_value: Value = spec.get("colors").unwrap_or(Value::Nil);
        let mut colors: BTreeMap<String, String> = BTreeMap::new();
        if let Value::Table(table) = colors_value {
            for pair in table.pairs::<String, String>() {
                let (key, value) = pair?;
                colors.insert(key, value);
            }
        }
        let mut state = register_state.lock().map_err(lock_poisoned)?;
        state
            .registrations
            .themes
            .push(ExtensionTheme { name, colors });
        Ok(())
    })?;
    themes.set("register", register)?;
    cockpit.set("themes", themes)?;
    Ok(())
}

fn install_panes(lua: &Lua, cockpit: &Table, state: &Arc<Mutex<SharedState>>) -> mlua::Result<()> {
    let panes = lua.create_table()?;
    let recipe_state = Arc::clone(state);
    let recipe = lua.create_function(move |_lua, spec: Table| {
        let name: String = spec.get("name")?;
        let command: String = spec.get("command")?;
        let layout_name: String = spec
            .get::<String>("layout")
            .unwrap_or_else(|_| "floating".to_string());
        let layout = match layout_name.to_ascii_lowercase().as_str() {
            "floating" => cockpit_config::ToolPaneLayout::Floating,
            "side-right" | "side_right" => cockpit_config::ToolPaneLayout::SideRight,
            "bottom" => cockpit_config::ToolPaneLayout::Bottom,
            other => {
                return Err(mlua::Error::external(format!(
                    "cockpit.panes.recipe: unknown layout `{other}` (floating, side-right, bottom)"
                )));
            }
        };
        let toggle: bool = spec.get("toggle").unwrap_or(true);
        let keybind: String = spec.get("keybind").unwrap_or_default();
        let detect: String = spec.get("detect").unwrap_or_default();

        let entry = cockpit_config::ToolPaneRecipe {
            command,
            layout,
            toggle,
            keybind,
            detect,
        };
        let mut state = recipe_state.lock().map_err(lock_poisoned)?;
        state.registrations.recipes.push(RegisteredRecipe {
            name,
            recipe: entry,
        });
        Ok(())
    })?;
    panes.set("recipe", recipe)?;
    cockpit.set("panes", panes)?;
    Ok(())
}

fn install_events(lua: &Lua, cockpit: &Table, state: &Arc<Mutex<SharedState>>) -> mlua::Result<()> {
    let events = lua.create_table()?;
    let on_state = Arc::clone(state);
    let on = lua.create_function(move |lua, (event_name, handler): (String, Function)| {
        let kind = EventKind::parse(&event_name).ok_or_else(|| {
            mlua::Error::external(format!(
                "cockpit.events.on: unknown event `{event_name}`. \
                 Known: editor.open, editor.save, editor.cursor, editor.mode, \
                 mux.pane_focus, mux.pane_exit, palette.open, project.open"
            ))
        })?;
        let key = lua.create_registry_value(handler)?;

        let mut state = on_state.lock().map_err(lock_poisoned)?;
        let id = state.next_handler_id;
        state.next_handler_id = state.next_handler_id.saturating_add(1);
        let slots = state.event_handlers.entry(kind).or_default();
        slots.push(EventHandlerSlot {
            id,
            registry_key: key,
            strikes: 0,
            disabled: false,
        });
        *state.registrations.handler_counts.entry(kind).or_default() += 1;
        Ok(id)
    })?;
    events.set("on", on)?;
    cockpit.set("events", events)?;
    Ok(())
}

fn install_toast_and_log(
    lua: &Lua,
    cockpit: &Table,
    extension_name: &str,
    state: &Arc<Mutex<SharedState>>,
) -> mlua::Result<()> {
    let ext_for_toast = extension_name.to_string();
    let toast_state = Arc::clone(state);
    let toast = lua.create_function(move |_lua, msg: String| {
        let mut state = toast_state.lock().map_err(lock_poisoned)?;
        state.effects.toasts.push(msg);
        let _ = ext_for_toast.clone();
        Ok(())
    })?;
    cockpit.set("toast", toast)?;

    let log = lua.create_table()?;
    let ext_info = extension_name.to_string();
    let info = lua.create_function(move |_lua, msg: String| {
        tracing::info!(extension = %ext_info, "{msg}");
        Ok(())
    })?;
    log.set("info", info)?;

    let ext_warn = extension_name.to_string();
    let warn = lua.create_function(move |_lua, msg: String| {
        tracing::warn!(extension = %ext_warn, "{msg}");
        Ok(())
    })?;
    log.set("warn", warn)?;

    let ext_err = extension_name.to_string();
    let err = lua.create_function(move |_lua, msg: String| {
        tracing::error!(extension = %ext_err, "{msg}");
        Ok(())
    })?;
    log.set("error", err)?;
    cockpit.set("log", log)?;

    // print → toast capture (the sandbox normally points print at the
    // default Lua stdout, which we don't want users to see in a GUI).
    let ext_print = extension_name.to_string();
    let capture_state = Arc::clone(state);
    let print = lua.create_function(move |_lua, args: mlua::Variadic<Value>| {
        let mut buf = String::new();
        for (i, v) in args.iter().enumerate() {
            if i > 0 {
                buf.push('\t');
            }
            match v {
                Value::String(s) => buf.push_str(&s.to_string_lossy()),
                Value::Integer(i) => buf.push_str(&i.to_string()),
                Value::Number(f) => buf.push_str(&f.to_string()),
                Value::Boolean(b) => buf.push_str(if *b { "true" } else { "false" }),
                Value::Nil => buf.push_str("nil"),
                other => buf.push_str(&format!("{other:?}")),
            }
        }
        if let Ok(mut state) = capture_state.lock() {
            state.print_capture.push_str(&buf);
            state.print_capture.push('\n');
        }
        tracing::debug!(extension = %ext_print, "lua print: {buf}");
        Ok(())
    })?;
    lua.globals().set("print", print)?;
    Ok(())
}

/// Helper: surface a Mutex poison as a Lua error.
fn lock_poisoned<T>(_err: std::sync::PoisonError<T>) -> mlua::Error {
    mlua::Error::external("cockpit Lua state mutex poisoned")
}

/// Capability check helper for future protected APIs (M9.4 `fs`,
/// `process`, `clipboard.*`). Returns an `mlua::Error` when `cap` is
/// not in `caps` — extensions can `pcall` around the failure and
/// degrade.
#[allow(dead_code)]
pub(crate) fn require_capability(
    caps: &crate::CapabilitySet,
    cap: Capability,
    where_: &str,
) -> mlua::Result<()> {
    if caps.contains(&cap) {
        Ok(())
    } else {
        Err(mlua::Error::external(format!(
            "{where_} requires capability `{cap}` — declare it in the \
             `--[[ @cockpit:requires {cap} ]]--` header and grant it in \
             extensions.toml"
        )))
    }
}
