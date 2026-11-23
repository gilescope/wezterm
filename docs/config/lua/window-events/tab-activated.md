# `tab-activated`

The `tab-activated` event is emitted after the active tab of a window
changes, by whatever means: a click on the tab bar, a key assignment,
tab search or the history buttons.

The parameters are the [window](../window/index.md) object, the
[pane](../pane/index.md) object for the newly active pane, and `away_secs`:
how many seconds ago this tab was last switched away from, or `nil` for a
tab that has never been left.

Unlike `update-status`, the event is not coalesced: every switch calls the
handler once. A handler is a good place to show a
[resume card](../window/show_card.md) when the absence was long enough to
have lost the thread:

```lua
local wezterm = require 'wezterm'

wezterm.on('tab-activated', function(window, pane, away_secs)
  if away_secs and away_secs > 300 then
    window:show_card {
      pane:get_title(),
      'away ' .. math.floor(away_secs / 60) .. ' min',
    }
  end
end)
```
