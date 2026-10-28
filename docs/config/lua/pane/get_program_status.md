# `pane:get_program_status()`

{{since('nightly')}}

Returns what the program running in the pane says it is doing, as reported
through the [Program Status Protocol (OSC 7501)](https://www.superlogical.com/rex/docs/build/program-status),
or `nil` when nothing has been reported.

Coding agents such as Claude Code and Codex emit these reports, so a tab can
show whether the agent is working, waiting on you, or finished.

A report has the form:

```
ESC ] 7501 ; state=STATE : key=value : ... ST
```

WezTerm keeps one record per `id` and returns the most urgent one:
`blocked`, then `error`, `working`, `done`, `idle`. The result is a table:

* `state` - one of `"idle"`, `"working"`, `"blocked"`, `"done"`, `"error"`
* `kind` - for `blocked`, why: `"permission"`, `"question"` or `"auth"`
* `progress` - 0-100 for `working`/`blocked`, or `nil` when indeterminate
* `app` - the program's stable name, such as `"claude"`
* `title` - a short label for the record, if given
* `msg` - one line describing what it is doing or waiting for, if given

`working`, `blocked` and `idle` records are dropped when the shell starts a
new prompt (OSC 133 `A`); `done` and `error` stay until replaced or cleared.
WezTerm answers the protocol's feature query, so programs that probe before
reporting will report.

When the status changes, the tab bar is updated and
[format-tab-title](../window-events/format-tab-title.md) fires; the same
information is available there as `pane.program_status`, alongside
`pane.spinner`, a glyph that advances every 100ms while the pane is working:

```lua
wezterm.on('format-tab-title', function(tab)
  local pane = tab.active_pane
  local s = pane.program_status
  local glyph = ''
  if s and s.state == 'blocked' then
    glyph = (s.kind == 'question') and '? ' or '⏸︎ '
  elseif s and s.state == 'working' then
    glyph = pane.spinner .. ' '
  elseif pane.has_unseen_command_result then
    glyph = '▸ '
  end
  return glyph .. pane.title
end)
```

See also [pane:has_unseen_command_result()](has_unseen_command_result.md) and
[pane:get_progress()](get_progress.md).
