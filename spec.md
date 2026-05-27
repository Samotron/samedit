# Coding Cockpit — Product Specification

> **Implementation language:** Rust. Early drafts considered Zig; that
> exploration is closed and the codebase is Rust. See [`IMPLEMENTATION_PLAN.md`]
> for the authoritative stack and crate layout. Project-signal files like
> `build.zig` remain in the detection list because *user projects* can still
> be Zig — cockpit itself is not.
>
> **Terminal multiplexer:** the v0.7 work replaced the Zellij hand-off with
> an in-process multiplexer (`cockpit-mux`) modelled on tmux. References to
> Zellij below describe the v0.1–v0.6 architecture and are kept for context;
> the current implementation spawns the host shell directly and owns
> splits / windows / sessions in-process. See the v0.7 section of
> [`IMPLEMENTATION_PLAN.md`] for the active surface.
>
> [`IMPLEMENTATION_PLAN.md`]: IMPLEMENTATION_PLAN.md

Testing is treated as a **core product principle**, not an afterthought, because
this app has lots of fiddly state: editor buffers, Vim modes, PTYs, terminal
sessions, project detection, keybindings, and cross-platform behaviour.

---

## 1. Vision

Build a **fast, native, multi-platform coding cockpit** written in **Rust**.

The app is for developers who like:

* Vim-style editing
* terminal-first development
* project-based IDE ergonomics
* fast startup
* native apps
* explicit project environments
* strong automated testing

The app should feel like a focused mix of:

```text
JetBrains project launcher
+ Vim-style editor
+ native file browser
+ termwiz-powered terminal
+ Zellij workspace
+ mise project environment
```

Default layout:

```text
┌────────────────────┬────────────────────────────────────┬──────────────────────────────┐
│ File Browser       │ Vim-style Editor                   │ Zellij Terminal Workspace    │
│                    │                                    │                              │
│ project/           │ src/main.rs                        │ zellij                       │
│  src/              │                                    │ ┌ shell                    ┐ │
│  tests/            │ fn main() {                        │ ├ git/lazygit              │ │
│  mise.toml         │     println!("hi");                │ ├ test runner              │ │
│  Cargo.toml        │ }                                  │ └ AI/agent tools           ┘ │
└────────────────────┴────────────────────────────────────┴──────────────────────────────┘
```

The product is **not** a VS Code clone. It is a **project-based, terminal-first, Vim-centred development environment**.

---

## 2. Core Product Shape

The app has three permanent surfaces:

```text
Left:   Project/file browser
Middle: Vim-style text editor
Right:  Integrated terminal running Zellij
```

The app owns:

```text
project launcher
file tree
editor
layout
keybindings
project metadata
mise integration
terminal bridge
```

The terminal/Zellij workspace owns:

```text
git
lazygit
gh
gh copilot
aider
claude
opencode
test runners
dev servers
build tools
REPLs
logs
```

This keeps the editor lean while still making it powerful.

---

## 3. Product Principles

1. **Fast before feature-rich.**
2. **Terminal-first, not terminal-as-afterthought.**
3. **Project-based, like JetBrains IDEs.**
4. **Vim-inspired editing in the centre.**
5. **Zellij handles terminal workspace management.**
6. **`mise` handles project tools, env vars, and tasks.**
7. **Rust owns the app shell and editor core.**
8. **Testing is a first-class design constraint.**
9. **No heavy background indexing by default.**
10. **No plugin marketplace in early versions.**
11. **Native and multi-platform from the start.**
12. **Use existing tools rather than reimplementing ecosystems.**

---

## 4. Target Platforms

The app should support:

```text
Windows 11
macOS
Linux
```

Windows is a first-class target.

The terminal layer should use:

```text
Windows: ConPTY
macOS:   Unix PTY
Linux:   Unix PTY
```

Zellij is preferred over tmux because Zellij 0.44.0 added native Windows support, along with CLI automation, a layout manager, file-path clicking, and related workspace improvements. ([zellij.dev][1])

---

## 5. Implementation Language

The app should be written primarily in **Rust**.

