//! Tab search: a dropdown under the tab bar's search box that fuzzy-matches
//! the window's tabs by title, directory and program, and switches to the
//! chosen one. Keyboard-driven like the command palette; the box in the bar
//! only opens it.

use crate::overlay::selector::{matcher_pattern, matcher_score};
use crate::tabbar::TabBarItem;
use crate::termwindow::box_model::*;
use crate::termwindow::modal::Modal;
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{DimensionContext, TermWindow};
use crate::utilsprites::RenderMetrics;
use config::keyassignment::KeyAssignment;
use config::Dimension;
use mux::pane::CachePolicy;
use mux::Mux;
use std::cell::{Ref, RefCell};
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};
use window::color::LinearRgba;

/// One tab as the search sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct TabChoice {
    /// Index in the mux window; what `activate_tab` wants.
    pub tab_idx: usize,
    /// Nested under a root tab.
    pub is_child: bool,
    /// What the tab bar shows for it, falling back to the pane title.
    pub label: String,
    /// Directory and program, shown dimmer after the label.
    pub detail: String,
}

impl TabChoice {
    /// Everything the matcher may hit: label, detail and the 1-based index
    /// the bar numbers tabs with.
    fn haystack(&self) -> String {
        format!("{} {} {}", self.tab_idx + 1, self.label, self.detail)
    }
}

/// Indices into `choices` ordered best match first; empty query keeps bar order.
pub fn compute_matches(query: &str, choices: &[TabChoice]) -> Vec<usize> {
    if query.is_empty() {
        return (0..choices.len()).collect();
    }
    let pattern = matcher_pattern(query);
    let mut scored: Vec<(u32, usize)> = choices
        .iter()
        .enumerate()
        .filter_map(|(idx, c)| matcher_score(&pattern, &c.haystack()).map(|s| (s, idx)))
        .collect();
    // Stable on index so equal scores keep bar order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, idx)| idx).collect()
}

/// Last path component, or the whole path for `/`.
fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

struct MatchResults {
    query: String,
    matches: Vec<usize>,
}

pub struct TabSearch {
    element: RefCell<Option<Vec<ComputedElement>>>,
    query: RefCell<String>,
    matches: RefCell<Option<MatchResults>>,
    selected_row: RefCell<usize>,
    top_row: RefCell<usize>,
    max_rows_on_screen: RefCell<usize>,
    choices: Vec<TabChoice>,
}

impl TabSearch {
    pub fn new(term_window: &mut TermWindow) -> Self {
        Self {
            element: RefCell::new(None),
            query: RefCell::new(String::new()),
            matches: RefCell::new(None),
            selected_row: RefCell::new(0),
            top_row: RefCell::new(0),
            max_rows_on_screen: RefCell::new(0),
            choices: Self::collect_choices(term_window),
        }
    }

    fn collect_choices(term_window: &TermWindow) -> Vec<TabChoice> {
        let mux = Mux::get();
        let window = match mux.get_window(term_window.mux_window_id) {
            Some(w) => w,
            None => return vec![],
        };
        // The bars hold the titles the user's format-tab-title produced; a
        // child of an inactive root has none, so fall back to pane data.
        let bar_title = |tab_idx: usize| -> Option<String> {
            term_window
                .tab_bar
                .items()
                .iter()
                .chain(term_window.sub_tab_bar.items().iter())
                .find(|e| matches!(e.item, TabBarItem::Tab { tab_idx: i, .. } if i == tab_idx))
                .map(|e| e.title.as_str().trim().to_string())
                .filter(|s| !s.is_empty())
        };
        window
            .iter_tabs()
            .enumerate()
            .map(|(tab_idx, tab)| {
                let pane = tab.get_active_pane();
                let pane_title = pane.as_ref().map(|p| p.get_title()).unwrap_or_default();
                let cwd = pane
                    .as_ref()
                    .and_then(|p| p.get_current_working_dir(CachePolicy::AllowStale))
                    .map(|u| u.path().to_string())
                    .unwrap_or_default();
                let proc = pane
                    .as_ref()
                    .and_then(|p| p.get_foreground_process_name(CachePolicy::AllowStale))
                    .map(|p| basename(&p).to_string())
                    .unwrap_or_default();
                let explicit = tab.get_title();
                let label = if !explicit.is_empty() {
                    explicit
                } else {
                    bar_title(tab_idx).unwrap_or(pane_title)
                };
                TabChoice {
                    tab_idx,
                    is_child: tab.get_parent().is_some(),
                    label,
                    detail: format!("{cwd} {proc}").trim().to_string(),
                }
            })
            .collect()
    }

