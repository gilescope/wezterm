# Wez's Terminal

<img height="128" alt="WezTerm Icon" src="https://raw.githubusercontent.com/wezterm/wezterm/main/assets/icon/wezterm-icon.svg" align="left"> *A GPU-accelerated cross-platform terminal emulator and multiplexer written by <a href="https://github.com/wez">@wez</a> and implemented in <a href="https://www.rust-lang.org/">Rust</a>*

User facing docs and guide at: https://wezterm.org/

![Screenshot](docs/screenshots/two.png)

*Screenshot of wezterm on macOS, running vim*

## This fork: tab supremacy

This is [gilescope](https://github.com/gilescope)'s fork. It keeps up with
upstream and adds a tab bar that does more than any browser's:

* **Nested tabs**: a side bar of root tabs, each with its own child tabs.
* **Back and forward** through the tabs you have shown, like a browser's history.
* **Tab search** over titles, directories and programs, and over pages a tab carries.
* **A browser in a tab**: the Kitty graphics fixes here (acknowledged transmits,
  shared memory on macOS, no hashing of streamed frames) let
  [awrit](https://github.com/gilescope/awrit) draw Chromium into a pane, so a web
  page is a child tab like any other. The Lua that manages those tabs is not in
  this repository yet.
* **Matrix rain** between tabs: the old tab, pictures included, falls away as the
  new one builds; scrolling through tabs holds the rain until you land, and it
  waits for a tab that is still loading.
* **Plain-click links**, even when the program in the pane has grabbed the mouse.
* **A resume card** when you return to a tab after a while away.

Most of it is opt-in through the config; see `docs/config/lua`.

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