```text
Language:      Rust
Build system:  Cargo (workspace)
Windowing:     winit
Rendering:     OpenGL via glow
Terminal:      termwiz
PTY:           portable-pty
Workspace:     Zellij
Project env:   mise
Config:        TOML, KDL, or both
Targets:       Windows, macOS, Linux
```

Rust should own:

```text
app shell
project launcher
file browser
editor core
Vim state machine
terminal integration
platform abstraction
rendering abstraction
testing harness
```

External tools and libraries should be integrated rather than reimplemented:

```text
mise         → tools, env vars, tasks
Zellij       → terminal workspace/session management
termwiz      → terminal emulation engine
portable-pty → cross-platform PTY and shell hosting
```

`termwiz` is the terminal-emulation crate from the WezTerm project, providing VT/ANSI parsing and an in-memory terminal screen model. It pairs with `portable-pty` — also from WezTerm — which abstracts ConPTY on Windows and Unix PTYs elsewhere behind one API. ([termwiz][2])

---

## 6. Project-Based Startup

The app should open like a JetBrains IDE rather than a plain text editor.

On launch, show a **project launcher**.

```text
┌──────────────────────────────────────────────┐
│ Coding Cockpit                               │
├──────────────────────────────────────────────┤
│ Recent Projects                              │
│                                              │
│  geotech-platform        ~/code/geotech      │
│  ags-tools               ~/code/ags-tools    │
│  qgis-plugin             ~/code/qgis-plugin  │
│                                              │
│ [Open Folder]  [Clone from Git]  [New Project]│
└──────────────────────────────────────────────┘
```

A project is a folder containing one or more of:

```text
mise.toml
.mise.toml
.git/
Cargo.toml
build.zig
pyproject.toml
package.json
go.mod
pom.xml
build.gradle
```

The strongest project signal should be:

```text
mise.toml
.mise.toml
```

because `mise` can describe tools, environment variables, and tasks.

---

## 7. Project Model

Internally, a project should be represented as:

```rust
pub struct Project {
    pub root_path: PathBuf,
    pub display_name: String,
    pub recent_files: Vec<PathBuf>,
    pub workspace_layout: WorkspaceLayout,
    pub mise: MiseProject,
    pub terminal: TerminalProjectState,
    pub editor: EditorProjectState,
}
```

Project state should include:

```text
open files
active file
pane widths
recent files
recent commands
Zellij session name
last selected mise task
terminal profile
workspace layout
```

This state should be cached per project.

The project launcher should use cached metadata so startup remains instant.

---

## 8. `mise` Integration

`mise` should be a first-class project environment provider.

It should be used for:

```text
tool versions
environment variables
project tasks
terminal environment
LSP environment later
test commands
```

The official `mise` docs describe it as a system for installing and activating the right tools, loading env vars, and wiring up tasks. ([mise-en-place][3]) Its task system supports project tasks for building, testing, linting, deployment, and everyday workflows, and tasks can be defined in `mise.toml` or standalone scripts. ([mise-en-place][4])

Example project config:

```toml
[tools]
rust = "1.88"
python = "3.13"
node = "24"

[env]
APP_ENV = "development"

[tasks.dev]
description = "Run development server"
run = "uv run fastapi dev"

[tasks.test]
description = "Run tests"
run = "cargo nextest run"

[tasks.lint]
description = "Run linting"
run = "cargo clippy --all-targets -- -D warnings"
```

### Required behaviour

When opening a project:

```text
detect mise.toml / .mise.toml
read configured tools
read available tasks
show project environment status
launch terminal through mise when enabled
```

The app should degrade gracefully if `mise` is not installed:

```text
open project anyway
show "mise not found"
disable mise tasks
launch normal terminal profile
```

The app should not auto-install tools by default.

Prompt instead:

```text
This project defines tools with mise, but some are not installed.
Run mise install?
```

### Suggested Rust interface

```rust
pub struct MiseProject {
    pub detected: bool,
    pub available: bool,
    pub tools: Vec<Tool>,
    pub tasks: Vec<Task>,
    pub env: Vec<EnvVar>,
}

pub fn detect_project(root_path: &Path) -> Result<MiseProject>;

pub fn list_tasks(root_path: &Path) -> Result<Vec<Task>>;

pub fn exec(root_path: &Path, argv: &[&str]) -> Result<Child>;
```

