---
tags:
  - tab_bar
---
# `show_browser_button_in_tab_bar = false`

When `true`, the tab bar shows a tab-styled `Browser` button beside the search
box. Clicking it emits the `browser-toggle` event, so a script decides what a
browser is. The button is drawn lit while the active tab is titled `Browser`,
so a script that hosts a browser in a tab of that name gets selection for free.

```lua
wezterm.on('browser-toggle', function(window, pane)
  -- show or hide your browser here
end)
```