    fn compute(
        term_window: &mut TermWindow,
        query: &str,
        choices: &[TabChoice],
        matches: &MatchResults,
        max_rows_on_screen: usize,
        selected_row: usize,
        top_row: usize,
    ) -> anyhow::Result<Vec<ComputedElement>> {
        let font = term_window
            .fonts
            .command_palette_font()
            .expect("to resolve command palette font");
        let metrics = RenderMetrics::with_font_metrics(&font.metrics())
            .scale_line_height(term_window.config.command_palette_line_height);

        let (top_bar_height, _) = term_window.horizontal_bar_heights();
        let (padding_left, _padding_top) = term_window.padding_left_top();
        let border = term_window.get_os_border();
        let top_pixel_y = top_bar_height + border.top.get() as f32;

        let bg: InheritableColor = term_window
            .config
            .command_palette_bg_color
            .to_linear()
            .into();
        let fg: InheritableColor = term_window
            .config
            .command_palette_fg_color
            .to_linear()
            .into();
        let dim: InheritableColor = term_window
            .config
            .command_palette_fg_color
            .to_linear()
            .mul_alpha(0.6)
            .into();

        let mut elements = vec![
            Element::new(&font, ElementContent::Text(format!("⌕ {query}_")))
                .colors(ElementColors {
                    border: BorderColor::default(),
                    bg: LinearRgba::TRANSPARENT.into(),
                    text: fg.clone(),
                })
                .display(DisplayType::Block),
        ];

        for (display_idx, choice) in matches
            .matches
            .iter()
            .map(|&idx| &choices[idx])
            .enumerate()
            .skip(top_row)
            .take(max_rows_on_screen)
        {
            let selected = display_idx == selected_row;
            let (row_bg, row_fg, row_dim) = if selected {
                (fg.clone(), bg.clone(), bg.clone())
            } else {
                (LinearRgba::TRANSPARENT.into(), fg.clone(), dim.clone())
            };
            let indent = if choice.is_child { "  " } else { "" };
            let row = vec![
                Element::new(
                    &font,
                    ElementContent::Text(format!(
                        "{indent}{:>2} {}",
                        choice.tab_idx + 1,
                        choice.label
                    )),
                ),
                Element::new(&font, ElementContent::Text(format!("  {}", choice.detail))).colors(
                    ElementColors {
                        border: BorderColor::default(),
                        bg: LinearRgba::TRANSPARENT.into(),
                        text: row_dim,
                    },
                ),
            ];
            elements.push(
                Element::new(&font, ElementContent::Children(row))
                    .colors(ElementColors {
                        border: BorderColor::default(),
                        bg: row_bg,
                        text: row_fg,
                    })
                    .padding(BoxDimension {
                        left: Dimension::Cells(0.25),
                        right: Dimension::Cells(0.25),
                        top: Dimension::Cells(0.),
                        bottom: Dimension::Cells(0.),
                    })
                    .min_width(Some(Dimension::Percent(1.)))
                    .display(DisplayType::Block),
            );
        }

        let dimensions = term_window.dimensions;
        let size = term_window.terminal_size;
        let cell_width = term_window.render_metrics.cell_size.width as f32;
        // Hang off the right edge under the search box, never wider than the window.
        let desired_width = (size.cols / 2).clamp(40, size.cols.max(1));
        let desired_pixel_width = desired_width as f32 * cell_width;
        let right_edge = dimensions.pixel_width as f32 - border.right.get() as f32;
        let x = (right_edge - desired_pixel_width - padding_left).max(0.);

        let element = Element::new(&font, ElementContent::Children(elements))
            .colors(ElementColors {
                border: BorderColor::new(
                    term_window
                        .config
                        .command_palette_fg_color
                        .to_linear()
                        .mul_alpha(0.4),
                ),
                bg: bg.clone(),
                text: fg.clone(),
            })
            .padding(BoxDimension {
                left: Dimension::Cells(0.25),
                right: Dimension::Cells(0.25),
                top: Dimension::Cells(0.25),
                bottom: Dimension::Cells(0.25),
            })
            .border(BoxDimension::new(Dimension::Pixels(1.)))
            .border_corners(Some(Corners {
                top_left: SizedPoly {
                    width: Dimension::Cells(0.25),
                    height: Dimension::Cells(0.25),
                    poly: TOP_LEFT_ROUNDED_CORNER,
                },
                top_right: SizedPoly {
                    width: Dimension::Cells(0.25),
                    height: Dimension::Cells(0.25),
                    poly: TOP_RIGHT_ROUNDED_CORNER,
                },
                bottom_left: SizedPoly {
                    width: Dimension::Cells(0.25),
                    height: Dimension::Cells(0.25),
                    poly: BOTTOM_LEFT_ROUNDED_CORNER,
                },
                bottom_right: SizedPoly {
                    width: Dimension::Cells(0.25),
                    height: Dimension::Cells(0.25),
                    poly: BOTTOM_RIGHT_ROUNDED_CORNER,
                },
            }))
            .min_width(Some(Dimension::Pixels(desired_pixel_width)))
            .max_width(Some(Dimension::Pixels(desired_pixel_width)));

        let computed = term_window.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: dimensions.dpi as f32,
                    pixel_max: dimensions.pixel_height as f32,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: dimensions.dpi as f32,
                    pixel_max: dimensions.pixel_width as f32,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds: euclid::rect(
                    x,
                    top_pixel_y,
                    desired_pixel_width,
                    size.rows as f32 * term_window.render_metrics.cell_size.height as f32,
                ),
                metrics: &metrics,
                gl_state: term_window.render_state.as_ref().unwrap(),
                zindex: 100,
            },
            &element,
        )?;

        Ok(vec![computed])
    }

    fn updated_input(&self) {
        *self.selected_row.borrow_mut() = 0;
        *self.top_row.borrow_mut() = 0;
    }

    fn move_up(&self) {
        let mut row = self.selected_row.borrow_mut();
        *row = row.saturating_sub(1);
        let mut top_row = self.top_row.borrow_mut();
        if *row < *top_row {
            *top_row = *row;
        }
    }

    fn move_down(&self) {
        let max_rows_on_screen = (*self.max_rows_on_screen.borrow()).max(1);
        let limit = self
            .matches
            .borrow()
            .as_ref()
            .map(|m| m.matches.len())
            .unwrap_or_else(|| self.choices.len())
            .saturating_sub(1);
        let mut row = self.selected_row.borrow_mut();
        *row = row.saturating_add(1).min(limit);
        let mut top_row = self.top_row.borrow_mut();
        if *row > *top_row + max_rows_on_screen - 1 {
            *top_row = row.saturating_sub(max_rows_on_screen - 1);
        }
    }

    /// The tab the highlighted row stands for, if any.
    fn selected_tab_idx(&self) -> Option<usize> {
        let selected = *self.selected_row.borrow();
        let matches = self.matches.borrow();
        let idx = *matches.as_ref()?.matches.get(selected)?;
        Some(self.choices[idx].tab_idx)
    }
}

