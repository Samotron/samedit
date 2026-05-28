# Lua extensions

Cockpit ships with a sandboxed Lua 5.4 runtime (v0.9). Extensions are
plain `.lua` files that **register** commands, keybindings, themes,
tool-pane recipes, and event handlers. They run in isolated VMs and
cannot spawn processes, touch the filesystem, render pixels, or escape
the sandbox unless the user explicitly grants a capability.

There is **no plugin marketplace**, no registry, and no in-app
installer. Extensions are user-authored or user-copied local files
only.

---

## Where extensions live

```
~/.config/cockpit/extensions/*.lua                  # Linux
~/Library/Application Support/dev.CodingCockpit.cockpit/extensions/*.lua   # macOS
%APPDATA%\CodingCockpit\cockpit\config\extensions\*.lua                    # Windows
```

The file stem becomes the extension name (`hello.lua` → `hello`).
Subdirectories are not scanned recursively.

Three example extensions ship in-binary:

| Name                              | Demonstrates                            |
|-----------------------------------|-----------------------------------------|
| `runtime.format-paragraph`        | command registration + `ctx.toast`      |
| `runtime.session-toast`           | `cockpit.events.on("mux.pane_exit")`    |
| `runtime.theme-by-time-of-day`    | `cockpit.themes.register`               |

Disable them in `~/.config/cockpit/extensions.toml`:

```toml
[extensions."runtime.format-paragraph"]
enabled = false
```

---

## The `cockpit.*` API surface

A single global, organised by namespace. Everything is **registration
+ read-only inspection** — there are no mutation primitives that
escape the registered command system.

### `cockpit.commands.register { id, title, run }`

Register a palette command. `id` becomes a [`CommandId`] in
`cockpit-commands`; `run` is called with a `ctx` table whenever the
command fires (palette dispatch, keybinding, or another extension's
`cockpit.commands.dispatch`).

```lua
cockpit.commands.register {
  id    = "user.format-paragraph",
  title = "Editor: Format Paragraph",
  run   = function(ctx)
    ctx.toast("formatting " .. (ctx.path or "no document"))
  end,
}
```

Available on `ctx`:

| Field           | Type     | Notes                                  |
|-----------------|----------|----------------------------------------|
| `command`       | string   | The dispatched command id.             |
| `path`          | string?  | Active editor path, if any.            |
| `project_root`  | string?  | Detected project root.                 |
| `project_name`  | string?  | Display name.                          |
| `toast(msg)`    | function | Show `msg` on the status line.         |
| `dispatch(id)`  | function | Re-enter the command dispatcher.       |

### `cockpit.commands.dispatch(id)`

Run a command by id. Useful for chaining built-in commands from
extensions (e.g. `cockpit.commands.dispatch("editor.save")`).

### `cockpit.keys.bind(chord, id)`

Bind a chord (e.g. `"Ctrl+Shift+P"`, `"<leader>fp"`) to a command id.
Substitution of `<leader>` follows the user's `keys.global.leader`
config.

### `cockpit.themes.register { name, colors }`

Register a named colour palette. `colors` is a table of hex strings.
Recognised keys: `background`, `pane_background`, `pane_border`,
`text`, `muted_text`, `accent`, `selection`, `cursor`,
`diagnostic_error`, `diagnostic_warning`, `diagnostic_info`,
`diagnostic_hint`. Unknown keys are kept but ignored by the renderer.

### `cockpit.panes.recipe { name, command, layout, toggle, keybind, detect }`

Same schema as `[panes.tools.<name>]` in `config.toml` (v0.8 M8.2).
`layout` is one of `floating | side-right | bottom`. The recipe
appears as `tool.<name>` in the palette.

### `cockpit.events.on(event, fn)`

Listen for a cockpit event. Returns an opaque numeric handle.

