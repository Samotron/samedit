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
│   ├── cockpit-mux              # native mux session/window/pane model
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
| `cockpit-mux`      | core         | `serde`                                   | ✅ fully          |
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
- [x] **M6.4 — Glyph atlas disk cache** — `cockpit-render::atlas_persist`
  is the codec (magic + version + manifest + RGBA8 buffer) plus the
  `default_cache_path()` / `load_from_disk` / `store_to_disk` helpers.
  `GlyphRasterCache` keeps a CPU shadow of every atlas write and grew
  `snapshot(font_system, hash)` + `rehydrate(snapshot, font_system)`,
  which replays the allocator in stored order and bails on rect drift
  so the on-disk pixel layout stays in sync with the live allocator.
  `FramePlanner::warm_from_disk` / `persist_to_disk` glue the two
  together; the harness calls warm before frame 1 and persist on exit.
  Invalidation is driven by `font_set_config_hash(atlas_w, atlas_h,
  padding, sorted post-script names)` — any new font / atlas size / etc.
  drops the cache instead of risking a glyph mismatch.
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

## 8c. v0.7 — Single event loop + native multiplexer  (NEW — post-spec)

Goal: fix the launcher → project hand-off (currently broken) and replace
the external `zellij` dependency with an **embedded** terminal multiplexer
modelled on tmux's philosophy (server/client, sessions/windows/panes,
prefix-driven command vocabulary). Embedded because real tmux does not run
on Windows, and Zellij's Windows story is shallow enough that shelling out
to it from the cockpit makes Windows users second-class.

### Why now

1. **Real bug on `main`:** `cockpit::main::run_launcher` calls
   `cockpit_render::run_app(...)` for the launcher, then on
   `LauncherResult::OpenProject` calls `run_app(...)` *again* for the
   project window. `winit::EventLoop` is hard-coded to one-per-process and
   panics on the second call with `EventLoop can't be recreated`. The
   "open a project from the launcher" path therefore does not work end
   to end today — only `--project` / `--fixture` / starting in a project
   directory survives.
2. **Strategic alignment:** an embedded multiplexer pays for the v0.6
   instant-load work (no second binary to spawn), removes the §10 spec
   caveat about Zellij's maturity on Windows (§25 risk register), and
   collapses the cockpit↔mux IPC surface (no `zellij-server` socket, no
   KDL handshake) into a typed Rust API.

### M7.1 — Single event loop / `AppShell` state machine  *(unblocks the launcher)*  ✅

- **Hard rule:** `cockpit_render::run_app` is called **at most once per
  process**. Enforced via a `static AtomicBool` (`RUN_APP_CALLED`) inside
  `cockpit-render::app`; a second call panics with a message pointing the
  reader at `AppShell`.
- `ShellState` (in `crates/cockpit/src/app.rs`) gained a
  `Launcher(LauncherModel)` variant alongside the existing
  `Hydrating(HydrationDriver)` / `Live(AppModel)` / `Failed(_)` arms.
  `AppShell` (one `CockpitApp` impl) delegates `paint`, `tick`, `on_key`,
  and the mouse callbacks to whichever state is active.
- Launcher → project transition runs **inside** the same event loop:
  when `LauncherModel::result()` returns `OpenProject(path)`, `AppShell::tick`
  swaps in `ShellState::Hydrating(HydrationDriver::new(path))` on the next
  frame. `wants_continuous_redraw()` is true during `Launcher` so the
  post-paint `tick` reliably spots the result. Window title currently
  stays as set at `run_app` time; a `RedrawHandle::set_title` hook is
  parked for the (rare) split window-title case — not needed for the
  launcher → project handoff.
- `LauncherModel` now implements `CockpitApp` directly (was
  `&mut LauncherModel`) so it can be embedded by value in `ShellState`.
  It is no longer the *outer* harness — it's the **inner** state of
  `AppShell`. `LauncherResult::Exit` flips an `exit_requested` flag on
  the shell; `wants_exit` returns true and the harness drops out.
- Tests (`ui-smoke`):
  `app_shell_launcher_hands_off_to_hydration_in_place` builds an
  `AppShell::launcher`, simulates Enter on the only recent, ticks once
  to land in `Hydrating`, then drives `tick` until `Live` and asserts
  `project_name()`; `app_shell_launcher_escape_requests_exit` covers
  the Esc → clean-exit path. All without restarting the harness.
- **Done when:** `mise run ci` green (workspace tests + clippy + fmt);
  `mise run test-ui-smoke` green. Real-window verification on all three
  OSes still requires a CI push (cannot be observed from this machine).

### M7.2 — `cockpit-mux` crate: data model  *(headless, fully unit-tested)*  ✅

New crate. Pure data + transitions, zero `winit`/`glow`/PTY dependencies.
It owns the multiplexer state; `cockpit-terminal` owns the per-pane PTY +
termwiz grid; `cockpit-render` paints what `cockpit-mux` lays out.

```diagram
╭─────────────╮          ╭──────────────╮          ╭─────────────╮
│  Session    │──owns──▶│   Window     │──owns──▶│    Pane     │
│  (≥1)       │         │   (≥1)       │         │   (≥1)      │
│  name       │         │   name       │         │   PtyHandle │
│  active win │         │   layout     │         │   grid view │
╰─────────────╯         │   active pane│         ╰─────────────╯
                        ╰──────────────╯
```

- `Session { id, name, windows: Vec<Window>, active: WindowId }`
- `Window { id, name, layout: LayoutNode, active: PaneId }`
- `LayoutNode = Leaf(PaneId) | Split { dir: H|V, ratio: f32, a, b }`
  — same recursive split tree tmux uses. Rebalancing on close = collapse
  the parent.
