# `tab:set_parent(TAB_ID)`

{{since('nightly')}}

Nests this tab under the root tab with the given id (from `tab:tab_id()`),
moving it to the end of that root's children; pass `nil` to make it a root
again. Returns true when the tab was moved. Only one level of nesting is
allowed: the parent must itself be a root.

```lua
local child, _, window = window:spawn_tab {}
child:set_parent(window:active_tab():tab_id())
```

See [SpawnChildTab](../keyassignment/SpawnChildTab.md).
