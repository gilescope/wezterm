---
tags:
  - tab_bar
---
# `show_tab_history_buttons_in_tab_bar = true`

{{since('nightly')}}

When `true` (the default) `←` and `→` buttons are drawn before the tabs of a
horizontal tab bar; clicking them walks the tab history as
[ActivateTabHistory](../keyassignment/ActivateTabHistory.md) does from the
keyboard. A button with nowhere to go is drawn faint. With a Left/Right bar
the buttons sit in the child row across the top.

Set to `false` to leave them out. The key bindings still work.
