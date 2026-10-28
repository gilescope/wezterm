# `pane:save_scrollback(PATH)`

{{since('nightly')}}

Writes the pane's primary-screen lines (scrollback plus viewport, with colours,
attributes, soft-wrap state and hyperlinks; images are dropped) to `PATH`, for
restoring with [pane:load_scrollback()](load_scrollback.md) after a restart.

Returns `true` when a file was written. It returns `false` without touching the
disk when nothing changed since the last save, when the last save was under
five seconds ago, or while the alternate screen (vim, less) is active, so it
is cheap to call from `update-status` for every pane.

The file is written atomically and readable only by the owner. Its format is
specific to the WezTerm build that wrote it; an incompatible file is refused
on load rather than guessed at.

```lua
wezterm.on('update-status', function(window, pane)
  for i, tab in ipairs(window:mux_window():tabs()) do
    local p = tab:active_pane()
    p:save_scrollback(wezterm.home_dir .. '/.local/state/wezterm/scrollback/' .. p:pane_id() .. '.bin')
  end
end)
```
