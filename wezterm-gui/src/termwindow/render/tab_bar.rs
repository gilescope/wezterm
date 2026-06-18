use crate::quad::TripleLayerQuadAllocator;
use crate::termwindow::render::RenderScreenLineParams;
use crate::termwindow::{UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{ConfigHandle, DimensionContext, TabBarPosition};
use mux::renderable::RenderableDimensions;
use wezterm_term::color::ColorAttribute;
use window::color::LinearRgba;

impl crate::TermWindow {
    /// Returns the effective tab bar position, taking into account both
    /// `tab_bar_position` and the legacy `tab_bar_at_bottom` config option.
    pub fn resolved_tab_bar_position(&self) -> TabBarPosition {
        self.config.resolved_tab_bar_position()
    }

    /// Returns true if the tab bar is positioned vertically (Left or Right).
    pub fn is_tab_bar_vertical(&self) -> bool {
        self.resolved_tab_bar_position().is_vertical()
    }

    /// Returns the effective use_fancy_tab_bar setting.
    /// Vertical tab bar positions require fancy mode, so we force it on.
    pub fn effective_use_fancy_tab_bar(&self) -> bool {
        if self.is_tab_bar_vertical() {
            true
        } else {
            self.config.use_fancy_tab_bar
        }
    }

    pub fn paint_tab_bar(&mut self, layers: &mut TripleLayerQuadAllocator) -> anyhow::Result<()> {
        if self.effective_use_fancy_tab_bar() {
            if self.fancy_tab_bar.is_none() {
                let palette = self.palette().clone();
                let tab_bar = self.build_fancy_tab_bar(&palette)?;
                self.fancy_tab_bar.replace(tab_bar);
            }

            self.ui_items.append(&mut self.paint_fancy_tab_bar()?);
            // A vertical tab bar gets a thin draggable handle straddling its
            // inner edge so the user can resize the bar width. Pushed last so
            // it wins hit-testing (resolve_ui_item scans in reverse).
            if self.is_tab_bar_vertical() {
                let border = self.get_os_border();
                let tab_bar_width = self.tab_bar_pixel_width();
                const HANDLE: f32 = 6.0;
                // The handle sits on the terminal side of the inner edge so it
                // never overlaps tab content (notably the close button, which
                // floats to the tab's inner edge on a Left bar).
                let handle_x = match self.resolved_tab_bar_position() {
                    TabBarPosition::Left => border.left.get() as f32 + tab_bar_width,
                    // Right (and any other vertical position): terminal is to
                    // the left of the bar, so the handle extends leftwards.
                    _ => {
                        self.dimensions.pixel_width as f32
                            - border.right.get() as f32
                            - tab_bar_width
                            - HANDLE
                    }
                };
                let height = (self.dimensions.pixel_height as f32
                    - (border.top + border.bottom).get() as f32)
                    .max(0.);
                self.ui_items.push(UIItem {
                    x: handle_x.max(0.) as usize,
                    y: border.top.get() as usize,
                    width: HANDLE as usize,
                    height: height as usize,
                    item_type: UIItemType::TabBarSeparator,
                });
            }
            return Ok(());
        }

        // Classic (retro) tab bar rendering — only for Top/Bottom positions
        let border = self.get_os_border();
        let pos = self.resolved_tab_bar_position();

        let palette = self.palette().clone();
        let tab_bar_height = self.tab_bar_pixel_height()?;
        let tab_bar_y = if pos == TabBarPosition::Bottom {
            ((self.dimensions.pixel_height as f32) - (tab_bar_height + border.bottom.get() as f32))
                .max(0.)
        } else {
            border.top.get() as f32
        };

        // Register the tab bar location
        self.ui_items.append(&mut self.tab_bar.compute_ui_items(
            tab_bar_y as usize,
            self.render_metrics.cell_size.height as usize,
            self.render_metrics.cell_size.width as usize,
        ));

        let window_is_transparent =
            !self.window_background.is_empty() || self.config.window_background_opacity != 1.0;
        let gl_state = self.render_state.as_ref().unwrap();
        let white_space = gl_state.util_sprites.white_space.texture_coords();
        let filled_box = gl_state.util_sprites.filled_box.texture_coords();
        let default_bg = palette
            .resolve_bg(ColorAttribute::Default)
            .to_linear()
            .mul_alpha(if window_is_transparent {
                0.
            } else {
                self.config.text_background_opacity
            });

        self.render_screen_line(
            RenderScreenLineParams {
                top_pixel_y: tab_bar_y,
                left_pixel_x: 0.,
                pixel_width: self.dimensions.pixel_width as f32,
                stable_line_idx: None,
                line: self.tab_bar.line(),
                selection: 0..0,
                cursor: &Default::default(),
                palette: &palette,
                dims: &RenderableDimensions {
                    cols: self.dimensions.pixel_width
                        / self.render_metrics.cell_size.width as usize,
                    physical_top: 0,
                    scrollback_rows: 0,
                    scrollback_top: 0,
                    viewport_rows: 1,
                    dpi: self.terminal_size.dpi,
                    pixel_height: self.render_metrics.cell_size.height as usize,
                    pixel_width: self.terminal_size.pixel_width,
                    reverse_video: false,
                },
                config: &self.config,
                cursor_border_color: LinearRgba::default(),
                foreground: palette.foreground.to_linear(),
                pane: None,
                is_active: true,
                selection_fg: LinearRgba::default(),
                selection_bg: LinearRgba::default(),
                cursor_fg: LinearRgba::default(),
                cursor_bg: LinearRgba::default(),
                cursor_is_default_color: true,
                white_space,
                filled_box,
                window_is_transparent,
                default_bg,
                style: None,
                font: None,
                use_pixel_positioning: self.config.experimental_pixel_positioning,
                render_metrics: self.render_metrics,
                shape_key: None,
                password_input: false,
            },
            layers,
        )?;

        Ok(())
    }

    pub fn tab_bar_pixel_height_impl(
        config: &ConfigHandle,
        fontconfig: &wezterm_font::FontConfiguration,
        render_metrics: &RenderMetrics,
    ) -> anyhow::Result<f32> {
        if config.use_fancy_tab_bar || config.resolved_tab_bar_position().is_vertical() {
            let font = fontconfig.title_font()?;
            Ok((font.metrics().cell_height.get() as f32 * 1.75).ceil())
        } else {
            Ok(render_metrics.cell_size.height as f32)
        }
    }

    pub fn tab_bar_pixel_height(&self) -> anyhow::Result<f32> {
        Self::tab_bar_pixel_height_impl(&self.config, &self.fonts, &self.render_metrics)
    }

    /// Returns the pixel width of the tab bar when positioned vertically (Left/Right).
    /// Returns 0.0 for horizontal (Top/Bottom) positions.
    pub fn tab_bar_pixel_width_impl(
        config: &ConfigHandle,
        render_metrics: &RenderMetrics,
        dimensions: &::window::Dimensions,
    ) -> f32 {
        if config.resolved_tab_bar_position().is_vertical() {
            let context = DimensionContext {
                dpi: dimensions.dpi as f32,
                pixel_max: dimensions.pixel_width as f32,
                pixel_cell: render_metrics.cell_size.width as f32,
            };
            config.tab_bar_width.evaluate_as_pixels(context)
        } else {
            0.0
        }
    }

    pub fn tab_bar_pixel_width(&self) -> f32 {
        // A drag-resized width takes precedence, but only while the bar is vertical.
        if let Some(width) = self.tab_bar_width_override {
            if self.config.resolved_tab_bar_position().is_vertical() {
                return width;
            }
        }
        Self::tab_bar_pixel_width_impl(&self.config, &self.render_metrics, &self.dimensions)
    }
}
