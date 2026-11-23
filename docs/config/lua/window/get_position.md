# `window:get_position()`

Returns a table `{x, y}` with the screen position, in pixels, of the top left
corner of the window's client area, or `nil` if the window system cannot report it.

Currently implemented on macOS only. The origin is the top left of the primary
screen, and units are physical pixels: divide by `dpi / 72` (see
[window:get_dimensions()](get_dimensions.md)) to get points, as used by
AppleScript window bounds.

```lua
wezterm.on('update-status', function(window, pane)
  local pos = window:get_position()
  if pos then
    wezterm.log_info('window at', pos.x, pos.y)
  end
end)
```