---

## 9. Optional Cockpit Metadata in `mise.toml`

The app should work with standard `mise.toml`, but may optionally recognise a metadata block.

```toml
[metadata.cockpit]
name = "Geotech Platform"
default_task = "dev"
terminal_workspace = "zellij"
zellij_layout = ".config/zellij/dev.kdl"
```

This should be optional.

The app should never require project-specific custom config to open a folder.

---

## 10. Zellij Terminal Workspace

The right-hand terminal pane should default to **Zellij**.

The app should launch or attach to one Zellij session per project:

```bash
mise exec -- zellij attach --create <project-name>
```

Conceptually:

```text
open project
  → detect mise
  → resolve project name
  → create terminal pane
  → start PTY
  → launch mise exec -- zellij attach --create project-name
```

Zellij handles internal terminal splitting:

```text
Right pane
  └── zellij
        ├── shell
        ├── git/lazygit
        ├── tests
        ├── dev server
        └── AI/agent tools
```

The app should eventually support project-specific Zellij layouts.

Recommended progression:

```text
v0.1: start or attach Zellij session
v0.2: choose terminal profile
v0.3: open configured Zellij layout file
v0.4: generate suggested layouts from mise tasks
```

---

## 11. Terminal Architecture

The app embeds terminal functionality using `termwiz` for emulation and `portable-pty` for the PTY backend.

```text
TerminalPane
  ├── PTY backend (portable-pty)
  │     ├── Windows ConPTY
  │     └── Unix PTY
  │
  ├── shell process
  │     └── mise exec -- zellij attach --create project
  │
  ├── terminal engine
  │     └── termwiz
  │
  ├── renderer
  │     ├── glyph grid
  │     ├── colours/styles
  │     ├── cursor
  │     └── scrollback
  │
  └── input handling
        ├── keyboard
        ├── mouse
        ├── paste
        └── resize
```

The terminal engine sits behind a `TerminalEngine` trait. Early prototypes may use a simpler backend, but the long-term design assumes `termwiz`, and the trait keeps an alternative backend swappable if needed.

---

## 12. Default Workspace Layout

The app-level layout is simple and stable:

```text
Left pane:    file browser
Centre pane:  Vim-style editor
Right pane:   terminal running Zellij
```

Default widths:

```text
left:   260px
right:  480px
centre: remaining space
```

The app should remember layout per project.

Useful shortcuts:

