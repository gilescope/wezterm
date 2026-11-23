# `window:get_dimensions()`

{{since('20210314-114017-04b7cedd')}}

Returns a Lua table representing the dimensions for the Window.

The table has the following fields:

- `pixel_width`: the width of the window in pixels
- `pixel_height`: the height of the window in pixels
- `dpi`: The DPI of the screen the window in on
- `is_full_screen`: whether the window is in full screen mode
- `content_left`: pixels from the left of the window to the panes (a left tab bar's width, else 0)
- `content_top`: pixels from the top of the window to the panes (a top tab bar's height, else 0)
