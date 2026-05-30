# AGENTS.md — Coding Cockpit

Guidance for AI coding agents (and humans skimming for orientation) working in
this repository. Read this **before** editing. Read [`spec.md`](spec.md) for
product behaviour and [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) for
the authoritative stack/architecture. Where the two disagree, the plan wins
(the spec still says "Zig" in places; the project is **Rust**).

---

## 1. What this project is

A fast, native, multi-platform **coding cockpit** in Rust: project launcher +
file browser + Vim-style editor + embedded multiplexer (`cockpit-mux`).
Think "JetBrains-style project IDE, terminal-first, Vim-centred".

One-line stack: **Rust + Cargo workspace + winit/glow + termwiz/portable-pty +
ropey + mise + native multiplexer (`cockpit-mux`)**. `mise` is also the only
developer task runner — there is no `justfile`, no `make`, no `xtask`.

---

## 2. Hard rules — do not violate

These are the load-bearing invariants of the architecture. Breaking them is a
revert, not a review comment.

1. **Only `cockpit-render` may depend on `winit`, `glow`, or any GPU/window
   crate.** Every other crate must build and test with no display server. If
   you find yourself adding `winit` to another crate's `Cargo.toml`, stop and
   rethink — the right place is almost certainly a view-model in `cockpit-ui`.

2. **Core logic must be headless-testable.** New behaviour goes in a core
   crate with `#[test]`s that need no window, no GPU, no PTY, no real
   filesystem (use `cockpit-testkit` fakes), no network.

3. **All non-determinism is injected via traits.** Filesystem, process
   spawning, and clock are dependencies, not globals. The traits live in
   `cockpit-project::env` — `FileSystem`, `ProcessRunner`, `Clock` — with
   `Std*` production impls and `Fake*` in-memory impls in the same module.
   If you reach for `std::fs`, `std::process::Command`, or `Instant::now()`
   inside core code, you are probably adding an untestable path — take the
   trait object instead and pair the call site with `_with` variants where
   the existing helpers expose them.

   Adding a new SQL backend? Same pattern: extend the `SqlEngine` trait in
   `cockpit-sql`, ship a real impl plus a `FakeSqlEngine` so the notebook
   view-model (`cockpit-notebook`) and dbt-lite project layer
   (`cockpit-analytics`) can keep running hermetic tests.

4. **No global async runtime.** PTY and child-process I/O run on dedicated OS
   threads with channels. Do not add `tokio` to a core crate.

5. **Commands are the single spine.** Keybindings, the command palette, the
   editor↔terminal bridge, and tests all dispatch the same `CommandId`s from
   `cockpit-commands`. Do not add a parallel dispatch path.

6. **No auto-install of tools.** The `mise` integration must never trigger
   `mise install` on its own (spec §8, §18.6, §24). Detect, surface, prompt —
   never silently install.

7. **No heavy background indexing, no LSP before v0.3, no plugin
   marketplace, registry, or in-app installer.** Out of scope on
   purpose (spec §3, §19, §23, §24). The v0.9 Lua extension system
   (see [`docs/extensions.md`](docs/extensions.md)) ships sandboxed
   **local-file** extensions only — there is no discovery server, no
   auto-update, no `:PackerInstall`. Adding one is a plan change.

8. **Don't make `spec.md` and `IMPLEMENTATION_PLAN.md` diverge further.** If
   the plan is wrong, update it; if the spec is wrong, annotate or update it.
   Never silently change behaviour described in either without updating both.

9. **Mouse goes through `cockpit-render`'s headless callbacks.** Winit
   mouse events translate into `MouseButton` + `PointerPosition` callbacks
   on the `CockpitApp` trait (M4.7). `cockpit-render` is the only crate
   that names a `winit` mouse type; the rest of the codebase hit-tests
   the latest `ComputedLayout` rectangle.

---

## 3. Repository layout

