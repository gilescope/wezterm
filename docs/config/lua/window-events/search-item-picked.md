# `search-item-picked`

Emitted after tab search activates a tab because the user picked one of the
tab's [search items](../MuxTab/set_search_items.md) rather than the tab itself.

The parameters are the [window](../window/index.md) object and the
[pane](../pane/index.md) object. Ask the active tab which item with
[tab:take_search_pick()](../MuxTab/take_search_pick.md):

```lua
wezterm.on('search-item-picked', function(window, pane)
  local tab = window:active_tab()
  local i = tab:take_search_pick()
  if i then
    wezterm.log_info('picked item ' .. i)
  end
end)
```
