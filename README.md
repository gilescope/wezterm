# Wez's Terminal

<img height="128" alt="WezTerm Icon" src="https://raw.githubusercontent.com/wezterm/wezterm/main/assets/icon/wezterm-icon.svg" align="left"> *A GPU-accelerated cross-platform terminal emulator and multiplexer written by <a href="https://github.com/wez">@wez</a> and implemented in <a href="https://www.rust-lang.org/">Rust</a>*

User facing docs and guide at: https://wezterm.org/

![Screenshot](docs/screenshots/two.png)

*Screenshot of wezterm on macOS, running vim*

## This fork: tab supremacy

This is [gilescope](https://github.com/gilescope)'s fork. It keeps up with
upstream and adds a tab bar that does more than any browser's. Everything is
opt-in through the config; each name below has a page under `docs/config/lua`.

### The tab bar

| Feature                                                | How                                                                          |
| ------------------------------------------------------ | ---------------------------------------------------------------------------- |
| Tab bar down the side                                  | `tab_bar_position = "Left"` or `"Right"`, `tab_bar_width`                    |
| Nested tabs: root tabs, each with child tabs           | `SpawnChildTab`, `ActivateRootRelative`, `tab:get_parent()` / `set_parent()` |
| Back and forward through the tabs you have shown       | `show_tab_history_buttons_in_tab_bar`, `ActivateTabHistory`                  |
| Search tabs by title, directory, program or page       | `show_tab_search_in_tab_bar`, `ShowTabSearch`, `tab:set_search_items()`      |
| A button for a browser tab                             | `show_browser_button_in_tab_bar`, `browser_button_label`                     |
| Reorder tabs                                           | `MoveTabRelative`                                                            |
| See what a tab's program is doing, or that it finished | `pane:get_program_status()`, `pane:has_unseen_command_result()`              |

### Switching tabs

* **Matrix rain** plays between tabs: the old tab falls away, pictures included,
  as the new one builds, with the occasional film quote decoded into the rain.
* **Scrolling through tabs** holds the rain with no destination until you stop,
  then resolves into the tab you landed on.
* **A tab that is still loading** can ask the rain to wait for its picture (the
  `wezterm_hold_rain_for_picture` user variable), so the load hides in the effect.
* **A resume card** can greet you when you return to a tab after a while away:
  the `tab-activated` event and `window:show_card()`.

### A browser in a tab

The Kitty graphics fixes here let [awrit](https://github.com/gilescope/awrit)
draw Chromium into a pane on macOS, so a web page is a child tab like any other:

* a transmit naming its image with `i=` is acknowledged;
* shared-memory frames work on macOS, and are not hashed when large;
* a shifted key reports Shift under the Kitty keyboard protocol, so `@` arrives as `@`.

Set `enable_kitty_graphics` and `enable_kitty_keyboard`. The Lua that opens,
unloads and reloads the browser tabs is not in this repository yet.

### Smaller things

* **Plain-click links** even when the program in the pane has grabbed the mouse:
  `click_hyperlinks_when_mouse_grabbed`.
* **Cmd-C** reaches a mouse-grabbing program when wezterm has no selection.
* **Paste** goes into the tab search box while it is open.
* **Scrollback survives a restart**: `pane:save_scrollback()` and `pane:load_scrollback()`.
* **Place a window over the pane area** from Lua: `window:get_position()`, and
  `content_left` / `content_top` in `window:get_dimensions()`.

## Installation

https://wezterm.org/installation

## Getting help

This is a spare time project, so please bear with me.  There are a couple of channels for support:

* You can use the [GitHub issue tracker](https://github.com/wezterm/wezterm/issues) to see if someone else has a similar issue, or to file a new one.
* Start or join a thread in our [GitHub Discussions](https://github.com/wezterm/wezterm/discussions); if you have general
  questions or want to chat with other wezterm users, you're welcome here!
* There is a [Matrix room via Element.io](https://matrix.to/#/#wezterm:matrix.org)
  for (potentially!) real time discussions.

The GitHub Discussions and Element/Gitter rooms are better suited for questions
than bug reports, but don't be afraid to use whichever you are most comfortable
using and we'll work it out.

## Supporting the Project

If you use and like WezTerm, please consider sponsoring it: your support helps
to cover the fees required to maintain the project and to validate the time
spent working on it!

[Read more about sponsoring](https://wezterm.org/sponsor.html).

* [![Sponsor WezTerm](https://img.shields.io/github/sponsors/wez?label=Sponsor%20WezTerm&logo=github&style=for-the-badge)](https://github.com/sponsors/wez)
* [Patreon](https://patreon.com/WezFurlong)
* [Ko-Fi](https://ko-fi.com/wezfurlong)
* [Liberapay](https://liberapay.com/wez)