- `Pane { id, pty: PtyHandle, scrollback_offset, mode: Live|Copy }`.
- Pure ops shipped: `new_window`, `kill_window`, `select_window`,
  `split_active(H|V)`, `kill_pane`, `next_pane`, `previous_pane`,
  `next_window`, `previous_window`, `swap_panes`, `resize_pane(±)`,
  `select_layout(preset)` (tmux's `even-horizontal`, `main-vertical`,
  `tiled`).
- Tests cover every operation above, with inline JSON snapshots for the
  full session tree after representative scripts. The crate has no
  `winit`/`glow`/PTY dependency; real terminals will map external handles
  to `PaneId` in M7.4.
- **Done when:** `cargo test -p cockpit-mux` green; workspace lint/test
  green before merge.

### M7.3 — Prefix-driven command vocabulary  *(reuses `cockpit-commands`)*  ✅

- Default prefix `Ctrl+b` (configurable; spec §20 `keys.terminal.prefix`).
  After the prefix, the next key is consumed as a mux command and
  **never** forwarded to the active pane's PTY — this is the
  "prefix → command" tmux interaction the spec doesn't yet describe.
- Tmux-subset bindings shipped by default:
  - `c` new window, `,` rename window, `&` kill window
  - `n` / `p` next / prev window, `0`–`9` select window N
  - `%` split horizontal, `"` split vertical, `x` kill pane
  - `o` cycle pane, `;` last pane, arrow keys to focus a direction
  - `z` zoom pane, `Space` cycle preset layouts
  - `[` enter copy mode, `]` paste, `d` detach
- Each binding resolves to a `CommandId` in `cockpit-commands` via
  `cockpit-mux::command_ids` and `PrefixDispatcher`; no PTY input path
  is involved. App palette registration lands with the M7.4 UI/render
  wiring, but the command vocabulary is already stable and testable.
- Pure FSM: `(state, key) → consumed? + Vec<MuxCommand>`. Tests cover
  every default binding, passthrough before prefix, unknown-key
  consumption after prefix, and the recorded keystream exit condition.
- **Done when:** `cargo test -p cockpit-mux` green; workspace lint/test
  green before merge.

### M7.4 — Multi-pane rendering inside the terminal area

- `cockpit-render` learns to subdivide the terminal-pane rectangle by
  walking the `LayoutNode` tree from `cockpit-mux`. Each leaf is a
  termwiz-grid blit (already shipped in M1.16).
- Implemented first headless/app slice:
  - `cockpit-mux::LayoutNode::pane_rects(bounds, active, border_px)`
    projects split trees into deterministic pane rectangles with border
    gaps; tests cover horizontal, vertical, nested, active-pane, and
    cramped-bounds cases.
  - `AppModel` now owns `MuxSession` + `PrefixDispatcher`; terminal focus
    gives `Ctrl+b` first refusal before the global router, so the mux
    prefix is not stolen by the legacy `Ctrl+b` files-pane toggle.
  - Prefix and palette-style mux command ids mutate the headless mux tree
    (`Ctrl+b %`, `Ctrl+b "`, window create/select/kill, pane cycle/kill,
    layout preset) and are visible in the debug pane-tree summary.
  - Terminal painting now walks the mux projection: inactive panes draw
    bordered placeholders, and the active pane renders the current
    `LiveTerminal` grid. This makes split state visible before the
    per-pane PTY map is introduced.
  - Mouse click focus is wired through the same projection:
    `cockpit-mux::Session::select_pane` updates the active pane, and
    app-level hit tests focus the clicked mux pane inside the terminal
    area.
  - Resize command ids (`mux.pane.resize_*`) are part of the prefix
    vocabulary: `Ctrl+b` then `Ctrl+arrow` adjusts the active pane's
    nearest enclosing split ratio through the same command dispatch path
    used by palette and tests.
  - The app now keys live terminals by `cockpit-mux::PaneId` instead of a
    single shared PTY. Input, paste/run helpers, terminal path detection,
    paint, cleanup, and resize fan-out target the active/visible mux
    pane.
  - Mouse drag on an internal mux pane border updates the active split
    ratio through the same headless `resize_pane` path; tests cover
    horizontal and vertical split resizing from projected rectangles.
    The drag records the pane adjacent to the divider so resizing follows
    the mouse target even if another mux pane was focused before the drag.
  - Built-in layout presets are tracked on each mux window, and
    `mux.layout.next` cycles `even-horizontal → main-vertical → tiled`
    instead of forcing a single preset.
  - `mux.pane.zoom` now toggles a headless per-window zoom flag; rendering
    projection returns only the active pane while zoomed without rewriting
    the underlying split tree, and focus changes while zoomed keep the
    newly active pane visible.
  - The command palette exposes the mux focus, swap, and resize
    vocabulary (`next/last/focus_up/down/left/right`, `swap_next`,
    `zoom`, and `resize_up/down/left/right`) plus numbered window
    selection commands, matching the prefix bindings and keeping those
    commands on the shared command spine.
- Pane borders: 1px lines from the theme; active pane gets an accent.
- Resize keys (`Ctrl+b` then `Ctrl+arrow`, tmux-style) adjust the
  enclosing split's `ratio`; visible leaf PTYs are resized from their
  projected rectangles.
- Mouse: drag a pane border to resize (reuses the M4.7 drag plumbing).
  Click a pane to focus.
- **Done when:** opening cockpit shows a single-pane window; `Ctrl+b %`
  produces two side-by-side shells, both responsive to input + resize.

### M7.5 — Detach / attach + session persistence

- Detach (`Ctrl+b d`): the active session keeps running in-process but
  becomes invisible; the workspace returns to the project's default
  view (or a "session list" overlay).
- Attach: from the session-list overlay, pick a session → it becomes
  visible again, layout + scrollback intact.
