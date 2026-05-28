-- runtime.format-paragraph — example extension (v0.9 M9.8).
--
-- Registers a `user.format-paragraph` palette command. The real
-- paragraph rewrap lives on the editor side and arrives via a
-- capability-gated buffer-edit API; today we just demonstrate the
-- registration + toast surface so users can copy the pattern.
--
-- Disable in extensions.toml:
--
--   [extensions."runtime.format-paragraph"]
--   enabled = false

cockpit.commands.register {
  id    = "user.format-paragraph",
  title = "Editor: Format Paragraph (Lua)",
  run   = function(ctx)
    if ctx.path then
      ctx.toast("format-paragraph called on " .. ctx.path)
    else
      ctx.toast("format-paragraph: no open document")
    end
  end,
}
