# `ShowTabSearch`

{{since('nightly')}}

Opens a dropdown under the tab bar's search box. Type to fuzzy-match the
window's tabs by title, working directory, foreground program or 1-based
index; `Up`/`Down` (or `CTRL-p`/`CTRL-n`) move the highlight, `Enter` switches
to the highlighted tab, `Escape` closes it. Clicking the `⌕ search` box in a
horizontal tab bar (including the child row of a Left/Right bar) does the
same; see [show_tab_search_in_tab_bar](../config/show_tab_search_in_tab_bar.md).

The default binding is `SUPER+SHIFT+f`.

```lua
config.keys = {
  { key = 'f', mods = 'SUPER|SHIFT', action = wezterm.action.ShowTabSearch },
}
```
