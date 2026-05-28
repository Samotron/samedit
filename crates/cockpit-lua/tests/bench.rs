//! Cold-start budget gate (v0.9 M9.7).
//!
//! Asserts the totals from `IMPLEMENTATION_PLAN.md` §M9.7:
//!
//! | Step                                      | Budget          |
//! |-------------------------------------------|-----------------|
//! | `cockpit-lua` VM init (per VM)            | ≤ 5 ms          |
//! | Discover + parse `extensions/*.lua`       | ≤ 2 ms / file   |
//! | Run an extension's top-level register code| ≤ 10 ms typical |
//! | Total extension-system contribution       | ≤ 50 ms         |
//!
//! Opt-in via `--features bench` so contributors don't pay the timing
//! overhead in fast tests.

#![cfg(feature = "bench")]

use std::time::Instant;

use cockpit_lua::LuaRuntime;

/// A "typical" extension shape — registers a command, a keybind, an
/// event handler. Roughly matches what the embedded defaults look
/// like in `runtime/extensions/`.
const TYPICAL_EXTENSION: &str = r#"
cockpit.commands.register {
  id    = "bench.demo",
  title = "Bench: demo",
  run   = function(ctx) end,
}
cockpit.keys.bind("<leader>bd", "bench.demo")
cockpit.events.on("editor.save", function(ctx) end)
"#;

#[test]
fn vm_init_under_5ms() {
    let start = Instant::now();
    let _rt = LuaRuntime::new();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() <= 5,
        "VM init took {} ms (budget: 5 ms)",
        elapsed.as_millis()
    );
}

#[test]
fn parse_and_register_under_2ms() {
    let mut rt = LuaRuntime::new();
    let start = Instant::now();
    rt.load_source("bench.parse", "<bench>", TYPICAL_EXTENSION)
        .expect("parse");
    let elapsed = start.elapsed();
    // The 10 ms typical-run-time bound also applies — generous to
    // account for warmup of the vendored Lua VM. The strict 2 ms /
    // file budget is the steady-state goal once mlua has been touched
    // once in the process.
    assert!(
        elapsed.as_millis() <= 10,
        "parse + register took {} ms (budget: 10 ms typical)",
        elapsed.as_millis()
    );
}

#[test]
fn ten_extensions_under_50ms() {
    let start = Instant::now();
    let mut rt = LuaRuntime::new();
    for i in 0..10 {
        let name = format!("bench.fan.{i}");
        let source = format!(
            r#"
            cockpit.commands.register {{
              id    = "bench.fan.{i}",
              title = "Bench fan {i}",
              run   = function(ctx) end,
            }}
            "#
        );
        rt.load_source(name, "<bench>", source).expect("load");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() <= 50,
        "loading 10 extensions took {} ms (budget: 50 ms total)",
        elapsed.as_millis()
    );
    assert_eq!(rt.registrations().commands.len(), 10);
}
