# `window:tab_history()`

Returns two values: an array table of the [MuxTab](../MuxTab/index.md) objects
the window has visited, oldest first (the tab-history that back/forward walk),
and the 1-based position of the current entry. The active tab is always the
entry at that position. Closed tabs may appear until they are stepped over.

Save it with the tab list to put back after a restart; see
[set_tab_history](set_tab_history.md).
