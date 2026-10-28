# `pane:load_scrollback(PATH)`

{{since('nightly')}}

Seeds the pane's scrollback from a file written by
[pane:save_scrollback()](save_scrollback.md). The restored lines go above
whatever the pane already holds, followed by a dim `── restored ──` rule, so
calling it right after `spawn_tab` in `gui-startup` puts the previous
session's history behind the new prompt. Lines wider than the new pane are
re-wrapped; the scrollback limit still applies.

Returns the number of lines restored.

```lua
wezterm.on('gui-startup', function()
  local tab, pane, window = wezterm.mux.spawn_window { cwd = wezterm.home_dir }
  pcall(function()
    pane:load_scrollback(wezterm.home_dir .. '/.local/state/wezterm/scrollback/last.bin')
  end)
end)
```