| Event              | Fired when                              | `ctx` fields                                |
|--------------------|-----------------------------------------|---------------------------------------------|
| `editor.open`      | A buffer becomes active                 | `path, language`                            |
| `editor.save`      | Save succeeded                          | `path, language, bytes`                     |
| `editor.cursor`    | Cursor moved (debounced 50 ms)          | `path, line, col`                           |
| `editor.mode`      | Vim mode changed                        | `path, mode` (`normal|insert|visual|...`)   |
| `mux.pane_focus`   | Active mux pane changed                 | `session, window, pane, command`            |
| `mux.pane_exit`    | Pane's process exited                   | `session, pane, exit_code`                  |
| `palette.open`     | Command palette opened                  | `query`                                     |
| `project.open`     | Project finished hydrating              | `root, name`                                |

Handlers run synchronously on the UI thread with a **5 ms** budget per
event. Overrunning handlers are killed and the extension surfaces a
one-line error in the status line. After **3** strikes the handler is
disabled until reload (`Debug: Reload Extensions`).

### `cockpit.toast(msg)`

Status-line notification.

### `cockpit.log.{info,warn,error}(msg)`

Lands in the `tracing` log under the extension's name.

### `print(…)`

Captured to an in-runtime buffer (debug only) and to `tracing` at
`DEBUG` level. The default Lua stdout is disabled.

---

## Capabilities

Default-deny. Extensions declare what they need; the user grants in
config.

```lua
--[[ @cockpit:requires fs.read.project, process ]]--

-- Without the grant, calls into the corresponding namespaces raise a
-- Lua error you can `pcall` around.
```

```toml
# ~/.config/cockpit/extensions.toml
[extensions."user.rust-toys"]
enabled = true
grants  = ["fs.read.project"]
```

Capability tokens recognised today:

- `fs.read.project` — read files inside the project root.
- `process` — spawn declared commands via the `ProcessRunner` seam.
- `clipboard.read` — read the OS clipboard.
- `clipboard.write` — write to the OS clipboard.

> **v0.9 note:** the declaration + grant machinery is in place, but
> the corresponding `cockpit.fs.*`, `cockpit.process.*`, and
> `cockpit.clipboard.*` namespaces arrive alongside their dedicated
> M9.4 follow-ups. Declaring a capability today is forward-compatible
> — once the namespace ships, the same script works without changes.

---

## Sandbox

The sandbox is layered on top of Lua 5.4 (`mlua` with the `lua54` +
`vendored` features). The following stdlib surfaces are stripped:

- `io` (every function)
- `dofile`, `loadfile`, `load`, `loadstring`
- `require`
- `collectgarbage`, `debug`, `package`
- `os.execute`, `os.exit`, `os.getenv`, `os.remove`, `os.rename`,
  `os.setlocale`, `os.tmpname`

What stays: `math`, `string`, `table`, `utf8`, plus the time-/date-only
helpers on `os` (`os.time`, `os.date`, `os.clock`, `os.difftime`).

Each extension runs in its **own** `mlua::Lua` VM, so a crash in one
extension never trashes another. Errors are logged to `tracing` and
surfaced on the status line; the cockpit never exits because of a bad
extension.

---

## Hot-reload

Saving an extension file reloads its VM — no restart, no other
extension affected. Cockpit watches the extensions directory via
`notify` and wakes the event loop when a `.lua` file changes. Manual
reload is available via the `Debug: Reload Extensions` palette
command.

`Debug: Show Extensions` summarises load state, registration counts,
and the last error per extension.

---

## What's intentionally absent

- No plugin marketplace, registry, or in-app installer.
- No coroutine scheduler exposed to user code; handlers run
  synchronously.
- No reflection on other extensions' state.
- No direct access to: filesystem (outside the gated namespace),
  process spawn (outside the gated namespace), PTY input/output,
  termwiz grid, GL painter, or `winit` callbacks.
- No network IO.
- No clipboard write without `clipboard.write`.

These are absent on purpose. The API surface is small and curated by
design (`AGENTS.md` §2 rule #7).