```diagram
samedit/                            # Cargo workspace root
├── crates/
│   ├── cockpit/                    # binary: app shell, event loop, wiring
│   ├── cockpit-editor/             # ropey buffer, cursor, undo, vim FSM, search
│   ├── cockpit-project/            # detection, mise, project cache, file tree
│   │                               #  + env::{FileSystem,ProcessRunner,Clock}
│   │                               #    seams (M4.10)
│   ├── cockpit-mux/                # native multiplexer state (M7.2+)
│   ├── cockpit-crew/               # parallel agents in git worktrees (v0.14)
│   ├── cockpit-terminal/           # pty, termwiz engine, path detect
│   ├── cockpit-http/               # Bruno-style .bru parser + model (v0.11)
│   ├── cockpit-lsp/                # LSP transport — codec, JSON-RPC, client
│   ├── cockpit-lua/                # sandboxed Lua 5.4 extension runtime (v0.9)
│   ├── cockpit-commands/           # command registry + keybinding resolution
│   ├── cockpit-config/             # serde config types, TOML/KDL loading
│   ├── cockpit-ui/                 # view-model tree, layout, panes, palette
│   ├── cockpit-render/             # winit + glow — ONLY GPU/window crate
│   ├── cockpit-sql/                # DuckDB / ggsql shell-out engines (v0.5)
│   ├── cockpit-notebook/           # Jupytext + Quarto notebook view-model (v0.5)
│   ├── cockpit-analytics/          # dbt-lite project mode (v0.5)
│   └── cockpit-testkit/  (dev)     # shared fixtures, bench helpers
├── tests/
│   └── fixtures/                   # rust-basic, mise-basic, file-tree, …
├── .github/workflows/              # Win/macOS/Linux CI matrix
├── Cargo.toml                      # workspace manifest
├── rust-toolchain.toml             # pinned stable Rust
├── mise.toml                       # canonical task runner (all dev workflows)
├── spec.md                         # product spec (Rust-aligned as of M4.9)
└── IMPLEMENTATION_PLAN.md          # authoritative stack + plan
```

Headless-testable crates (everything except `cockpit-render` and the binary)
should never gain a window/GPU dependency.

---

## 4. Where things go — a decision table

| If you are adding…                       | It belongs in…           |
|------------------------------------------|--------------------------|
| Buffer / cursor / undo / Vim mode logic  | `cockpit-editor`         |
| Project detection, mise parsing, tasks   | `cockpit-project`        |
| Filesystem / process / clock seams       | `cockpit-project::env`   |
| PTY, terminal engine, path parse         | `cockpit-terminal`       |
| Mux state (sessions/windows/panes)       | `cockpit-mux`            |
| Parallel agent runs / worktrees (v0.14)  | `cockpit-crew`           |
| LSP codec / JSON-RPC / client transport  | `cockpit-lsp`            |
| A new command ID or keybinding           | `cockpit-commands`       |
| Config schema, TOML/KDL parsing          | `cockpit-config`         |
| View-model state (panes, palette, tree)  | `cockpit-ui`             |
| Splash / hydration progress state        | `cockpit-ui::hydration`  |
| Anything calling `winit` or `glow`       | `cockpit-render`         |
| Per-frame state advance (M6.2 `tick`)    | `cockpit-render::CockpitApp` |
| Glyph atlas disk cache codec / IO        | `cockpit-render::atlas_persist` |
| DuckDB / ggsql / new SQL backend         | `cockpit-sql`            |
| Notebook cell parser / view-model        | `cockpit-notebook`       |
| dbt-lite analytics (detect / DAG / build)| `cockpit-analytics`      |
| Sandboxed Lua extensions (v0.9)          | `cockpit-lua`            |
| `.bru` parser + HTTP collection model    | `cockpit-http`           |
| Cold-start phase work (detect/tree/...)  | `cockpit::hydration`     |
| Splash painter (uses Painter+Theme)      | `cockpit::splash`        |
| Wiring crates together, CLI flags        | `cockpit` (binary)       |
| Fixtures, fakes, bench helpers           | `cockpit-testkit`        |

When in doubt, prefer adding to the *most headless* crate that can express the
behaviour. UI is thin; cores are fat.

---

## 5. Build, test, run

**`mise` is the only task runner.** Every workflow goes through
`mise run <task>` (see [`mise.toml`](mise.toml) for the full list, or
`mise tasks`). Do not invoke cross-cutting `cargo` commands directly when a
mise task exists — keep workflows discoverable and CI-aligned.

```bash
mise run build         # cargo build --workspace
mise run test          # cargo test --workspace          (fast, hermetic)
mise run test-unit     # library unit tests only
mise run test-golden   # snapshot tests (insta)
mise run fmt           # cargo fmt --all
mise run fmt-check     # CI-style format check
mise run lint          # cargo clippy --workspace --all-targets -- -D warnings
mise run ci            # fmt-check + lint + build + test  ← run this before declaring done
mise run run           # cargo run -p cockpit
mise run run-fixture -- mise-basic
```

