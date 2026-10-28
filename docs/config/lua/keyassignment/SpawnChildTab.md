# `SpawnChildTab`

{{since('nightly')}}

Create a new tab nested under the current tab's root, in the same working
directory as the current pane. The argument names the *domain*, exactly as for
[SpawnTab](SpawnTab.md).

With the tab bar on the Left or Right, the side bar lists only root tabs; the
active root's children (and the root itself) show in a row across the top of
the panes, so a project tab can carry a few related shells, say an agent and a
plain prompt in the same directory, that you flip between without leaving the
side tab. The row is hidden while a root has no children. With a Top or
Bottom bar a child is simply placed after its root.

Closing a root promotes its children to roots. Dragging a root in the side bar
moves its children with it.

```lua
local act = wezterm.action

config.keys = {
  { key = 't', mods = 'SUPER|SHIFT', action = act.SpawnChildTab 'CurrentPaneDomain' },
}
```

See also [ActivateRootRelative](ActivateRootRelative.md),
[window:spawn_tab{parent_tab_id=}](../mux-window/spawn_tab.md) and
[tab:set_parent()](../MuxTab/set_parent.md).
