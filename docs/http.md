# HTTP requests (Bruno collections)

Cockpit reads [Bruno](https://www.usebruno.com/)-style `.bru` collections
(v0.11). Open a `.bru` file and the editor pane splits into a **request**
half (the file, edited with the normal Vim surface) and a **response**
half (Body / Headers / Timing / Raw tabs). Sending runs on a worker
thread through a real `reqwest` engine; the response lands on the next
frame.

There is **no separate HTTP client window** and no proprietary format —
the collection is plain `.bru` text that round-trips through git like any
other source file.

---

## Where collections live

Cockpit recognises a Bruno collection at the project root when **either**:

- a `bruno.json` file exists at the root, **or**
- one or more `.bru` files exist at the root.

The loader then walks the tree, parses every `.bru` request, and reads
environment files from an `environments/` subdirectory:

```
my-api/
├── bruno.json                 # optional collection marker
├── list-users.bru
├── create-user.bru
└── environments/
    ├── dev.bru                # {{baseUrl}} = https://dev.api…
    └── prod.bru
```

Each environment file's stem (`dev.bru` → `dev`) is the environment name.

---

## Opening a request

Open any `.bru` file (file browser, `Ctrl+P`, `:e`). The pane splits:

```
┌──────────────────────────────────────┐
│ get {                                 │  ← request editor (Vim)
│   url: {{baseUrl}}/users              │
│   auth: bearer                        │
│ }                                     │
│ NORMAL                                │  ← request mode line
├──────────────────────────────────────┤  ← drag to resize
│ Body  Headers  Timing  Raw            │  ← response tab strip
│ Content-Type: application/json        │
│                                       │
│ {                                     │  ← active tab body
│   "ok": true                          │
│ }                                     │
└──────────────────────────────────────┘
```

- **Drag the divider** to resize the split (clamped so neither half
  collapses).
- **Click a tab** in the strip to switch the response view, or use the
  keybindings below.

---

## Commands & keybindings

`<leader>` is `Space`. Re-bind any command by id in your config; the
defaults are:

| Command                         | Id                          | Default      |
|---------------------------------|-----------------------------|--------------|
| Send Request                    | `http.send_request`         | `<leader>hs` |
| Switch Environment              | `http.switch_environment`   | `<leader>he` |
| Show Body tab                   | `http.tab.body`             | `<leader>h1` |
| Show Headers tab                | `http.tab.headers`          | `<leader>h2` |
| Show Timing tab                 | `http.tab.timing`           | `<leader>h3` |
| Show Raw tab                    | `http.tab.raw`              | `<leader>h4` |
| Next / Previous tab             | `http.next_tab` / `…prev_tab` | (palette)  |
| Send All In Folder              | `http.send_all_in_folder`   | (palette)    |
| Copy As cURL                    | `http.copy_as_curl`         | (palette)    |
| Save Response To File           | `http.save_response`        | (palette)    |
| Cancel In-flight Request        | `http.cancel`               | `Esc`        |

- **Send All In Folder** queues every `.bru` in the open file's directory
  and sends them sequentially; the status shows `[i/N]` progress and
  `Cancel` aborts the whole queue.
- **Copy As cURL** renders the selected request (with interpolation
  applied) as a POSIX-quoted `curl` invocation and copies it to the
  clipboard.
- **Save Response To File** writes the latest body to
  `<collection-root>/responses/<name>.<ext>`, picking the extension from
  the response `Content-Type` (`application/json` → `.json`, `text/html`
  → `.html`, …, unknown → `.bin`). Existing files prompt before being
  overwritten.

---

## Environments & interpolation

Requests use `{{var}}` placeholders resolved against the active
environment:

```
get {
  url: {{baseUrl}}/users
}

auth:bearer {
  token: {{authToken}}
}
```

- `<leader>he` opens a sub-palette listing every environment plus a
  `(none)` entry (run with no environment — templates without `{{vars}}`
  still work).
- The chosen environment **survives a restart** — it round-trips through
  the project cache.
- A `{{var}}` with no matching value is a send error, surfaced in the
  status bar rather than sent as a literal.

Supported request features: `get`/`post`/`put`/`patch`/`delete`/`head`/
`options`, `headers`, `query`, request bodies (`json`, `text`, `xml`,
`form-urlencoded`), and `auth` (`basic`, `bearer`). Disabled (`~`-prefixed)
rows are preserved on save.

---

## Scripts (Lua, capability-gated)

Bruno's JavaScript pre/post scripts **do not run**. Cockpit substitutes
**Lua** (reusing the v0.9 sandbox — see [`extensions.md`](extensions.md)):

```
script:lua-pre-request {
  cockpit.http.set_var("nonce", tostring(os.time and 0 or 0))
}

script:lua-post-response {
  local r = cockpit.http.response()
  if r.status == 200 then
    cockpit.http.set_var("authToken", "…")  -- feeds the next request
  end
}
```

- `cockpit.http.set_var(name, value)` / `cockpit.http.var(name)` read and
  write the active environment in place (post-response writes feed the
  next request).
- `cockpit.http.response()` (post-response only) returns a read-only
  table: `status`, `headers` (1-indexed `{name, value}` rows), `body`.
- The sandbox blocks `io`, `os.execute`, `package.loadlib`, and `require`.

**Scripts are default-deny.** Running any script requires the
`http.scripts` capability, granted per collection in
`~/.config/cockpit/extensions.toml`:

```toml
[http]
granted_collections = ["/home/me/work/my-api"]
```

Granting a parent directory covers nested collections. Until a collection
is granted, its Lua scripts are skipped with a status note, and a request
carrying JS scripts always shows:

> *Lua scripting only; JS scripts skipped. See docs/http.md.*

---

## What's not supported

- **JavaScript scripts** — Lua only (above). JS blocks are recognised but
  skipped, never silently dropped.
- **OAuth2 device-code flow** — deferred to v0.11.1. Use a pre-request
  Lua script or paste a bearer token for now.
- **GraphQL response introspection panel** — deferred. GraphQL requests
  send fine as a JSON body; there's no schema explorer.
- No request history beyond the file's git history, no cookie jar UI, no
  collection-level test runner/assertions.

---

## Security model

- **TLS** is handled by `reqwest`'s default verified client — no flag to
  disable certificate verification.
- **Secrets** belong in environment variables (`{{authToken}}`), kept out
  of the committed request files. Cockpit never stores passwords in
  cleartext on its own; `auth:basic` credentials live in the `.bru` you
  author and control.
- Sends run on a dedicated worker thread and are **cancellable** (`Esc`),
  so a hung host never blocks the UI.
- Lua scripts run sandboxed and capability-gated (above) — a collection
  can't execute scripts until you explicitly grant it.

---

## Worked example

Using the in-repo fixture collection (`tests/fixtures/http/`):

`list-users.bru` — a `GET` with a bearer token from the environment:

```
get {
  url: https://api.example.com/users
  body: none
  auth: bearer
}

headers {
  Accept: application/json
}

query {
  limit: 10
  page: 2
}

auth:bearer {
  token: {{authToken}}
}
```

1. Open `list-users.bru`.
2. `<leader>he` → pick an environment that defines `authToken`.
3. `<leader>hs` to send. The Body tab shows the pretty-printed JSON; the
   Timing tab shows elapsed time, redirect count, and the final URL.
4. `<leader>h2` to inspect response headers, or run **Copy As cURL** to
   reproduce the request on the command line.

`create-user.bru` shows a `POST` with a JSON body and `auth:basic`.
