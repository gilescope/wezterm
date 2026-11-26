---
tags:
  - mouse
---
# `click_hyperlinks_when_mouse_grabbed = false`

When `true`, a plain click on a highlighted hyperlink opens it even if the
program in the pane has enabled mouse reporting; neither the press nor the
release is sent to the program. When `false` (the default) you must hold
[bypass_mouse_reporting_modifiers](bypass_mouse_reporting_modifiers.md)
(`SHIFT` by default) to click the link.

Only a click that starts on the link is taken. A drag that starts elsewhere and
crosses or ends on a link still goes to the program, so its own text selection
works over links.

Trade-off: a program that draws its own clickable text over something matching
a hyperlink rule no longer receives that click.