`test-integration` runs the opt-in, `integration`-gated tests: the real-PTY
terminal tests and the `mise` CLI tests (which skip cleanly when `mise` is not
installed). `test-ui-smoke` runs the `ui-smoke`-gated tests in `crates/cockpit`
that assert on the `cockpit-ui` view-model tree (spec §18.8 — no pixel checks).

### Before you say "done"

1. `mise run fmt`
2. `mise run lint`  — clippy is warnings-as-errors
3. `mise run test` — all green
4. Ideally `mise run ci` end-to-end

If you can't run it (sandbox, offline, etc.), say so explicitly. Never claim
green without evidence.

---

## 6. Coding conventions

- **Edition / toolchain:** Rust 2024 edition, pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml). Do not bump without a reason.
- **Errors:** `thiserror` in libraries (typed errors), `anyhow` only in the
  binary. Do not leak `anyhow::Error` from a library crate's public API.
- **Logging:** `tracing` everywhere; never `println!`/`eprintln!` outside
  tests and CLI output. Use spans for cross-thread work (PTY, child procs).
- **No `unwrap()` / `expect()` in non-test code** unless the invariant is
  proven locally and commented. Prefer `?` and typed errors.
- **No `unsafe`** without an explicit `// SAFETY:` justification and a test.
- **Internal crate deps** go through `cockpit-foo.workspace = true` (see
  [`Cargo.toml`](Cargo.toml)) — never hard-code a `path =` in a member crate.
- **Minimum-diff edits.** Don't reformat unrelated code, don't reshuffle
  modules in a feature PR, don't introduce a new abstraction for one caller.
- **Comments explain *why*, not *what*.** The code says what.

---

## 7. Testing — first-class, not an afterthought

Spec §18 makes testing a product principle. Follow the pyramid:

```diagram
                ╭─────────────────╮
                │  e2e  / smoke   │   few
                ├─────────────────┤
                │   integration   │   some  (opt-in feature flag)
                │   PTY tests     │
                ├─────────────────┤
                │  golden (insta) │   many
                │  unit  / prop   │
                ╰─────────────────╯
```

- **Unit tests** colocated with the code they test (`#[cfg(test)] mod tests`).
- **Golden tests** with [`insta`] for Vim FSM output, path detection,
  project/mise extraction, file-tree rendering, palette filtering. Review
  snapshot changes by hand; don't blanket-accept.
- **Property tests** with [`proptest`] for editor invariants (insert/delete
  round-trip, undo/redo, byte↔line/col, rope vs reference string).
- **Integration / PTY / UI-smoke tests** are opt-in via Cargo features so
  `cargo test` stays hermetic and fast. Don't make them default.
- **Fakes over mocks.** `cockpit-testkit` provides fake filesystem, process,
  and clock implementations — use them instead of mocking frameworks.
- **Fixtures** live in [`tests/fixtures/`](tests/fixtures). Small and
  deterministic; generate large ones at runtime, don't commit them.

When you add behaviour, add the test in the same change. A PR without tests
for new core logic is incomplete.

---

## 8. Workflow expectations for agents

1. **Read before editing.** If the user references a file, open it. If you're
   touching a crate, skim its `lib.rs` and the relevant module first.
2. **Plan the smallest correct change.** Identify the single crate that owns
   the behaviour. Resist creating new files / modules / traits unless the
   current layout genuinely can't host the change.
3. **Implement + test in the same step.** Headless tests for headless code.
4. **Verify with `mise run ci` (or at minimum `mise run test && mise run lint`)** and
   report the actual output. If something fails, fix it or say so — don't
   paper over failing tests or downgrade lints to make CI pass.
5. **Don't expand scope.** A bug fix doesn't refactor neighbours. A new
   command doesn't redesign the registry. Note follow-ups in your reply
   instead of doing them.
6. **Clean up after yourself.** Delete scratch files, debug prints, and dead
   experiments before declaring done.
7. **Surface disagreements.** If the spec or plan looks wrong, say so in your
   reply — don't silently diverge.

---

## 9. Pointers

- Product spec: [`spec.md`](spec.md)
- Build/architecture plan (authoritative for stack): [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md)
- Workspace manifest: [`Cargo.toml`](Cargo.toml)
- Task runner: [`mise.toml`](mise.toml) — sole entry point for dev workflows
- Pinned toolchain: [`rust-toolchain.toml`](rust-toolchain.toml)
- CI: [`.github/workflows/`](.github/workflows)

[`insta`]: https://docs.rs/insta
[`proptest`]: https://docs.rs/proptest
