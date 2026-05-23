# Implementation Plan — Coding Cockpit (Rust)

> Implementation plan for `spec.md` ("Zig Coding Cockpit — Product Specification").
> **Language decision overridden:** the product is built in **Rust**, not Zig.
> `spec.md` still says Zig throughout (§5, §21, §22, code samples, `build.zig`,
> `zig build` commands). Treat **this document as authoritative** for stack and
> structure; `spec.md` remains authoritative for *product behaviour*. The spec
> should be revised to match, or annotated as superseded.

---

## 0. Stack summary

| Concern            | Choice                                                        |
|--------------------|---------------------------------------------------------------|
| Language           | Rust (stable, pinned via `rust-toolchain.toml`)               |
| Build / workspace  | Cargo workspace, multiple crates                              |
| Windowing          | `winit`                                                       |
| Rendering          | OpenGL via `glow`                                             |
| Text (UI/editor)   | `cosmic-text` for shaping/raster → glyph atlas (`etagere`)    |
| Terminal engine    | `termwiz` (wezterm VT stack)                                  |
| PTY                | `portable-pty` (ConPTY on Windows, Unix PTY elsewhere)        |
| Editor buffer      | `ropey` rope + explicit undo stack (supersedes spec §15)      |
| Config             | `serde` + `toml`; `kdl` for Zellij layouts (v0.3)             |
| Project env        | shell out to the `mise` binary                                |
| Terminal workspace | shell out to the `zellij` binary                              |
| Golden tests       | `insta`                                                       |
| Property tests     | `proptest`                                                    |
| Test runner        | `cargo test` today; `cargo nextest` remains a future hardening option |
| Task runner        | `mise` tasks — single source of truth, no `just`/`make`/`xtask` |
| Logging            | `tracing` + `tracing-subscriber`                              |
| Errors             | `thiserror` in libraries, `anyhow` in the binary              |
| CLI args           | `clap`                                                        |

---

## 1. Locked architecture decisions

1. **Cargo workspace of focused crates** — replaces the spec's `src/` module tree
   (§21). One crate per bounded context; the binary only wires them together.

