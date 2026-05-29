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

### M7.10 — Windows parity  ✅ (CI matrix; real-window verification still requires a push)

- `portable-pty` already abstracts ConPTY; the multiplexer is pure Rust
  + threads, no Unix sockets, no PIDs to track. Every v0.7 feature
  now runs on the Linux / macOS / Windows CI matrix: PTY spawn,
  resize, prefix dispatch, splits, copy-mode yank (clipboard),
  detach/attach.
- Windows-side quirks (PTY blocking wait, golden newline mismatches)
  landed in `e75938b` / `5163473` / `f187867` / `773be0d` so the
  integration leg passes on `windows-latest`.
- The v0.9 `cockpit-lua` crate (mlua + vendored Lua 5.4) compiles
  cleanly on all three OSes — the M9.7 `bench` job exercises VM
  init + load + register on every OS, surfacing any
  Windows-specific regression in cold-start cost.
- **Done when:** the v0.7 exit checklist below is green on
  `windows-latest`. Real-window verification (`mise run run`,
  `Ctrl+b %`, `Ctrl+b [`, `Ctrl+b d`, `<leader>g`) still requires
  pushing the branch.

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
- ✅ Leader chord support: `keys.global.leader` (default `Space`) is
  substituted into recipe keybinds, so the default `<leader>g`
  Lazygit binding becomes the two-stroke chord `Space g` and fires
  from the keymap without needing the palette. The dispatch path
  buffers the leader stroke when it lands as a single chord and
  combines it with the next chord to look up multi-stroke matches.
  Tools that don't carry a real `<leader>` substitution still work
  via their palette entry.
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

### M9.1 — `cockpit-lua` crate scaffold  ✅

- New workspace member. Wraps `mlua::Lua` (Lua 5.4 + vendored), runs
  every extension through [`api::apply_sandbox`] (strips `io`,
  `os.execute`, `dofile`, `loadfile`, `require`, `package`, `debug`,
  `collectgarbage`), installs the `cockpit.*` global, and surfaces
  errors as typed `LuaError` (`thiserror`).
- One Lua VM per extension, not one shared VM — `LuaRuntime.extensions`
  is `BTreeMap<String, Extension>` with one `mlua::Lua` per row.
  Isolates state so a panicking extension never trashes another.
- Tests (headless): VM constructs, sandbox forbids `os.execute`,
  `print` is redirected to a captured buffer, every default `cockpit.*`
  namespace is reachable.

### M9.2 — Lua API surface  ✅

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

### M9.3 — Event hooks  ✅

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

### M9.4 — Capabilities  ✅ (declaration + grant in place; namespaces in follow-ups)

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

### M9.5 — Hot-reload + error surfacing  ✅

- Extension files watched via `notify`; on change, the VM for that
  file is torn down and rebuilt. Other extensions are unaffected.
- Load/runtime errors land on the status line (`status` toast) and
  in the `tracing` log; **the cockpit never crashes on a bad
  extension** (AGENTS §6: "no `unwrap()` in non-test code").
- `Debug: Extensions` palette command (extends M3.7's debug
  surfaces) lists each extension's state: loaded / failed /
  disabled, plus the last error and timing snapshot.

### M9.6 — Docs + non-marketplace stance  ✅

- New `docs/extensions.md` — API reference + worked examples + the
  capability list.
- Update `AGENTS.md` §2 hard rule #7 from
  *"no plugin marketplace"* to
  *"no plugin marketplace, registry, or in-app installer — Lua
  extensions are user-authored or user-copied local files only."*
  Same intent, clarified.
- Update `spec.md` §10 to reference extensions; §24 to add the
  extension-load step to the cold-start budget.

### M9.7 — Performance gate  ✅

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

### M9.8 — Example extensions shipped in `runtime/extensions/`  ✅

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

- [x] Drop `~/.config/cockpit/extensions/hello.lua` containing
      `cockpit.commands.register{…}` → the command appears in the
      palette without restart. Covered by
      `cockpit::app::tests::load_lua_extensions_registers_palette_entries_and_dispatches_command`.
      Real 3-OS verification still needs a CI push.
- [x] Lua extension that calls `os.execute("rm -rf /")` raises a
      sandbox error and is logged; cockpit stays up. Covered by
      `cockpit_lua::tests::sandbox_blocks_os_execute`.
- [x] An extension exceeding the 5 ms event budget gets disabled
      with a status-line message; other extensions keep running.
      Covered by
      `cockpit_lua::tests::event_handler_overrun_eventually_disables`.
- [x] The three `runtime/` examples ship as embedded defaults
      (`runtime.format-paragraph`, `runtime.session-toast`,
      `runtime.theme-by-time-of-day`) and are individually
      disablable via `extensions.toml`.
- [x] `mise run ci` green; new `bench` CI leg runs the M9.7 budget
      tests on Linux, macOS, and Windows under
      `--features bench --release`.

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

## 8f. v0.10 — Go as a first-class language  (NEW — post-spec)

Goal: bring Go to parity with Rust, TypeScript, and Python — highlighting,
LSP, format-on-save, project detection, test runner. Smallest of the v0.10+
features; uses existing patterns end to end. No new crates.

### Scope

Go-the-language only. **Out of scope:** debugging (`delve`), profiling
hookups, build-tag-aware analysis, custom Go-template highlighting.
Those are follow-ups; the v0.10 exit checklist intentionally mirrors
"can I drive a Go project the way I drive a Rust one today".

### M10.1 — Highlighting + extension routing  ✅

- `cockpit-editor::highlight`: add `Language::Go`. Map `.go` (and the
  `.tmpl` Go-template files? — **no**, out of scope) via
  `Language::from_extension`. Dependency: `tree-sitter-go` (latest on
  crates.io). Wire a `GO_CONFIG` `thread_local!` matching the existing
  `RUST_CONFIG` lazy-init pattern (M6.3).
- Highlight capture table: extend the dotted-prefix `KIND_MAP` so Go's
  upstream highlight queries map to the existing `Kind` palette. Reuse
  the table; do not invent a Go-specific kind without a real palette
  need.
- **Note (post-shipping):** tree-sitter-go's upstream highlights query
  ends with a bare `(identifier) @variable` rule that overrides earlier
  function captures because tree-sitter-highlight resolves later patterns
  last. `build_go_config` strips that single rule from the query string
  before configuration so `func main()` declarations keep their
  `@function` highlight. Field-identifier vs method-name has the same
  shape (`@property` later than `@function.method`) but is left intact
  — methods render with the Variable kind today; revisit if a Go-shaped
  syntax-only PR justifies a fuller custom query.
- **Golden tests:** `tests/snapshots/golden_highlight__golden_go_basic.snap`
  covers imports, `package`/`func` keywords, struct + interface
  declarations, channels (`<-`), `//go:generate`, method receivers
  (`func (w *World) Hello`), and a `fmt.Sprintf` call.
- **Done when:** opening a `.go` file paints with the same kind set as
  the existing Rust fixtures, snapshot tests green.

### M10.2 — `gopls` LSP wiring  ✅ (unit + registry; integration deferred)

- `cockpit-lsp::registry`: add a `Language::Go` arm returning
  `ServerConfig { binary: "gopls", args: vec![], ... }`. Launch via
  `mise exec` (M4.0 contract) so per-project Go toolchains win.
- Defer-LSP-spawn (M6.5) applies unchanged: `gopls` only starts when a
  `.go` file opens and the file is under `LSP_MAX_BYTES`.
- All v0.4 LSP features — diagnostics (M4.1), definition/hover (M4.2),
  rename (M4.3a), completion (M4.3b), code actions (M4.5) — light up
  automatically once the registry knows about Go. No per-feature work.
- **Tests:**
  - Unit: `ServerConfig::for_language(Language::Go)` resolves.
  - Integration (`integration` feature): scripted `gopls` exchange via
    the existing fake JSON-RPC harness — `textDocument/definition` on
    a stdlib symbol, `publishDiagnostics` from a deliberate type
    error.
- **Done when:** `cmd-click` on a Go symbol jumps to its definition;
  type errors render in the gutter on a fixture project.

### M10.3 — Project detection + mise  ✅

- `cockpit-project::detect`: add `go.mod` to the signal-file list with
  a confidence equal to `Cargo.toml` / `package.json`. The mise layer
  already supports `[tools] go = "1.22"`; nothing new to parse.
- New fixture: `tests/fixtures/go-basic/` — minimal `go.mod`, one
  package, one `main.go`, one `_test.go`. Used by detection +
  highlight tests.
- **Done when:** opening the `go-basic` fixture lights up the project
  launcher with `Go` as the project kind and `gopls` as the lazy LSP.

### M10.4 — Format-on-save (mise task wins)  ✅ (planner; UI prompt unchanged)

- Reuse the M4.4 contract: if a `format` (or `format:go`) mise task
  exists, that wins. If no task exists but `gofmt` or `goimports` is
  detectable (in `[tools]` or on PATH), surface the M4.4 prompt:
  *"Add `format` task to `mise.toml` running `gofmt -w .`? [Y/n]"*.
  Never silently write.
- Server-side `textDocument/formatting` via `gopls` is the fallback
  when no formatter is detectable — same precedence as the existing
  languages.
- **Done when:** saving a deliberately mis-indented `.go` file in the
  `go-basic` fixture rewrites to canonical `gofmt` style.

### M10.5 — Test runner palette commands  ✅ (no `go list` cache; package = file's directory)

- Extend `cockpit-editor::nearest_test` with a `Language::Go` arm:
  walk the AST (already available from tree-sitter-go) to find the
  enclosing `func TestXxx(t *testing.T)`; return the test name.
