//! Lua bridge for `.bru` request scripts (v0.11 M11.6.1).
//!
//! Bruno's JS pre/post-request scripts are intentionally not supported —
//! cockpit substitutes a tiny, sandboxed Lua surface. The script blocks
//! that ship in this milestone:
//!
//! ```lua
//! -- script:lua-pre-request
//! cockpit.http.set_var("token", "abc")
//!
//! -- script:lua-post-response
//! local r = cockpit.http.response()
//! if r.status >= 400 then error("api error: " .. r.status) end
//! ```
//!
//! Scripts run on a fresh Lua VM with the same sandbox the v0.9
//! extensions get ([`apply_sandbox`]); they cannot reach the
//! filesystem, spawn processes, or persist state across runs.
//! Variable mutations land back on the caller's [`BTreeMap`] via an
//! `Arc<Mutex<…>>` shared between the binding closure and the host —
//! that's the only way Lua handles can hand state out of the VM.
//!
//! The capability gate (`http.scripts`, M11.6 default-deny) lives in
//! the caller — the binary checks the user's grant set before invoking
//! anything here, so this module is dumb-but-safe: it only knows how
//! to run scripts, never whether it's *allowed* to.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mlua::{Function, Lua, Value};
use thiserror::Error;

use crate::api::apply_sandbox;

/// Minimal read-only response shape exposed to `script:lua-post-response`.
/// Kept here (not in `cockpit-http`) so this crate doesn't take a
/// cross-dep — the binary converts a real [`cockpit_http::Response`]
/// into this shape before calling [`run_post_response`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptResponseView {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Failure modes for [`run_pre_request`] and [`run_post_response`].
#[derive(Debug, Error)]
pub enum HttpScriptError {
    /// Script source failed to evaluate. Carries the raw Lua message —
    /// the view-model surfaces it in the status line.
    #[error("script raised an error: {0}")]
    Script(String),
    /// VM construction or sandboxing failed. Should never happen in
    /// practice; isolated from `Script` so the caller can distinguish
    /// "user script bug" from "cockpit bug".
    #[error("lua VM initialisation failed: {0}")]
    Vm(String),
}

impl From<mlua::Error> for HttpScriptError {
    fn from(err: mlua::Error) -> Self {
        Self::Script(err.to_string())
    }
}

/// Run a `script:lua-pre-request` block. Mutations made via
/// `cockpit.http.set_var(name, value)` are written back to `env` in
/// place; the source script sees the current env via
/// `cockpit.http.var(name)`.
///
/// The script runs to completion — there is no timeout or instruction
/// limit yet. Bruno collections are user-authored, so a runaway script
/// is a user bug; if that becomes a problem in practice, M11.6.2 can
/// add `mlua::Lua::set_interrupt`.
pub fn run_pre_request(
    source: &str,
    env: &mut BTreeMap<String, String>,
) -> Result<(), HttpScriptError> {
    let shared = Arc::new(Mutex::new(std::mem::take(env)));
    let lua = build_vm()?;
    install_http_globals(&lua, &shared, None)?;
    lua.load(source).exec()?;
    *env = std::mem::take(&mut *shared.lock().expect("http script env mutex"));
    Ok(())
}

/// Run a `script:lua-post-response` block. The script sees the
/// just-received response via `cockpit.http.response()` (read-only) and
/// can still mutate `env` via `cockpit.http.set_var` — useful for
/// stashing a token returned by the previous request for the next one
/// to pick up.
pub fn run_post_response(
    source: &str,
    response: &ScriptResponseView,
    env: &mut BTreeMap<String, String>,
) -> Result<(), HttpScriptError> {
    let shared = Arc::new(Mutex::new(std::mem::take(env)));
    let lua = build_vm()?;
    install_http_globals(&lua, &shared, Some(response))?;
    lua.load(source).exec()?;
    *env = std::mem::take(&mut *shared.lock().expect("http script env mutex"));
    Ok(())
}

fn build_vm() -> Result<Lua, HttpScriptError> {
    let lua = Lua::new();
    apply_sandbox(&lua).map_err(|err| HttpScriptError::Vm(err.to_string()))?;
    Ok(lua)
}

