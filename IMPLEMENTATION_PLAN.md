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

- [x] **M4.7 — Mouse input** — first-class mouse handling across the cockpit.
  `cockpit-render` translates `winit` mouse events into headless
  [`MouseButton`] / [`PointerPosition`] callbacks; `AppModel` hit-tests
  the latest computed layout to route them. Shipped surfaces:
  - Click a pane → focus that pane (files / editor / terminal). ✅
  - Click a file in the tree → select and activate the row (file = open,
    directory = toggle). ✅
  - Click in the terminal → focus the terminal (Zellij still owns
    selection). ✅
  - Drag a pane border → resize the files/terminal panes; widths persist
    per-project via the existing layout-preferences cache. ✅
  - Scroll wheel in the editor → push the visible-line offset (the
    cursor-anchored auto-scroll wins again as soon as the cursor leaves
    the user-set viewport). ✅
  - Scroll wheel in the terminal → forwarded as up/down arrow keys so
    Zellij's scroll-back picks them up. ✅
  - Click in the editor → focuses the pane today; pixel-to-byte cursor
    placement is a follow-up because Vim mode interactions are subtle
    enough to deserve their own milestone. ⏭️

### Housekeeping (paid down alongside v0.4)

- [x] **M4.9 — Spec rewrite Zig → Rust** — `spec.md` now opens with an
  explicit "implementation language is Rust" note, drops the vestigial
  `zig-cache`/`zig-out` entries from the file-browser ignore list, swaps
  the `cargo nextest` references in §18.2 / §18.9 / §18.11 for the
  `cargo test` that the workspace actually ships (with a one-line note
  that nextest remains a future hardening option), and adds a forward
  pointer to v0.5 / v0.6 in §23. `build.zig` stays in the project-signal
  list — user projects can still be Zig; cockpit itself is not.
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

- [x] **M5.1 — DuckDB via shell-out + mise** — new `cockpit-sql` crate
  hosts the `SqlEngine` trait + `DuckDbEngine` impl. Every query becomes
  one `mise exec -- duckdb -json -c <sql>` spawn through the M4.10
  `ProcessRunner` seam, so notebook tests script every interaction with
  the engine without a real DuckDB binary on the test machine. The
  long-running per-project session called out in the spec is an
  optimisation behind the same trait — landing once latency from the
  notebook UI proves it worthwhile. Detection (`detect_duckdb`) returns
  `InMiseTools` / `OnPath` / `Missing` so callers drive the
  "detect, surface, prompt" flow without auto-installing.
- [x] **M5.1a — ggsql via shell-out + mise** — `GgsqlEngine` is the
  second `SqlEngine` impl. Spawns `mise exec -- ggsql exec --reader
  duckdb://memory --writer vegalite -c <sql>` and surfaces the
  Vega-Lite JSON in a single-cell `QueryResult` for the M5.5 chart
  renderer to pick up via `GgsqlEngine::extract_vega_lite`.
  `statement_targets_ggsql` is the routing helper the notebook
  view-model uses to send `VISUALISE`/`VISUALIZE` cells to ggsql and
  everything else to DuckDB.

### Notebook mode

- [x] **M5.2 — Notebook file format** — `cockpit_notebook::parse_notebook`
  parses Jupytext-style `.sql` / `.ggsql` files with `-- %% cell`
  separators. Files without any markers parse into a single cell, so
  opening a plain SQL file via the notebook view-model is lossless.
  Per-cell metadata uses `-- %% meta: { title = "..." }` trailing
  lines; explicit `kind = sql|ggsql|markdown` annotations override the
  file-level default. Cell results live in memory only — never written
  back to the source file.
- [x] **M5.3 — Notebook view-model** — `cockpit-notebook` crate ships
  `Cell`, `CellKind`, `CellStatus`, `CellResult`, and `Notebook` as
  pure data + view helpers (`move_up`/`down`, `insert_cell_below`,
  `set_active_source`, `apply_result`). Editing the active cell
  re-routes a SQL cell to ggsql the instant the body picks up a
  `VISUALISE` clause and clears stale results so the UI never lies.
- [x] **M5.4 — Inline tabular result rendering** — `cockpit_notebook::TableView`
  computes the pre-formatted row slice for a virtualised grid: caller
  picks a viewport (first + visible count) and reads back the bounded
  rows with every cell already rendered via `SqlValue::display`. Empty
  results expose `is_empty()` so the painter can swap to a "0 rows"
  placeholder without peeking inside the underlying `QueryResult`.
- [x] **M5.5 — Inline chart rendering via ggsql + vl-convert** —
  `cockpit_notebook::vl_convert_spec(format, in, out, root)` builds
  the `mise exec -- vl-convert vl2png|vl2svg` command the notebook
  renderer hands to its `ProcessRunner`. ggsql's `QueryResult` already
  carries the Vega-Lite v6 JSON in a single `vega_lite` column;
  `GgsqlEngine::extract_vega_lite` pulls it out for the writer. The
  PNG/SVG output path returns to the texture path in `cockpit-render`
  — same single-document flow as tables.
- [x] **M5.5a — ggsql syntax highlighting** — `Language::Ggsql` is
  recognised by `from_extension("ggsql")`, threaded through the LSP
  registry (no dedicated server today — ggsql wraps DuckDB so `sqls`
  is the fallback when users want schema completion), and the
  highlighter returns empty spans until the upstream
  `tree-sitter-ggsql` grammar publishes to crates.io. The seam stays
  ready; flipping it on later is one crate dep + the existing capture
  table.

