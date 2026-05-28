-- runtime.session-toast — example extension (v0.9 M9.8).
--
-- Listens on `mux.pane_exit` and surfaces a status-line toast when a
-- pane's process exits with a non-zero code. Demonstrates the
-- `cockpit.events.on(event, fn)` API and the in-handler `ctx.toast`.
--
-- Disable in extensions.toml:
--
--   [extensions."runtime.session-toast"]
--   enabled = false

cockpit.events.on("mux.pane_exit", function(ctx)
  if ctx.exit_code ~= 0 then
    cockpit.toast(
      "Pane " .. ctx.pane .. " exited with " .. ctx.exit_code
    )
  end
end)
