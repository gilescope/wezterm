# `pane:has_unseen_command_result()`

{{since('nightly')}}

Returns true if a command finished in the pane since the last time the pane
was focused.

This is the quieter cousin of [pane:has_unseen_output()](has_unseen_output.md):
that one is set by any output at all, so a prompt redraw or a spinner frame
trips it. This one needs the shell to mark the end of a command with the OSC
133 `D` [shell integration](../../../shell-integration.md) sequence, and so
only fires when something actually completed while you were elsewhere.

It is cleared when the pane regains focus. Also available as
`pane.has_unseen_command_result` in [PaneInformation](../PaneInformation.md).