impl Modal for TabSearch {
    fn perform_assignment(
        &self,
        _assignment: &KeyAssignment,
        _term_window: &mut TermWindow,
    ) -> bool {
        false
    }

    fn mouse_event(&self, _event: MouseEvent, _term_window: &mut TermWindow) -> anyhow::Result<()> {
        Ok(())
    }

    fn key_down(
        &self,
        key: KeyCode,
        mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        match (key, mods) {
            (KeyCode::Escape, KeyModifiers::NONE) | (KeyCode::Char('g'), KeyModifiers::CTRL) => {
                term_window.cancel_modal();
            }
            (KeyCode::UpArrow, KeyModifiers::NONE) | (KeyCode::Char('p'), KeyModifiers::CTRL) => {
                self.move_up();
            }
            (KeyCode::DownArrow, KeyModifiers::NONE) | (KeyCode::Char('n'), KeyModifiers::CTRL) => {
                self.move_down();
            }
            (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT) => {
                self.query.borrow_mut().push(c);
                self.updated_input();
            }
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                self.query.borrow_mut().pop();
                self.updated_input();
            }
            (KeyCode::Char('u'), KeyModifiers::CTRL) => {
                self.query.borrow_mut().clear();
                self.updated_input();
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let target = self.selected_tab_idx();
                term_window.cancel_modal();
                if let Some(tab_idx) = target {
                    if let Err(err) = term_window.activate_tab(tab_idx as isize) {
                        log::error!("tab search: activate tab {tab_idx}: {err:#}");
                    }
                }
                return Ok(true);
            }
            _ => return Ok(false),
        }
        term_window.invalidate_modal();
        Ok(true)
    }

    fn computed_element(
        &self,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<Ref<'_, [ComputedElement]>> {
        let query = self.query.borrow();
        let query = query.as_str();
        let mut results = self.matches.borrow_mut();

        let font = term_window
            .fonts
            .command_palette_font()
            .expect("to resolve command palette font");
        let metrics = RenderMetrics::with_font_metrics(&font.metrics())
            .scale_line_height(term_window.config.command_palette_line_height);
        let max_rows_on_screen = ((term_window.dimensions.pixel_height * 8 / 10)
            / metrics.cell_size.height as usize)
            .saturating_sub(2)
            .max(1);
        *self.max_rows_on_screen.borrow_mut() = max_rows_on_screen;

        let stale = results.as_ref().map(|m| m.query != query).unwrap_or(true);
        if stale {
            results.replace(MatchResults {
                query: query.to_string(),
                matches: compute_matches(query, &self.choices),
            });
        }
        let matches = results.as_ref().unwrap();

        if self.element.borrow().is_none() {
            let element = Self::compute(
                term_window,
                query,
                &self.choices,
                matches,
                max_rows_on_screen,
                *self.selected_row.borrow(),
                *self.top_row.borrow(),
            )?;
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

#[cfg(test)]
mod test {
    use super::*;

    fn choice(tab_idx: usize, label: &str, detail: &str) -> TabChoice {
        TabChoice {
            tab_idx,
            is_child: false,
            label: label.into(),
            detail: detail.into(),
        }
    }

    fn choices() -> Vec<TabChoice> {
        vec![
            choice(0, "tab-perf", "/Users/g/git/gilescope/wezterm zsh"),
            choice(1, "main", "/Users/g/git/midnightntwrk/midnight-node tig"),
            choice(
                2,
                "fix-sync-stall",
                "/Users/g/git/midnightntwrk/midnight-node claude",
            ),
        ]
    }

    #[test]
    fn empty_query_keeps_bar_order() {
        assert_eq!(compute_matches("", &choices()), vec![0, 1, 2]);
    }

    #[test]
    fn matches_title_directory_and_program() {
        // Fuzzy: "tig" also hits "gi(t)/g(i)lescope", so check the ranking,
        // not the whole list.
        let c = choices();
        assert_eq!(compute_matches("perf", &c), vec![0]);
        assert_eq!(compute_matches("tig", &c)[0], 1);
        assert_eq!(compute_matches("claude", &c), vec![2]);
        let node = compute_matches("midnight-node", &c);
        assert_eq!(&node[..2], &[1, 2]);
    }

    #[test]
    fn number_finds_the_bar_index() {
        assert_eq!(compute_matches("3", &choices())[0], 2);
    }

    #[test]
    fn fuzzy_query_ranks_the_closer_title_first() {
        assert_eq!(compute_matches("fss", &choices())[0], 2);
    }

    #[test]
    fn basename_handles_trailing_slash_and_root() {
        assert_eq!(basename("/usr/bin/tig"), "tig");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("tig"), "tig");
    }
}
