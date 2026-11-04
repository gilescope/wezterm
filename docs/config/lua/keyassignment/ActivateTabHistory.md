# `ActivateTabHistory(delta)`

{{since('nightly')}}

Walks the window's tab history like a browser's back and forward buttons:
every tab you switch to is appended, going back (negative `delta`) returns
to the tab shown before, going forward (positive `delta`) returns to the one
you went back from, and switching to a fresh tab drops the forward entries.
Closed tabs are stepped over. At either end this has no effect.

The `←` and `→` buttons at the left of a horizontal tab bar (including the
child row of a Left/Right bar) do the same with the mouse; see
[show_tab_history_buttons_in_tab_bar](../config/show_tab_history_buttons_in_tab_bar.md).

The default bindings are `SUPER+[` (back) and `SUPER+]` (forward).

```lua
config.keys = {
  { key = '[', mods = 'SUPER', action = wezterm.action.ActivateTabHistory(-1) },
  { key = ']', mods = 'SUPER', action = wezterm.action.ActivateTabHistory(1) },
}
```

Unlike [ActivateLastTab](ActivateLastTab.md), which toggles between the two
most recent tabs, this keeps the whole trail.
