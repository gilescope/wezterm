---
tags:
  - tab_bar
---
# `switch_to_last_active_tab_when_closing_tab = false`

{{since('20220905-102802-7d4b8249')}}

If set to `true`, when the active tab is closed, the previously activated tab
will be activated.

Otherwise, the tab to the left of the active tab will be activated.

In this fork the tab history comes first: closing the active tab goes back to
the most recent tab in the history that still exists. This option only decides
what happens when the history has nowhere to go back to.
