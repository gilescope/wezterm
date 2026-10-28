# `ActivateRootRelative`

{{since('nightly')}}

Activate a root (side-bar) tab relative to the current one, skipping child
tabs created with [SpawnChildTab](SpawnChildTab.md). `-1` is the previous root,
`1` the next; it wraps at either end. Without nested tabs it behaves like
[ActivateTabRelative](ActivateTabRelative.md).

```lua
local act = wezterm.action

config.keys = {
  { key = 'UpArrow', mods = 'SUPER|ALT', action = act.ActivateRootRelative(-1) },
  { key = 'DownArrow', mods = 'SUPER|ALT', action = act.ActivateRootRelative(1) },
}
```