- `Test: Run Nearest` / `Test: Run Current File` / `Test: Run All`
  (M3.3) gain Go resolution rules:
  - All → mise `test` task if present, else `go test ./...`.
  - Current file → `go test -run . ./<package-path>` (derive package
    from `go list -f '{{.ImportPath}}' <file>` — cached per file via
    `ProcessRunner` seam so repeated invocations don't re-spawn).
  - Nearest → `go test -run '^<TestName>$' ./<package>`.
- **Shipped behaviour vs plan:** package resolution skips the `go list`
  cache — `fallback_test_command` uses the file's directory verbatim
  as `./<dir>` (collapsing root-level files to `.`). This works for
  the typical one-package-per-directory layout; tag-based or
  multi-package directories fall back to the same `./<dir>` arg and
  let `go test` filter. A `go list` cache via `ProcessRunner` remains
  the right next step if real projects need it.
- `Go: Generate` palette command runs `go generate ./...`. No
  watch mode in v0.10. ✅ Shipped — refuses outside detected Go
  projects so the wrong toolchain never gets typed into a pane.
- **Tests:** golden `nearest_test` over `_test.go` fixtures; command
  construction tests for the three test scopes (in
  `cockpit-editor::test_runner`).
- **Done when:** every test scope works against the fixture; output
  appears in the active mux pane via the existing M2.2 "run task in
  the active pane" path.

### M10.6 — Ignore list + status badges  ⚙ (vendor/ ignored; `*.pb.go` glob WIP)

- `cockpit-project` default ignore list (spec §13): add `vendor/`
  (Go's vendored deps) and `**/*.pb.go` (generated protobuf). Do
  **not** ignore generated files broadly — users want to read them
  occasionally.
- Git status badges (M3.4) need no Go-specific work; `git
  status --porcelain` already covers `.go` files.

### v0.10 exit checklist

- [ ] Opening the `go-basic` fixture detects it as a Go project and
      lists the `go.mod`-defined toolchain.
- [ ] Syntax highlighting on `main.go` matches the Rust-equivalent
      coverage (functions, types, strings, comments, keywords,
      operators).
- [ ] `gd` jumps to a stdlib symbol; `K` shows hover docs.
- [ ] Saving an unformatted `.go` file applies `gofmt` via the
      mise task (or the M4.4 prompt fires once and never again).
- [ ] `Test: Run Nearest` from inside a `func TestFoo` runs only
      that test in the active mux pane.
- [ ] `mise run ci` green; CI matrix Linux + macOS + Windows
      includes a Go leg (CI installs Go via `mise install go@1.22`
      in the matrix step).

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| `tree-sitter-go` major version skew           | Pin via Cargo `=` constraint; document upgrade as a milestone-level change with a snapshot re-review. |
| `gopls` startup cost dominates LSP perf       | Deferred spawn (M6.5) means cost is paid on first `.go` open, never on launch. The bench leg (M6.1) does not need a Go fixture in v0.10. |
| `go list` is slow on cold caches              | Cache the file → package mapping in `ProjectCache::file_index` (M6.6); invalidate on `go.mod` mtime change. |
| Generated files (`*.pb.go`) cause LSP noise   | `gopls` already filters by build tags; document the `gopls.directoryFilters` config knob in the user-facing docs but ship no in-app UI for it. |
| CI installs of Go inflate matrix time         | Use `mise install` cache action in CI; expected +60 s per leg, acceptable for the value. |

### Sequencing

```diagram
M10.1 (highlight) ──┐
                    ├──▶ ready in parallel
M10.3 (detect)  ────┘
                    │
                    ▼
            M10.2 (gopls) ──▶ M10.4 (format) ──▶ M10.5 (tests) ──▶ M10.6 (ignore)
```

M10.1 + M10.3 are headless and independent — ship together as the first
PR. M10.2 builds on M10.1 (highlight enum). M10.4–M10.6 each chain off
M10.2 because the test/format flows can fall back to LSP when no mise
task exists.

---

## 8g. v0.11 — HTTP request runner (Bruno-style, git-tracked)  (NEW — post-spec)

Goal: a first-class HTTP client surface modelled on **Bruno** — requests
live as plain-text `.bru` files in the repo, environments are separate
files, results render inline beside the request. No proprietary store,
no SaaS sync, no separate app to keep in sync. The repo is the source of
truth.

### Why Bruno-compatible

- `.bru` files are git-friendly today, code-reviewable, and version
  controllable — matches the spec's headless-first ethos and the
  v0.5 notebook pattern (M5.2: files-as-source-of-truth).
- Cross-tool: a teammate using the upstream Bruno desktop app can
  open the same collection. We *consume* Bruno's format; we don't
  fork it.
- The format is small enough to parse ourselves — a custom grammar
  in `cockpit-http::parse` rather than an upstream JS dep we'd have
  to shell out to.

### Architecture

New crate `cockpit-http` (headless, AGENTS §2 #2). Splits into:

- `parse` — `.bru` file parser + serialiser (round-trip preserves
  comments and ordering, same constraint as `cockpit-config`'s
  `toml_edit` work in M8.1).
- `model` — `Request`, `Response`, `Collection`, `Environment`,
  `Variable` data types. Pure data.
- `engine` — `HttpEngine` trait + `ReqwestEngine` impl. **No tokio**
  (AGENTS §2 #4); `reqwest::blocking` on a dedicated I/O thread,
  responses flow back via a channel. A `FakeHttpEngine` records
  scripted exchanges for tests.
- `runner` — orchestrates pre-request scripts (Lua, reusing M9.x),
  variable interpolation, and authenticated sends.

`cockpit-notebook` is the *reference* — same shape (parser →
view-model → inline result rendering), different domain. Cribbing
its layering is the cheapest way to get this done.

UI lives in `cockpit-ui::http` (view-model) and `cockpit-render`
(painter). The "send" command goes through `cockpit-commands` like
everything else (AGENTS §2 #5).

### M11.1 — `.bru` parser + serialiser  ✅ (canonical round-trip; byte-identical deferred)

- Hand-rolled parser over the documented Bruno grammar
  (block sections: `meta`, `<method>`, `headers`, `body:<kind>`,
  `query`, `vars:pre-request`, `vars:post-response`, `auth:*`,
  `script:pre-request`, `script:post-response`, `assert`, `docs`).
  Tokenisation is line-oriented; bodies inside braces are
  free-text until the matching `}` at column 0.
- Round-trip preservation: parse → serialise must be a no-op on a
  well-formed file (golden tests with `insta`). Adding a header
  via the UI inserts at the documented position; never reorders
  unrelated content.
- **Shipped behaviour vs plan:** parse → serialise round-trips
  **semantically** (the second parse yields the same [`Request`]),
  not byte-identical. The serialiser is opinionated about block
  order (`meta`, verb, headers, query, body, auth, docs) and uses
  two-space indentation inside body blocks so JSON `}` characters
  don't terminate the block early. The byte-identical round-trip
  needs an edit-AST layered on top (similar to `toml_edit` for
  `cockpit-config`) and lands with the M11.4 view-model.
- Recognised today: `meta`, every HTTP verb block, `headers`,
  `query`, `body:{none,text,json,xml,form-urlencoded}`,
  `auth:{none,basic,bearer}`, `docs`. Unknown blocks
  (`vars:*`, `script:*`, `assert`, future OAuth) silently round-trip
  by being ignored on parse — preserved-but-not-edited surfacing is
  M11.4 work.
- **Tests:** `tests/fixtures/http/` — `list-users.bru` (GET + bearer
  auth + docs) and `create-user.bru` (POST + JSON body + basic auth).
  `tests/golden_round_trip.rs` parses every fixture and checks the
  parse → serialise → parse loop stays semantically stable.
- **Done when:** every fixture round-trips byte-identical; malformed
  files surface a typed `ParseError` with line:col, never panic.

### M11.2 — Collection + environment model  ✅ (interpolation; OS-env fallback deferred)

- Detect a `bruno.json` or `cockpit-http/` directory at the project
  root → `Collection { root, requests: Vec<Request>, environments:
  Vec<Environment> }`. Collections nest: subdirectories become
  folders.
- `Environment` parses `environments/<name>.bru` (Bruno's spec) into
  `{ name, vars: HashMap<String, String> }`. Active environment is
  per-project state (persisted in `ProjectCache`, M6.6).
- Variable interpolation: `{{varName}}` resolved against the active
  environment, falling back to OS env when the env file declares
  `process.env.FOO`. Cycles are detected and surfaced as a typed
  error.
- **Shipped behaviour vs plan:** `detect_collection_root`,
  `load_collection`, environment file parsing, transitive
  interpolation with cycle detection, and disabled-row (`~`) skipping
  are all in place. `process.env.FOO` style OS-env fallback is *not*
  shipped — it lands with the M11.3 engine where the
  environment-vs-process boundary actually matters. Active-environment
  persistence in `ProjectCache` is left for the binary wiring (lands
  with M11.4).
- **Tests:** env switching, variable resolution, missing-var
  diagnostics, cyclic references.

### M11.3 — `HttpEngine` trait + reqwest impl  ✅ (worker, cancellation, integration tests; redirect-chain capture deferred to M11.4)

- `trait HttpEngine { fn send(&self, req: PreparedRequest, cancel:
  &CancelHandle) -> Result<Response, HttpError>; }` — blocking,
  synchronous on the caller's perspective. Real impl uses
  `reqwest::blocking::Client` on a per-call worker thread with an
  `mpsc` channel boundary (so the UI thread never blocks on network).
- `PreparedRequest`: post-interpolation, post-script, ready-to-ship
  HTTP. `Response`: status, headers, body bytes, timing, redirects.
- TLS: default platform native-tls (reqwest default); custom CA
  bundles work via `SSL_CERT_FILE` (the standard OpenSSL convention).
- Cancellation: the engine takes a `CancelHandle` (clonable
  `Arc<AtomicBool>`) that the UI can trip when the user hits `Ctrl-C`
  on an in-flight request. The caller's thread polls the handle every
  50 ms while waiting on the worker, so cancellation actually wakes
  the call rather than waiting for reqwest's timeout.
- **Shipped behaviour vs plan:** the engine trait, every type
  (`PreparedRequest`, `PreparedBody`, `Response`, `RedirectHop`,
  `HttpError`, `CancelHandle`), `FakeHttpEngine`, and `ReqwestEngine`
  are all in place. `prepare_request(&Request, &Environment) ->
  PreparedRequest` handles `{{var}}` interpolation across
  URL/headers/query/body/auth, appends url-encoded query strings,
  derives `Authorization` headers (Basic base64-encoded; Bearer
  literal) with a user-`Authorization`-wins rule, sets default
  `Content-Type` per body kind, and skips disabled (`~`) rows.
  Redirect-chain capture in `Response.redirects` is left empty for
  now — needs a custom `redirect::Policy` and only matters once the
  M11.4 view-model surfaces the chain. `final_url` is already filled,
  so callers can detect that a redirect happened.
- **Tests:** 48 unit tests + 2 golden + 7 integration. Integration
  suite lives in `tests/integration_reqwest.rs` behind the
  `integration` feature, runs against a hand-rolled
  `TcpListener`-based mock (avoids a wiremock/tokio dev-dep) and
  covers happy-path GET, redirect chain, 4xx, 5xx, per-request
  timeout, unreachable host, and a real cooperative-cancel mid-flight.
  CI runs them on Ubuntu/macOS/Windows via the existing `integration`
  job; mirrored in `mise run test-integration`.

### M11.4 — View-model + render  ✅ (view-model + tabs + send pipeline + M11.4.1 painter)

- `cockpit-ui::http`: `HttpView` holds the active collection,
  selected request, in-progress send state, latest response. Mirrors
  the notebook view-model structure (`Notebook` → `Cell`s) — one
  active request at a time, history is the file's git history.
- Render layout (inside the editor pane when a `.bru` file is
  active):
  - Top half — request editor (Vim mode, normal editor surface —
    no special key handling needed beyond `:HttpSend`).
  - Bottom half — response panel. Tabs: **Body** (pretty-printed
    JSON if `application/json`; XML if XML; raw otherwise),
    **Headers**, **Timing**, **Raw**. Switching tabs is a palette
    command (default `<leader>h1..h4`).
- Resize the split with the existing M4.7 mouse-drag plumbing.
- **Shipped behaviour vs plan:** the headless view-model is in —
  `HttpView` (collection + selected request + per-request `RequestRun`
  + active environment + `SplitLayout` ratio with `[0.15, 0.85]`
  clamp), `ResponseTab` (Body / Headers / Timing / Raw with `next` /
  `prev` cycle + direct setters), `ResponseView` (per-tab headless
  render including hand-rolled JSON pretty-printer that preserves key
  order without a serde round-trip), and the `send_selected` /
  `prepare_selected` helpers that bind `HttpView` to any `HttpEngine`
  impl (real `ReqwestEngine` or scripted `FakeHttpEngine`). 22 unit
  tests cover the state machine, JSON pretty-printer, tab cycle,
  split clamps, environment switching with unknown-name guard, and
  the engine round-trip. The actual painter wiring inside
  `cockpit-render` (mouse-drag resize handle, tab strip rendering,
  `cockpit::hydration` recognising `.bru` files and constructing
  `HttpView` on open) lands in M11.4.1 — same pattern as how the
  notebook view-model shipped before its painter.
- **M11.4.1 update (headless hit-testing geometry):** the painter's
  mouse maths is now headless-testable in `cockpit-ui::http`, ahead of
  the GPU glyph work (AGENTS §2 #9 — mouse goes through layout
  rectangles, not `winit` types outside `cockpit-render`).
  `SplitLayout` gained a `handle: Rect` — a `SPLIT_HANDLE_THICKNESS`
  (6 px, matching the M4.7 pane-border band) divider band centred on
  the request/response boundary and clamped inside the viewport — plus
  `SplitLayout::handle_contains(x, y)`. `HttpView::drag_split_to(viewport,
  pointer_y)` maps a live drag's pointer-y to a clamped split ratio.
  For the tab strip, `HttpView::tab_strip(response_pane, cell_width,
  row_height)` lays out one `Rect` per `ResponseTab` left-to-right
  (monospaced: width = `(label.len() + 2 * TAB_PADDING_CELLS) *
  cell_width`, gap-free), returning a `TabStrip { tabs, active }`;
  `TabStrip::hit(x, y)` and the binary-facing
  `HttpView::click_response_tab(...)` route a click to the tab under the
  pointer (right/bottom edges exclusive so adjacent tabs never both
  claim a column). 7 new unit tests (handle centring, `handle_contains`
  band, `drag_split_to` clamp, gap-free strip layout, click-activates-
  hit, click-misses-below).
- **M11.4.1 update (painter wired — M11.4 closed):** the binary now
  paints the `.bru` split and routes its mouse. `AppModel::paint_http`
  splits the editor content with `http_layout` (shared pure geometry:
  request half on top, a 2 px divider, response panel below), renders
  the request through the existing `paint_document` into the top sub-
  rect, then draws the tab strip from `HttpView::tab_strip` (so the
  painted rectangles are byte-identical to the hit-test rectangles via a
  shared integer `http_tab_cell_width`) and the active tab body from
  `response_view_lines`. Mouse: `handle_http_click` starts a
  `DragState::HttpSplit` on the divider band or activates a tab via
  `HttpView::click_response_tab`; `on_pointer_move` feeds the drag into
  `drag_split_to`. `.bru` recognition was already wired in
  `open_document` (`recognise_http_request`). 3 binary tests
  (tab-click activates, divider-drag grows the request half, request-
  half click is a no-op) on top of the 7 view-model tests. Workspace
  green: fmt + clippy `-D warnings` + full test suite.
- **Done when:** opening a `.bru` file shows the split; running
  the request populates the response panel; tab switching works. ✅

### M11.5 — Commands + keybinds  ✅ (folder batch, save-response, env persistence; painter-bound polish in M11.4.1)

Registered in `cockpit-commands`:

- `Http: Send Request` (default `<leader>hs`) — sends the active
  request through the engine, populates the response panel.
- `Http: Send All In Folder` — runs every request in the current
  folder sequentially; results stream into a side log pane.
- `Http: Switch Environment` (default `<leader>he`) — opens a
  palette listing parsed environments.
- `Http: Copy As cURL` — emits the equivalent curl command to the
  clipboard for sharing.
- `Http: Save Response To File` — writes the latest body to
  `responses/<request-name>.<ext>`, never overwrites without
  confirm.
- **Shipped behaviour vs plan:** `cockpit_ui::http::command_ids`
  defines every stable id (`http.send_request`,
  `http.switch_environment`, `http.copy_as_curl`,
  `http.save_response`, `http.next_tab`, `http.prev_tab`,
  `http.tab.{body,headers,timing,raw}`) plus a
  `default_keybindings()` table shipping `<leader>hs`, `<leader>he`,
  and `<leader>h1..h4`. `cockpit_http::PreparedRequest::to_curl`
  emits POSIX-quoted curl invocations (URL + `-X METHOD` when non-GET
  + `-H` per header + `--data-binary` for bodies; form bodies
  url-encoded by `PreparedBody::to_bytes`). The binary
  (`crates/cockpit/src/app.rs`) recognises `.bru` files on open via
  `recognise_http_request` → constructs an `HttpView` (auto-selecting
  the request whose `meta.name` matches the file stem), binds the
  default chords via `bind_http_chords` at init, exposes every HTTP
  command in the palette, and dispatches the synchronous commands:
  `Http: Copy As cURL` (writes to the OS clipboard via the existing
  `arboard` hook), `Http: Show {Body|Headers|Timing|Raw} Tab`, and
  `Http: Next/Previous Response Tab`. `Http: Send Request`,
  `Http: Switch Environment`, `Http: Send All In Folder`, and
  `Http: Save Response To File` register and surface a status
  pointing at M11.5.2 — they need the async-engine thread + a
  sub-palette and land with the M11.4.1 painter.
- **M11.5.2 update (async send + env sub-palette):** the binary now
  owns a lazily-built `Arc<dyn HttpEngine + Send + Sync>` and an
  `HttpInFlight` slot (channel `Receiver<Result<Response, HttpError>>`
  + `CancelHandle` + `SentSummary`). `Http: Send Request` spawns a
  named `cockpit-http-send` worker that calls `engine.send` off the
  UI thread, nudges the `RedrawHandle` on completion, and stores the
  receiver; `tick()` calls `poll_http_inflight` each frame, applies
  the result to the active `HttpView`, and updates the status with
  the round-trip time. `Http: Cancel In-flight Request`
  (`http.cancel`) trips the cancel handle. `Http: Switch Environment`
  opens `PaletteMode::HttpEnvironments` — a sub-palette listing every
  parsed environment plus a `(none)` entry; selection routes through
  `apply_http_environment` (uses the `(none)` sentinel since
  environment names are file stems and never `(none)`). A second
  Send while one is in flight bails with a status pointing at the
  cancel command. Six new tests in `app::tests` (drive via
  `FakeHttpEngine` end-to-end, both success and failure paths, env
  palette population, the in-flight guard, and the cancel no-op).
  Remaining for M11.4.1: the painter (mouse-drag split handle + tab
  strip glyphs) and `Http: Send All In Folder` / `Http: Save Response
  To File` — those need the painter's pane-context to know which
  folder the cursor is in.

### M11.6 — Scripts (Lua, capability-gated)  ✅ (parse, capability, warnings, execution, per-collection grants)

- Bruno's JS pre/post scripts don't run; cockpit substitutes
  **Lua** scripts (reuses M9.x sandbox + capability model).
- New script block `script:lua-pre-request` and
  `script:lua-post-response`. Same env access as v0.9 extensions:
  `cockpit.http.set_var(name, value)`,
  `cockpit.http.response()` (read-only access to the just-received
  response), with sandbox restrictions intact.
- Capability `http.scripts` required to run any script; default-deny.
  User grants per collection in `~/.config/cockpit/extensions.toml`.
- **Out of scope:** JavaScript runtime, full Bruno script
  compatibility. Users with JS scripts get a clear toast: *"Lua
  scripting only; JS scripts skipped. See docs/http.md."*
- **Shipped behaviour vs plan:** the parser recognises
  `script:lua-pre-request` and `script:lua-post-response` blocks and
  pulls them into `Request::{pre_script, post_script}` (both
  `Option<String>`); the serialiser emits them back so round-trip
  stays semantic. `script:js-pre-request` / `script:js-post-response`
  (and the unprefixed `pre-request` / `post-response` Bruno spellings)
  set the new `Request::has_js_scripts` flag instead of dropping
  silently. `cockpit_lua::Capability::HttpScripts` is registered with
  token `http.scripts`, so user `extensions.toml` grants and the
  `parse_requires_header` parser already accept it. `cockpit_ui::http::script_warnings(view,
  http_scripts_granted)` emits the toast lines — the JS-skipped
  message and the default-deny Lua skip — and the binary's
  `Http: Send Request` handler logs them via `tracing::warn!` before
  dispatching. Actual Lua execution (running the pre-script against a
  mutable env, the post-script against a read-only response) lands
  with M11.6.1 — needs a per-collection capability store and a Lua
  bridge into `cockpit-http`'s `Environment`. 5 new tests in
  cockpit-http (round-trip + JS-flag + unknown-variant) + 4 new in
  cockpit-ui (script_warnings matrix).
- **M11.6.1 update (Lua execution wired):** `cockpit_lua::http_scripts`
  ships `run_pre_request(source, &mut env.vars)` and
  `run_post_response(source, response, &mut env.vars)`. Both spin a
  fresh `mlua` VM with the same `apply_sandbox` the v0.9 extensions
  use (no `io`, no `os.execute`, no `package.loadlib`, no `require`).
  The bridge installs `cockpit.http.set_var(name, value)` and
  `cockpit.http.var(name)` against an `Arc<Mutex<BTreeMap>>` so
  mutations land back on the caller's environment in place; post-
  response scripts additionally get `cockpit.http.response()`
  returning a read-only table with `status`, `headers` (1-indexed
  list of `{name, value}` rows so Lua's `#` works), and `body`. Pre-
  request scripts that call `cockpit.http.response()` raise a clear
  error rather than returning nil. The binary owns a new
  `http_scripts_granted: bool` field (default-deny; tests flip it
  directly — a per-collection grant store reading
  `~/.config/cockpit/extensions.toml` lands in M11.6.2). `Http: Send
  Request` now clones the active env, runs the pre-script when
  granted, threads the mutated env through
  `cockpit_http::prepare_request`, then dispatches — script failures
  land in the status without ever spawning a worker. 8 new tests in
  `cockpit_lua::http_scripts` (set/read vars, sandbox blocks
  `os.execute`, runtime errors, response-shape iteration) + 3 in the
  binary (granted path mutates headers, ungranted path skips, script
  failure aborts before dispatch).
- **M11.6.2 update (post-response wired):** `HttpInFlight` now captures
  the request's `post_script` source + `active_environment_name` at
  send time, so a mid-flight selection switch can't swap which script
  runs. `poll_http_inflight` drains those fields before applying the
  result, then on `Ok(response)` runs
  `cockpit_lua::run_post_response`. `HttpView::replace_environment_vars(name,
  vars)` writes the script's `cockpit.http.set_var` mutations back to
  the in-memory environment so the next request picks them up; on-disk
  persistence to `environments/<name>.bru` still routes through the
  M11.4.1 editor surface. Script failures surface as a `(post-script
  failed: …)` suffix on the round-trip status without flipping the
  response status (the engine call still succeeded). 5 new tests
  (3 binary integration: write-back, failure surfacing, ungranted
  skip; 2 view-model: replace-vars happy path + unknown-name error).
  Remaining for full M11.6: a per-collection grant store reading
  `~/.config/cockpit/extensions.toml` so the binary's
  `http_scripts_granted` is sourced from real config instead of a
  test-only flag.
- **M11.5 / M11.6 closeout (this milestone):**
  - `Http: Save Response To File` writes the latest body to
    `<collection-root>/responses/<sanitised-name>.<ext>`, picking
    the extension from the response `Content-Type` (`application/json`
    → `json`, `text/html` → `html`, …; unknown types fall through to
    `.bin`). File-stem sanitisation replaces every char outside
    `[A-Za-z0-9._-]` with `_` and collapses runs, and conflicts open
    a `ConfirmPrompt` (`PromptIntent::OverwriteHttpResponse`) rather
    than clobbering silently.
  - `Http: Send All In Folder` walks the open `.bru` file's parent
    directory, matches every `.bru` filename against the collection's
    `Request::meta.name`, queues those indices in
    `AppModel::http_batch`, and dispatches them sequentially through
    the same `http_send_request` path used by single sends.
    `poll_http_inflight` advances the queue after each response and
    suffixes the status with `[i/N]` progress; `Http: Cancel` aborts
    the queue and the in-flight worker together.
  - `cockpit_lua::http_grants` parses
    `~/.config/cockpit/extensions.toml`'s `[http]
    granted_collections` array and answers `is_granted` against any
    collection root via a parent-path prefix walk (so granting a
    monorepo root covers nested `cockpit-http/` directories). The
    binary loads grants alongside the rest of the config in
    `apply_config_file` and consults them at send time —
    `http_scripts_granted` is now the union of the test-only override
    and the on-disk grant.
  - `ProjectCache::active_http_environment` round-trips the chosen
    Bruno environment across restarts; `apply_cache` stashes it in
    `AppModel::pending_http_env` so the freshly-built `HttpView`
    applies it the first time `open_document` constructs one.
    Unknown env names from a stale cache are silently dropped rather
    than aborting the open.
  - 13 new tests across `cockpit-lua::http_grants` (parser + grant
    semantics) and the binary (`sanitize_file_stem`,
    `response_extension`, `http_save_response` happy path + overwrite
    confirm, `http_scripts_grants_unlock_pre_script_for_listed_collection`).
    Workspace 851 pass / 0 fail, fmt + clippy `-D warnings` green.

  M11.4.1 painter and M11.7 docs were the last v0.11 work outstanding;
  both are now done (see the M11.4.1 painter note above and M11.7 below),
  closing v0.11.

### M11.7 — Docs  ✅

- `docs/http.md` — collection layout/detection, opening the split,
  the command + keybinding table, environments + `{{var}}`
  interpolation, the Lua scripting model (capability-gated,
  default-deny; JS skipped), what's not supported (JS scripts, the
  OAuth2 device-code flow until v0.11.1, the GraphQL response
  introspection panel), the security model (verified TLS, secrets via
  env vars, no cleartext password storage, sandboxed scripts), and a
  worked example on the `tests/fixtures/http/` collection.

### v0.11 exit checklist

- [x] Cockpit recognises a Bruno collection at the project root
      (M11.2: `detect_collection_root` + `load_collection`; `.bru`
      files open straight into the split via `recognise_http_request`.
      A dedicated launcher row/badge for HTTP collections is a small
      cosmetic follow-up, not on the v0.11 critical path).
- [x] Opening a `.bru` file shows the split request/response view.
      View-model (`HttpView` + `SplitLayout`) plus the M11.4.1 painter
      (`AppModel::paint_http`) draw the request half, divider, tab
      strip, and active-tab body; the divider drags and tabs click.
- [x] `<leader>hs` sends through `ReqwestEngine` on a worker thread;
      `poll_http_inflight` lands the response on the view next tick
      (sub-frame for the fake engine in tests).
- [x] `{{baseUrl}}`-style interpolation works against the active
      environment (M11.2 + M11.3 `prepare_request`).
- [x] Switching environments via `<leader>he` survives a restart —
      `ProjectCache::active_http_environment` round-trips through
      `apply_cache` / `build_cache`.
- [x] `Http: Copy As cURL` produces a valid POSIX-quoted curl
      invocation including headers and body
      (`PreparedRequest::to_curl`).
- [x] Bruno fixture collections round-trip (`tests/golden_round_trip.rs`).
- [x] `cargo test --workspace` + clippy `-D warnings` green; the
      `integration` job runs the `cockpit-http` engine against the
      hand-rolled `TcpListener` mock (a wiremock dep would have
      pulled tokio).

### Sequencing

```diagram
M11.1 (parse) ──▶ M11.2 (collection)
                       │
                       ▼
              M11.3 (engine) ──▶ M11.4 (view) ──▶ M11.5 (commands)
                                                      │
                                                      ▼
                                             M11.6 (lua) ──▶ M11.7 (docs)
```

M11.1 + M11.2 are pure-text-processing and ship as the first PR.
M11.3 is independent (just needs the model from M11.2) and can run
in parallel with M11.4 once the data shape is locked. M11.6 depends
on the v0.9 Lua VM; if v0.9 lands first this is "free", otherwise
it's the long pole.

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| `.bru` format changes upstream                | Pin parser to a documented Bruno version; ship a `cockpit-http: format mismatch` toast on unknown blocks and skip them rather than crashing. |
| `reqwest` brings in a chunky dep tree         | Use `reqwest` with `default-features = false` + `rustls-tls`, `json`, `gzip`, `stream` only. Measure binary-size delta against the v0.6 budget. |
| Secret leakage via copy-as-curl               | Mask `Authorization:` headers in the clipboard output unless the user holds `Shift` (palette suffix variant); show a toast either way. |
| Stuck requests block the UI                   | Engine runs on a dedicated thread; CancelHandle ships with every send. Test: a `wiremock` `delay(60s)` request cancels cleanly in <100 ms. |
| JS-only Bruno scripts break user workflows    | Doc the gap loudly; provide a `Help: Translate Script To Lua` palette command that opens an issue template with the JS source pre-filled. |
| Bruno auth flows (OAuth2 device-code)         | Ship Basic + Bearer + API-key in v0.11; OAuth2 device-code is v0.11.1, gated on a real user asking. |

---

## 8h. v0.12 — Ambient services + Org-mode jot  (NEW — post-spec)

Goal: an Emacs-Org-compatible task/note surface that's reachable **even
when the cockpit window is minimised or closed**, plus the shared
infrastructure that the v0.13 launcher reuses. Users point cockpit at a
folder of `.org` files (default `~/org/`); the same files open
unchanged in Emacs, Logseq, Beorg, Orgzly, or any other Org tool. This
is the first v0.x to ship a sibling binary with its own process,
system-tray icon, and global hotkey.

### Why Org-mode

- Same pattern as v0.11's Bruno-compat: the **files on disk are the
  source of truth**, in a format other tools already speak. No
  proprietary store, no schema migrations to fear, no lock-in.
- Org is decades-stable, plain-text, diff-friendly, and the agenda /
  capture / scheduling vocabulary is already what most note-and-task
  systems eventually converge on.
- Storage is "a folder" — works with any sync layer the user already
  runs (Syncthing, git, Dropbox, iCloud Drive). Sync is the user's
  problem, not ours.

### Why a sibling binary

- AGENTS.md §1.7 ("M7.1: at most one `winit::EventLoop` per process")
  is non-negotiable. A second always-on window means a second process.
- A sibling binary keeps the cockpit's instant-load budget intact (M6.x)
  — the tray app boots independently and is a few hundred kilobytes.
- The split lets a user run jot/launcher without ever opening the
  full cockpit, which is the whole point of the "even when minimised"
  ask.
- IPC is the boundary, not a shared `static`: same crate workspace, but
  two binaries that talk via a documented Unix socket / Windows named
  pipe protocol.

### Architectural pieces

| Piece                            | New crate / location                  |
|----------------------------------|----------------------------------------|
| System tray icon + menu          | `cockpit-tray` (wraps `tray-icon`)     |
| Global hotkey registration       | `cockpit-hotkey` (wraps `global-hotkey`)|
| IPC protocol + serde types       | `cockpit-ipc` (Unix socket + named pipe)|
| Frameless floating window shell  | `cockpit-popover` (winit + glow, headless-tested view-models elsewhere) |
| Org-mode parser + index          | `cockpit-org` (wraps `orgize`)         |
| Capture template engine          | `cockpit-org::capture`                 |
| Agenda query engine              | `cockpit-org::agenda`                  |

The first four crates are **shared infrastructure** — v0.13 reuses
them for the launcher. The `cockpit-org` crate is jot-specific and
the only entirely new domain crate in v0.12.

### Org-mode subset shipped in v0.12

In scope:

- Hierarchical headlines (`*`, `**`, `***`…) with title, tags
  (`:work:urgent:`), and TODO keyword.
- TODO state cycling on the default `TODO | DONE` workflow.
  Custom workflows (`TODO | NEXT | WAIT | DONE`) are v0.12.x.
- `SCHEDULED:` and `DEADLINE:` timestamps with the standard
  `<2026-06-01 Mon>` / `<2026-06-01 Mon 09:00>` / repeater
  (`+1w`, `++1d`) syntax.
- Active vs inactive timestamps (`<...>` vs `[...]`).
- Plain-text body paragraphs and lists.

Out of scope (explicit non-goals; revisit via v0.12.x as users ask):

- `:PROPERTIES:` / `:END:` drawers, custom TODO workflows,
  effort estimates, archive files.
- Babel executable code blocks, org tables + table calc,
  clocking (`CLOCK:` / time tracking), org-roam-style note
  graphs, LaTeX/math rendering.
- Emacs-style link resolution (`[[file:foo.org::Headline]]`).
  External URLs render as links; org-internal links render as
  inert text in v0.12.

### M12.1 — `cockpit-ipc`: protocol + transport

- Wire format: length-prefixed CBOR (small, fast, schema-evolution
  friendly via `serde`). One newline-delimited JSON fallback for
  debugging via `socat`.
- Transport: Unix domain socket at `$XDG_RUNTIME_DIR/cockpit.sock`
  (Linux/macOS), Windows named pipe `\\.\pipe\cockpit`. Single
  socket; messages are tagged with a target service name.
- Services: `jot`, `launcher`, `cockpit` — each is one channel.
- Connection lifecycle: server (whichever process is the host)
  registers services; clients open a connection and address messages.
- Auth: the socket lives in a user-only directory; no cross-user
  reach. Document explicitly that this is *not* a remote protocol.
- **Tests:** scripted client/server pair in a tempdir; message
  ordering, reconnect, malformed-frame handling, per-OS smoke.
- **Impl notes (M12.1):** shipped as `cockpit-ipc`. Frame =
  4-byte BE length + 1-byte encoding tag + body; the tag (`0`=CBOR,
  `1`=JSON) lets one socket accept either, so a `socat`/JSON poke and
  the production CBOR stream interoperate (tested). CBOR via
  `ciborium`, JSON via `serde_json`. The transport is **generic over
  the payload** (`Envelope<T> { service: ServiceId, payload: T }`),
  so each service defines its own message enum. Unix-socket transport
  (`IpcListener`/`Connection`) is `#[cfg(unix)]` and fully tested
  (round-trip, ordering, reconnect, stale-socket cleanup); the wire
  types + codec are platform-independent. `MAX_FRAME` (16 MiB) guards
  against a hostile length prefix. **Windows named-pipe transport is
  a documented follow-up** (`WINDOWS_PIPE_NAME` constant exists; the
  sandbox is Linux-only, so the pipe impl ships when it can be
  smoke-tested). The `ipc-smoke` CI leg / per-OS smoke is pending the
  Windows impl.

### M12.2 — `cockpit-tray`: system tray icon

- Thin wrapper over `tray-icon` (the same crate Tauri uses; mature
  on all three OSes).
- Public API: `Tray::new(icon)`, `tray.set_menu(...)`,
  `tray.on_left_click(...)`, `tray.on_menu_item(...)`. Headless
  fake (`FakeTray`) for tests.
- **Tests:** unit on the headless fake; smoke on real `tray-icon`
  behind the `ui-smoke` feature (gated to non-headless CI legs).
- **Impl notes (M12.2):** shipped as `cockpit-tray` — the
  backend-free half. A flat [`Menu`] of `Action`/`Separator` items
  (no submenus), a [`Tray`] seam (`set_menu`/`set_tooltip`), and a
  `TrayEvent` stream (`LeftClick` / `MenuItem(id)`). `FakeTray`
  records the menu/tooltip and simulates clicks; activating a
  missing or disabled item is a no-op. The real `tray-icon` backend
  is deferred behind the `ui-smoke` feature (needs a tray host to
  smoke-test); it isn't a dependency yet, so the default build pulls
  no GUI crates.

### M12.3 — `cockpit-hotkey`: global hotkey registration

- Wrapper over `global-hotkey` (also Tauri-aligned). Maps chord
  strings (`Ctrl+Alt+J`) to typed `Hotkey` records.
- Conflict detection: if the requested chord is already owned by
  another app at register time, the wrapper returns a typed error
  and surfaces a toast in the tray. Never silently fails.
- **Tests:** headless fake hotkey bus; integration tests behind
  `ui-smoke` register a low-collision chord
  (`Ctrl+Alt+F12`-class) and assert the callback fires.
- **Impl notes (M12.3):** shipped as `cockpit-hotkey` — the
  backend-free half. `Hotkey::parse` normalises chords
  (case-insensitive; `cmd`/`win`/`meta`→super; chars upper-cased;
  `F1`..=`F24`; named keys) so conflict comparison is by value, with
  typed `HotkeyError`s (`EmptyChord`/`MissingKey`/`UnknownToken`/
  `UnknownKey`/`Conflict`). The `HotkeyBus` seam + `FakeHotkeyBus`
  model external ownership (conflict fires, never silent) and
  simulated presses. The real `global-hotkey` backend is deferred
  behind `ui-smoke` (needs a windowing leg); not a dependency yet.

### M12.4 — `cockpit-popover`: frameless floating window

- New `winit`+`glow` shell, **independent of `cockpit-render`** —
  smaller, no editor / no mux / no tab-bar. AGENTS §1's "only one
  crate uses winit" rule reads as *per binary* (M7.1: "one event
  loop per process"); sibling binaries each get their own.
- Frameless, always-on-top, centred or anchored to the active
  display. ESC dismisses. Loses focus → dismisses (configurable).
- Reuses `cockpit-render`'s glyph atlas + theme + painter via a
  trimmed-down `Painter` extracted into a `cockpit-paint` crate
  during this milestone (one-line job — the painter is already
  decoupled). **Atlas disk cache (M6.4) is shared across both
  binaries via the existing on-disk codec.**
- The popover's *content* is a view-model implementing a small
  `PopoverContent` trait (`tick`, `paint`, `on_key`, `wants_exit`).
  The jot app and the launcher each provide an impl.
- **Tests:** unit on `PopoverContent` impls (headless); smoke on
  the real window behind `ui-smoke`.
- **Impl notes (M12.4 headless half):** the painter + theme extraction
  shipped as the new headless `cockpit-paint` crate — `theme` (`Color`,
  `SyntaxTheme`, `Theme`) and `painter` (`Painter`, `Rect`, `TextRun`,
  `DrawCommand`, `RectBatch`) moved verbatim out of `cockpit-render`,
  which now re-exports them under their original module paths
  (`cockpit_render::{painter, theme}`) so every existing caller compiles
  unchanged. `cockpit-paint` depends only on `cockpit-commands` (for the
  `KeyChord` the popover trait consumes) — no `winit`/`glow` — so the
  sibling popover binaries paint with the same primitives and share the
  on-disk glyph atlas. The `PopoverContent` trait (`theme` / `tick` /
  `paint` / `on_key` / `on_text` / `wants_exit`) + `PopoverViewport`
  live in `cockpit_paint::popover`; `on_key` returns whether the content
  consumed the chord, and the pure `esc_should_dismiss(consumed, chord)`
  helper encodes the "unconsumed bare `Escape` dismisses" shell policy
  (focus-loss dismissal stays the shell's concern). A reference
  `PopoverContent` impl in the unit tests exercises every method.
  The concrete jot content impl shipped alongside as
  `cockpit_jot::JotPopover` (see M12.6) — proving the trait hosts a real
  multi-surface view-model. Remaining display-bound work: the
  `winit`+`glow` popover shell that hosts a `PopoverContent`, and the
  v0.13 launcher's content impl.

### M12.5 — `cockpit-org`: parser, store, view-model

- New crate. Wraps `orgize` for parsing. Domain types
  are thin layers over orgize's AST so we don't fight the upstream
  model:
  - **Version deviation (M12.5 impl):** pinned to **`orgize 0.9`**
    (stable), not `0.10`. The only published `0.10` is an
    `alpha` rewrite with an unstable, undocumented API; the stable
    `0.9` parses the v0.12 headline grammar (title / keyword /
    priority / tags) cleanly. Round-trip is unaffected — we never
    use orgize's serialiser (see the line-range rule below).
  - **Timestamp deviation (M12.5 impl):** orgize `0.9`'s timestamp
    parser silently *drops* any stamp carrying a repeater/delay
    cookie (`<2026-06-01 Mon +1w>` fails to parse, losing the whole
    `SCHEDULED`). Since the agenda (M12.5b) needs repeaters, the
    `SCHEDULED:/DEADLINE:/CLOSED:` planning grammar is parsed by a
    small in-crate parser (`timestamp.rs`) instead. orgize still
    owns the inline headline grammar.
  - **Position deviation (M12.5 impl):** orgize `0.9` exposes no
    source ranges, so `line_range`s are computed by scanning
    headline lines (`^\*+(space|eol)`) and zipping them, in
    document order, against orgize's pre-order headlines (1:1 for
    the v0.12 subset, which has no code/example blocks).
  - `OrgFile { path, content_hash, headings: Vec<Heading> }`.
  - `Heading { level, title, todo_state, tags, scheduled,
    deadline, body, children, line_range }`.
  - `Timestamp { date, time, repeater, is_active }`.
  - `OrgRoot { root_dir, files: BTreeMap<PathBuf, OrgFile> }`.
- **Round-trip preservation is non-negotiable.** Same constraint
  as v0.11 / M8.1: parse → mutate → serialise leaves untouched
  blocks byte-identical (whitespace, comments, blank lines). We
  edit by **line-range replacement** on the original source
  buffer, never by re-emitting the full AST. Golden round-trips
  guard this on every fixture.
- Storage: a single user-configured root folder, default `~/org/`.
  Cockpit walks the folder once on start, watches it via `notify`
  (already a dep — M9.5) for live updates, and keeps every
  `.org` file's parsed form in memory. **No database.** No
  separate "cockpit metadata" sidecar files — anything cockpit
  needs is encoded in standard Org syntax.
- Default folder layout written on first launch (only if the root
  is empty, never overwrites):
  ```
  ~/org/
    inbox.org       # capture default lands here
    tasks.org       # project work, scheduled items
    notes.org       # zettel-style notes
    journal.org     # date-tree, one heading per day
  ```
  Users can repoint anywhere — the layout is a default, not a
  requirement. A folder full of pre-existing `.org` files Just
  Works.
- View-models in `cockpit-ui::org` (reusable from both the main
  cockpit and the popover): `OrgListView`, `AgendaView`,
  `CaptureView`. All pure data — same shape used by notebooks
  (M5.3).
- **Tests:** `orgize` round-trip on a corpus of Emacs-authored
  fixtures (committed in `tests/fixtures/org/`); headline
  enumeration; timestamp parsing for every form documented in
  the Org manual (active/inactive/date-only/with-time/repeater);
  line-range edit preserves siblings byte-identical.

### M12.5a — Capture templates

- A capture template is a typed slot the user fills in once and
  commits to a specific destination file + heading. The templates
  themselves live in `~/.config/cockpit/org.toml`:
  ```toml
  [org]
  root        = "~/org"
  default_todo_keywords = ["TODO", "DONE"]

  [[org.capture]]
  key      = "t"
  name     = "Todo"
  target   = { file = "inbox.org", under = "Tasks" }
  template = """
  * TODO %?
    :PROPERTIES:
    :CREATED: %U
    :END:
  """

  [[org.capture]]
  key      = "n"
  name     = "Note"
  target   = { file = "notes.org", under = "Inbox" }
  template = "* %? :note:\n  Captured %U from %a"

  [[org.capture]]
  key      = "j"
  name     = "Journal"
  target   = { file = "journal.org", datetree = true }
  template = "* %U %?"
  ```
- Substitution tokens (mirrors Emacs `org-capture-templates`):
  - `%?` — cursor position after expansion.
  - `%U` / `%u` — inactive date-time / inactive date.
  - `%t` / `%T` — active date / active date-time.
  - **Impl note (M12.5a):** implemented with Emacs semantics — the
    `u`/`U` pair is *inactive* (`[...]`), the `t`/`T` pair is
    *active* (`<...>`), and the upper-case member of each pair
    carries the time. (The earlier prose paired these as
    "inactive/active timestamp of now", which mislabels the axis;
    "mirrors Emacs" wins.) `%(...)` resolves via an injected
    evaluator — the `cockpit-lua` wiring (capability
    `org.capture.lua`) lands with the jot binary in M12.6; absent an
    evaluator the token expands to empty. Expansion lives in
    `cockpit-org::capture`; "now" is an injected `NowStamp`, not the
    `Clock` trait, keeping the crate hermetic (the binary converts
    `clock.system_now()` to a calendar value).
  - `%a` — annotation: the editor's `path:line` if a buffer is
    active, otherwise empty. (Cockpit's contribution beyond
    Emacs's `%a`: when capture fires from a launcher action that
    carries context, the context becomes the annotation.)
  - `%i` — initial content: the current selection if any.
  - `%(lua-expr)` — Lua expression evaluated under the v0.9
    sandbox (capability `org.capture.lua`, default-granted).
- `target.datetree = true` slots the entry under
  `* 2026` → `** 2026-05` → `*** 2026-05-28 Thu`, creating
  missing levels.
- **Tests:** each token's substitution against frozen-clock fakes
  (the M4.10 `Clock` trait); date-tree insertion in an empty file
  and in one with existing date headings; capture preserves
  unrelated headings byte-identical.

### M12.5b — Agenda

- Agenda view aggregates SCHEDULED / DEADLINE items across every
  org file under the root. View modes:
  - **Today** — items due / scheduled for the current day,
    overdue items (DEADLINE in the past, still TODO).
  - **Next 7 days** — calendar-style week view, one block per
    day.
  - **TODO list** — every headline in a TODO state, grouped by
    file, ignoring dates.
- Filtering: by tag (`+work-personal`), by TODO keyword, by file.
  The query syntax is a small subset of Org's agenda search
  syntax — full Org agenda filtering is out of scope; ship the
  common cases and let users fall through to in-cockpit search
  for the rest.
- Repeater handling: a repeating headline (`SCHEDULED: <2026-06-01
  Mon +1w>`) marked DONE in cockpit bumps the timestamp forward
  one period, exactly as Emacs does. Tested against a corpus of
  repeater forms (`+1d`, `+1w`, `++1m`, `.+2d`).
- Performance: agenda is computed from the in-memory `OrgRoot`,
  not from disk on every paint. Re-index on `notify` events.
  For 10k headlines across 100 files (a reasonable upper bound),
  the agenda must render in < 50 ms — asserted in the bench leg
  (M6.1's harness, new `agenda_perf` test).
- **Tests:** agenda content for a fixture root containing every
  scheduling permutation; today-view against a frozen clock;
  repeater bump on DONE.
- **Impl notes (M12.5b):** shipped as pure functions in
  `cockpit-org::agenda` (`today`, `next_7_days`, `todo_list`,
  `complete`) over the in-memory `OrgRoot`; "today" is an injected
  `OrgDate`, not a global clock. Calendar arithmetic
  (`days_from_civil`/`civil_from_days`, weekday lookup, day/week/
  month/year shifts) lives in `cockpit-org::date` — **no `chrono`
  dependency**. Repeater bump on DONE handles `+`/`++`/`.+` with
  `today` as the reference, keeps the keyword in its first open
  state (Emacs repeat semantics), and rewrites only the timestamp on
  the planning line. Today view surfaces overdue *scheduled* items
  too (open + past), not just deadlines. Perf gate is the
  `agenda_perf` test in `tests/bench.rs`, opt-in behind the crate's
  `bench` feature (mirrors `cockpit-lua`'s bench gate).

### M12.6 — Sibling binary `cockpit-jot`

- New binary in the workspace. Standalone.
- Owns: tray icon (M12.2), global hotkey (M12.3), popover (M12.4)
  hosting the Org view-models (M12.5/.5a/.5b), IPC server (M12.1)
  exposing the `org` service so the main cockpit can drive the
  same `OrgRoot`.
- **Impl note (org service contract):** the `org` service
  request/response protocol ships ahead of the binary in
  `cockpit-org::service` — `OrgRequest` (`Reload` / `Today` /
  `TodoList` / `Complete`) and `OrgResponse` (`Reloaded` / `Agenda`
  / `Updated` / `Error`), plain `serde` types that ride as the
  payload of `cockpit_ipc::Envelope`. The pure `handle(&mut OrgRoot,
  req)` applies a request to the live root and (for `Complete`)
  returns the new file source for the caller to persist — so the
  jot-IPC and cockpit-direct-write paths share the same edit
  primitives and stay byte-identical. An integration test round-trips
  the messages through the real `cockpit-ipc` CBOR codec. The jot
  binary's event loop / winit popover that hosts this is the
  remaining (display-bound) work.
- **Impl note (controller):** the `cockpit-jot` crate is split into a
  tested, backend-free `JotController` and a thin glue `main.rs`. The
  controller owns the live `OrgRoot`, the capture/agenda/overview
  view-models, and the tray menu, and maps **events** (hotkey, tray,
  popover keys — `NowStamp` injected) to **intents** (`ShowPopover` /
  `DismissPopover` / `WriteFile` / `OpenInCockpit` / `Quit`); capture
  commit files via `apply_capture` and syncs the live root.
  `loader::{load_root,now_stamp}` is the disk/clock glue (the latter
  derives the calendar date from `SystemTime` via
  `cockpit-org::date`, no `chrono`). Until the `ui-smoke` event loop
  lands, `main.rs` is a headless CLI over the same controller:
  `agenda` / `overview` print the view, and `capture <key>
  [--annotate S] [--initial S] [title...]` runs a configured template
  to completion and writes the entry (the same `WriteFile` intent the
  popover carries out) — usable from scripts / editor keybindings
  today. The capture context (`%a` annotation / `%i` initial) is no
  longer hardcoded empty: `JotController::open_capture_with(ctx)` feeds
  it into the next pick (the bare hotkey path resets it), so the
  cockpit-driven (IPC) and CLI captures can supply the editor's
  `path:line` / selection. A `tests/capture_cli.rs` integration test
  drives the real binary end-to-end. The real `tray-icon` +
  `global-hotkey` + winit popover loop is the display-bound follow-up.
- **Impl note (popover content):** the popover's brain shipped as
  `cockpit_jot::JotPopover`, a headless `cockpit_paint::PopoverContent`
  impl (M12.4) wrapping the `JotController`. It maps the popover's
  key/text events onto the controller for every surface — capture
  (template picker → editor → `Ctrl+Enter` commit), agenda
  (`↑↓`/`j`/`k` navigation, `Tab` mode cycle, `Enter` jump-to-cockpit,
  `/` filter), and overview — paints each surface with the shared
  `Painter`, and queues the resulting `JotIntent`s for the shell to
  drain. The `on_key`/`on_text` split matches `cockpit-render`'s shell
  (a plain printable arrives on both channels; each surface reads only
  the one it needs, and a one-shot guard swallows the `/` that the shell
  echoes when opening the filter). Fully unit-tested; the controller
  gained `capture_backspace`/`capture_move_left`/`capture_move_right`/
  `agenda_set_filter` to back the editor and filter keys. The winit
  shell that hosts this `PopoverContent` is the remaining display-bound
  glue.
- **Impl note (`org.toml` loading):** `OrgConfig::from_toml_str`
  parses the documented `[org]` + `[[org.capture]]` grammar
  (foreign sections tolerated, missing `[org]` → defaults) as a
  first-class API in `cockpit-org`, replacing the integration
  test's ad-hoc wrapper. `loader::{load_config, default_config_path,
  resolve_org_root}` is the binary glue: a missing file is not an
  error (defaults apply) but a malformed one is, and the org root
  resolves `--root` > config `root` (leading `~` expanded) > `~/org`.
  `main.rs` reads `~/.config/cockpit/org.toml` by default, so
  configured capture templates now flow into the controller instead
  of the previous hardcoded empty set.
- **Default hotkeys** (configurable in `~/.config/cockpit/org.toml`):
  - `Ctrl+O` — **capture** (opens the capture-template picker;
    pressing the template key triggers immediate quick-entry).
  - `Ctrl+Alt+A` — **agenda** (opens the popover on today's
    agenda).
  - `Ctrl+Alt+O` — **org overview** (opens the popover on the
    list-of-files view).

  All three chords run through `cockpit-hotkey`'s conflict
  detection (M12.3). `Ctrl+O` collides with "Open file" inside
  most editors / browsers — the conflict detector flags this at
  register time, and the docs explicitly call out that the
  global override means apps lose their local `Ctrl+O`. Users
  who don't want that pick a different chord.
- Tray menu: *Capture…*, *Agenda*, *Open inbox*, *Open root in
  cockpit*, *Settings*, *Quit*.
- Capture flow:
  1. Hotkey fires; popover opens on the template picker
     (`t` Todo / `n` Note / `j` Journal / …).
  2. User picks a key; the popover swaps to a single-field
     editor with the template pre-expanded and the cursor at the
     `%?` slot.
  3. `Ctrl+Enter` commits — writes the entry to the right file
     under the right heading, dismisses the popover, returns
     focus to wherever it was. Sub-100 ms hotkey-to-focus-in-field
     is the target (asserted under `--features bench`).
- Agenda flow: popover opens on the Today view; arrow keys move
  the cursor between headlines; `Enter` jumps into the
  cockpit (via IPC) and opens the underlying `.org` file at the
  right line. `Tab` cycles Today / Next-7 / TODO list. `/` filters.
- Background behaviour: process keeps running after the popover
  dismisses; tray icon + `notify` watcher are the only persistent
  surfaces. The in-memory `OrgRoot` survives across popover
  dismissals — re-opening agenda is instant.
- Autostart: opt-in. Provide platform install scripts:
  - Linux: `~/.config/autostart/cockpit-jot.desktop`.
  - macOS: `~/Library/LaunchAgents/dev.cockpit.jot.plist`.
  - Windows: registry `Run` key.
  - The cockpit binary surfaces a *"Enable jot autostart"* palette
    command that writes the right file with explicit confirm
    (AGENTS §2 #6 spirit).

### M12.7 — Main-cockpit integration

- The cockpit already paints `.org` files via the editor (no new
  language for the buffer — Org gets a `Language::Org` highlight
  arm here, following the v0.10 pattern for Go: `tree-sitter-org`
  for highlight, `cockpit-org` for structural ops).
- New in-cockpit pane: **Agenda** (floating, reuses M8.2 floating-
  pane primitive) shows the same view-model as the tray app via
  the `org` IPC service.
- Palette commands:
  - `Org: Capture` (`<leader>oc`) — template picker, same flow
    as `Ctrl+O` from the tray.
  - `Org: Agenda` (`<leader>oa`) — floating agenda.
  - `Org: Jump To Inbox` (`<leader>oi`).
  - `Org: Refile` — move the headline under the cursor to a
    chosen target (file + parent heading).
  - `Org: Toggle TODO State` (`<leader>ot`) — cycles
    TODO → DONE → no-keyword on the cursor's headline. Pure
    editor-buffer operation; respects line-range round-trip rule.
  - `Org: Schedule` / `Org: Deadline` — prompt for a date,
    insert/update the timestamp on the cursor's headline.
  - `Org: Toggle Project Filter` — scopes the agenda to org
    files tagged with the current project's name. (Optional;
    Org isn't intrinsically project-scoped the way the SQLite
    plan was.)
- When `cockpit-jot` isn't running, the cockpit reads/writes the
  `.org` files directly. When it is, edits go through IPC so the
  tray app's in-memory `OrgRoot` stays live and `notify` doesn't
  trigger a redundant re-parse. Either path produces byte-
  identical file contents — they share the M12.5 line-range
  edit primitives.
- **Tests:** in-cockpit pane scripted edits; IPC + direct-write
  parity on the same fixture; round-trip preservation under
  TOGGLE-TODO + SCHEDULE on a fixture authored in Emacs.
- **Impl notes (M12.7 headless half):** the buffer ops behind
  `Org: Toggle TODO State` / `Org: Schedule` / `Org: Deadline` ship
  as `cockpit-org::{cycle_todo, set_scheduled, set_deadline}` —
  line-range edits that touch only the headline or planning line
  (insert a new un-indented planning line when absent, update the
  stamp in place when present, append the keyword when the planning
  line exists without it), all byte-identical elsewhere with tests.
  `Timestamp::{active_date,active_datetime}` build the value the
  date prompt will supply. The palette command ids + default leader
  bindings live in `cockpit_ui::org::commands`
  (`org.capture`/`agenda`/`jump_to_inbox`/`toggle_todo`/`schedule`/
  `deadline`/`refile`; `<leader>o{c,a,i,t}`). The IPC-vs-direct
  parity is structural: both paths call the same
  `cockpit-org` edit primitives (the `org` service's `Complete`
  returns the identical source a direct `cycle_todo`/`complete`
  produces). `Org: Refile` also ships headlessly:
  `cockpit-org::{cut_subtree, paste_subtree, refile}` move a
  heading's whole subtree (same-file or cross-file), demoting levels
  to nest under the target and keeping every other line
  byte-identical. `Language::Org` is recognised by the editor
  (`cockpit-editor::highlight`, `.org` extension; no LSP entry) —
  highlight spans await `tree-sitter-org`, the same staged approach
  as `Ggsql`. Remaining display-bound work: the floating agenda
  pane, the IPC client wiring, and registering these commands +
  handlers in the cockpit binary's event loop.

### M12.8 — Sync (effectively free)

- The org root is a folder of plain-text files. Sync is the
  user's existing solution: git, Syncthing, iCloud Drive,
  Dropbox, NextCloud — all work without cockpit doing anything.
- Cockpit ships **no** sync engine. We document the patterns in
  `docs/org.md`:
  - **git** — point a working copy at the root; `git pull` /
    `git push` from the cockpit's mux terminal (or via the v0.8
    Lazygit recipe). Merge conflicts are resolved in the editor
    like any other text file.
  - **Syncthing / iCloud / Dropbox** — point the sync folder at
    the org root. The `notify` watcher picks up remote writes
    and re-indexes within ~1 s.
- The only cockpit-side concern is graceful handling of
  concurrent writes (remote sync touches a file while cockpit
  has it open). Mitigation: every edit reloads the file's
  on-disk content first, checks the recorded content hash, and
  if it changed surfaces a *"file changed on disk; reload?"*
  toast instead of clobbering. Same model the editor already
  uses for external buffer changes.

### v0.12 exit checklist

- [ ] `cockpit-jot` runs as a separate process; tray icon present
      on all three OSes.
- [ ] `Ctrl+O` from any focused window opens the capture-template
      picker in < 150 ms cold / < 50 ms warm on a low-end Linux
      laptop.
- [ ] Picking the `t` template, typing a title, and pressing
      `Ctrl+Enter` appends a `* TODO ...` heading to `inbox.org`
      under the configured parent heading; the file is immediately
      readable in Emacs / Logseq with no syntax surprises.
- [ ] `Ctrl+Alt+A` opens the agenda; SCHEDULED items for today and
      DEADLINE items in the next 7 days appear in the correct
      buckets; repeater on DONE bumps the timestamp.
- [ ] Editing a `.org` file in the main cockpit shows up in the
      tray app's in-memory `OrgRoot` within ~1 s (notify watcher).
- [ ] An Emacs-authored `.org` file containing every fixture's
      syntax (headlines, tags, TODO states, SCHEDULED/DEADLINE
      with repeaters, active/inactive timestamps) round-trips
      byte-identical through cockpit's edit-then-save path.
- [ ] Closing the main cockpit does not disturb the tray app;
      reopening picks up the same org root.
- [ ] Autostart command writes the right file and survives a
      reboot.
- [ ] `mise run ci` green; new `ipc-smoke` CI leg exercises the
      IPC protocol per OS; new `org_roundtrip` bench test gates
      agenda perf (< 50 ms on a 10k-headline corpus).

### Sequencing

```diagram
M12.1 (ipc) ───┐
M12.2 (tray) ──┼──▶ M12.4 (popover) ──┐
M12.3 (hotkey)─┘                       │
                                       ▼
                  M12.5 (org parse) ──▶ M12.5a (capture) ──▶ M12.5b (agenda)
                                                                  │
                                                                  ▼
                                                M12.6 (jot bin) ──▶ M12.7 (cockpit) ──▶ M12.8 (sync docs)
```

M12.1–M12.3 ship as a single "infrastructure" PR — they're the v0.13
launcher's foundation too. M12.4 + M12.5 are independent of the infra
PR. M12.5a builds on M12.5; M12.5b can run in parallel with M12.5a
once M12.5 is in. M12.6 + M12.7 close the loop.

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| `tray-icon` quirks per OS                     | Stick to the API surface Tauri uses; CI smoke per OS catches regressions. Tray menu kept minimal — 6 items, no nested submenus. |
| `global-hotkey` chord conflicts               | Detect at register; surface clearly; suggest alternatives in the toast. Never grab a chord behind the user's back. `Ctrl+O` collision with browser/editor "Open file" is documented loudly; users can remap. |
| `orgize` API drift                            | Pinned to stable `0.9` (not the `0.10` alpha); fixture corpus triggers every supported feature. orgize owns only the inline headline grammar — timestamps and positions are parsed in-crate, so an orgize bump can't silently change scheduling/round-trip behaviour. Major-version bumps are a milestone-level change. |
| Round-trip byte-identical edits drift         | Line-range replacement on the original source buffer (never AST re-emit) + golden fixture corpus authored in Emacs. Property test: parse → mutate-no-op → serialise == input on a generator of fuzzy `.org` strings. |
| Two writers to the same `.org` file          | The editor's "file changed on disk" toast catches the race; cockpit reloads before overwriting. Plain-text + git-friendly format means worst case is a 3-way merge, not data loss. |
| Process supervision (jot crashes)             | Tray app is intentionally simple; if it crashes the user notices because the tray icon vanishes. No respawner in v0.12 — out of scope, adds surface for very rare failure. |
| Users want cloud sync                         | v0.12 documents the patterns. Plain-text org files mean any sync solution works without cockpit code. |
| Agenda perf on large vaults                   | Bench leg asserts < 50 ms on 10k headlines. If perf regresses, the agenda index becomes a separate `BTreeMap<Date, Vec<HeadlineRef>>` recomputed on `notify` rather than per-paint. |
| Two binaries means two release artifacts      | The release workflow already packages per-OS; add a `--bins cockpit cockpit-jot` flag. Document in `docs/install.md`. |
| Bonus binary breaks `mise run run`            | `mise.toml` task list grows: `run` keeps launching the main cockpit, new `run-jot` launches the sibling, `run-all` starts both. |
| Org user expects full Emacs parity            | Document the v0.12 subset loudly in `docs/org.md`. Out-of-scope features listed up front, with the "open in Emacs for X" escape hatch made explicit. |

---

## 8i. v0.13 — Quick-action launcher (Raycast-style)  (NEW — post-spec)

Goal: a system-wide, frameless, hotkey-summoned launcher with **mise tasks
and Lua extensions as backends**. Conceptually Raycast / Albert / Ulauncher
— pop up from anywhere, fuzzy-search across action providers, hit Enter
to dispatch. Reuses every piece of v0.12 infrastructure (tray, hotkey,
popover, IPC).

### Scope

Three primary action providers in v0.13:

1. **mise tasks** — every task across known mise projects becomes a
   launcher entry; running it spawns through `mise exec` in the
   right project root.
2. **Lua extensions** — v0.9's `cockpit-lua` gets a new namespace
   `cockpit.launcher.action { id, title, run }` that registers
   actions visible to the launcher.
3. **Built-ins** — `Open Project`, `Recent Files`, `Calculator`
   (`=2+2*3`), `Switch Theme`, `Open URL` (paste a URL → open in
   browser).

**Out of scope:** web search, Spotify control, system actions
(volume, brightness), Raycast's commercial AI assistant tier,
extensions discovery / marketplace. AGENTS §2 #7's no-marketplace
rule still binds.

### Architecture

```diagram
╭───────────────╮     ╭─────────────────╮     ╭──────────────────╮
│ cockpit-quick │────▶│ cockpit-        │────▶│  Providers       │
│ (binary)      │ ipc │ launcher        │     │  - mise          │
│  tray + hotkey│     │ (headless core) │     │  - lua           │
│  popover      │     │  fuzzy match    │     │  - builtins      │
╰───────────────╯     │  action exec    │     ╰──────────────────╯
                       ╰─────────────────╯
```

New crates:

- `cockpit-launcher` — headless core. Action registry, provider
  trait, fuzzy matcher (`nucleo`, already a workspace dep), action
  dispatch. Fully unit-testable.
- `cockpit-quick` — sibling binary. Same shape as `cockpit-jot`:
  tray icon, global hotkey, popover, IPC client. Hosts the
  launcher view-model on top of `cockpit-popover` (M12.4).

### M13.1 — `cockpit-launcher`: provider trait + matcher

- `trait ActionProvider { fn id(&self) -> &str; fn search(&self,
  query: &str) -> Vec<Action>; }`.
- `struct Action { id: String, title: String, subtitle:
  Option<String>, icon: ActionIcon, run: ActionRun }`.
- `ActionRun` is a typed enum: `Command(CommandId,
  Vec<ActionArg>)`, `Process(ProcessSpec)`, `OpenUrl(Url)`,
  `OpenPath(PathBuf)`, `Lua(LuaActionHandle)`.
  Crucially: every dispatch path lands on
  `cockpit-commands::CommandId` eventually (AGENTS §2 #5) so the
  palette and the launcher share one execution spine.
- Matcher: `nucleo`-backed (the same crate the in-cockpit palette
  uses). Multi-provider ranking: providers return scored
  candidates; the launcher merges by total score with a per-
  provider quota to avoid one provider drowning the list.
- **Tests:** ranking with synthetic providers; quota enforcement;
  empty-query "favourites" listing; query escaping.
- **Impl note (M13.1):** shipped as the headless `cockpit-launcher`
  crate. `ActionProvider` gained two defaulted methods beyond the
  plan sketch — `quota()` (the per-provider cap, default 5) and
  `fuzzy_filtered()`. The latter resolves the "providers return
  scored candidates" tension cleanly: list providers (mise) return
  every candidate and let the launcher fuzzy-score titles, while
  *verbatim* providers (calculator, URL) emit one already-relevant
  action that the launcher keeps at a high base score so it floats
  to the top, Raycast-style. `Launcher::search` is the two-stage
  merge: score → per-provider quota → cross-provider sort by score,
  then title, then id (fully deterministic, no dependence on
  provider iteration order), capped to `max_rows` (default 8).
  Ranking reuses `nucleo-matcher` exactly as `cockpit_ui::file_finder`
  does. `ActionRun::OpenUrl` carries a `String` (validated at
  provider time) rather than a `url::Url`, avoiding a new dep; the
  enum is otherwise as specified, with `Lua(LuaActionHandle)` modelled
  now so the binary (M13.5) only has to wire the runtime. The
  crate stays backend-free — the only seam it touches is
  `cockpit-project::env` for `ProcessSpec` and the test `FakeFileSystem`.

### M13.2 — Provider: mise tasks

- Mise project discovery (independent of any cockpit window): walk
  `~/.config/cockpit/launcher.toml`'s `[mise.projects]` table —
  explicit list of project paths. **No filesystem crawl.** Users
  add a project by running `Launcher: Add Mise Project` from the
  cockpit (or editing the file).
- For each known project, parse `mise.toml` (reuse
  `cockpit-project::mise`) and expose every task as an
  `Action { title: "<project>: <task>", run: Process(...) }`.
- Re-scan trigger: file watcher on the listed `mise.toml`s (notify
  crate, reused from M9.5).
- **Out of scope:** running tasks against the cockpit's *open*
  project specifically — that's the in-cockpit palette's job.
- **Impl note (M13.2):** `MiseTasksProvider::from_projects(fs, roots)`
  parses each root's `mise.toml` through `cockpit_project::
  detect_mise_project_with` over an injected `FileSystem` — no real
  disk, fully testable. The mise-availability probe is stubbed with a
  never-spawning `ProcessRunner` (discovery only reads the config, it
  never runs `mise --version`). Tasks are emitted as
  `Action { title: "<project>: <task>", run: Process(mise run <task>,
  cwd = project root) }`; the project label prefers `[metadata.cockpit]
  name`, else the directory name. A stale/missing root is skipped, not
  fatal. Entries are sorted (project, task) so the empty-query listing
  is stable. The disk-watch re-scan (`notify`) and the
  `Launcher: Add Mise Project` cockpit command are binary-side wiring,
  deferred with M13.5/M13.7.

### M13.3 — Provider: Lua extensions

- Extends v0.9's `cockpit-lua` with a new `cockpit.launcher`
  namespace:
  ```lua
  cockpit.launcher.action {
    id     = "user.weather",
    title  = "Today's weather",
    run    = function(ctx)
      ctx.toast("23°C, partly cloudy")
    end,
  }
  ```
- Capability: actions that need network / filesystem still go
  through M9.4's capability gate. Pure-Lua actions (e.g.
  calculator) need nothing.
- **Discovery:** the sibling binary loads
  `~/.config/cockpit/extensions/*.lua` on start, same as the
  cockpit. They share the directory; an extension that registers
  both palette commands and launcher actions appears in both
  surfaces.
- **Tests:** scripted Lua action registration; sandbox enforcement
  identical to v0.9.
- **Impl note (M13.3, launcher side):** the provider ships as
  `cockpit_launcher::lua::LuaActionsProvider` over plain `LuaAction
  { extension, id, title }` descriptors — so `cockpit-launcher` does
  **not** depend on `cockpit-lua`; the `cockpit-quick` binary bridges
  the two. Each action emits `ActionRun::Lua(LuaActionHandle{extension,
  id})`; the binary routes Enter back to the owning VM via the existing
  `LuaRuntime::dispatch_command` path (same sandbox + capability gate,
  so a pure-Lua action needs nothing and a network/fs one still hits
  M9.4). The `cockpit.launcher.action {…}` registrar in `cockpit-lua`'s
  sandbox (harvesting into `Registrations`) and the shared
  `extensions/*.lua` hot-reload are the binary-side follow-up, behind
  the same `ui-smoke`-gated event loop as the rest of M13.5.

### M13.4 — Provider: built-ins

- `Open Project` → reads the cockpit's recent-projects cache (M2.7,
  M6.6) via the IPC service, opens the selected project in the
  cockpit (sending an `OpenProject(path)` IPC message — the
  cockpit either focuses an existing project window or
  launches one).
- `Calculator` — query starting with `=` is parsed by a tiny
  expression evaluator (no dep; ~100 LoC). Result is the action
  itself ("Press Enter to copy `42` to clipboard").
- `Open URL` — query matching a URL regex shows an action to open
  in `$BROWSER`.
- `Switch Theme` — same as the in-cockpit `Theme: Switch …`
  palette, but dispatched over IPC.
- `Org Capture: <Template>` — one launcher entry per template
  configured in v0.12's `org.toml`. Hitting Enter routes through
  the `org` IPC service to trigger the same capture flow the
  tray app uses. Means the launcher's universal hotkey is a
  second entry point to capture, complementing `Ctrl+O`.
- `Org Agenda` — opens the agenda popover via the `org` IPC
  service (or launches the tray app if it isn't running).
- **Impl note (M13.4, headless subset):** the two self-contained
  built-ins ship now in `cockpit_launcher::builtins`. `Calculator` is
  a verbatim provider: `=<expr>` runs a dependency-free recursive-descent
  evaluator (`calc.rs` — `+ - * /`, parentheses, unary minus, correct
  precedence so `=2+2*3` → `8`; div-by-zero / malformed → no row) and
  emits one action whose Enter dispatches `clipboard.copy` with the
  result (so even the calculator rides the `CommandId` spine). `Open URL`
  recognises an `http(s)://` query with a real host (conservative,
  regex-free) and emits an `OpenUrl` action. The IPC-backed built-ins —
  `Open Project`, `Switch Theme`, `Org Capture: <Template>`,
  `Org Agenda` — need a live IPC client, so they land with the
  `cockpit-quick` binary (M13.5) alongside the `cockpit` IPC service
  variants (M13.7).

### M13.5 — `cockpit-quick`: sibling binary

- Same shape as `cockpit-jot`: tray icon (M12.2), global hotkey
  (M12.3, default `Ctrl+Space`; collides with input-method
  switchers on some setups — the conflict path from M12.3
  surfaces a toast), popover (M12.4), IPC client (M12.1).
- Popover content: text input on top, results list below (max 8
  rows), provider tag per row, keyboard-only navigation. Enter
  dispatches. `Tab` cycles secondary actions on the focused row
  (Raycast convention).
- Cold-start budget: ≤ 200 ms hotkey-to-popover-visible on a
  low-end Linux laptop. Process is long-lived; the popover is
  cheap to summon.
- **Tests:** popover view-model goldens; provider integration
  smoke per OS.
- **Impl note (M13.5):** shipped split like `cockpit-jot` — a tested,
  backend-free `QuickController` (event → intent) plus a thin
  `main.rs`. The controller owns the live `Launcher` and the
  query/results/selection, mapping `QuickEvent`
  (`SetQuery`/`MoveUp`/`MoveDown`/`Submit`/`Dismiss`) onto `QuickIntent`
  (`CopyToClipboard`/`OpenUrl`/`OpenPath`/`DispatchCommand`/`RunLua`/
  `RunProcess`/`Dismiss`). Every `ActionRun` lowers to exactly one
  intent — the calculator's `clipboard.copy` command becomes a local
  `CopyToClipboard`, every other `Command` becomes a `DispatchCommand`
  the binary sends over the `cockpit`/`org` IPC service. `loader.rs` is
  the config glue: `build_launcher(config, inputs, home, fs)` assembles
  the enabled providers from `launcher.toml` (expanding a leading `~`
  in project paths) over an injected `FileSystem`, taking the
  IPC-sourced runtime data (recent projects, themes, org templates, Lua
  actions) as plain `ProviderInputs`. Until the `ui-smoke` event loop
  lands, `main.rs` is a headless CLI over the same controller —
  `search` / `run` / `providers` — usable from scripts today. The real
  `tray-icon` + `global-hotkey` + winit popover (text input on top,
  results list, `Tab` secondary actions, cold-start budget) is the
  display-bound follow-up behind the `ui-smoke` feature, same staging
  as `cockpit-jot`'s shell.

### M13.6 — Settings + config

- Single config file `~/.config/cockpit/launcher.toml`:
  ```toml
  [hotkey]
  chord = "Ctrl+Space"

  [providers]
  mise     = true
  lua      = true
  builtins = true

  [mise.projects]
  paths = ["~/code/work", "~/code/personal"]

  [launcher.ui]
  max_rows = 8
  position = "centred"   # centred | top
  theme    = "inherit"   # inherit | catppuccin-mocha | …
  ```
- Schema lives in `cockpit-config`; reuses the M8.1 toml_edit
  write-path for `Settings` palette flips.

### M13.7 — Cockpit-side cross-launch hooks

- New IPC messages on the `cockpit` service:
  - `OpenProject(PathBuf)` — focus or launch.
  - `DispatchCommand(CommandId, args)` — run an arbitrary
    command in the cockpit.
- These are the same surface v0.12 used to drive the jot pane,
  extended with two new variants. Same auth model (user-only
  socket).
- **Security note:** `DispatchCommand` is intentionally
  unrestricted (the user-only socket already implies trust).
  Document explicitly that anyone with write access to the socket
  can drive the cockpit, and that's by design.

### v0.13 exit checklist

- [ ] `Ctrl+Space` from any focused window opens the launcher in
      < 200 ms cold / < 50 ms warm.
- [ ] Typing `te` shows every mise `test*` task across registered
      projects, ranked sensibly.
- [ ] A Lua extension registering `cockpit.launcher.action` appears
      in results within 1 s of writing the file (hot-reload).
- [ ] `=2+2*3` shows `8`; Enter copies to clipboard.
- [ ] Pasting a URL shows an "Open in browser" action.
- [ ] `Open Project: <name>` in the launcher brings up that project
      in the cockpit (focus or launch).
- [ ] All providers survive the cockpit process being closed and
      reopened; the launcher process is independent.
- [ ] `mise run ci` green; the existing `ui-smoke` and new
      `ipc-smoke` legs cover the launcher.

### Sequencing

```diagram
v0.12 (infra) ──▶ M13.1 (launcher core)
                       │
                       ├──▶ M13.2 (mise)    ─┐
                       ├──▶ M13.3 (lua)     ─┤
                       ├──▶ M13.4 (builtins)─┤
                       │                     ▼
                       └──▶ M13.5 (binary) ──▶ M13.6 (config) ──▶ M13.7 (hooks)
```

v0.12's infra must ship before v0.13 starts — this is a hard
dependency, not a soft one. M13.2/M13.3/M13.4 can ship in any
order once M13.1 is in.

### Risk notes

| Risk                                          | Mitigation                                              |
|-----------------------------------------------|---------------------------------------------------------|
| `Ctrl+Space` collides with IME switchers      | M12.3's conflict detection fires loudly; default config falls back to `Ctrl+Alt+Space` on detection. |
| Provider explosion (everyone wants their own) | Provider trait stays small; new built-ins require a plan update (same rule as v0.9 API surface). Lua is the escape hatch for one-off actions. |
| Cold-start regression                         | Hotkey-to-popover budget asserted in CI under `--features bench`. Sibling binary stays small (no editor / no mux / no LSP). |
| Fuzzy match quality                           | `nucleo` is the same matcher the in-cockpit palette uses; ranking-quality regressions show up there first. |
| User expects deep system integration          | Document the v0.13 scope clearly: actions over text and processes, not OS-control. App-launcher / system-control providers are a deliberate non-goal. |
| Two long-lived sibling binaries inflate RAM   | Measure on the v0.6 bench leg; each binary should land under 60 MB resident on Linux. If it doesn't, share the popover/atlas via a `cockpit-shell` daemon — a v0.14 question, not a v0.13 blocker. |

### Future (not v0.13)

- **`cockpit-shell` daemon** — collapse `cockpit-jot` and
  `cockpit-quick` into one always-on tray process that hosts both
  surfaces. Worth it if RAM becomes a complaint; not worth it
  pre-emptively.
- **Action history** — last-N dispatched actions float to the top
  on empty query. Trivial; deferred until users ask.
- **Multi-monitor anchoring** — popover follows the focused
  display; today it centres on the primary. Single-monitor users
  are the majority.

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