- **In-memory only for v0.7.** Cross-restart persistence (write-out the
  session tree + each PTY's scrollback on shutdown) is M7.5a, deferred.
  Spec §10 explicitly does not promise tmux's "survive cockpit exit"
  behaviour yet — say so.
- Headless data model: `cockpit_mux::SessionRegistry` holds every
  in-flight `Session`, tracks which is active and whether the workspace
  is attached, allocates fresh session ids on `create` / `add`, and
  exposes `detach` / `attach(id)` / `kill(id)` operations.
  `Session::set_name` is the rename hook the overlay will drive.
  Cross-session pane-id collisions are resolved by handing each
  registry-created session a disjoint id stride
  (`SESSION_ID_STRIDE = 1_000_000`) via the new
  `Session::with_id_base` constructor — splits inside that session keep
  allocating within its own range.
- Binary wire-up: `AppModel::mux_attached` flips on `Ctrl+b d`. While
  detached the terminal pane paints a session-list overlay and the
  PTYs keep running in the background. `Mux: New Session`,
  `Mux: Next Session`, and `Mux: Previous Session` palette commands
  park the current session into `AppModel::mux_parked` and swap a
  fresh / next session in; the pane-id stride keeps each session's
  PTYs reachable in the shared `terminals` map. The overlay walks
  the active session followed by every parked session — Up/Down or
  j/k move the cursor and Enter attaches to the highlighted entry.
- Tests: scripted detach → attach round-trip preserves the layout tree
  and pane focus.

### M7.6 — Scrollback + copy mode

- Each `Pane` tracks a bounded scrollback ring (config: lines per pane,
  default 10 000 — matches the spec §11 terminal expectations).
- Copy mode (`Ctrl+b [`): vi-keys (`h j k l w b 0 $ gg G /search`) over
  the scrollback buffer, mirrors the editor Vim FSM (M1.2 patterns) so
  there's exactly **one** Vim-style FSM in the codebase. Selection +
  yank → OS clipboard via `winit`.
  - Implemented base state: `mux.copy_mode.enter` records `PaneMode::Copy`
    on the active pane, resets its scrollback offset to the live edge, and
    terminal-focused keys are consumed until Escape returns the pane to
    `PaneMode::Live`. `j/k` update the active pane's scrollback offset
    with saturation; `h/l` move a copy-mode cursor inside the visible
    viewport; `0`/`$` jump the cursor to line edges; and `v` toggles a
    headless selection anchor at the cursor. This gives the later
    scrollback-ring renderer a stable viewport, cursor, and selection
    state. Copy-mode panes render their mode, offset, cursor position,
    and selection range in the mux pane label, and the command is also
    exposed through the palette.
  - Motions layer: `gg` / `G` jump to the top of the scrollback /
    live edge; `w` / `b` walk word boundaries on the active line using
    the visible grid row text. The chord handler tracks a single-key
    `g` pending flag so the second `g` completes `gg` without a parallel
    FSM. Word motions clamp to the same max column used by `h/l/0/$`.
  - Yank + search layer: `y` extracts the current selection text from
    the visible terminal grid, stashes it in `AppModel::mux_copy_yank`,
    and pushes it to the OS clipboard via `arboard`. Headless / display-
    less environments fail the clipboard write gracefully and surface
    `(clipboard unavailable)` in the status line so the workflow still
    finishes. `/` enters a search-input substate; characters accumulate
    into the pane's `CopySearch::query`, Backspace pops, Escape cancels,
    and Enter runs the forward search across the visible rows and jumps
    the cursor to the first match. `n` repeats the last completed
    search forward from the cursor.
- Tests: golden of the rendered selection after a recorded key script
  on a fixture scrollback.

### M7.7 — Status line / mode-line

- Bottom strip of the terminal pane: session name, window list with
  active marker, time, optional `mise` task status.
- Pure view-model in `cockpit-ui`; painter in `cockpit-render`.
- Configurable via spec §20 (`terminal.status.format`).
- Headless data + default render shipped: `cockpit_mux::Session::status_summary`
  emits a `StatusSummary { session_name, windows }` snapshot keyed by
  window index, name, active marker, and per-window pane count.
  `StatusSummary::render(extras)` produces the default tmux-style
  `[<session>] 0:name 1:name* …` formatted line, with optional
  `extras` (time, mise task, etc.) appended after a `│` separator.
- Painter: `paint_terminal` reserves an `STATUS_LINE_H` strip at the
  bottom of the terminal pane and renders the rendered status text on
  a theme-accent background. The mux pane-rect projection used by
  hit-tests, drags, and resize-sync now subtracts the same strip so
  the painted strip and the layout share bounds.
- Live extras: `AppModel::last_mise_task` records the most recent task
  kicked off via `run_mise_task`. The mode-line painter now drives a
  configurable template — `cockpit_config::TerminalStatusConfig::format`
  defaults to `"[{session}] {windows}"` and the substitutor recognises
  `{session}`, `{windows}`, `{task}`, `{pane}` plus `{{` / `}}` for
  literal braces. Unknown `{token}` references stay verbatim so users
  spot typos. A tz-aware `{time}` extra remains an explicit non-goal
  (would pull in a new dependency) until a real user asks for it.

### M7.8 — Native layout config

- New `cockpit_layout` field on `[metadata.cockpit]` (spec §9) — replaces
  `zellij_layout`. Format: KDL (keeps the `kdl` crate that v0.3 already
  pulled in for Zellij layouts; reuse parser, swap schema).
- Schema mirrors the `LayoutNode` data model: nested `split` nodes,
  leaf `pane` nodes with optional `command` strings (run on first
  attach). Smaller surface than the Zellij KDL — no plugin slots, no
  themes, no swap layouts (out of scope per AGENTS §2 hard rule #7).
- Loader lives in `cockpit-config::cockpit_layout`. ✅
  `CockpitLayout::from_kdl` / `::load` parse the schema into a
  `CockpitLayoutNode` tree with optional per-pane commands.
  `cockpit_mux::LayoutDescription` is the matching, runtime-side
  description type, and `Session::from_layout(name, description)`
  builds a session whose layout tree, pane ids, and active pane all
  derive from the description plus a `Vec<(PaneId, Option<String>)>`
  the caller can spawn against on first attach.
- Binary wire-up: `cockpit::mux_layout::resolve_cockpit_layout` reads
  `metadata.cockpit.cockpit_layout` from the project detection,
  loads + parses the KDL, and `AppModel::apply_cockpit_layout`
  replaces the default single-pane mux session with one built from
  the layout. Per-pane first-attach commands live in
  `AppModel::mux_pane_commands` until `ensure_terminal` consumes
  them — at which point the PTY spawns the layout command through
  the host shell (`sh -c` on Unix, `cmd.exe /C` on Windows) instead
  of the Zellij / fallback launcher. Bad / missing layout paths
  surface in the status line and keep the default session running.
- `CockpitMetadata::cockpit_layout` joins the existing `zellij_layout`
  field on the project metadata block so the v0.7 multiplexer can
  pick up the new schema while the legacy Zellij wiring is still
  live; M7.9 removes the Zellij field.

### M7.9 — Remove Zellij surface

- Delete `crates/cockpit-terminal/src/zellij.rs`,
  `crates/cockpit-config/src/zellij_layout.rs`, related tests/snapshots.
- Drop the `mise exec -- zellij attach --create …` plan from
  `crates/cockpit-terminal/src/session.rs`; replace with a
  `cockpit-mux::Session` bootstrapped in-process.
- Update `spec.md` §10 ("Terminal workspace integration"), §9 (metadata
  fields), §20 (config schema), §25 (risk register: drop the "Zellij
  Windows maturity" row, add "embedded mux: parity with tmux must be
  bounded"). Keep §11/§17 unchanged — the terminal engine is still
  `termwiz`.
- Update [`AGENTS.md`](AGENTS.md) §3–§4: rename
  "zellij" → "embedded multiplexer (`cockpit-mux`)" in the layout +
  decision table.
- Progress: cockpit no longer spawns Zellij from its own terminal path.
  `ensure_terminal` always spawns the host shell directly through the
  embedded multiplexer; `plan_launch` / `LaunchPlan` / `ShellProfile` /
  `PathBinaryLookup` / `resolve_zellij_layout` are out of the cockpit
  binary. `crates/cockpit-terminal/src/zellij.rs` and
  `crates/cockpit-config/src/zellij_layout.rs` are deleted along with
  the `ConfigError::ZellijLayout` variant; `CommandSpec` moved to
  `cockpit_terminal::command::CommandSpec`. The `--print` CLI keeps
  surfacing the legacy `zellij_layout` field as "deprecated, ignored"
  so existing user configs that still reference it keep parsing —
  the `CockpitMetadata::zellij_layout` field is preserved for
  serde compatibility but no code reads it. AGENTS.md now points at
  the embedded multiplexer in §1 and §3–§4; updating spec.md §10 /
  §9 / §20 / §25 is the remaining doc churn.

### M7.10 — Windows parity

- `portable-pty` already abstracts ConPTY; the multiplexer is pure Rust
  + threads, no Unix sockets, no PIDs to track. Walk through every
  v0.7 feature on a Windows CI runner and assert behaviour matches
  Linux: PTY spawn, resize, prefix dispatch, splits, copy-mode yank
  (clipboard), detach/attach.
- Add a Windows-only integration leg to CI under `--features integration`.
- **Done when:** the v0.7 exit checklist below is green on
  `windows-latest`.

### v0.7 exit checklist

- [ ] `mise run run` (no args) → launcher → pick recent → land in project
      workspace without process restart, on Linux, macOS, Windows.
- [ ] Inside a project, `Ctrl+b %` and `Ctrl+b "` produce splits, both
      panes are interactive shells.
- [ ] `Ctrl+b c` / `n` / `p` / `0`–`9` manage windows.
- [ ] `Ctrl+b [` enters copy mode; vi-style selection and yank work on
      all three OSes (clipboard verified).
- [ ] `Ctrl+b d` detaches; session-list overlay reattaches the same
      layout + scrollback.
- [ ] Per-project `cockpit_layout` KDL opens with the expected splits
      and runs each leaf's `command` on first attach.
- [ ] `zellij` binary is **not** in the dependency / detection / spawn
      paths; grep for `zellij` returns only deprecation comments.
- [ ] All three CI legs green: fast, integration (Linux + Windows),
      ui-smoke.

### Sequencing

```diagram
M7.1  ──▶  unblocks all UI work
            │
            ▼
M7.2 ─ M7.3 ─ M7.4   (headless mux ▶ key dispatch ▶ render)
            │
            ▼
M7.5  ─ M7.6  ─ M7.7   (detach/attach, copy-mode, status)
            │
            ▼
M7.8  ─ M7.9   (config schema, remove Zellij)
            │
            ▼
M7.10   (Windows parity gate)
```

M7.1 should ship as its own PR — it's a small, surgical fix that
unblocks the launcher today and does not need to wait for the
multiplexer work. M7.2 onward can start in parallel.

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| Scope creep — tmux is huge                    | Ship the v0.7 exit-checklist subset; everything else is M7.x+ follow-ups. No plugins, no scripting language, no nested sessions. |
| Two Vim FSMs (editor + copy mode) diverge     | Share the motion/operator core from `cockpit-editor`; copy-mode is a thin adapter. |
| Window-title updates per state transition     | Add `RedrawHandle::set_title`; if `winit` proves awkward, store the title on `AppShell` and reapply each `resumed`. |
| Cross-platform clipboard from copy mode       | Use `winit`'s clipboard API (already a `cockpit-render` dep); fall back to `arboard` only if forced. |
| Lose Zellij users who depended on plugins     | Document the removal; no plugin marketplace was ever promised (AGENTS §2 hard rule #7). |

---

## 8d. v0.8 — Catppuccin + tool-pane recipes  (NEW — post-spec)

Goal: ship the **Catppuccin** colour scheme as a first-class theme, and
turn the v0.7 multiplexer into a launchpad for the external tools we
actually use day-to-day — **Lazygit** for git, **Claude Code** /
**Codex** for AI — via reusable, keybindable "tool-pane recipes."

No AI engine code lives in cockpit. No git engine code beyond v0.3's
`git status --porcelain` badges. Both surfaces are just mux panes
running upstream CLIs the user already has. That is the whole point.

### Dependencies

- **M8.1 (Catppuccin)** is independent — ships now, does not need v0.7.
  Standalone PR, ~one file.
- **M8.2 / M8.3 (recipes + defaults)** require v0.7's `cockpit-mux` —
  specifically the floating-pane and toggle-pane primitives. Land
  after M7.4.

### M8.1 — Catppuccin theme

- ✅ `cockpit-render::Theme::catppuccin_latte() / _frappe() / _macchiato()
  / _mocha()` constructors ship with palette values pasted verbatim
  from https://catppuccin.com/palette (each `Color::hex(0x…)` carries
  the palette name in a trailing comment). `Color::hex` /
  `Color::hex_with_alpha` are the new const helpers backing them.
- ✅ `Theme::from_name(&str) -> Option<Self>` resolves
  `dark | latte | frappe | macchiato | mocha`, case-insensitively, and
  strips an optional `catppuccin-` alias prefix so
  `"catppuccin-mocha"` and `"mocha"` route the same way.
- ✅ `AppModel::apply_user_config` reads `ui.theme` through
  `Theme::from_name`. Unknown names log a `tracing::warn!` and leave
  the active theme alone — typo'd configs never crash the cockpit.
- ✅ `Theme: Switch <Flavour>` palette commands hot-swap the active
  theme immediately and persist the choice via
  `cockpit_config::write_ui_theme`, which round-trips the user config
  through `toml_edit` so comments / ordering / surrounding whitespace
  survive. The cockpit binary records the resolved user-config path
  in `AppModel::user_config_path` during hydration so the write-back
  doesn't re-resolve the platform config dir on every dispatch.
- ✅ Tests cover hex decoding, `from_name` aliasing + unknowns,
  per-flavour opacity, and the brightness ordering
  Mocha < Macchiato < Frappé < Latte (catches palette typos). The
  cockpit-side `apply_user_config_resolves_catppuccin_theme_names`
  test exercises the wiring end-to-end.
- **Done when:** `ui.theme = "mocha"` in the user config opens the
  cockpit in Mocha; `Theme: Switch Latte` from the palette
  hot-swaps without restart on Linux + macOS + Windows.

### M8.2 — Tool-pane recipes  *(needs M7.4)*

- ✅ Config schema in `cockpit-config::PanesConfig`:
  ```toml
  [panes.tools.lazygit]
  command  = "lazygit"
  layout   = "floating"   # floating | side-right | bottom
  toggle   = true         # second invocation hides the pane
  keybind  = "<leader>g"
  detect   = "lazygit"    # binary name (mise exec first, then PATH)
  ```
  Defaults: `layout = floating`, `toggle = true`, `detect = <first
  command word>`. `ToolPaneRecipe::detect_binary` returns the
  effective binary name for the probe.
- ✅ Each registered recipe becomes a `tool.<name>` palette command
  (dispatched via the existing `cockpit-commands` spine, AGENTS §2
  hard rule #5). `AppModel::apply_user_config` snapshots
  `config.panes.tools` into `AppModel::tool_recipes`; the palette
  `open` path appends the dynamic recipe entries to the static set.
- ✅ Detection: `run_tool_recipe` probes `recipe.detect_binary()`
  against the project's mise `[tools]` list and the host `$PATH` and
  refuses to dispatch with a "`<binary>` not found. `mise use
  <binary>@latest`?" toast (AGENTS §2 hard rule #6).
- ✅ Floating-pane primitive: `cockpit-mux::Session::open_floating`,
  `toggle_floating`, `show_floating`, `hide_floating`, `close_floating`
  manage a single overlay per session. `Session::floating_rect`
  projects an 80% × 80% centred rectangle for the painter, which now
  draws the overlay above the regular split tree.
  `mux.floating.toggle` / `mux.floating.close` palette commands manage
  the slot; tool recipes with `layout = "floating"` open into it. A
  repeat dispatch with `toggle = true` hides the overlay while the
  PTY keeps running; the third press resumes it without respawning.
- Side-right and bottom slotted layouts still type the command into
  the active pane today — dedicated docked panes are a follow-up.
- Mux gains two primitives on top of M7.4's split tree:
  - **Floating pane** — overlay rectangle centred over the project,
    sized 80% × 80%, drawn above the regular layout. Single floating
    pane per session; opening another replaces it. `Esc` or the
    recipe's keybind dismisses (when `toggle = true`).
  - **Toggle behaviour** — second keybind press hides the pane
    without killing the PTY; third press re-shows it with scrollback
    intact. Closes when the underlying process exits.
- Detection is the M4.10 `ProcessRunner` seam — try
  `mise exec -- which <name>` first (project tools win), then
  `which <name>`. Missing → palette toast: *"`lazygit` not found.
  `mise use lazygit@latest`?"* — never auto-install (AGENTS §2 hard
  rule #6).
- Tests (headless): recipe parses, keybind resolves to the right
  command, mux state transitions across show/hide/close, missing
  binary produces the expected toast.
- **Done when:** an arbitrary recipe in the user config gets a
  keybind, a palette entry, and a floating-or-docked pane on
  trigger.

### M8.3 — Default recipes shipped out of the box

Three default recipes baked into the binary (overridable in
user config — same merge rule as the existing `[keys.global]`):

| Name          | Command  | Layout    | Keybind        | Notes                                       |
|---------------|----------|-----------|----------------|---------------------------------------------|
| `lazygit`     | `lazygit`| floating  | `<leader>g`    | Lazyvim convention. Toggle on second press. |
| `claude-code` | `claude` | side-right| `<leader>aa`   | Full-height pane on the right.              |
| `codex`       | `codex`  | side-right| `<leader>ac`   | Same slot as Claude — only one at a time.   |

- Side-right panes share a slot: opening `codex` while `claude-code`
  is visible hides Claude and shows Codex. Avoids fighting over
  screen real estate while keeping both PTYs alive in the
  background. (Mux floating + slotted layout still pending — see
  M8.2.)
- ✅ All three recipes ship under `PanesConfig::default()` via
  `ToolPaneRecipe::default_lazygit / _claude_code / _codex`. An empty
  `[panes.tools]` section inherits them; users replace the whole
  table by re-declaring `[panes.tools.*]` (per-field merge across
  defaults is a follow-up if it proves valuable).
- **No** in-cockpit chat UI, file-context injection, diff-apply, or
  prompt rendering. The user's task #2 ("see #1") was explicit:
  *the mux is the integration.*
- **Done when:** fresh install of cockpit + `lazygit` + `claude` +
  `codex` on PATH → all three open with their default keybinds
  with zero config.

### M8.4 — Exit criteria

- [ ] `mise run run` opens the cockpit in Catppuccin Mocha when
      `ui.theme = "mocha"` (Linux + macOS + Windows).
- [ ] `Theme: Switch Latte` palette command hot-swaps the theme;
      config file picks up `theme = "latte"`.
- [ ] `<leader>g` toggles a floating Lazygit overlay on a real git
      repo; `Esc` dismisses.
- [ ] `<leader>aa` opens Claude Code in a side pane; `<leader>ac`
      replaces it with Codex; the hidden one stays alive in the
      background and resumes on toggle-back.
- [ ] Cold-start tracing shows the theme switch + recipe
      registration adds < 5 ms total (no regression on the v0.6
      100 ms budget).
- [ ] All three CI legs green: fast, integration, ui-smoke.

### Sequencing

```diagram
M8.1 (Catppuccin) ──┐
                    ├──▶  ships now, independent
                    │
M7.x (multiplexer) ─┴──▶  M8.2 (recipes) ──▶ M8.3 (defaults) ──▶ M8.4
```

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| Catppuccin palette drift over time            | Comment each `Color::rgb(…)` with the upstream hex; luminance assertion catches obvious typos. |
| Hot-swap theme leaves stale glyph atlas       | Atlas is glyph-keyed, not colour-keyed — colours are vertex attributes, so a theme swap re-paints next frame without atlas churn. Verify in a smoke test. |
| Recipe schema bloat (themes, env, cwd, args…) | Ship the minimal schema in M8.2; defer extensions until a real user asks. |
| Users expect deep AI integration              | Document that v0.8 ships AI as a pane, full stop. Inline diff-apply is a future v0.9+ candidate, not promised. |
| Lazygit / claude / codex missing on Windows   | Each recipe's `detect` step lets the palette surface a clean "not installed" toast — never crash, never auto-install. |
| Config writer corrupts user comments          | Use `toml_edit` for `Theme: Switch…` write-back (preserves comments + ordering); add a round-trip test on a commented fixture. |

---

## 8e. v0.9 — Lua extension system  (NEW — post-spec)

Goal: let power users extend the cockpit in **Lua** without turning it
into Neovim. The extension surface is **sandboxed behaviour
extensions** — extensions can register commands, keybinds, themes,
tool-pane recipes, and react to a small set of editor/mux events. They
**cannot** spawn processes, touch the filesystem, render pixels, or
escape the sandbox unless the user explicitly grants a capability.

This is intentionally a smaller surface than Neovim's plugin runtime.
We are not building a plugin marketplace (AGENTS.md §2 hard rule #7
stands — see M9.6 for how we re-state it). Extensions are local files
the user wrote or copied in. There is no `:PackerInstall`, no
discovery server, no auto-update.

### Why now (v0.9, not earlier)

- The TOML schema landed in v0.1 and grew through v0.2/v0.3/v0.7/v0.8.
  Extensions need a stable surface to bind to — registering a `theme`
  in v0.8's schema or a `tool-pane recipe` in v0.8 is *only* useful
  once both exist. Building Lua on top of moving schemas would force
  rewrites every release.
- v0.7's `cockpit-mux` and v0.8's recipe registry are the two
  biggest things extensions want to extend. Without them the API
  surface is too thin to justify the dependency.

### Runtime choice — **mlua + vendored Lua 5.4**

- `mlua` with the `lua54` + `vendored` features. Mature, fast, the
  same VM every other Lua-extensible editor ships, and vendoring
  avoids a system Lua dependency on Windows. The 5.4 binary cost
  (≈200 kB on Linux release) fits the v0.6 instant-load budget — we
  measure in M9.7.
- `mlua::Lua::sandbox(true)` strips `os.execute`, `io.popen`,
  `package.loadlib`, `loadfile`, `dofile`, and `require` of arbitrary
  paths. We layer additional restrictions on top (M9.4).
- Rejected: `piccolo` (still pre-1.0, smaller ecosystem); `rlua`
  (effectively unmaintained); `wasm` plugins (ten times the
  complexity for the same surface).

### Architecture

```diagram
╭──────────────────╮         ╭─────────────────╮       ╭──────────────╮
│ Extension files  │──load──▶│  cockpit-lua    │──reg──▶│ cockpit-     │
│ *.lua            │         │  (mlua, sandbox)│       │ commands     │
│                  │◀─event──│  event bus      │       │ cockpit-ui   │
╰──────────────────╯         │  capability gate│       │ cockpit-mux  │
                             ╰─────────────────╯       ╰──────────────╯
```

- New crate `cockpit-lua` owns the VM lifecycle, sandbox setup, event
  bus, and capability checks. **Headless** (AGENTS §2 #2) — does not
  depend on `winit`/`glow`/PTY. Tests run with no display server.
- The cockpit binary wires `cockpit-lua` to `cockpit-commands`,
  `cockpit-ui`, and (where relevant) `cockpit-mux`. The Lua bridge
  is a **registrar**, not an executor — when an extension
  `cockpit.keys.bind(…)`s, that lands as a normal `CommandId`
  binding (AGENTS §2 #5: commands are the single spine).
- Non-determinism (FS, process, clock) is **never** exposed to Lua
  directly. Capabilities go through the M4.10
  `FileSystem`/`ProcessRunner`/`Clock` seams so extensions stay
  testable and reproducible (AGENTS §2 #3).

### M9.1 — `cockpit-lua` crate scaffold

- New workspace member. Wraps `mlua::Lua`, creates the sandbox,
  installs the `cockpit.*` global, surfaces errors as typed
  `LuaError` (`thiserror`).
- One Lua VM per extension, not one shared VM. Isolates state so a
  panicking extension never trashes another. Memory cost is small
  (~50 kB per VM); benchmark in M9.7.
- Tests (headless): VM constructs, sandbox forbids `os.execute`,
  `print` is redirected to a captured buffer (verifies output
  capture path).

### M9.2 — Lua API surface

A single `cockpit` global, organised by namespace. The whole surface
is **registration + read-only inspection** — no mutation primitives
that escape the registered command system.

```lua
-- Register a palette command.
cockpit.commands.register {
  id    = "user.format-paragraph",
  title = "Format Paragraph",
  run   = function(ctx) ctx.toast("Hello from Lua!") end,
}

-- Bind a key to a command (resolves through cockpit-commands).
cockpit.keys.bind("<leader>fp", "user.format-paragraph")

-- Register a theme (palette is just a table of named colors).
cockpit.themes.register {
  name = "user.rose-pine",
  colors = { background = "#191724", text = "#e0def4", -- … },
}

-- Register a tool-pane recipe (same schema as v0.8 TOML).
cockpit.panes.recipe {
  name    = "user.btop",
  command = "btop",
  layout  = "floating",
  toggle  = true,
  keybind = "<leader>t",
  detect  = "btop",
}

-- React to editor/mux events.
cockpit.events.on("save", function(ctx)
  if ctx.language == "rust" then
    ctx.log.info("rust file saved: " .. ctx.path)
  end
end)
```

Available namespaces (full list):

| Namespace          | Calls                                       | Notes                                  |
|--------------------|---------------------------------------------|----------------------------------------|
| `cockpit.commands` | `register{…}`, `dispatch(id, args?)`        | Registers a `CommandId`.               |
| `cockpit.keys`     | `bind(chord, id)`, `unbind(chord)`          | Goes through `cockpit-commands`.       |
| `cockpit.themes`   | `register{name, colors}`, `current()`       | Theme name appears in `Theme: Switch…` palette. |
| `cockpit.panes`    | `recipe{…}` (v0.8 schema)                   | Recipe is registered with the mux.     |
| `cockpit.events`   | `on(event, fn)`, `off(handle)`              | Event names listed in M9.3.            |
| `cockpit.buffer`   | `text()`, `cursor()`, `language()`, `path()`| **Read-only.** Returns active buffer.  |
| `cockpit.project`  | `root()`, `name()`, `tasks()`               | Read-only project metadata.            |
| `cockpit.toast`    | `cockpit.toast("…")`                        | Status-line notification.              |
| `cockpit.log`      | `log.info/warn/error(…)`                    | Lands in the `tracing` log.            |

Explicit non-API (not exposed, ever): direct file IO, direct process
spawn, direct PTY/grid access, direct GL/painter calls, network,
clipboard write (read possibly later — capability-gated), reflection
on other extensions.

### M9.3 — Event hooks

Cockpit emits a small fixed set of events. Adding to this list is a
plan change, not a runtime change — the surface stays auditable.

| Event              | Fired when                                  | Context fields                                    |
|--------------------|---------------------------------------------|---------------------------------------------------|
| `editor.open`      | A buffer becomes active                     | `path, language`                                  |
| `editor.save`      | Save succeeded (after format-on-save)       | `path, language, bytes`                           |
| `editor.cursor`    | Cursor moved (debounced 50 ms)              | `path, line, col`                                 |
| `editor.mode`      | Vim mode changed                            | `path, mode` (`normal|insert|visual|command`)     |
| `mux.pane_focus`   | Active mux pane changed                     | `session, window, pane, command`                  |
| `mux.pane_exit`    | Pane's process exited                       | `session, pane, exit_code`                        |
| `palette.open`     | Command palette opened                      | `query`                                           |
| `project.open`     | Project finished hydrating                  | `root, name`                                      |

- Handlers run synchronously on the UI thread with a **5 ms** budget
  per event. Overrunning handlers are killed and the extension
  surfaces a one-line error in the status line. Repeated overruns
  disable the handler until reload.
- Tests: scripted event stream against a `FakeEventBus` asserts
  handler ordering, payload shape, and budget enforcement.

### M9.4 — Capabilities

Default-deny. Extensions declare what they need; the user grants in
config.

- Capability tokens (initial set):
  - `fs.read.project` — read files inside the project root
  - `process` — spawn declared commands via the `ProcessRunner`
    seam (subject to user-config allowlist)
  - `clipboard.read` / `clipboard.write` — OS clipboard access
- Declaration in the extension header:
  ```lua
  --[[ @cockpit:requires fs.read.project, process ]]--
  ```
- Approval in `~/.config/cockpit/extensions.toml`:
  ```toml
  [extensions."user.rust-toys"]
  enabled = true
  grants  = ["fs.read.project"]
  ```
- An ungranted capability raises a Lua error at call site; the
  extension can `pcall` and degrade gracefully. We never silently
  no-op a capability call.
- First-launch UX: an extension that declares a capability not yet
  granted shows a one-time palette prompt: *"`user.rust-toys`
  requests `process`. Grant? [y/N]"* — explicit user action only
  (AGENTS §2 #6 spirit).

### M9.5 — Hot-reload + error surfacing

- Extension files watched via `notify`; on change, the VM for that
  file is torn down and rebuilt. Other extensions are unaffected.
- Load/runtime errors land on the status line (`status` toast) and
  in the `tracing` log; **the cockpit never crashes on a bad
  extension** (AGENTS §6: "no `unwrap()` in non-test code").
- `Debug: Extensions` palette command (extends M3.7's debug
  surfaces) lists each extension's state: loaded / failed /
  disabled, plus the last error and timing snapshot.

### M9.6 — Docs + non-marketplace stance

- New `docs/extensions.md` — API reference + worked examples + the
  capability list.
- Update `AGENTS.md` §2 hard rule #7 from
  *"no plugin marketplace"* to
  *"no plugin marketplace, registry, or in-app installer — Lua
  extensions are user-authored or user-copied local files only."*
  Same intent, clarified.
- Update `spec.md` §10 to reference extensions; §24 to add the
  extension-load step to the cold-start budget.

### M9.7 — Performance gate

Cold-start regression budget (hard limits, asserted in CI under
`--features bench`):

| Step                                      | Budget          |
|-------------------------------------------|-----------------|
| `cockpit-lua` VM init (per VM)            | ≤ 5 ms          |
| Discover + parse `extensions/*.lua`       | ≤ 2 ms / file   |
| Run an extension's top-level register code| ≤ 10 ms typical |
| Total extension-system contribution       | ≤ 50 ms         |

If the budget is blown, M9 ships the API surface but defers loading
to first-use instead of cold start (lazy-load on first `cockpit.*`
call). Stretch goal, not a v0.9 blocker.

### M9.8 — Example extensions shipped in `runtime/extensions/`

Three default extensions — each demonstrates one event type and serves
as living documentation. Users can disable them in
`extensions.toml`:

1. **`runtime.format-paragraph`** — registers a command that
   re-wraps the active paragraph at the configured `editor.column`.
   Demonstrates `commands.register` + `buffer.text` + (with
   capability) buffer edit via a registered command.
2. **`runtime.session-toast`** — on `mux.pane_exit` with a non-zero
   exit code, surfaces a status toast. Demonstrates events + toast.
3. **`runtime.theme-by-time-of-day`** — switches between Catppuccin
   Latte and Mocha based on a daily schedule. Demonstrates theme
   registration + `clock` access (requires capability).

These ship in the binary as embedded strings (no extra IO cost on
fresh installs) and can be force-disabled in CI.

### Exit criteria

- [ ] Drop `~/.config/cockpit/extensions/hello.lua` containing
      `cockpit.commands.register{…}` → the command appears in the
      palette without restart on all three OSes.
- [ ] Lua extension that calls `os.execute("rm -rf /")` raises a
      sandbox error and is logged; cockpit stays up.
- [ ] An extension exceeding the 5 ms event budget gets disabled
      with a status-line message; other extensions keep running.
- [ ] The three `runtime/` examples work out of the box and are
      individually disablable.
- [ ] `mise run ci` green; bench leg confirms ≤ 50 ms cold-start
      contribution.

### Sequencing

```diagram
M7.x (mux) ─┐
            ├─▶ M8.x (recipes) ─┐
            │                   │
            │                   ▼
            └────────────▶ M9.1 (crate) ─▶ M9.2 (API) ─▶ M9.3 (events)
                                                              │
                                                              ▼
                                              M9.4 (caps) ─▶ M9.5 (reload)
                                                              │
                                                              ▼
                                              M9.6 (docs) ─▶ M9.7 (perf) ─▶ M9.8 (defaults)
```

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| Lua sandbox bypass                            | `mlua::Lua::sandbox` + explicit stripping of forbidden globals; fuzz tests assert the bypass list. |
| Cold-start regression                         | Hard 50 ms budget asserted in CI; lazy-load escape hatch (M9.7). |
| C dep makes Windows builds painful            | `lua54-vendored`; CI Windows leg already on the matrix. |
| API surface bloat over time                   | New `cockpit.*` namespace requires a plan update and a doc page — no drive-by additions. |
| Users want Neovim plugins to "just work"      | Document loudly that we are not Neovim; the API is small and curated by design. |
| One extension crashes another                 | One `mlua::Lua` per extension; budget enforcement; surface errors via status line. |
| Extensions become a load-bearing dependency   | Default extensions ship in-binary and are disablable; the cockpit is fully functional with zero extensions. |

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
