# Org-mode capture & agenda (jot)

Cockpit reads a folder of [Org-mode](https://orgmode.org/) `.org` files
(v0.12). The files on disk are the source of truth, in a format other
tools already speak — the same files open unchanged in Emacs, Logseq,
Beorg, Orgzly, or any other Org tool. There is **no proprietary store**,
no database, and no sidecar metadata files: everything cockpit needs is
encoded in standard Org syntax.

This is the first feature to ship a sibling binary — `cockpit-jot` — with
its own process, so capture and agenda are reachable **even when the
cockpit window is minimised or closed**.

---

## Where org files live

Point cockpit at a single root folder (default `~/org/`). On first launch
into an empty root, cockpit writes a default layout — and never overwrites
an existing file:

```
~/org/
├── inbox.org       # capture default lands here
├── tasks.org       # project work, scheduled items
├── notes.org       # zettel-style notes
└── journal.org     # date-tree, one heading per day
```

A folder full of pre-existing `.org` files Just Works — the layout is a
default, not a requirement. Repoint the root anywhere via `org.toml`
(below).

---

## What's supported (the v0.12 subset)

In scope:

- Hierarchical headlines (`*`, `**`, `***`…) with title, tags
  (`:work:urgent:`), priority cookies (`[#A]`), and a TODO keyword.
- The default `TODO | DONE` workflow, with keyword cycling. Custom
  workflows (`TODO | NEXT | WAIT | DONE`) are configurable via
  `default_todo_keywords`.
- `SCHEDULED:`, `DEADLINE:`, and `CLOSED:` planning timestamps, in active
  (`<2026-06-01 Mon>`) and inactive (`[2026-06-01 Mon]`) forms, date-only
  or with a time (`<2026-06-01 Mon 09:00>`), time ranges
  (`09:00-10:00`), inter-day ranges (`<a>--<b>`), and repeater / warning
  cookies (`+1w`, `++1m`, `.+2d`, `-1d`).
- Plain-text body paragraphs and lists.

Out of scope in v0.12 (open the file in Emacs for these):

- `:PROPERTIES:` drawers, effort estimates, archive files.
- Babel code blocks, org tables + table calc, clocking, org-roam graphs,
  LaTeX/math rendering.
- Emacs-style internal link resolution
  (`[[file:foo.org::Headline]]`). External URLs render as links;
  org-internal links render as inert text.

> **Editing is byte-safe.** Cockpit edits `.org` files by line-range
> replacement on the original buffer — it never re-emits a whole file from
> a parsed tree. Blank lines, comments, indentation, and anything cockpit
> doesn't understand round-trip byte-for-byte through a toggle / schedule /
> capture / refile.

---

## Configuration — `org.toml`

Capture templates and the org root live in `~/.config/cockpit/org.toml`:

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

A capture **target** files the entry either `under` a named heading
(created if missing) or into a `datetree = true` (`* 2026` → `** 2026-05`
→ `*** 2026-05-28 Thu`, creating any missing levels), or — with neither —
at the end of the file. The entry's heading levels are demoted so it
nests correctly under its parent.

### Substitution tokens

Mirroring Emacs `org-capture-templates`:

| Token | Expands to |
|-------|------------|
| `%?`  | where the cursor lands after expansion |
| `%t` / `%T` | active date / active date-time (`<…>`) |
| `%u` / `%U` | inactive date / inactive date-time (`[…]`) |
| `%a`  | annotation — the editor's `path:line`, if any |
| `%i`  | initial content — the current selection, if any |
| `%(expr)` | a Lua expression (capability `org.capture.lua`) |
| `%%`  | a literal `%` |

---

## Agenda

The agenda aggregates `SCHEDULED` / `DEADLINE` items across every file
under the root, computed from the in-memory index (not re-read from disk
on every paint). Three views:

- **Today** — items scheduled or due today, plus overdue open items
  (a past date still in a TODO state).
- **Next 7 days** — one block per day for the coming week.
- **TODO list** — every open TODO headline, grouped by file, ignoring
  dates.

**Filtering** uses a small subset of Org's agenda syntax: `+work-personal`
requires the `work` tag and excludes `personal`. Keyword and file filters
are also available.

**Repeating tasks.** Completing a headline that carries a repeater bumps
its timestamp forward instead of marking it permanently done, exactly as
Emacs does:

- `+1w` — shift by one interval.
- `++1m` — shift by the interval, repeating until the date is in the
  future.
- `.+2d` — schedule relative to today.

The keyword returns to its first open state and the new weekday is
recomputed; only the timestamp on the planning line changes.

---

## Editing org inside the cockpit

`.org` files open in the normal Vim editor surface (syntax highlighting
arrives with the `tree-sitter-org` grammar; structural commands work
today). Palette commands (default leader bindings):

| Command | Binding | Effect |
|---------|---------|--------|
| `Org: Capture` | `<leader>oc` | template picker → quick-entry |
| `Org: Agenda` | `<leader>oa` | floating agenda |
| `Org: Jump To Inbox` | `<leader>oi` | open `inbox.org` |
| `Org: Toggle TODO State` | `<leader>ot` | cycle `TODO → DONE → none` |
| `Org: Schedule` | — | set/update `SCHEDULED:` on the cursor's headline |
| `Org: Deadline` | — | set/update `DEADLINE:` on the cursor's headline |
| `Org: Refile` | — | move the headline's subtree under another heading |

---

## The jot tray app (`cockpit-jot`)

`cockpit-jot` is a small, standalone process. It keeps the org root
indexed in memory and watches it for changes, so capture and agenda are
instant and available without the full cockpit open. Default global
hotkeys (configurable):

- `Ctrl+O` — **capture** (opens the template picker; the template key
  triggers immediate quick-entry).
- `Ctrl+Alt+A` — **agenda** (today's agenda).
- `Ctrl+Alt+O` — **org overview** (the list of files).

> `Ctrl+O` is a global override and collides with "Open file" in most
> editors and browsers — those apps lose their local `Ctrl+O` while jot is
> running. Chord conflicts are detected at registration and surfaced as a
> tray toast (never a silent failure); pick a different chord if you'd
> rather keep `Ctrl+O` local.

When the main cockpit edits an `.org` file it goes through jot (over a
local Unix socket) so jot's in-memory index stays live; when jot isn't
running, the cockpit reads and writes the files directly. **Both paths
produce byte-identical files** — they share the same line-range edit
primitives.

> **Status.** The headless engine (parser, capture, agenda, repeaters,
> the IPC service, and the controller) ships in this release. The tray
> icon, global-hotkey registration, and the floating popover window are
> wired up on platforms with a desktop session; on a headless build the
> same controller is reachable via the `cockpit-jot` CLI
> (`cockpit-jot [--root DIR] [agenda|overview]`).

---

## Sync — bring your own

The org root is a folder of plain-text files, so any sync layer you
already run works without cockpit doing anything. Cockpit ships **no**
sync engine.

- **git** — point a working copy at the root and `git pull` / `git push`
  from the cockpit terminal (or the v0.8 Lazygit recipe). Merge conflicts
  resolve in the editor like any other text file.
- **Syncthing / iCloud Drive / Dropbox / NextCloud** — point the sync
  folder at the org root. Cockpit's file watcher picks up remote writes
  and re-indexes within about a second.

The one cockpit-side concern is concurrent writes — a sync client
touching a file cockpit has open. Before overwriting, cockpit re-reads the
file's on-disk content and checks its recorded hash; if it changed, you
get a *"file changed on disk; reload?"* prompt instead of a clobber — the
same model the editor already uses for external buffer changes. Worst
case with plain-text + git-friendly files is a three-way merge, not data
loss.
