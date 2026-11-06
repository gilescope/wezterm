# `window:show_card(lines)`

Shows a card of text centred over the active pane, decoded out of matrix
rain the way the tab-switch easter egg is: `lines` is a table of strings,
already wrapped to taste, drawn left-aligned on a dark backing. The card
stays until any key or mouse click, which is then delivered to the pane as
usual, and dissolves back into the rain; a tab change drops it at once.

It is meant for the "where was I" cue on returning to a tab after a while;
see the [`tab-activated`](../window-events/tab-activated.md) event for the
trigger and the `away_secs` it carries.

```lua
window:show_card {
  'fix-sync-stall · idle 25 min',
  'goal   make the sync stall reproducible under Antithesis',
  'asked  why does the second run never finish?',
}
```
