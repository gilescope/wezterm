# `window:set_tab_history(tabs, pos)`

Replaces the window's tab-history with the array of
[MuxTab](../MuxTab/index.md) objects `tabs`, with `pos` (1-based) as the
current entry. Tabs not in the window are dropped. Call it after activating
the tab that should be current, so back and forward work straight away.

```lua
local tabs, pos = window:tab_history()
-- ... later, in a rebuilt window:
window:set_tab_history(tabs, pos)
```