```text
Ctrl+h          focus file browser
Ctrl+j          focus editor
Ctrl+l          focus terminal
Ctrl+`          toggle terminal
Ctrl+b          toggle file browser
Ctrl+p          fuzzy open file
Ctrl+Shift+p    command palette
Ctrl+s          save
```

When the terminal is focused, Zellij should own almost all keys.

Only a very small set of global focus/toggle shortcuts should be intercepted.

---

## 13. File Browser

The file browser should provide:

```text
project tree
keyboard navigation
open file
create file
create folder
rename
delete
reveal current file
collapse/expand folders
Git status badges later
mise/task section later
```

Ignore common folders by default:

```text
.git
node_modules
target
dist
build
.venv
__pycache__
```

The file tree should be lazy.

Do not recursively scan huge projects on startup.

---

## 14. Editor

The middle pane is a fast text editor with Vim bindings.

### Initial modes

```text
Normal
Insert
Command
```

Later:

```text
Visual
Visual line
Replace
```

### Initial Vim commands

```text
h j k l
w b e
0 ^ $
gg G
i a o O
x
dd
yy
p
u
Ctrl+r
/search
:w
:q
:wq
```

The goal is **Vim-style editing**, not full Vim compatibility.

---

## 15. Editor Buffer

The buffer is built on a **rope**, using the `ropey` crate.

Reasons:

```text
mature and battle-tested (used by Helix and Lapce)
efficient insert/delete
built-in line/column ↔ byte-offset mapping
handles large files well
```

Undo/redo is provided by a separate reversible-edit history stacked on top of the buffer, so the undo model does not depend on the buffer's internal representation.

Large-file mode should degrade gracefully:

```text
disable expensive syntax highlighting
avoid full-file parsing
render visible lines first
make search incremental
avoid blocking the UI
```

---

## 16. Command Palette

Initial commands:

```text
Project: Open Project
Project: Recent Projects
Project: Close Project
File: Open
File: Save
File: Reveal in Tree
Editor: Toggle Relative Line Numbers
Terminal: Focus
Terminal: Restart Zellij
Terminal: New Zellij Session
Mise: Run Task
Mise: Install Tools
Mise: Open Config
Mise: Show Tools
Test: Run All
Test: Run Current File
Test: Run Nearest
```

The command palette should be backed by the same command system used by keybindings and tests.

---

## 17. Editor ↔ Terminal Bridge

The most important workflow feature is the bridge between editor and Zellij.

Examples:

```text
send current file path to terminal
send selected text to terminal
run current mise task
run tests
open terminal output path in editor
open test failure location
```

Path detection should recognise:

```text
src/main.rs:42:13
tests/test_api.py:120
app/foo.py:88
```

This gives IDE-like navigation while keeping execution visible in the terminal.

---

# 18. Testing Strategy

Testing should be treated as a core architecture feature.

The app should be designed so most behaviour can be tested without opening a real GUI window.

The guiding rule:

> **Core logic must be headless-testable. UI should be thin.**

## 18.1 Test Pyramid

```text
Many:      unit tests
Many:      golden tests
Some:      integration tests
Some:      terminal/PTY tests
Few:       UI smoke tests
Few:       full end-to-end tests
```

The project should avoid relying only on manual UI testing.

---

## 18.2 Rust Unit Tests

Use Rust's built-in test framework heavily. The runner today is plain
`cargo test`; switching to `cargo nextest run` for process isolation
(especially around PTY tests) remains a future hardening option.

Core modules should have colocated tests:

```text
crates/cockpit-editor/src/buffer.rs
crates/cockpit-editor/src/vim.rs
crates/cockpit-editor/src/search.rs
crates/cockpit-project/src/mise.rs
crates/cockpit-ui/src/layout.rs
crates/cockpit-terminal/src/path_detect.rs
```

Run with:

```bash
cargo test --workspace
```

Example test areas:

```text
buffer insert/delete
undo/redo
cursor movement
line/column mapping
Vim mode transitions
keybinding resolution
project detection
mise config detection
path parsing
layout sizing
```

---

## 18.3 Golden Tests

Golden tests should be used where behaviour is easiest to verify with snapshots. The `insta` crate is the snapshot tool.

Good candidates:

```text
file tree rendering model
command palette filtering
Vim command output
syntax token spans
terminal path detection
project metadata extraction
layout serialization
```

Example:

```text
input:
  keys: ihello<Esc>dd

expected buffer:
  ""

expected mode:
  Normal
```

Golden test files:

```text
tests/golden/
  vim/
    insert_then_delete.input
    insert_then_delete.expected
  project/
    mise_basic.input
    mise_basic.expected
  terminal/
    path_detection.input
    path_detection.expected
```

This makes editor behaviour much easier to stabilise.

---

## 18.4 Property-Based Tests

The editor core should include property-style tests, using the `proptest` crate.

Important invariants:

```text
insert then delete returns original text
undo after edit returns previous state
redo after undo returns edited state
line/column ↔ byte offset round trips
buffer text equals reference string
cursor never moves outside valid bounds
save/load round trip preserves content
```

A simple random-operation test can compare the buffer against a plain reference string.

Example operations:

```text
insert random text
delete random range
move cursor
undo
redo
```

This is especially useful for preventing subtle buffer corruption.

---

## 18.5 Vim State Machine Tests

The Vim layer should be tested as a pure state machine.

Input:

```text
initial buffer
initial cursor
key sequence
```

Output:

```text
final buffer
final cursor
final mode
register contents
command result
```

Examples:

```text
ihello<Esc>       → buffer contains hello, mode Normal
dd                → deletes current line
yy p              → duplicates current line
w                 → moves to next word
gg                → moves to first line
G                 → moves to last line
:w                → emits Save command
:q                → emits Quit command
```

This allows Vim behaviour to be developed without a UI.

---

## 18.6 Project and `mise` Tests

`mise` integration should be tested in two layers.

### Pure detection tests

These use fake directories and files:

```text
detect mise.toml
detect .mise.toml
detect no mise
detect project name
detect optional metadata.cockpit
```

### CLI integration tests

These run against a real `mise` binary when available.

They should be optional by default:

```bash
mise run test-integration
```

Use a test project:

```text
tests/fixtures/mise-basic/
  mise.toml
