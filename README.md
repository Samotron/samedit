# Coding Cockpit

A fast, native, multi-platform coding cockpit written in Rust: a project
launcher, a file browser, a Vim-style editor, and an integrated terminal
running a per-project Zellij workspace.

See [`spec.md`](spec.md) for the product specification and
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) for the build plan.

## Status

v0.1 in progress. The coding-cockpit shell runs end to end: a three-pane
workspace with a lazy file browser, a Vim-style editor (open / edit / save), a
command palette, and an integrated terminal pane backed by a real PTY. Open a
bundled fixture with `mise run run-fixture -- mise-basic`, or a real project
with `mise run run -- --project <path>`.

## Layout

This is a Cargo workspace. Only `cockpit-render` depends on the windowing/GPU
stack; every other crate builds and tests headless.

| Crate              | Responsibility                                  |
|--------------------|-------------------------------------------------|
| `cockpit`          | Binary: app shell and wiring                    |
| `cockpit-config`   | Typed configuration (TOML/KDL)                  |
| `cockpit-commands` | Command registry and keybinding resolution      |
| `cockpit-project`  | Project detection, mise integration, file tree  |
| `cockpit-editor`   | Rope buffer, cursor, undo, Vim state machine    |
| `cockpit-terminal` | PTY, terminal engine, Zellij, path detection    |
| `cockpit-ui`       | View-model tree, layout, panes                  |
| `cockpit-render`   | winit + glow window and rendering               |
| `cockpit-testkit`  | Shared test fixtures and fakes                  |

## Development

```sh
mise run build      # build the workspace
mise run test       # run all tests
mise run fmt        # format
mise run lint       # clippy (warnings as errors)
mise run ci         # everything CI runs
```

Requires the Rust toolchain pinned in `rust-toolchain.toml` and
[`mise`](https://mise.jdx.dev/) as the task runner. List all tasks with
`mise tasks`.

On Linux the windowing layer (`cockpit-render`) needs the X11 and Wayland
development libraries:

```sh
sudo apt-get install libx11-dev libxkbcommon-dev libwayland-dev \
  libxcursor-dev libxrandr-dev libxi-dev
```
