-- runtime.theme-by-time-of-day — example extension (v0.9 M9.8).
--
-- Registers an in-Lua theme entry so users see the shape of a
-- third-party theme registration in their palette debug surface.
-- Schedule-driven theme switching needs a `clock` capability and the
-- live `cockpit.themes.switch(name)` API, both of which arrive in a
-- follow-up alongside the capability-gated namespaces.
--
-- Disable in extensions.toml:
--
--   [extensions."runtime.theme-by-time-of-day"]
--   enabled = false

cockpit.themes.register {
  name   = "user.example",
  colors = {
    background      = "#11111b",
    pane_background = "#181825",
    text            = "#cdd6f4",
    accent          = "#89b4fa",
  },
}