```

The tests should verify:

```text
mise is detected
tasks are listed
tools are listed
mise exec can run a command
missing mise degrades gracefully
```

Because `mise` can install tools, integration tests must not accidentally install toolchains unless explicitly enabled.

Default rule:

```text
tests must not run mise install automatically
```

---

## 18.7 Terminal and PTY Tests

Terminal tests should be split into:

```text
PTY backend tests
terminal parser/engine tests
Zellij launch tests
terminal bridge tests
```

### PTY tests

Verify:

```text
can start shell
can write command
can read output
can resize PTY
can terminate process
```

These should be platform-specific and may run only in integration CI.

### Zellij tests

Zellij should be optional in tests unless available.

Test cases:

```text
zellij binary missing → clean error
zellij session command generated correctly
project name converted to safe session name
mise + zellij command generated correctly
```

Do not require a full interactive Zellij session in normal unit tests.

### Terminal bridge tests

These should be pure:

```text
detect file paths in terminal output
parse line/column
open matching file in editor model
send selected text command
send current file path command
```

---

## 18.8 UI Smoke Tests

The app should have a thin UI smoke test layer.

These tests should verify:

```text
app starts
project launcher renders
project opens
three panes render
file can be opened
terminal pane can be created
basic keybindings work
app exits cleanly
```

Do not over-test pixels.

Prefer testing the UI state tree rather than screenshots.

Screenshot tests can be added later for layout regressions, but they should not be the main testing strategy.

---

## 18.9 Cross-Platform CI

CI should run on:

```text
Windows
macOS
Linux
```

Minimum CI jobs:

```text
cargo fmt --check
cargo clippy
cargo build
cargo test --workspace
```

Additional jobs:

```text
mise run test-integration
mise run test-ui-smoke
mise run package
```

Suggested CI matrix:

```text
Windows latest
macOS latest
Ubuntu latest
```

The integration tests can be split:

```text
fast integration tests: every PR
slow/platform tests: nightly
installer/package tests: release only
```

---

## 18.10 Test Fixtures

Keep real fixture projects in the repo.

```text
tests/fixtures/
  rust-basic/
    Cargo.toml
    src/main.rs

  mise-basic/
    mise.toml
    src/main.rs

  large-file/
    generated.txt

  file-tree/
    src/
    tests/
    target/
    node_modules/

  terminal-output/
    test-failure.txt
    rust-error.txt
    python-traceback.txt
```

Fixtures should be small and deterministic.

Large fixtures should be generated during tests, not committed.

---

## 18.11 Test Commands

Expose clean workflow commands as **`mise` tasks**. `mise` is the sole task
runner — there is no `justfile`, no `make`, no `xtask`.

```bash
mise run test
mise run test-unit
mise run test-golden
mise run test-integration
mise run test-ui-smoke
mise run test-all
```

Defined in `mise.toml`, each task calls `cargo` directly:

```toml
[tasks.test]
description = "Run all normal tests"
run = "cargo test --workspace"

[tasks.test-unit]
run = "cargo test --workspace --lib"

[tasks.test-integration]
run = "cargo test --workspace --features integration"

[tasks.fmt]
run = "cargo fmt --all"

[tasks.fmt-check]
run = "cargo fmt --all --check"