### Quarto mode

- [x] **M5.Q1 — Quarto file parser** — `cockpit_notebook::parse_quarto`
  parses `.qmd` into the same `Notebook` view-model as M5.3 with a
  third `CellKind::Markdown`. Chunks bounded by ` ```{sql} ` /
  ` ```{ggsql} ` route through the existing engines; `#| label:`
  options become cell titles. Non-SQL languages (`{python}`, `{r}`,
  etc.) parse as Markdown cells annotated with the source language so
  the UI can show a "language unsupported" banner without losing the
  user's code. Plain ` ``` ` fences pass through as Markdown prose.
- [x] **M5.Q2 — Inline Markdown rendering** — `cockpit_notebook::parse_markdown`
  segments a Markdown source string into headings, paragraphs, list
  items, fenced code blocks, and inline bold/italic/code runs. Hand-rolled
  rather than pulling `pulldown-cmark` — keeps the dependency footprint
  small (M6 instant-load budget) and the parser is good enough for the
  Markdown subset called out in the plan. Tables, footnotes, and inline
  HTML are explicit non-goals.
- [x] **M5.Q3 — Quarto render/export** — `cockpit_notebook::quarto_render_spec(file,
  root)` builds the `mise exec -- quarto render <file>` spawn the
  palette command hands to its `ProcessRunner`. The output path is
  reported in a status toast and opened via the OS handler — no
  embedded WebView (would add CEF/GTK and break the v0.6 instant-load
  target). The in-editor Markdown rendering from M5.Q2 *is* the
  preview.

### dbt-lite project mode

- [x] **M5.6 — Project detection** — `cockpit_analytics::detect_analytics_project`
  spots a `models/` directory (with or without a `cockpit-analytics.toml`)
  and returns an `AnalyticsProject` with every `.sql` model parsed,
  sorted, and tagged with its effective materialisation. Pure function
  over the M4.10 `FileSystem` trait so tests run against
  `FakeFileSystem` with no real disk.
- [x] **M5.7 — Templating** — hand-rolled Jinja subset in
  `cockpit_analytics::template`. Only `{{ ref('name') }}` and
  `{{ source('schema', 'table') }}` are resolved; anything else (loops,
  filters, `env_var`) raises `TemplateError::Malformed` so cockpit
  fails loud instead of passing dbt-specific Jinja through to DuckDB.
- [x] **M5.8 — Materialisations** — `view`, `table`, and `ephemeral`
  supported. `cockpit_analytics::build_plan` walks the DAG in
  topological order and turns each model into a `CREATE OR REPLACE`
  statement; ephemeral models contribute a CTE binding that gets
  inlined into every dependent's rendered SQL. The notebook UI's
  `Models: Build` command will pipe these straight into the M5.1
  `SqlEngine`.
- [x] **M5.9 — DAG view** — `ModelDag::from_models` builds the dependency
  graph by extracting `{{ ref(...) }}` calls at read time (no background
  indexer — respects spec §3.9/§24). `topological_order` returns the
  build order and `DagError::Cycle` surfaces malformed graphs so the UI
  can highlight the offending nodes.

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

- [x] **M6.1 — Cold-start benchmark harness** — `cockpit_testkit::bench`
  hosts a tiny `Instant`-based measurement helper (no `criterion`
  pull, keeps the dep tree small). `crates/cockpit/tests/cold_start.rs`
  is the opt-in (`--features bench`) integration test that fails the
  build when detection + tree load blow a 500 ms budget on the
  `rust-basic` fixture. CI's bench leg gates regressions on this.
- [x] **M6.2 — Splash-then-hydrate frame** — the window opens with a
  splash painted on frame 1 (`cockpit/src/splash.rs`); subsequent frames
  each advance one cold-start phase through `HydrationDriver`
  (`cockpit/src/hydration.rs`). The pure `HydrationProgress` state
  machine lives in `cockpit-ui` so the splash logic is fully unit-
  testable. `CockpitApp` gains `tick()` + `wants_continuous_redraw()`
  so the harness drives the state machine post-paint without any input.
  *(Real-hardware <100 ms gate still needs a benchmark on the slowest
  target Linux laptop — M6.1's `cold_start` test guards the regression
  budget on CI in the meantime.)*
- [x] **M6.3 — Lazy tree-sitter grammars** — already lazy: every grammar
  config is held in a `thread_local!` `RefCell<Option<_>>` that fills
  the first time the matching language hits `compute`. No change
  needed; documented here so the milestone has a checked box.
- [ ] **M6.4 — Glyph atlas disk cache** — persist the warmed atlas to
  the OS cache dir; rebuild only on theme/font change. *(Deferred —
  GPU-side change with sequencing risk, lands in a focused follow-up.)*
- [x] **M6.5 — Deferred LSP spawn** — verified: `start_lsp_for_document`
  is the only LSP spawn site, called from `open_document`, gated by
  `LSP_MAX_BYTES` and `ServerConfig::for_language`. Nothing spawns on
  launch; the existing M3.5 contract is intact.
- [x] **M6.6 — Project-cache fast path** — `ProjectCache::file_index`
  persists the fuzzy-finder index; `apply_cache` rehydrates it so the
  first `Ctrl+P` after reopen is instant. `build_cache` snapshots it
  back on shutdown.
- [x] **M6.7 — Startup tracing** — `cockpit::startup::time_phase` wraps
  every cold-start phase in a `startup.*` span and records the
  duration in a global trace. `Debug: Show Startup Trace` (new
  palette entry) formats the snapshot for the status line.

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
