# `tab:set_search_items(ITEMS)`

Gives a tab pages that tab search can hit besides the tab itself: a list of
`{ title = ..., url = ... }` tables. For a tab that stands for something outside
the mux, such as the window of a browser the tab drives.

Each item becomes an extra row under the tab, matched on title and URL, and
only shown once a query is typed. Picking one activates the tab and emits
[`search-item-picked`](../window-events/search-item-picked.md).

```lua
tab:set_search_items {
  { title = 'Example Domain', url = 'https://example.com/' },
}
```