fn install_http_globals(
    lua: &Lua,
    env: &Arc<Mutex<BTreeMap<String, String>>>,
    response: Option<&ScriptResponseView>,
) -> mlua::Result<()> {
    let cockpit = lua.create_table()?;
    let http = lua.create_table()?;

    // cockpit.http.set_var(name, value): mutate the active env.
    let env_for_set = Arc::clone(env);
    let set_var: Function = lua.create_function(move |_, (name, value): (String, String)| {
        env_for_set
            .lock()
            .expect("http script env mutex")
            .insert(name, value);
        Ok(())
    })?;
    http.set("set_var", set_var)?;

    // cockpit.http.var(name): read from the active env. Returns nil
    // for unknown keys (the Lua-idiomatic way to signal absence).
    let env_for_get = Arc::clone(env);
    let var: Function = lua.create_function(move |_, name: String| {
        let guard = env_for_get.lock().expect("http script env mutex");
        Ok(guard.get(&name).cloned())
    })?;
    http.set("var", var)?;

    if let Some(response) = response {
        // cockpit.http.response(): read-only snapshot of the last
        // engine round-trip. We re-build the table on each call so a
        // script can't mutate one returned value and corrupt another
        // script's view (defensive even though both scripts share a
        // single VM today).
        let snapshot = response.clone();
        let response_fn: Function = lua.create_function(move |lua, _: ()| {
            let table = lua.create_table()?;
            table.set("status", snapshot.status)?;
            let headers = lua.create_table()?;
            for (i, (name, value)) in snapshot.headers.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("name", name.as_str())?;
                row.set("value", value.as_str())?;
                headers.set(i + 1, row)?;
            }
            table.set("headers", headers)?;
            table.set("body", String::from_utf8_lossy(&snapshot.body).into_owned())?;
            Ok(table)
        })?;
        http.set("response", response_fn)?;
    } else {
        // Pre-request scripts have no response yet — calling
        // `cockpit.http.response()` here raises a clear error rather
        // than returning nil that the user might then index into.
        let response_fn: Function = lua.create_function(|_, _: ()| {
            Err::<Value, _>(mlua::Error::RuntimeError(
                "cockpit.http.response() is only available in script:lua-post-response".into(),
            ))
        })?;
        http.set("response", response_fn)?;
    }

    cockpit.set("http", http)?;
    lua.globals().set("cockpit", cockpit)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn pre_request_script_can_set_a_variable() {
        let mut e = env(&[]);
        run_pre_request("cockpit.http.set_var('token', 'abc')", &mut e).unwrap();
        assert_eq!(e.get("token").map(String::as_str), Some("abc"));
    }

    #[test]
    fn pre_request_script_can_read_existing_variables() {
        let mut e = env(&[("base", "https://api.test")]);
        run_pre_request(
            "cockpit.http.set_var('derived', cockpit.http.var('base') .. '/v1')",
            &mut e,
        )
        .unwrap();
        assert_eq!(
            e.get("derived").map(String::as_str),
            Some("https://api.test/v1")
        );
    }

    #[test]
    fn pre_request_var_returns_nil_for_unknown_keys() {
        let mut e = env(&[]);
        run_pre_request(
            "assert(cockpit.http.var('missing') == nil, 'should be nil')",
            &mut e,
        )
        .unwrap();
    }

    #[test]
    fn pre_request_response_call_errors() {
        let mut e = env(&[]);
        let err = run_pre_request("return cockpit.http.response()", &mut e).unwrap_err();
        let HttpScriptError::Script(msg) = err else {
            panic!("expected Script error, got {err:?}");
        };
        assert!(msg.contains("only available"), "{msg}");
    }

    #[test]
    fn pre_request_script_error_surfaces_as_script_variant() {
        let mut e = env(&[]);
        let err = run_pre_request("error('boom')", &mut e).unwrap_err();
        let HttpScriptError::Script(msg) = err else {
            panic!("expected Script error, got {err:?}");
        };
        assert!(msg.contains("boom"), "{msg}");
    }

    #[test]
    fn post_response_script_sees_status_and_body() {
        let mut e = env(&[]);
        let response = ScriptResponseView {
            status: 201,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: br#"{"id":42}"#.to_vec(),
        };
        run_post_response(
            "local r = cockpit.http.response(); \
             cockpit.http.set_var('status', tostring(r.status)); \
             cockpit.http.set_var('body', r.body)",
            &response,
            &mut e,
        )
        .unwrap();
        assert_eq!(e.get("status").map(String::as_str), Some("201"));
        assert_eq!(e.get("body").map(String::as_str), Some("{\"id\":42}"));
    }

    #[test]
    fn post_response_can_iterate_headers() {
        let mut e = env(&[]);
        let response = ScriptResponseView {
            status: 200,
            headers: vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())],
            body: Vec::new(),
        };
        run_post_response(
            "local h = cockpit.http.response().headers; \
             cockpit.http.set_var('count', tostring(#h)); \
             cockpit.http.set_var('first', h[1].name)",
            &response,
            &mut e,
        )
        .unwrap();
        assert_eq!(e.get("count").map(String::as_str), Some("2"));
        assert_eq!(e.get("first").map(String::as_str), Some("X-A"));
    }

    #[test]
    fn sandbox_blocks_io_and_process_calls() {
        let mut e = env(&[]);
        // `os.execute` is stripped by apply_sandbox; calling it raises
        // "attempt to call a nil value".
        let err = run_pre_request("os.execute('echo pwned')", &mut e).unwrap_err();
        let HttpScriptError::Script(msg) = err else {
            panic!("expected Script error, got {err:?}");
        };
        assert!(msg.contains("nil value"), "{msg}");
    }
}