2. **Headless core / thin render layer — the testability contract.** This is the
   load-bearing decision behind spec §18 ("Core logic must be headless-testable.
   UI should be thin."). Only **one** crate (`cockpit-render`) depends on
   `winit`/`glow`. Every other crate compiles and tests with no window, no GPU,
   no display server.

3. **UI = pure state tree + immediate-mode painter.** `cockpit-ui` holds a
   retained *view-model* tree (panes, launcher, file tree, palette state) that is
   a plain data structure — fully unit-testable. Each frame the painter turns
   that tree into draw calls. Spec §18.8 ("test the UI state tree rather than
   screenshots") falls out of this for free.

4. **The command system is the single spine.** `cockpit-commands` owns a command
   registry. Keybindings, the command palette, the editor↔terminal bridge, and
   tests all dispatch the *same* command IDs (spec §16: "backed by the same
   command system used by keybindings and tests").

5. **`ropey` rope + explicit undo stack** instead of the spec's piece table
   (§15). Rationale: in Rust the rope *is* the simple, battle-tested option
   (`ropey` powers Helix and Lapce), with built-in line/column ↔ byte-offset
   mapping. Piece-table's stated win was a "good undo model" — that comes from a
   separate, reversible-edit undo stack regardless of buffer type. The spec's
   large-file degradation rules (§15) still apply unchanged.

6. **Terminal engine behind a trait.** `TerminalEngine` trait with a `termwiz`
   implementation. This keeps the door open for a future `libghostty` backend
   without touching call sites, and directly satisfies the spec's §25
   "prototype/keep alternatives" risk posture.

7. **All non-determinism is injectable.** Filesystem, process spawning, and
   the clock are accessed through the `FileSystem`, `ProcessRunner`, and
   `Clock` traits in `cockpit-project::env` (M4.10). Production callers pass
   `Std*` impls; tests pass `Fake*` impls from the same module. Remaining
   direct `std::fs` use in `cockpit-project` is limited to the file-tree
   walk and lazy children load — those still touch real directories because
   they walk arbitrary trees; an in-memory fs trait abstraction over
   `read_dir` is a future cleanup.

8. **No global async runtime.** PTY and child-process I/O run on dedicated OS
   threads with channels. `termwiz`/`portable-pty` are blocking-I/O friendly.
   Avoids dragging `tokio` through headless core crates.

9. **Pinned toolchain.** `rust-toolchain.toml` pins a stable Rust version so CI
   across three OSes is reproducible.

---

## 2. Crate / dependency map

```
cockpit/                         # Cargo workspace root
├── crates/
│   ├── cockpit            (bin) # main, app wiring, event loop ownership
│   ├── cockpit-editor           # ropey buffer, cursor, undo, vim FSM, search, syntax
│   ├── cockpit-project          # detection, mise, project cache, tasks, file tree
│   ├── cockpit-terminal         # pty wrapper, termwiz engine, zellij, path detect, bridge
│   ├── cockpit-commands         # command registry, keybinding resolution
│   ├── cockpit-config           # serde config types, TOML/KDL loading, defaults
│   ├── cockpit-ui               # view-model tree, layout, panes, palette/launcher models
│   ├── cockpit-render           # winit + glow, glyph atlas, theme  ← ONLY GPU/window crate
│   └── cockpit-testkit   (dev)  # shared fixtures, fakes, golden helpers
├── tests/                       # fixtures + integration/ui-smoke harnesses
├── mise.toml                    # named build/test workflows (sole task runner)
├── rust-toolchain.toml
└── Cargo.toml                   # workspace manifest
```

| Crate              | Layer        | Key deps                                  | Headless-testable |
|--------------------|--------------|-------------------------------------------|-------------------|
| `cockpit-editor`   | core         | `ropey`, `tree-sitter` (v0.2+)            | ✅ fully          |
| `cockpit-project`  | core         | `serde`, `toml`                           | ✅ fully          |
| `cockpit-commands` | core         | —                                         | ✅ fully          |
| `cockpit-config`   | core         | `serde`, `toml`, `kdl` (v0.3+)            | ✅ fully          |
| `cockpit-terminal` | core + I/O   | `termwiz`, `portable-pty`                 | ✅ path-detect / zellij-cmd; ⚠️ PTY = integration |
| `cockpit-ui`       | view-model   | `cockpit-*` cores, `nucleo` (v0.2+)       | ✅ fully          |
| `cockpit-render`   | shell        | `winit`, `glow`, `cosmic-text`, `etagere` | ⚠️ smoke only     |
| `cockpit` (bin)    | wiring       | all of the above, `clap`, `anyhow`        | ⚠️ smoke / e2e    |

**Spec §21 simplifications enabled by this stack:** the spec's
`platform/{windows,macos,linux}/window.zig` and `clipboard.zig` files are
unnecessary — `winit` abstracts windowing/input cross-platform and provides
clipboard. The only genuinely platform-specific code is the PTY, and
`portable-pty` already abstracts ConPTY vs Unix PTY. The `platform/` layer
effectively disappears.

---

## 3. Spec need → crate mapping

| Spec requirement                        | Rust crate / approach                          |
|-----------------------------------------|------------------------------------------------|
| Terminal emulation (§11)                | `termwiz`                                      |
| ConPTY / Unix PTY (§4, §11)             | `portable-pty`                                 |
| Editor buffer (§15)                     | `ropey` + custom undo stack                    |
| Vim state machine (§14, §18.5)          | hand-rolled FSM in `cockpit-editor`            |
| Syntax highlighting (§23 v0.2)          | `tree-sitter` + per-language grammar crates    |
| Fuzzy file open (§23 v0.2)              | `nucleo`                                       |
| Config TOML (§20)                       | `serde` + `toml`                               |
| Zellij layout KDL (§9, §10)             | `kdl`                                          |
| Git status badges (§23 v0.3)            | shell out to `git status --porcelain`; `gix` later |
| LSP (§19, §23 v0.3/v0.4)                | `lsp-types` + JSON-RPC over stdio on a thread  |
| Golden tests (§18.3)                    | `insta`                                        |
| Property tests (§18.4)                  | `proptest`                                     |
| Logging / diagnostics (§18.13)          | `tracing` + `tracing-subscriber`               |

---

## 4. Phase 0 — Foundations & de-risking spikes

Goal: a workspace that builds and tests green on three OSes, plus early proof
that the two riskiest dependencies (`termwiz`/`portable-pty`, `winit`/`glow`
text) actually work. Nothing here is throwaway — spikes graduate into real code.

### M0.1 — Workspace skeleton
- `cargo new` workspace; create all eight crates as empty libs + the binary.
- `rust-toolchain.toml` pins stable Rust.
- `mise.toml` at repo root: `[tasks]` define `build`, `run`, `run-fixture`,
  `test`, `test-unit`, `test-golden`, `test-integration`, `test-ui-smoke`,
  `test-all`, `fmt`, `fmt-check`, `lint`, `package`, `ci` — each calling
  `cargo` directly (maps spec §18.11 / §22 to Cargo).
- Rust toolchain itself stays pinned in `rust-toolchain.toml`.
- **Done when:** `mise run build` and `mise run test` succeed locally.

### M0.2 — Test harness conventions
- Current runner is `cargo test`; `insta` and `proptest` are dev-deps.
  `cargo nextest` remains desirable for process isolation, especially PTY tests,
  but is not currently wired into `mise.toml` or CI.
- Integration tests gated behind a Cargo feature `integration`; UI-smoke behind
  `ui-smoke` — so `cargo test` stays fast and hermetic (spec §18.6/§18.7: slow
  tests opt-in).
- `cockpit-testkit`: `tempdir` fixture builders, fake FS/process/clock impls,
  golden-file helpers.
- `tests/fixtures/` seeded per spec §18.10 (`zig-basic`→`rust-basic`,
  `mise-basic`, `file-tree`, `terminal-output`); large fixtures generated at
  runtime, not committed.
- **Done when:** a sample unit test, an `insta` snapshot, and a `proptest` all run.

### M0.3 — `winit` + `glow` window spike
- Open a window, create a GL context, run the event loop, clear to a colour,
  handle resize and close. Graduates into `cockpit-render`.
- **Done when:** a window opens and closes cleanly on Linux + at least one of
  Windows/macOS.

### M0.4 — Text rendering spike
- `cosmic-text` rasterises glyphs → pack into an atlas (`etagere`) → draw a
  string of monospace text via `glow`. Establishes the `text.rs` + `font_cache.rs`
  primitives (spec §21 `render/`).
- **Done when:** a line of styled text renders at a stable position.

### M0.5 — `termwiz` + `portable-pty` spike — **decision gate**
- Spawn a real shell through `portable-pty`, feed bytes to a `termwiz` surface,
  read the screen grid. Confirms the `TerminalEngine` trait shape.
- **Done when:** `ls` output is visible in the parsed grid; resize works. If
  `termwiz` proves unworkable, this is the cheap point to reconsider the backend.

### M0.6 — Logging & diagnostics scaffolding
- Wire `tracing`; env-controlled log level; lay groundwork for the spec §18.13
  debug surfaces (key events, command log, pane tree, project state).
- **Done when:** structured logs appear with `RUST_LOG`/`COCKPIT_LOG`.

### M0.7 — CI skeleton
- GitHub Actions matrix: `windows-latest`, `macos-latest`, `ubuntu-latest`.
- Jobs: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build`,
  `cargo test --workspace`.
- Linux runner installs `winit` system deps (X11 + Wayland dev libraries).
- **Done when:** all three legs are green.

---

## 5. v0.1 — Project cockpit  (spec §23 v0.1)

Delivers: project launcher, three-pane layout, file browser, basic Vim editor,
integrated terminal running Zellij, save/open, command palette, pane focus
shortcuts, unit + golden harnesses, CI on all three OSes.

Four tracks. **A** (headless cores) and **B** (render shell) run in parallel
after Phase 0. **C** (terminal) depends on M0.5. **D** (wire-up) depends on
A + B + C. **E** (hardening) closes the version.

### Track A — Headless cores  *(no `winit`/`glow`)*

**M1.1 — `cockpit-editor`: buffer, cursor, undo, search**
- `ropey`-backed `Buffer`; `Cursor` as byte offset + derived line/col.
- Reversible-edit undo/redo stack.
- Incremental substring search.
- Tests: insert/delete, undo/redo, line↔offset mapping, cursor-bounds, search.
- Done when: editing + undo + search pass unit tests; no UI dependency.

**M1.2 — `cockpit-editor`: Vim state machine**
- Modes Normal / Insert / Command (spec §14). Pure FSM:
  `(state, key) → (state, Vec<Action>)` where `Action` is a buffer edit or an
  app command (`:w`→Save, `:q`→Quit).
- Command set from spec §14: `h j k l w b e 0 ^ $ gg G i a o O x dd yy p u
  Ctrl+r /search :w :q :wq`.
- Tests: `insta` golden cases using the exact I/O contract of spec §18.5.
- Done when: every §18.5 example produces its expected buffer/cursor/mode.

**M1.3 — `cockpit-commands`: registry + keybindings**
- `CommandId` registry `{id, title, handler}`; key-chord → command resolution
  driven by config.
- Tests: keybinding resolution, conflict handling, dispatch.
- Done when: a chord resolves to and invokes a command in tests.

**M1.4 — `cockpit-config`: loader**
- `serde` structs for the full spec §20 config (`ui`, `editor`, `project`,
  `mise`, `terminal`, `terminal.profiles`, `keys.global`); TOML load; defaults
  when the file is absent.
- Tests: parse the §20 example; defaults; malformed-file handling.
- Done when: §20 sample round-trips into typed config.

**M1.5 — `cockpit-project`: detection, mise, cache**
- Project detection from spec §6 signal files; `mise.toml` strongest signal.
- mise layer (spec §8): parse `[tools]`/`[tasks]`/`[env]` + optional
  `[metadata.cockpit]` (§9); graceful "mise not found" degradation; `exec()`
  wrapper around `mise exec`. No auto-install (§8 / §24).
- `project_cache`: per-project state (spec §7) serialised to the OS cache dir.
- Tests: detection via tempdir fixtures, mise parse, missing-mise degradation,
  cache round-trip. (CLI integration deferred to v0.2, spec §18.6.)
- Done when: `mise-basic` fixture detects correctly and lists tasks/tools.

**M1.6 — `cockpit-project`: file browser model**
- Lazy tree (children loaded on expand — spec §13 "do not recursively scan").
- Default ignore list (spec §13: `.git node_modules target dist build .venv
  __pycache__` …; drop `zig-cache`/`zig-out`, add `target`).
- create / rename / delete operations.
- Tests: ignore filtering, lazy expansion, file ops via `file-tree` fixture.
- Done when: tree model navigable and mutable in tests.

**M1.7 — `cockpit-terminal`: path detection**
- Parse `path:line:col` forms from spec §17 (`src/main.rs:42:13`,
  `tests/test_api.py:120`, `app/foo.py:88`). Pure function.
- Tests: `insta` golden over the `terminal-output` fixtures.
- Done when: all spec §17 path forms parse correctly.

### Track B — Render shell & UI view-models

**M1.8 — `cockpit-render`: pipeline**
- Graduate M0.3/M0.4: GL renderer with batched rects + text runs, glyph atlas,
  `theme` (spec §21 `render/`). Immediate-mode painter API.
- Tests: atlas packing unit tests; rendering itself covered by smoke later.
- Done when: arbitrary rects + text draw each frame at 60fps idle.

**M1.9 — `cockpit-ui`: layout & panes**
- Three-pane layout model: left 260px, right 480px, centre flex (spec §12);
  per-project width persistence; focus state.
- Tests: layout sizing across window sizes, focus transitions (pure).
- Done when: layout math is unit-tested with zero render dependency.

**M1.10 — Input mapping**
- `winit` keyboard/mouse → key chords → `cockpit-commands`. Unicode text via the
  char/IME path. Global shortcuts intercepted; everything else passes through
  (spec §12: "when terminal focused, Zellij owns almost all keys").
- Tests: event→chord mapping.
- Done when: a keypress reaches a command handler end-to-end.

### Track C — Terminal integration

**M1.11 — `cockpit-terminal`: PTY + engine**
- `portable-pty` wrapper; `TerminalEngine` trait + `termwiz` impl; dedicated
  I/O thread; grid/screen model exposed to the UI.
- Tests: grid model unit tests; live PTY tests behind the `integration` feature
  (spec §18.7).
- Done when: a shell runs in the engine and the grid reflects output.

**M1.12 — `cockpit-terminal`: Zellij launcher**
- Build `mise exec -- zellij attach --create <project-name>` (spec §10);
  project-name → safe session-name conversion; clean error if `zellij`/`mise`
  absent; fallback plain-shell profile (spec §25).
- Tests: command construction, name sanitisation, missing-binary handling.
- Done when: command-building + degradation are fully unit-tested.

### Track D — Wire-up  *(needs A + B + C)*

**M1.13 — Project launcher UI** (spec §6) — recent projects from cache, Open Folder.

**M1.14 — Editor view** (spec §15) — render buffer/cursor/mode; wire Vim FSM and Buffer; save/open files.
**M1.15 — File browser view** (spec §11) — render the tree; keyboard nav; open file into the editor.
**M1.16 — Terminal view** (spec §14) — render the termwiz grid; forward input; resize the PTY.
**M1.17 — Command palette UI** (spec §16) — fuzzy search for app commands.
**M1.18 — Focus/toggle & end-to-end** (spec §12) — wire global shortcuts; open → edit → save → terminal.

### Track E — Hardening

**M1.19 — Golden suite buildout** (spec §18.5) — `insta` coverage for Vim, project/mise, path detection.
**M1.20 — CI green ×3** (spec §21) — Windows, macOS, Linux tests on every PR.
**M1.21 — `run-fixture` dev mode** (spec §18.12) — cargo run -- --fixture mise-basic.

### v0.1 exit checklist  *(spec §23 success criteria)*
- [x] Opens a real project; edits and saves files.
- [x] Runs Zellij in the right pane.
- [x] Detects mise tasks.
- [x] Fast pane switching.
- [x] `cargo test --workspace` green locally and wired in Windows/macOS/Linux CI.
- [ ] Optional hardening: switch CI and `mise.toml` from `cargo test` to
      `cargo nextest run` if/when nextest is adopted.

---

## 6. v0.2 — Useful daily driver  (spec §23 v0.2)

Status: mostly implemented.

- [x] **M2.1 — Fuzzy file open** — `nucleo` matcher over the lazy tree; `Ctrl+P` UI.
- [x] **M2.2 — Mise task picker + run in Zellij** — palette `Mise: Run Task`; send the
  chosen task into the Zellij session.
- [x] **M2.3 — Persist project layout** — project cache persists pane widths and
  active/open file state. Zellij session-name persistence is still optional
  hardening.
- [x] **M2.4 — Better Vim** — Visual / Visual-line / Replace modes; counts, more
  motions and operators; expanded §18.5 golden suite.
- [x] **M2.5 — Syntax highlighting** — `tree-sitter` integration; token spans →
  themed render; large-file degradation (spec §15). Golden tests on token spans
  (spec §18.3).
- [x] **M2.6 — Terminal→editor path navigation** — wire M1.7 detection to jump:
  open the matched file at line:col (spec §17). Mouse click affordances can be
  refined later.
- [x] **M2.7 — Project metadata cache hardening** — launcher uses a recent-projects
  cache so startup does not re-detect every project.
- [x] **M2.8 — Editor property tests** — `proptest` invariants from spec §18.4
  (insert/delete round-trip, undo/redo, offset round-trips, rope vs reference
  string).
- [x] **M2.9 — PTY integration tests** — spec §18.7: start shell, write, read,
  resize, terminate; behind the `integration` feature, run in CI integration leg.
- [x] **M2.10 — mise CLI integration tests** — spec §18.6: run against a real `mise`
  when present; must never trigger `mise install` (spec §18.6 hard rule).

---

## 7. v0.3 — Strong workflow integration  (spec §23 v0.3)

Status: mostly implemented.

- [x] **M3.1 — Zellij layout support** — parse layout KDL with the `kdl` crate;
  open the configured per-project layout (spec §9 `[metadata.cockpit]`, §10 v0.3).
- [x] **M3.2 — Editor↔terminal bridge** — send selection / current file path to
  the terminal; the main spec §17 bridge surface.
- [x] **M3.3 — Run current file / run nearest test** — palette `Test: Run All /
  Run Current File / Run Nearest` (spec §16); resolve commands via mise tasks.
- [x] **M3.4 — Git status badges** — file-browser badges via
  `git status --porcelain` (shell-out first; `gix` as a later pure-Rust upgrade).
- [x] **M3.5 — LSP foundation** — JSON-RPC client over stdio on a thread;
  `lsp-types`; lazy start (spec §19: not on launch, not until a relevant file
  opens, never blocking, never for huge files); servers launched via `mise exec`
  (spec §19).
- [x] **M3.6 — UI smoke tests** — spec §18.8: assert on the `cockpit-ui`
  view-model tree (app starts, launcher renders, project opens, three panes,
  file opens, terminal pane created, keybindings, clean exit). Behind the
  `ui-smoke` feature with a dedicated CI leg.
- [x] **M3.7 — Debug surfaces** — spec §18.13 commands: Show Key Events /
  Command Log / Pane Tree / Project State / Reload Config.

---

## 8. v0.4 — Coding intelligence + mouse  (spec §23 v0.4, extended)

Goal: LSP coding-intelligence breadth across **rust-analyzer,
typescript-language-server, basedpyright, sqls**, plus first-class mouse
support and the spec/architecture housekeeping debt. Single milestone — full
LSP feature set ships together (user decision: breadth over polish).

LSP servers are launched via `mise exec` (M4.0, already shipped) so they
inherit the project environment (spec §19).

### LSP coding intelligence

- [x] **M4.1 — Diagnostics** — ingest `publishDiagnostics` and render LSP
  diagnostics in the editor gutter/inline.
- [x] **M4.2 — Navigation** — `textDocument/definition` + `hover`. New
  commands: `Go to Definition` (default `gd`), `Show Hover` (default `K`).
  Reuses the existing path-jump plumbing from
  `cockpit-terminal`/`bridge.rs` to open the target file at line:col.
- [x] **M4.3a — Rename** — `prepareRename` + `rename`; **inline edit at
  cursor** (LazyVim/VSCode style), then apply the returned `WorkspaceEdit`.
- [x] **M4.3b — Completion** — `textDocument/completion` (+ `resolve` for
  detail/docs). **Manual trigger only (`Ctrl+Space`)** — no
  on-keystroke debounce in v0.4 to avoid fighting the Vim FSM. UI is
  inline ghost text **and** a popup list with docs; view-model lives in
  `cockpit-ui`, keys in `cockpit-commands`.
- [x] **M4.4 — Format on save** — **mise task wins, always.** If a `format`
  (or `format:<lang>`) mise task exists, use it. If no task exists and a
  known formatter is detectable (`[tools]` or PATH: `rustfmt`, `prettier`,
  `ruff`, `black`, `sqlfluff`), surface a prompt: *"Add `format` task to
  `mise.toml`? [Y/n]"* — write only on user confirm (AGENTS.md hard rule
  #6: "Detect, surface, prompt — never silently modify"). LSP `formatting`
  is used **only** when no formatter is detectable and the server
  advertises the capability.
- [x] **M4.5 — Code actions / quick-fix** — `textDocument/codeAction` wired
  to current diagnostic; palette command + keybinding (default `<leader>ca`).
- [x] **M4.6 — Vim/editor conformance** — extend the golden suite for the
  new motions and operators introduced by navigation/rename (`gd`, `K`,
  rename interactions). Property tests gain a rename-round-trip case.
- [x] **M4.8 — SQL LSP** — `sqls` (most mature, cross-DB). `postgrestools`
  deferred to a later milestone. Registry entry in `cockpit-lsp`.

### Mouse support (new — not in spec §12)

- [ ] **M4.7 — Mouse input** — first-class mouse handling across the cockpit.
  `winit` events → `cockpit-ui` view-model → `cockpit-commands`. All
  behaviour is unit-testable on the view-model tree (no pixel checks,
  spec §18.8). Surfaces:
  - Click a pane → focus that pane (launcher / file tree / editor / terminal).
  - Click a file in the tree → open it in the editor.
  - Click in the editor gutter/text → move the Vim cursor.
  - Click in the terminal → focus the terminal (Zellij owns selection).
  - Drag a pane border → resize side panes; widths persist per-project.
  - Scroll wheel in editor → scroll buffer; in terminal → scroll back.

### Housekeeping (paid down alongside v0.4)

- [ ] **M4.9 — Spec rewrite Zig → Rust** — update `spec.md` §5, §21, §22,
  `build.zig` references, and code samples so spec and plan stop diverging
  (AGENTS.md hard rule #8).
- [x] **M4.10 — Trait injection cleanup** (architecture item from §1.7) —
  `cockpit-project::env` now hosts `FileSystem`, `ProcessRunner`, and `Clock`
  traits with `Std*` production impls and `Fake*` in-memory test impls.
  `detect_mise_project`, `git_status`, `ProjectCache::load/store`, and
  `RecentProjects::load/store` all gained `_with` variants that take the
  trait objects (the unadorned wrappers keep the existing call sites
  unchanged). `AppModel::with_env` lets the app inject the seam end to
  end; the format-on-save flow now has a hermetic test that scripts every
  spawn and snapshots every write without touching real disk.

---

## 8a. v0.5 — SQL notebooks + dbt-lite analytics  (NEW — post-spec)

Goal: turn the cockpit into a first-class local analytics environment on
top of **DuckDB**, with executable notebooks and Quarto documents. Three
composed features:

- **Notebook mode (B)** — cell-based SQL/ggsql execution with **inline**
  table and chart results in the same view as the source.
- **Quarto mode** — `.qmd` files (Markdown with `{sql}` / `{ggsql}` code
  chunks) treated as a peer of the Jupytext notebook format. Chunks
  execute in-place, outputs render inline, exported via `quarto render`.
- **dbt-lite project mode (C)** — a project type for `models/*.sql` with
  `{{ ref(...) }}` / `{{ source(...) }}` templating, materialisations,
  and a DAG view. Minus the warehouse, minus the Python.

`sqls` (from v0.4 M4.8) continues to provide schema completion/hover
inside all three modes — they're orthogonal layers.

### Engine integration

- [ ] **M5.1 — DuckDB via shell-out + mise** — execute plain SQL cells by
  spawning `mise exec -- duckdb`. No embedded `duckdb` crate (keeps the
  binary small and protects the future instant-load target). If `duckdb`
  is not in `[tools]`, surface the standard prompt: *"Add `duckdb` to
  `mise.toml` `[tools]`? [Y/n]"* — never auto-install (AGENTS.md rule
  #6, spec §8). Long-running session per project: spawn one `duckdb`
  subprocess on first execution, feed statements over stdin, parse JSON
  over stdout. Lives in a new crate **`cockpit-sql`** (headless-testable
  — the DuckDB transport is behind a `SqlEngine` trait with a fake impl
  in `cockpit-testkit`, mirroring the `TerminalEngine` pattern).
- [ ] **M5.1a — ggsql via shell-out + mise** — visualisation cells go
  through `mise exec -- ggsql exec --reader duckdb://memory --writer
  vegalite`, returning Vega-Lite v6 JSON. ggsql is a Posit project
  (alpha, Rust, `cargo install ggsql-cli` or via `mise use cargo:ggsql-cli`)
  that wraps DuckDB internally and emits Vega-Lite specs. Same prompt
  pattern as M5.1 when absent. Same `SqlEngine` trait — a second
  `GgsqlEngine` impl, swappable in tests.

### Notebook mode

- [ ] **M5.2 — Notebook file format** — Jupytext-style: **plain `.sql` or
  `.ggsql` files with `-- %% cell` separators**, opened in a
  `cockpit-notebook` view-model when the marker is detected. Rationale:
  diffs cleanly in git, opens as plain SQL/ggsql in any editor, no JSON
  envelope. A cell is routed to ggsql if its body contains a `VISUALISE`
  (or `VISUALIZE`) clause, otherwise to DuckDB. Per-cell metadata
  (title, options) goes in trailing `-- %% meta: { ... }` KDL lines.
  Cell results are *not* persisted in the file — they live in a sibling
  `.cockpit/results/<file>.json` cache so they survive reopens without
  polluting source.
- [ ] **M5.3 — Notebook view-model** — new crate `cockpit-notebook`. State
  tree: `Notebook { cells: Vec<Cell { source, kind, status, result } }`,
  where `kind ∈ { Sql, Ggsql }`. Pure data, fully unit-testable. Vim FSM
  works inside an active cell; global keys move between cells.
- [ ] **M5.4 — Inline tabular result rendering** — virtualised scrollable
  grid for DuckDB results (no full-load), rendered **inline beneath the
  source cell** — no separate pane, no popout. JSON pretty for
  non-tabular; error pane for failures. Lives in `cockpit-ui`; renderer
  in `cockpit-render`.
- [ ] **M5.5 — Inline chart rendering via ggsql + vl-convert** — ggsql
  cells emit Vega-Lite v6 JSON; we render that JSON to PNG via
  `mise exec -- vl-convert vl2png` (or `vl2svg`). vl-convert is Vega's
  official converter (Rust crate `vl-convert-rs` exists but pulls a
  bundled Deno runtime, so we shell out instead to stay small and honour
  the future instant-load budget). PNG bytes feed the existing texture
  path in `cockpit-render` and display **inline directly below the
  cell's source** — same single-document flow as tables, never in a
  side pane. Same detect-and-prompt flow for the `vl-convert` tool as
  for `duckdb` / `ggsql`. No bespoke chart DSL — ggsql's grammar of
  graphics (`VISUALISE … DRAW point/line/bar/histogram/boxplot … SCALE …
  LABEL`) is the chart API.
- [ ] **M5.5a — ggsql syntax highlighting** — adopt the upstream
  `tree-sitter-ggsql` grammar so `.ggsql` files and ggsql cells get
  proper highlighting via the existing tree-sitter pipeline in
  `cockpit-editor`. Zero new render work — it's just another grammar
  registration.

### Quarto mode

- [ ] **M5.Q1 — Quarto file parser** — `.qmd` parses into the same
  `Notebook` view-model as M5.3, but with a third cell kind
  `Markdown`. Code chunks bounded by ```` ```{sql} ```` / ```` ```{ggsql} ````
  (plus chunk options like `#| label:`, `#| echo: false`) feed the same
  DuckDB/ggsql execution path. Non-SQL chunk languages (e.g. `{python}`,
  `{r}`) are parsed and shown but **not executed in v0.5** — they get a
  "language unsupported" status banner; full multi-kernel support is a
  v0.6+ decision.
- [ ] **M5.Q2 — Inline Markdown rendering** — render Markdown cells
  inline (between code chunks, in the same document view) via
  `pulldown-cmark` → styled text runs in `cockpit-render`. Headings,
  emphasis, lists, code spans, links, and images covered in v0.5; tables
  and footnotes deferred. Lives in a small new module in `cockpit-ui`
  (`notebook::markdown`) reused by both `.qmd` and any future
  Markdown-aware view.
- [ ] **M5.Q3 — Quarto render/export** — palette command
  `Quarto: Render` shells out to `mise exec -- quarto render <file>`
  to produce HTML/PDF/etc. Output path is reported in a status toast and
  opened via the OS handler — **no embedded WebView** (would add CEF/GTK
  deps and break the v0.6 instant-load target). Live preview is
  explicitly out of scope for v0.5; the in-editor inline rendering *is*
  the preview.

### dbt-lite project mode

- [ ] **M5.6 — Project detection** — a `models/` directory with `.sql`
  files and a `cockpit-analytics.toml` (or `[metadata.cockpit.analytics]`
  in `mise.toml`) marks an analytics project. Adds a new "Models" pane.
- [ ] **M5.7 — Templating** — minimal Jinja-subset for `{{ ref('name') }}`
  and `{{ source('schema', 'table') }}` only. Hand-rolled parser — no
  full Jinja dep. Resolution produces a CTE-wrapped final query.
- [ ] **M5.8 — Materialisations** — `view`, `table`, `ephemeral`
  (CTE-inlined). Configured via in-file `-- %% config: { materialized:
  table }`. Build command: `Models: Build All` / `Build Selected`.
- [ ] **M5.9 — DAG view** — read-time DAG (re-parsed on save, no
  background indexer — respects spec §3.9/§24). Renders model
  dependencies in the right pane; clicking a node opens the model.

### Sequencing note

M4.10 (trait injection) is a hard prerequisite for M5.1's `SqlEngine`
trait pattern. Notebook and dbt-lite can ship in either order after M5.1;
notebook is the smaller lift and proves the DuckDB transport.

---

## 8b. v0.6 — Instant load  (NEW — post-spec, displaced from v0.5)

Goal: cockpit feels native and instant on a **low-end Linux laptop**.
Hard targets:

| Metric                                    | Target  |
|-------------------------------------------|---------|
| Cold start → interactive window           | ≤ 100ms |
| Project open → three panes visible        | ≤ 100ms |
| First keystroke responsive                | ≤ 100ms |

These targets supersede the looser spec §24 numbers (which only required
"<2s cold start"). Spec §24 should be updated to match (folded into M4.9).

**Architectural note / risk:** 100ms cold start is aggressive — `winit` +
GL context creation alone can run 30–60ms on cold Mesa. The plan assumes a
*splash-then-hydrate* pattern: paint a shell at frame 1, finish init
behind it. If this proves unachievable on the slowest target hardware, we
will negotiate the budget rather than ship a synthetic green. Note that
v0.5's DuckDB integration is shell-out specifically to protect this
budget — the binary stays small, and the first query pays the spawn cost.

- [ ] **M6.1 — Cold-start benchmark harness** — `criterion` benches in
  `cockpit-testkit` + a CI-gated "cold start" integration test that fails
  on regression. Establishes the baseline number before any optimisation.
- [ ] **M6.2 — Splash-then-hydrate frame** — paint the empty three-pane
  shell on frame 1; defer project detection, tree-sitter grammar load,
  glyph atlas warm-up, and config parse to subsequent frames.
- [ ] **M6.3 — Lazy tree-sitter grammars** — grammars load on first file of
  that language, not at startup (currently eager).
- [ ] **M6.4 — Glyph atlas disk cache** — persist the warmed atlas to the
  OS cache dir; rebuild only on theme/font change.
- [ ] **M6.5 — Deferred LSP spawn** — verify spec §19 is honoured end to
  end (LSP starts on first relevant keystroke, never on launch).
- [ ] **M6.6 — Project-cache fast path** — recent-project open reuses the
  cached file-tree snapshot before re-walking the filesystem.
- [ ] **M6.7 — Startup tracing** — `tracing` spans tagged `startup.*`;
  debug command "Show Startup Trace" surfaces the breakdown.

---

## 9. Testing strategy realised  (maps spec §18)

| Spec §        | Realisation                                                       |
|---------------|-------------------------------------------------------------------|
| §18.1 pyramid | Many unit + golden; some integration + PTY; few smoke; few e2e.   |
| §18.2 unit    | `#[test]` colocated in every core crate; currently `cargo test`.  |
| §18.3 golden  | `insta` snapshots; `tests/golden/` per spec layout.               |
| §18.4 property| `proptest` on the editor buffer (rope vs reference string).       |
| §18.5 vim FSM | Pure `(buffer,cursor,keys)→(buffer,cursor,mode,registers)` goldens.|
| §18.6 project | Pure detection (tempdir) + opt-in real-`mise` tests; no installs. |
| §18.7 PTY     | `portable-pty` tests + Zellij command-construction tests; opt-in. |
| §18.8 smoke   | Assertions on the `cockpit-ui` view-model tree, not pixels.       |
| §18.9 CI      | `windows/macos/ubuntu-latest` matrix.                             |
| §18.10 fixtures| `tests/fixtures/`; small + deterministic; large ones generated.  |
| §18.11 commands| `mise` tasks (sole task runner) calling `cargo` directly.        |
| §18.12 manual | `cargo run -- --fixture <name>`.                                  |
| §18.13 logs   | `tracing` + the M3.7 debug surfaces.                              |

**Hermetic by default:** `cargo test --workspace` runs only fast,
deterministic tests. Integration and UI-smoke tests are Cargo-feature-gated and
opt-in (spec §25: slow/platform tests opt-in or nightly). `cargo nextest`
remains an optional future replacement for the default runner.

---

## 10. CI evolution

- **Phase 0 / v0.1:** fmt · clippy · build · `cargo test` on the 3-OS matrix.
- **v0.2:** add an integration leg (`--features integration`) — PTY + real mise.
- **v0.3:** add a UI-smoke leg (`--features ui-smoke`, offscreen GL); split fast
  integration (every PR) vs slow/platform (nightly) per spec §18.9.
- **Release:** `package` job builds per-OS release binaries; installer/package
  tests run release-only (spec §18.9).

---

## 11. Risk register  (extends spec §25)

| Risk                              | Mitigation                                              |
|-----------------------------------|---------------------------------------------------------|
| `termwiz` API fit / maturity      | `TerminalEngine` trait; proven in M0.5 before committing.|
| Zellij Windows maturity (§25)     | Fallback profiles: PowerShell / WSL / Git Bash / no mux. |
| Text-render perf on large files   | Glyph atlas + render-visible-lines-first (spec §15/§24). |
| Cross-platform PTY flakiness      | `portable-pty` abstracts it; pure vs integration split;  |
|                                   | slow/platform tests nightly (spec §25).                  |
| `tree-sitter` needs a C compiler  | Document toolchain in CI; pin grammar versions.          |
| `winit` Linux system deps         | Install X11 + Wayland dev libs in CI; document for devs. |
| Vim scope creep (§25)             | Ship a practical subset; lock it with golden conformance.|
| `spec.md` still says Zig          | Update or annotate the spec so the two don't diverge.    |

---

## 12. Sequencing & effort

Effort is T-shirt sized (S ≈ days, M ≈ 1–2 weeks, L ≈ 3+ weeks for one dev).
Calendar assumes a small team (≈2–3 devs) running Tracks A/B in parallel.

| Phase    | Milestones        | Effort | Notes                                  |
|----------|-------------------|--------|----------------------------------------|
| Phase 0  | M0.1–M0.7         | M      | Spikes de-risk before feature work.    |
| v0.1     | M1.1–M1.21        | L      | The bulk; A/B parallel, then C, then D.|
| v0.2     | M2.1–M2.10        | L      | Syntax + property/integration tests.   |
| v0.3     | M3.1–M3.7         | L      | LSP foundation is the long pole.       |
| v0.4     | M4.1–M4.6         | M–L    | LSP feature surface.                   |

**Critical path:** M0.5 (termwiz spike) → M1.11/M1.12 (terminal) → M1.16
(terminal view) → M1.18 (end-to-end). Start M0.5 on day one.

**Parallelism:** the headless cores (M1.1–M1.7) need neither a window nor the
terminal — a developer can build and fully test the editor, Vim FSM, project/
mise layer, and path detection while another brings up the render shell. That
parallelism *is* the payoff of decision #2.

---

## Out of scope (per spec)

No plugin marketplace (spec §10 principle, §3.10). No heavy background indexing
(§3.9, §24). LSP not before v0.3 (§19). No auto-install of mise tools (§8, §24).