[tasks.ci]
depends = ["fmt-check", "lint", "build", "test"]
```

`cargo nextest` is a candidate replacement for the default runner once the
toolchain is wired up; switching is a future hardening pass, not a v0.1
prerequisite.

---

## 18.12 Manual Testing Harness

The app should include a development mode that opens a known fixture project.

Example:

```bash
cargo run -- --fixture mise-basic
```

This should launch the app with:

```text
known project
known file tree
known mise config
known terminal profile
debug logging enabled
```

This gives fast manual testing without clicking through setup every time.

---

## 18.13 Logging and Diagnostics

Testing is easier if the app exposes useful diagnostics.

Add:

```text
debug logs
event tracing
command log
key event inspector
pane state inspector
terminal session log
project detection log
```

Development commands:

```text
Debug: Show Key Events
Debug: Show Command Log
Debug: Show Pane Tree
Debug: Show Project State
Debug: Reload Config
```

These are useful both for development and future user support.

---

## 19. LSP Strategy

LSP should be added later.

When implemented, LSP should use the project environment from `mise`.

Examples:

```bash
mise exec -- rust-analyzer
mise exec -- pyright-langserver --stdio
mise exec -- zls
mise exec -- typescript-language-server --stdio
```

LSP should be lazy-loaded:

```text
do not start LSP on app launch
do not start LSP until relevant file is opened
do not block editing while LSP starts
do not start LSP for huge files
```

---

## 20. Configuration

User config example:

```toml
[ui]
theme = "dark"
font = "JetBrains Mono"
font_size = 13
left_width = 260
right_width = 480

[editor]
vim_mode = true
line_numbers = true
relative_line_numbers = true
tab_width = 4

[project]
environment_provider = "mise"
project_launcher = true

[mise]
enabled = true
auto_detect = true
auto_install = false
use_for_terminal = true
use_for_tasks = true
use_for_lsp = true

[terminal]
engine = "termwiz"
workspace = "zellij"
default_profile = "project-zellij"

[terminal.profiles.project-zellij]
label = "Project Zellij"
command = "mise"
args = ["exec", "--", "zellij", "attach", "--create", "{project_name}"]

[keys.global]
focus_files = "Ctrl+h"
focus_editor = "Ctrl+j"
focus_terminal = "Ctrl+l"
toggle_terminal = "Ctrl+`"
toggle_files = "Ctrl+b"
command_palette = "Ctrl+Shift+p"
fuzzy_open = "Ctrl+p"
```

---

## 21. Internal Architecture

The app is a **Cargo workspace**. Only `cockpit-render` depends on `winit`/`glow`;
every other crate builds and tests headless.

```text
cockpit/                          # Cargo workspace root
  Cargo.toml                      # workspace manifest
  rust-toolchain.toml
  mise.toml                       # sole task runner

  crates/
    cockpit/                      # binary: main, app wiring
      src/
        main.rs
        app.rs
        launcher.rs
        workspace.rs

    cockpit-config/
      src/
        lib.rs
        config.rs

    cockpit-commands/
      src/
        lib.rs
        registry.rs
        keybindings.rs

    cockpit-project/
      src/
        lib.rs
        project.rs
        project_cache.rs
        mise.rs
        task.rs
        file_tree.rs

    cockpit-editor/
      src/
        lib.rs
        buffer.rs
        cursor.rs
        vim.rs
        undo.rs
        search.rs
        syntax.rs

    cockpit-terminal/
      src/
        lib.rs
        terminal.rs
        engine.rs                 # termwiz-backed TerminalEngine
        zellij.rs
        pty.rs                    # portable-pty wrapper
        path_detect.rs
        bridge.rs

    cockpit-ui/
      src/
        lib.rs
        layout.rs
        pane.rs
        file_browser.rs
        editor_view.rs
        terminal_view.rs
        command_palette.rs
        project_launcher.rs

    cockpit-render/               # only crate depending on winit/glow
      src/
        lib.rs
        renderer.rs
        text.rs
        font_cache.rs
        theme.rs

    cockpit-testkit/              # dev-dependency: shared fixtures + fakes
      src/
        lib.rs

  tests/
    golden/
    fixtures/
    integration/
    ui_smoke/
