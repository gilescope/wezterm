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
use crate::termwindow::{DimensionContext, TermWindow, TermWindowNotif, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::keyassignment::{ClipboardPasteSource, KeyAssignment};
use config::Dimension;
use mux::pane::CachePolicy;
use mux::Mux;
use std::cell::{Ref, RefCell};
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};
use window::color::LinearRgba;
use window::{Clipboard, WindowOps};

/// A pasted block as a one-line query: whitespace runs become single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One tab as the search sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct TabChoice {
    /// Index in the mux window; what `activate_tab` wants.
    pub tab_idx: usize,
    /// Nested under a root tab.
    pub is_child: bool,
    /// What the tab bar shows for it, falling back to the pane title.
    pub label: String,
    /// Directory and program, shown dimmer after the label (a page's URL on a page row).
    pub detail: String,
    /// `Some(i)`: this row is the tab's i-th search item (a page in the browser
    /// window the tab stands for), not the tab itself. `tab_idx` is still the tab.
    pub item: Option<usize>,
}

impl TabChoice {
    /// Everything the matcher may hit: label, detail and the 1-based index
    /// the bar numbers tabs with.
    fn haystack(&self) -> String {
        // A page has no bar number of its own: its tab's would make "2" hit every page of tab 2.
        match self.item {
            Some(_) => format!("{} {}", self.label, self.detail),
            None => format!("{} {} {}", self.tab_idx + 1, self.label, self.detail),
        }
    }
}

/// Indices into `choices` ordered best match first; empty query keeps bar order.
pub fn compute_matches(query: &str, choices: &[TabChoice]) -> Vec<usize> {
    if query.is_empty() {
        // Pages are found by searching, not listed: 47 tabs of them would bury the bar.
        return (0..choices.len())
            .filter(|&i| choices[i].item.is_none())
            .collect();
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
            .flat_map(|(tab_idx, tab)| {
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
                let pages = tab
                    .get_search_items()
                    .into_iter()
                    .enumerate()
                    .map(move |(i, page)| TabChoice {
                        tab_idx,
                        is_child: true,
                        label: if page.title.is_empty() {
                            page.url.clone()
                        } else {
                            page.title
                        },
                        detail: page.url,
                        item: Some(i),
                    });
                std::iter::once(TabChoice {
                    tab_idx,
                    is_child: tab.get_parent().is_some(),
                    label,
                    detail: format!("{cwd} {proc}").trim().to_string(),
                    item: None,
                })
                .chain(pages)
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
            let head = match choice.item {
                Some(_) => format!("{indent}   ↳ {}", choice.label),
                None => format!("{indent}{:>2} {}", choice.tab_idx + 1, choice.label),
            };
            let row = vec![
                Element::new(&font, ElementContent::Text(head)),
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
                    .display(DisplayType::Block)
                    .item_type(UIItemType::TabSearchRow(display_idx)),
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
    fn selected_choice(&self) -> Option<&TabChoice> {
        let selected = *self.selected_row.borrow();
        let matches = self.matches.borrow();
        let idx = *matches.as_ref()?.matches.get(selected)?;
        self.choices.get(idx)
    }

    /// Close the dropdown and switch to the highlighted tab.
    fn activate_selected(&self, term_window: &mut TermWindow) {
        let target = self.selected_choice().map(|c| (c.tab_idx, c.item));
        term_window.cancel_modal();
        if let Some((tab_idx, item)) = target {
            if let Err(err) = term_window.activate_tab(tab_idx as isize) {
                log::error!("tab search: activate tab {tab_idx}: {err:#}");
                return;
            }
            if let Some(item) = item {
                term_window.pick_search_item(item);
            }
        }
    }

    /// Mouse over a row highlights it; a click takes it.
    pub fn mouse_row(&self, display_idx: usize, click: bool, term_window: &mut TermWindow) {
        if *self.selected_row.borrow() != display_idx {
            *self.selected_row.borrow_mut() = display_idx;
            term_window.invalidate_modal();
        }
        if click {
            self.activate_selected(term_window);
        }
    }
}

impl Modal for TabSearch {
    /// Paste goes into the query, not through to the terminal underneath.
    fn perform_assignment(&self, assignment: &KeyAssignment, term_window: &mut TermWindow) -> bool {
        let KeyAssignment::PasteFrom(source) = assignment else {
            return false;
        };
        let clipboard = match source {
            ClipboardPasteSource::Clipboard => Clipboard::Clipboard,
            ClipboardPasteSource::PrimarySelection => Clipboard::PrimarySelection,
        };
        let Some(window) = term_window.window.clone() else {
            return true;
        };
        let future = window.get_clipboard(clipboard);
        promise::spawn::spawn(async move {
            let Ok(text) = future.await else { return };
            window.notify(TermWindowNotif::Apply(Box::new(move |tw| {
                let modal = tw.get_modal();
                if let Some(search) = modal.as_ref().and_then(|m| m.downcast_ref::<TabSearch>()) {
                    search.query.borrow_mut().push_str(&one_line(&text));
                    search.updated_input();
                    tw.invalidate_modal();
                }
            })));
        })
        .detach();
        true
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
                self.activate_selected(term_window);
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

    /// The query is one line: a pasted block collapses to its words.
    #[test]
    fn pasted_text_becomes_one_line() {
        assert_eq!(one_line("foo\nbar\r\n  baz "), "foo bar baz");
        assert_eq!(one_line("\n\n"), "");
        assert_eq!(one_line("tab\tsep"), "tab sep");
    }

    fn choice(tab_idx: usize, label: &str, detail: &str) -> TabChoice {
        TabChoice {
            tab_idx,
            is_child: false,
            label: label.into(),
            detail: detail.into(),
            item: None,
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

    fn item_choice(tab_idx: usize, item: usize, title: &str, url: &str) -> TabChoice {
        TabChoice {
            tab_idx,
            is_child: true,
            label: title.into(),
            detail: url.into(),
            item: Some(item),
        }
    }

    /// A Browser tab (index 1) carrying two pages, after its root and before the next tab.
    fn with_pages() -> Vec<TabChoice> {
        vec![
            choice(0, "main", "/Users/g/git/wezterm zsh"),
            choice(1, "Browser", ""),
            item_choice(
                1,
                0,
                "Pull request 2521",
                "https://github.com/midnightntwrk/midnight-node/pull/2521",
            ),
            item_choice(1, 1, "Example Domain", "https://example.com/"),
            choice(2, "other", "/tmp zsh"),
        ]
    }

    #[test]
    fn page_rows_stay_out_of_the_unfiltered_list() {
        assert_eq!(compute_matches("", &with_pages()), vec![0, 1, 4]);
    }

    #[test]
    fn a_page_title_or_url_finds_its_row() {
        let c = with_pages();
        assert_eq!(compute_matches("example domain", &c)[0], 3);
        assert_eq!(compute_matches("pull/2521", &c)[0], 2);
    }

    #[test]
    fn a_tab_number_does_not_hit_page_rows() {
        // "2" is the bar number of tab 1; page rows have no number of their own.
        assert!(!compute_matches("2", &with_pages()).contains(&3));
    }

    #[test]
    fn basename_handles_trailing_slash_and_root() {
        assert_eq!(basename("/usr/bin/tig"), "tig");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("tig"), "tig");
    }
}
