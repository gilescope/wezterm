//! Resume card: a box of text centred over the pane, shown by Lua when the
//! user returns to a tab after an absence (`window:show_card`). The cue the
//! interruption literature asks for: what this tab is about, read in one
//! glance. Any key or click dismisses it and still reaches the pane.

use crate::termwindow::box_model::*;
use crate::termwindow::modal::Modal;
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{DimensionContext, TermWindow};
use crate::utilsprites::RenderMetrics;
use config::Dimension;
use std::cell::{Ref, RefCell};
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};
use window::color::LinearRgba;

pub struct ResumeCard {
    /// Already wrapped by the caller; the first line is the header.
    lines: Vec<String>,
    element: RefCell<Option<Vec<ComputedElement>>>,
}

impl ResumeCard {
    pub fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            element: RefCell::new(None),
        }
    }

    fn compute(&self, term_window: &mut TermWindow) -> anyhow::Result<Vec<ComputedElement>> {
        let font = term_window
            .fonts
            .command_palette_font()
            .expect("to resolve command palette font");
        let metrics = RenderMetrics::with_font_metrics(&font.metrics())
            .scale_line_height(term_window.config.command_palette_line_height);

        let fg: InheritableColor = term_window
            .config
            .command_palette_fg_color
            .to_linear()
            .into();
        let bg: InheritableColor = term_window
            .config
            .command_palette_bg_color
            .to_linear()
            .into();
        let body: InheritableColor = term_window
            .config
            .command_palette_fg_color
            .to_linear()
            .mul_alpha(0.85)
            .into();

        let rows: Vec<Element> = self
            .lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let text = if i == 0 { fg.clone() } else { body.clone() };
                Element::new(&font, ElementContent::Text(line.clone()))
                    .colors(ElementColors {
                        border: BorderColor::default(),
                        bg: LinearRgba::TRANSPARENT.into(),
                        text,
                    })
                    .display(DisplayType::Block)
            })
            .collect();

        // Centre the box in the pane area, between the bars.
        let dims = term_window.dimensions;
        let border = term_window.get_os_border();
        let (top_bar, bottom_bar) = term_window.horizontal_bar_heights();
        let (padding_left, padding_top) = term_window.padding_left_top();
        let cell_w = metrics.cell_size.width as f32;
        let cell_h = metrics.cell_size.height as f32;
        let area_x = term_window.left_bar_width() + border.left.get() as f32 + padding_left;
        let area_w = dims.pixel_width as f32 - border.right.get() as f32 - area_x;
        let area_y = top_bar + border.top.get() as f32 + padding_top;
        let area_h = dims.pixel_height as f32 - bottom_bar - border.bottom.get() as f32 - area_y;
        let widest = self
            .lines
            .iter()
            .map(|l| textwrap::core::display_width(l))
            .max()
            .unwrap_or(0) as f32;
        let width = ((widest + 1.5) * cell_w)
            .min(area_w * 0.85)
            .max(20. * cell_w);
        let height = (self.lines.len() as f32 + 0.5) * cell_h;
        let x = area_x + ((area_w - width) / 2.).max(0.);
        let y = area_y + ((area_h - height) / 2.).max(0.);

        let corner = |poly| SizedPoly {
            width: Dimension::Cells(0.25),
            height: Dimension::Cells(0.25),
            poly,
        };
        let element = Element::new(&font, ElementContent::Children(rows))
            .colors(ElementColors {
                border: BorderColor::new(
                    term_window
                        .config
                        .command_palette_fg_color
                        .to_linear()
                        .mul_alpha(0.4),
                ),
                bg,
                text: fg,
            })
            .padding(BoxDimension {
                left: Dimension::Cells(0.75),
                right: Dimension::Cells(0.75),
                top: Dimension::Cells(0.25),
                bottom: Dimension::Cells(0.25),
            })
            .border(BoxDimension::new(Dimension::Pixels(1.)))
            .border_corners(Some(Corners {
                top_left: corner(TOP_LEFT_ROUNDED_CORNER),
                top_right: corner(TOP_RIGHT_ROUNDED_CORNER),
                bottom_left: corner(BOTTOM_LEFT_ROUNDED_CORNER),
                bottom_right: corner(BOTTOM_RIGHT_ROUNDED_CORNER),
            }))
            .min_width(Some(Dimension::Pixels(width)))
            .max_width(Some(Dimension::Pixels(width)));

        let computed = term_window.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: dims.dpi as f32,
                    pixel_max: dims.pixel_height as f32,
                    pixel_cell: cell_h,
                },
                width: DimensionContext {
                    dpi: dims.dpi as f32,
                    pixel_max: dims.pixel_width as f32,
                    pixel_cell: cell_w,
                },
                bounds: euclid::rect(x, y, width, area_h - (y - area_y)),
                metrics: &metrics,
                gl_state: term_window.render_state.as_ref().unwrap(),
                zindex: 100,
            },
            &element,
        )?;
        Ok(vec![computed])
    }
}

impl Modal for ResumeCard {
    fn mouse_event(&self, _event: MouseEvent, _term_window: &mut TermWindow) -> anyhow::Result<()> {
        Ok(())
    }

    /// Any key closes the card and is not consumed: the user was going to
    /// type it into the pane anyway.
    fn key_down(
        &self,
        _key: KeyCode,
        _mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        term_window.cancel_modal();
        Ok(false)
    }

    fn computed_element(
        &self,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<Ref<'_, [ComputedElement]>> {
        if self.element.borrow().is_none() {
            let element = self.compute(term_window)?;
            self.element.borrow_mut().replace(element);
        }
        Ok(Ref::map(self.element.borrow(), |v| {
            v.as_ref().unwrap().as_slice()
        }))
    }

    fn reconfigure(&self, _term_window: &mut TermWindow) {
        self.element.borrow_mut().take();
    }
}