```

---

## 22. Build System

Use Cargo for the underlying build, with **`mise` tasks** as the single,
discoverable entry point for every workflow. No `justfile`, no `make`.

```bash
mise run build
mise run run
mise run run-fixture -- <name>
mise run test
mise run test-unit
mise run test-golden
mise run test-integration
mise run test-ui-smoke
mise run package
```

Eventually, for cross-compilation:

```bash
cargo build --target x86_64-pc-windows-msvc
cargo build --target aarch64-apple-darwin
cargo build --target x86_64-unknown-linux-gnu
```

---

## 23. MVP Scope

### v0.1 — Project cockpit

```text
Project launcher
Recent projects
Open folder as project
Detect mise.toml
Three-pane layout
File browser
Basic Vim-style editor
Integrated terminal
Launch Zellij session
Save/open files
Basic command palette
Pane focus shortcuts
Unit test harness
Golden test harness
CI for Windows/macOS/Linux
```

Success criteria:

```text
can open a real project
can edit and save files
can run Zellij in the right pane
can detect mise tasks
can switch between panes quickly
tests run on all target platforms
```

### v0.2 — Useful daily driver

```text
Fuzzy file open
Mise task picker
Run task in Zellij
Remember project layout
Better Vim motions
Syntax highlighting
Terminal file path detection
Project metadata cache
Property tests for editor buffer
PTY integration tests
```

### v0.3 — Strong workflow integration

```text
Zellij layout support
Open configured Zellij layout per project
Send selection/path to terminal
Run current file
Run nearest test
Git status badges
LSP foundation
UI smoke tests
Fixture-based manual testing
```

### v0.4 — Coding intelligence

```text
LSP diagnostics
Go to definition
Hover
Rename symbol
Format on save
Completion
Code actions / quick-fix
SQL language server (sqls)
Use mise env for LSP
More editor conformance tests
Mouse input across the cockpit
```

> Beyond v0.4: `IMPLEMENTATION_PLAN.md` introduces **v0.5** (SQL notebooks
> and dbt-lite analytics on DuckDB) and **v0.6** (instant-load
> performance targets that supersede §24's `< 500 ms`). Those phases are
> deliberately not in this spec — the plan is authoritative for them.

---

## 24. Performance Targets

```text
Cold start:              < 500 ms target
Project launcher:         instant from cache
Open project:             no blocking full scan
Open small file:          instant
Open large file:          render visible content first
Typing latency:           imperceptible
Terminal latency:         imperceptible
Mise detection:           async/non-blocking
Zellij startup:           visible and recoverable
Test suite:               fast enough to run constantly
```

Avoid:

```text
blocking startup on project indexing
starting LSP immediately
scanning node_modules
running mise install automatically
hidden task execution by default
heavy plugin loading
fragile manual-only testing
```

---

## 25. Risks

### Zellij Windows maturity

Zellij's native Windows support is new, so the app should support fallback terminal profiles:

```text
plain PowerShell
WSL shell
Git Bash
MSYS2
no multiplexer
```

### termwiz API maturity

`termwiz` is a solid conceptual fit, but its API and screen model should be prototyped early. The terminal engine sits behind a `TerminalEngine` trait so an alternative backend can be swapped in if needed.

### Vim compatibility scope

Full Vim compatibility is enormous.

Mitigation:

```text
test a practical subset first
document unsupported behaviour
do not promise plugin compatibility
```

### Testing terminal behaviour

PTY and terminal tests can be flaky across platforms.

Mitigation:

```text
separate pure tests from integration tests
make slow/platform tests opt-in or nightly
use deterministic fixtures where possible
```

---

## 26. One-Sentence Summary

A fast, native, multi-platform coding cockpit written in Rust, opening projects like a JetBrains IDE, using `mise` for project tools/tasks/environment, providing a file browser on the left, a Vim-style editor in the middle, a `termwiz`-powered terminal running a project-specific Zellij workspace on the right, and treating automated testing as a first-class architecture concern.

[1]: https://zellij.dev/news/remote-sessions-windows-cli/?utm_source=chatgpt.com "Remote Sessions, Windows Support, CLI Automation"
[2]: https://docs.rs/termwiz/ "termwiz — terminal emulation crate (WezTerm project)"
[3]: https://mise.jdx.dev/?utm_source=chatgpt.com "mise-en-place: Home"
[4]: https://mise.jdx.dev/tasks/?utm_source=chatgpt.com "Tasks"
