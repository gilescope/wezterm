use crate::termwindow::{PaneInformation, TabInformation, UIItem, UIItemType};
use config::{ConfigHandle, TabBarColors, TabBarPosition};
use finl_unicode::grapheme_clusters::Graphemes;
use mlua::FromLua;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use termwiz::cell::{unicode_column_width, Cell, CellAttributes};
use termwiz::color::{AnsiColor, ColorSpec};
use termwiz::escape::csi::Sgr;
use termwiz::escape::parser::Parser;
use termwiz::escape::{Action, ControlCode, CSI};
use termwiz::surface::SEQ_ZERO;
use termwiz_funcs::{format_as_escapes, FormatColor, FormatItem};
use wezterm_term::{BlockedKind, Line, ProgramState, ProgramStatus, Progress};
use window::{IntegratedTitleButton, IntegratedTitleButtonAlignment, IntegratedTitleButtonStyle};

#[derive(Clone, Debug, PartialEq)]
pub struct TabBarState {
    line: Line,
    items: Vec<TabEntry>,
    /// When a tab shows the built-in indeterminate progress spinner, the instant
    /// at which the tab bar should be rebuilt to advance to the next frame;
    /// None when no spinner is visible.
    next_progress_frame_due: Option<Instant>,
    /// First-pass (unhovered, full-width) title per tab id, so an animation
    /// frame can rebuild without re-asking Lua for tabs that did not change.
    titles: Vec<(usize, TitleText)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabBarItem {
    None,
    LeftStatus,
    RightStatus,
    Tab { tab_idx: usize, active: bool },
    NewTabButton,
    WindowButton(IntegratedTitleButton),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabEntry {
    pub item: TabBarItem,
    pub title: Line,
    x: usize,
    width: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct TitleText {
    items: Vec<FormatItem>,
    len: usize,
    /// True when the built-in title path rendered an indeterminate spinner
    /// frame. A custom format-tab-title callback always leaves this false.
    has_indeterminate: bool,
}

/// One `format-tab-title` request: which tab, whether it is hovered, and
/// the width budget handed to the callback.
#[derive(Clone, Copy, Debug)]
struct TitleRequest {
    tab_idx: usize,
    hover: bool,
    max_width: usize,
}

/// Run `format-tab-title` for every request in one Lua round-trip. The
/// `tabs`, `panes` and `config` tables are built once per call rather than
/// once per tab: with N tabs the old per-tab path serialised the whole
/// Config 2N+1 times and N tab/pane objects 2N times, which is the CPU
/// sink with ~40 tabs. Returns `None` for a request when no callback is
/// registered or it returned nil.
fn call_format_tab_titles(
    tab_info: &[TabInformation],
    pane_info: &[PaneInformation],
    config: &ConfigHandle,
    requests: &[TitleRequest],
) -> Vec<Option<TitleText>> {
    use mlua::IntoLua;

    if requests.is_empty() {
        return vec![];
    }
    match config::run_immediate_with_lua_config(|lua| {
        let Some(lua) = lua else {
            return Ok(vec![None; requests.len()]);
        };
        let tabs = lua.create_sequence_from(tab_info.iter().cloned())?;
        let panes = lua.create_sequence_from(pane_info.iter().cloned())?;
        let config = (**config).clone().into_lua(&*lua)?;

        requests
            .iter()
            .map(|req| {
                let tab: mlua::Value = tabs.raw_get(req.tab_idx + 1)?;
                let v = config::lua::emit_sync_callback(
                    &*lua,
                    (
                        "format-tab-title".to_string(),
                        (
                            tab,
                            tabs.clone(),
                            panes.clone(),
                            config.clone(),
                            req.hover,
                            req.max_width,
                        ),
                    ),
                )?;
                title_text_from_lua(&*lua, v)
            })
            .collect::<anyhow::Result<Vec<_>>>()
    }) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("format-tab-title: {}", err);
            vec![None; requests.len()]
        }
    }
}

fn title_text_from_lua(lua: &mlua::Lua, v: mlua::Value) -> anyhow::Result<Option<TitleText>> {
    match &v {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Table(_) => {
            let items = <Vec<FormatItem>>::from_lua(v, lua)?;

            let esc = format_as_escapes(items.clone())?;
            let line = parse_status_text(&esc, CellAttributes::default());

            Ok(Some(TitleText {
                items,
                len: line.len(),
                has_indeterminate: false,
            }))
        }
        _ => {
            let s = String::from_lua(v, lua)?;
            let line = parse_status_text(&s, CellAttributes::default());
            Ok(Some(TitleText {
                len: line.len(),
                items: vec![FormatItem::Text(s)],
                has_indeterminate: false,
            }))
        }
    }
}

/// The spinner frame for `pane_id` right now; exposed to Lua as
/// `pane.spinner` so a custom title can animate a working pane.
pub fn spinner_glyph_for_pane(pane_id: usize) -> char {
    indeterminate_spinner_glyph(spinner_phase(pane_id))
}

/// Whether the pane's state is animated (a spinner frame is due).
fn pane_is_animated(pane: &PaneInformation) -> bool {
    matches!(pane.progress, Progress::Indeterminate)
        || matches!(
            &pane.program_status,
            Some(ProgramStatus {
                state: ProgramState::Working,
                progress: None,
                ..
            })
        )
}

/// pct is a percentage in the range 0-100.
/// We want to map it to one of the nerdfonts:
///
/// * `md-checkbox_blank_circle_outline` (0xf0130) for an empty circle
/// * `md_circle_slice_1..=7` (0xf0a9e ..= 0xf0aa4) for a partly filled
///   circle
/// * `md_circle_slice_8` (0xf0aa5) for a filled circle
///
/// We use an empty circle for values close to 0%, a filled circle for values
/// close to 100%, and a partly filled circle for the rest (roughly evenly
/// distributed).
fn pct_to_glyph(pct: u8) -> char {
    match pct {
        0..=5 => '\u{f0130}',    // empty circle
        6..=18 => '\u{f0a9e}',   // centered at 12 (slightly smaller than 12.5)
        19..=31 => '\u{f0a9f}',  // centered at 25
        32..=43 => '\u{f0aa0}',  // centered at 37.5
        44..=56 => '\u{f0aa1}',  // half-filled circle, centered at 50
        57..=68 => '\u{f0aa2}',  // centered at 62.5
        69..=81 => '\u{f0aa3}',  // centered at 75
        82..=94 => '\u{f0aa4}',  // centered at 88 (slightly larger than 87.5)
        95..=100 => '\u{f0aa5}', // filled circle
        // Any other value is mapped to a filled circle.
        _ => '\u{f0aa5}',
    }
}

/// How long each indeterminate progress spinner frame is shown before the next
/// is due.
const INDETERMINATE_SPINNER_INTERVAL: Duration = Duration::from_millis(100);

/// Reference instant for the indeterminate spinner. The displayed frame and its
/// next-due time are both derived from the elapsed time since this instant, so
/// they stay in step no matter when a repaint happens to rebuild the tab bar.
static SPINNER_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Renders `value` as a braille cell whose lit dots count up in the cell's
/// reading order, down the left column then down the right column. Stepping
/// `value` through 0..=255 reproduces the `dots8Bit` animation of
/// https://github.com/sindresorhus/cli-spinners without a lookup table.
fn braille_counter(value: u8) -> char {
    // Unicode braille dot bit values in reading order: dots 1, 2, 3, 7 fill the
    // left column and dots 4, 5, 6, 8 the right column.
    const DOTS: [u32; 8] = [0x01, 0x02, 0x04, 0x40, 0x08, 0x10, 0x20, 0x80];
    let mut pattern = 0u32;
    for (bit, dot) in DOTS.iter().enumerate() {
        if value & (1 << bit) != 0 {
            pattern |= dot;
        }
    }
    char::from_u32(0x2800 + pattern).expect("braille pattern is a valid codepoint")
}

/// Returns the spinner glyph to show for the current moment, advancing one
/// frame per INDETERMINATE_SPINNER_INTERVAL. `seed` offsets the starting frame
/// so that tabs busy at the same time do not animate in lock step.
fn indeterminate_spinner_glyph(seed: u64) -> char {
    let elapsed = SPINNER_EPOCH.elapsed().as_millis() as u64;
    let interval = INDETERMINATE_SPINNER_INTERVAL.as_millis() as u64;
    // braille_counter wraps at 256, matching the animation's frame count.
    braille_counter((elapsed / interval + seed) as u8)
}

/// Returns the instant at which the spinner next advances a frame, snapped to
/// the frame grid measured from SPINNER_EPOCH. Because the result falls on a
/// grid boundary rather than a fixed offset from now, repeated rebuilds within
/// one frame all return the same instant and the animation advances steadily
/// even when unrelated repaints rebuild the tab bar in between.
fn next_spinner_frame_due() -> Instant {
    let interval = INDETERMINATE_SPINNER_INTERVAL.as_nanos();
    let elapsed = SPINNER_EPOCH.elapsed().as_nanos();
    let next_frame = elapsed / interval + 1;
    *SPINNER_EPOCH + Duration::from_nanos((next_frame * interval) as u64)
}

/// Scrambles a tab id into a spinner phase offset. Tab ids are usually handed
/// out sequentially, which would leave adjacent tabs only one frame apart; the
/// splitmix64 finalizer avalanches the low bits so their spinners spread across
/// the animation instead.
fn spinner_phase(tab_id: usize) -> u64 {
    let mut z = tab_id as u64;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Titles for `requests`, in order: the Lua callback's answer where it
/// gave one, else the built-in rendering.
fn compute_tab_titles(
    tab_info: &[TabInformation],
    pane_info: &[PaneInformation],
    config: &ConfigHandle,
    requests: &[TitleRequest],
) -> Vec<TitleText> {
    call_format_tab_titles(tab_info, pane_info, config, requests)
        .into_iter()
        .zip(requests)
        .map(|(title, req)| {
            let tab = &tab_info[req.tab_idx];
            match title {
                Some(mut title) => {
                    // The callback draws its own frames (via `pane.spinner`);
                    // it still needs the bar rebuilt on the spinner grid.
                    title.has_indeterminate =
                        tab.active_pane.as_ref().is_some_and(pane_is_animated);
                    title
                }
                None => builtin_tab_title(tab, config),
            }
        })
        .collect()
}

fn builtin_tab_title(tab: &TabInformation, config: &ConfigHandle) -> TitleText {
    let mut items = vec![];
    let mut len = 0;
    let mut has_indeterminate = false;

    if let Some(pane) = &tab.active_pane {
        let mut title = if tab.tab_title.is_empty() {
            pane.title.clone()
        } else {
            tab.tab_title.clone()
        };

        let classic_spacing = if effective_use_fancy_tab_bar(config) {
            ""
        } else {
            " "
        };
        if config.show_tab_index_in_tab_bar {
            let index = format!(
                "{classic_spacing}{}: ",
                tab.tab_index
                    + if config.tab_and_split_indices_are_zero_based {
                        0
                    } else {
                        1
                    }
            );
            len += unicode_column_width(&index, None);
            items.push(FormatItem::Text(index));

            title = format!("{}{classic_spacing}", title);
        }

        // Program status (OSC 7501) outranks the progress bar: a blocked
        // agent is the thing to look at, a working one gets the spinner.
        let status_glyph = match &pane.program_status {
            Some(ProgramStatus {
                state: ProgramState::Blocked,
                kind,
                ..
            }) => Some((
                if *kind == Some(BlockedKind::Question) {
                    "? "
                } else {
                    "\u{23f8}\u{fe0e} "
                },
                AnsiColor::Yellow,
            )),
            Some(ProgramStatus {
                state: ProgramState::Error,
                ..
            }) => Some(("\u{2717} ", AnsiColor::Red)),
            Some(ProgramStatus {
                state: ProgramState::Done,
                ..
            }) => Some(("\u{25c8} ", AnsiColor::Green)),
            Some(ProgramStatus {
                state: ProgramState::Working,
                progress: Some(pct),
                ..
            }) => {
                let graphic = format!("{} ", pct_to_glyph(*pct));
                len += unicode_column_width(&graphic, None);
                items.push(FormatItem::Foreground(FormatColor::AnsiColor(
                    AnsiColor::Green,
                )));
                items.push(FormatItem::Text(graphic));
                items.push(FormatItem::Foreground(FormatColor::Default));
                None
            }
            Some(ProgramStatus {
                state: ProgramState::Working,
                ..
            }) => {
                has_indeterminate = true;
                let graphic = format!(
                    "{} ",
                    indeterminate_spinner_glyph(spinner_phase(tab.tab_id))
                );
                len += unicode_column_width(&graphic, None);
                items.push(FormatItem::Foreground(FormatColor::AnsiColor(
                    AnsiColor::Green,
                )));
                items.push(FormatItem::Text(graphic));
                items.push(FormatItem::Foreground(FormatColor::Default));
                None
            }
            Some(ProgramStatus {
                state: ProgramState::Idle | ProgramState::Clear,
                ..
            })
            | None => {
                if pane.has_unseen_command_result && pane.program_status.is_none() {
                    Some(("\u{25b8} ", AnsiColor::Green))
                } else {
                    None
                }
            }
        };
        if let Some((glyph, color)) = status_glyph {
            len += unicode_column_width(glyph, None);
            items.push(FormatItem::Foreground(FormatColor::AnsiColor(color)));
            items.push(FormatItem::Text(glyph.to_string()));
            items.push(FormatItem::Foreground(FormatColor::Default));
        }

        match pane.progress {
            Progress::None => {}
            Progress::Percentage(pct) | Progress::Error(pct) => {
                let graphic = format!("{} ", pct_to_glyph(pct));
                len += unicode_column_width(&graphic, None);
                let color = if matches!(pane.progress, Progress::Percentage(_)) {
                    FormatItem::Foreground(FormatColor::AnsiColor(AnsiColor::Green))
                } else {
                    FormatItem::Foreground(FormatColor::AnsiColor(AnsiColor::Red))
                };
                items.push(color);
                items.push(FormatItem::Text(graphic));
                items.push(FormatItem::Foreground(FormatColor::Default));
            }
            Progress::Indeterminate => {
                has_indeterminate = true;
                let graphic = format!(
                    "{} ",
                    indeterminate_spinner_glyph(spinner_phase(tab.tab_id))
                );
                len += unicode_column_width(&graphic, None);
                items.push(FormatItem::Foreground(FormatColor::AnsiColor(
                    AnsiColor::Green,
                )));
                items.push(FormatItem::Text(graphic));
                items.push(FormatItem::Foreground(FormatColor::Default));
            }
        }

        // We have a preferred soft minimum on tab width to make it
        // easier to click on tab titles, but we'll still go below
        // this if there are too many tabs to fit the window at
        // this width.
        if !effective_use_fancy_tab_bar(config) {
            while len + unicode_column_width(&title, None) < 5 {
                title.push(' ');
            }
        }

        len += unicode_column_width(&title, None);
        items.push(FormatItem::Text(title));
    } else {
        let title = " no pane ".to_string();
        len += unicode_column_width(&title, None);
        items.push(FormatItem::Text(title));
    };

    TitleText {
        len,
        items,
        has_indeterminate,
    }
}

fn is_tab_hover(mouse_x: Option<usize>, x: usize, tab_title_len: usize) -> bool {
    return mouse_x
        .map(|mouse_x| mouse_x >= x && mouse_x < x + tab_title_len)
        .unwrap_or(false);
}

fn effective_use_fancy_tab_bar(config: &ConfigHandle) -> bool {
    config.use_fancy_tab_bar || config.resolved_tab_bar_position().is_vertical()
}

impl TabBarState {
    pub fn default() -> Self {
        Self {
            line: Line::with_width(1, SEQ_ZERO),
            items: vec![TabEntry {
                item: TabBarItem::None,
                title: Line::from_text(" ", &CellAttributes::blank(), 1, None),
                x: 1,
                width: 1,
            }],
            next_progress_frame_due: None,
            titles: vec![],
        }
    }

    pub fn line(&self) -> &Line {
        &self.line
    }

    pub fn items(&self) -> &[TabEntry] {
        &self.items
    }

    pub fn next_progress_frame_due(&self) -> Option<Instant> {
        self.next_progress_frame_due
    }

    fn integrated_title_buttons(
        mouse_x: Option<usize>,
        x: &mut usize,
        config: &ConfigHandle,
        items: &mut Vec<TabEntry>,
        line: &mut Line,
        colors: &TabBarColors,
    ) {
        let default_cell = if effective_use_fancy_tab_bar(config) {
            CellAttributes::default()
        } else {
            colors.new_tab().as_cell_attributes()
        };

        let default_cell_hover = if effective_use_fancy_tab_bar(config) {
            CellAttributes::default()
        } else {
            colors.new_tab_hover().as_cell_attributes()
        };

        let window_hide =
            parse_status_text(&config.tab_bar_style.window_hide, default_cell.clone());
        let window_hide_hover = parse_status_text(
            &config.tab_bar_style.window_hide_hover,
            default_cell_hover.clone(),
        );

        let window_maximize =
            parse_status_text(&config.tab_bar_style.window_maximize, default_cell.clone());
        let window_maximize_hover = parse_status_text(
            &config.tab_bar_style.window_maximize_hover,
            default_cell_hover.clone(),
        );

        let window_close =
            parse_status_text(&config.tab_bar_style.window_close, default_cell.clone());
        let window_close_hover = parse_status_text(
            &config.tab_bar_style.window_close_hover,
            default_cell_hover.clone(),
        );

        for button in &config.integrated_title_buttons {
            use IntegratedTitleButton as Button;
            let title = match button {
                Button::Hide => {
                    let hover = is_tab_hover(mouse_x, *x, window_hide_hover.len());

                    if hover {
                        &window_hide_hover
                    } else {
                        &window_hide
                    }
                }
                Button::Maximize => {
                    let hover = is_tab_hover(mouse_x, *x, window_maximize_hover.len());

                    if hover {
                        &window_maximize_hover
                    } else {
                        &window_maximize
                    }
                }
                Button::Close => {
                    let hover = is_tab_hover(mouse_x, *x, window_close_hover.len());

                    if hover {
                        &window_close_hover
                    } else {
                        &window_close
                    }
                }
            };

            line.append_line(title.to_owned(), SEQ_ZERO);

            let width = title.len();
            items.push(TabEntry {
                item: TabBarItem::WindowButton(*button),
                title: title.to_owned(),
                x: *x,
                width,
            });

            *x += width;
        }
    }

    /// Build a new tab bar from the current state
    /// mouse_x is some if the mouse is on the same row as the tab bar.
    /// title_width is the total number of cell columns in the window.
    /// window allows access to the tabs associated with the window.
    pub fn new(
        title_width: usize,
        mouse_x: Option<usize>,
        tab_info: &[TabInformation],
        pane_info: &[PaneInformation],
        colors: Option<&TabBarColors>,
        config: &ConfigHandle,
        left_status: &str,
        right_status: &str,
        reuse: Option<&TabBarState>,
    ) -> Self {
        let colors = colors.cloned().unwrap_or_else(TabBarColors::default);

        let active_cell_attrs = colors.active_tab().as_cell_attributes();
        let inactive_hover_attrs = colors.inactive_tab_hover().as_cell_attributes();
        let inactive_cell_attrs = colors.inactive_tab().as_cell_attributes();
        let new_tab_hover_attrs = colors.new_tab_hover().as_cell_attributes();
        let new_tab_attrs = colors.new_tab().as_cell_attributes();

        let new_tab = parse_status_text(
            &config.tab_bar_style.new_tab,
            if effective_use_fancy_tab_bar(config) {
                CellAttributes::default()
            } else {
                new_tab_attrs.clone()
            },
        );
        let new_tab_hover = parse_status_text(
            &config.tab_bar_style.new_tab_hover,
            if effective_use_fancy_tab_bar(config) {
                CellAttributes::default()
            } else {
                new_tab_hover_attrs.clone()
            },
        );

        let use_integrated_title_buttons = config
            .window_decorations
            .contains(window::WindowDecorations::INTEGRATED_BUTTONS);

        // We ultimately want to produce a line looking like this:
        // ` | tab1-title x | tab2-title x |  +      . - X `
        // Where the `+` sign will spawn a new tab (or show a context
        // menu with tab creation options) and the other three chars
        // are symbols representing minimize, maximize and close.

        let tab_titles: Vec<TitleText> = if config.show_tabs_in_tab_bar {
            // On an animation frame only the animated tabs can have changed;
            // everything else keeps the title from the previous build.
            let mut tab_titles: Vec<Option<TitleText>> = tab_info
                .iter()
                .map(|tab| {
                    let reuse = reuse?;
                    if tab.active_pane.as_ref().is_some_and(pane_is_animated) {
                        return None;
                    }
                    reuse
                        .titles
                        .iter()
                        .find(|(id, _)| *id == tab.tab_id)
                        .map(|(_, title)| title.clone())
                })
                .collect();
            let requests: Vec<TitleRequest> = tab_titles
                .iter()
                .enumerate()
                .filter(|(_, t)| t.is_none())
                .map(|(tab_idx, _)| TitleRequest {
                    tab_idx,
                    hover: false,
                    max_width: config.tab_max_width,
                })
                .collect();
            let computed = compute_tab_titles(tab_info, pane_info, config, &requests);
            for (req, title) in requests.iter().zip(computed) {
                tab_titles[req.tab_idx] = Some(title);
            }
            tab_titles
                .into_iter()
                .map(|t| t.expect("every title filled"))
                .collect()
        } else {
            vec![]
        };
        let titles: Vec<(usize, TitleText)> = tab_info
            .iter()
            .map(|t| t.tab_id)
            .zip(tab_titles.iter().cloned())
            .collect();
        let titles_len: usize = tab_titles.iter().map(|s| s.len).sum();
        let number_of_tabs = tab_titles.len();

        let available_cells =
            title_width.saturating_sub(number_of_tabs.saturating_sub(1) + new_tab.len());
        let tab_width_max = if effective_use_fancy_tab_bar(config) || available_cells >= titles_len
        {
            // We can render each title with its full width
            usize::max_value()
        } else {
            // We need to clamp the length to balance them out
            available_cells / number_of_tabs
        }
        .min(config.tab_max_width);

        let mut line = Line::with_width(0, SEQ_ZERO);

        let mut x = 0;
        let mut items = vec![];
        let mut has_indeterminate_progress = false;

        let black_cell = Cell::blank_with_attrs(
            CellAttributes::default()
                .set_background(ColorSpec::TrueColor(*colors.background()))
                .clone(),
        );

        if use_integrated_title_buttons
            && config.integrated_title_button_style == IntegratedTitleButtonStyle::MacOsNative
            && !effective_use_fancy_tab_bar(config)
            && config.resolved_tab_bar_position() == TabBarPosition::Top
        {
            for _ in 0..10 as usize {
                line.insert_cell(0, black_cell.clone(), title_width, SEQ_ZERO);
                x += 1;
            }
        }

        if use_integrated_title_buttons
            && config.integrated_title_button_style != IntegratedTitleButtonStyle::MacOsNative
            && config.integrated_title_button_alignment == IntegratedTitleButtonAlignment::Left
        {
            Self::integrated_title_buttons(mouse_x, &mut x, config, &mut items, &mut line, &colors);
        }

        let left_status_line = parse_status_text(left_status, black_cell.attrs().clone());
        if left_status_line.len() > 0 {
            items.push(TabEntry {
                item: TabBarItem::LeftStatus,
                title: left_status_line.clone(),
                x,
                width: left_status_line.len(),
            });
            x += left_status_line.len();
            line.append_line(left_status_line, SEQ_ZERO);
        }

        for (tab_idx, tab_title) in tab_titles.iter().enumerate() {
            let tab_title_len = tab_title.len.min(tab_width_max);
            // `tab_info` may be a subset of the window's tabs (a side bar
            // lists roots only), so the item carries the mux index.
            let info = &tab_info[tab_idx];
            let active = info.is_active;
            let hover = !active && is_tab_hover(mouse_x, x, tab_title_len);

            // The first pass already asked for this title unhovered with the
            // full width budget; only hover or a clamped width can change the
            // answer, so only then pay for another callback.
            let recomputed;
            let tab_title = if hover || tab_title.len > tab_title_len {
                let req = TitleRequest {
                    tab_idx,
                    hover,
                    max_width: tab_title_len,
                };
                recomputed = compute_tab_titles(tab_info, pane_info, config, &[req]).remove(0);
                &recomputed
            } else {
                tab_title
            };

            let cell_attrs = if active {
                &active_cell_attrs
            } else if hover {
                &inactive_hover_attrs
            } else {
                &inactive_cell_attrs
            };

            let tab_start_idx = x;

            has_indeterminate_progress |= tab_title.has_indeterminate;

            let esc = format_as_escapes(tab_title.items.clone()).expect("already parsed ok above");
            let mut tab_line = parse_status_text(
                &esc,
                if effective_use_fancy_tab_bar(config) {
                    CellAttributes::default()
                } else {
                    cell_attrs.clone()
                },
            );

            let title = tab_line.clone();
            if tab_line.len() > tab_width_max {
                tab_line.resize(tab_width_max, SEQ_ZERO);
            }

            let width = tab_line.len();

            items.push(TabEntry {
                item: TabBarItem::Tab {
                    tab_idx: info.tab_index,
                    active,
                },
                title,
                x: tab_start_idx,
                width,
            });

            line.append_line(tab_line, SEQ_ZERO);
            x += width;
        }

        // New tab button
        if config.show_new_tab_button_in_tab_bar {
            let hover = is_tab_hover(mouse_x, x, new_tab_hover.len());

            let new_tab_button = if hover { &new_tab_hover } else { &new_tab };

            let button_start = x;
            let width = new_tab_button.len();

            line.append_line(new_tab_button.clone(), SEQ_ZERO);

            items.push(TabEntry {
                item: TabBarItem::NewTabButton,
                title: new_tab_button.clone(),
                x: button_start,
                width,
            });

            x += width;
        }

        // Reserve place for integrated title buttons
        let title_width = if use_integrated_title_buttons
            && config.integrated_title_button_style != IntegratedTitleButtonStyle::MacOsNative
            && config.integrated_title_button_alignment == IntegratedTitleButtonAlignment::Right
        {
            let window_hide =
                parse_status_text(&config.tab_bar_style.window_hide, CellAttributes::default());
            let window_hide_hover = parse_status_text(
                &config.tab_bar_style.window_hide_hover,
                CellAttributes::default(),
            );

            let window_maximize = parse_status_text(
                &config.tab_bar_style.window_maximize,
                CellAttributes::default(),
            );
            let window_maximize_hover = parse_status_text(
                &config.tab_bar_style.window_maximize_hover,
                CellAttributes::default(),
            );
            let window_close = parse_status_text(
                &config.tab_bar_style.window_close,
                CellAttributes::default(),
            );
            let window_close_hover = parse_status_text(
                &config.tab_bar_style.window_close_hover,
                CellAttributes::default(),
            );

            let hide_len = window_hide.len().max(window_hide_hover.len());
            let maximize_len = window_maximize.len().max(window_maximize_hover.len());
            let close_len = window_close.len().max(window_close_hover.len());

            let mut width_to_reserve = 0;
            for button in &config.integrated_title_buttons {
                use IntegratedTitleButton as Button;
                let button_len = match button {
                    Button::Hide => hide_len,
                    Button::Maximize => maximize_len,
                    Button::Close => close_len,
                };
                width_to_reserve += button_len;
            }

            title_width.saturating_sub(width_to_reserve)
        } else {
            title_width
        };

        let status_space_available = title_width.saturating_sub(x);

        let mut right_status_line = parse_status_text(right_status, black_cell.attrs().clone());
        items.push(TabEntry {
            item: TabBarItem::RightStatus,
            title: right_status_line.clone(),
            x,
            width: status_space_available,
        });

        while right_status_line.len() > status_space_available {
            right_status_line.remove_cell(0, SEQ_ZERO);
        }

        line.append_line(right_status_line, SEQ_ZERO);
        while line.len() < title_width {
            line.insert_cell(x, black_cell.clone(), title_width, SEQ_ZERO);
        }

        if use_integrated_title_buttons
            && config.integrated_title_button_style != IntegratedTitleButtonStyle::MacOsNative
            && config.integrated_title_button_alignment == IntegratedTitleButtonAlignment::Right
        {
            x = title_width;
            Self::integrated_title_buttons(mouse_x, &mut x, config, &mut items, &mut line, &colors);
        }

        Self {
            line,
            items,
            next_progress_frame_due: has_indeterminate_progress.then(next_spinner_frame_due),
            titles,
        }
    }

    pub fn compute_ui_items(&self, y: usize, cell_height: usize, cell_width: usize) -> Vec<UIItem> {
        let mut items = vec![];

        for entry in self.items.iter() {
            items.push(UIItem {
                x: entry.x * cell_width,
                width: entry.width * cell_width,
                y,
                height: cell_height,
                item_type: UIItemType::TabBar(entry.item),
            });
        }

        items
    }
}

pub fn parse_status_text(text: &str, default_cell: CellAttributes) -> Line {
    let mut pen = default_cell.clone();
    let mut cells = vec![];
    let mut ignoring = false;
    let mut print_buffer = String::new();

    fn flush_print(buf: &mut String, cells: &mut Vec<Cell>, pen: &CellAttributes) {
        for g in Graphemes::new(buf.as_str()) {
            let cell = Cell::new_grapheme(g, pen.clone(), None);
            let width = cell.width();
            cells.push(cell);
            for _ in 1..width {
                // Line/Screen expect double wide graphemes to be followed by a blank in
                // the next column position, otherwise we'll render incorrectly
                cells.push(Cell::blank_with_attrs(pen.clone()));
            }
        }
        buf.clear();
    }

    let mut parser = Parser::new();
    parser.parse(text.as_bytes(), |action| {
        if ignoring {
            return;
        }
        match action {
            Action::Print(c) => print_buffer.push(c),
            Action::PrintString(s) => print_buffer.push_str(&s),
            Action::Control(c) => {
                flush_print(&mut print_buffer, &mut cells, &pen);
                match c {
                    ControlCode::CarriageReturn | ControlCode::LineFeed => {
                        ignoring = true;
                    }
                    _ => {}
                }
            }
            Action::CSI(csi) => {
                flush_print(&mut print_buffer, &mut cells, &pen);
                match csi {
                    CSI::Sgr(sgr) => match sgr {
                        Sgr::Reset => pen = default_cell.clone(),
                        Sgr::Intensity(i) => {
                            pen.set_intensity(i);
                        }
                        Sgr::Underline(u) => {
                            pen.set_underline(u);
                        }
                        Sgr::Overline(o) => {
                            pen.set_overline(o);
                        }
                        Sgr::VerticalAlign(o) => {
                            pen.set_vertical_align(o);
                        }
                        Sgr::Blink(b) => {
                            pen.set_blink(b);
                        }
                        Sgr::Italic(i) => {
                            pen.set_italic(i);
                        }
                        Sgr::Inverse(inverse) => {
                            pen.set_reverse(inverse);
                        }
                        Sgr::Invisible(invis) => {
                            pen.set_invisible(invis);
                        }
                        Sgr::StrikeThrough(strike) => {
                            pen.set_strikethrough(strike);
                        }
                        Sgr::Foreground(col) => {
                            if let ColorSpec::Default = col {
                                pen.set_foreground(default_cell.foreground());
                            } else {
                                pen.set_foreground(col);
                            }
                        }
                        Sgr::Background(col) => {
                            if let ColorSpec::Default = col {
                                pen.set_background(default_cell.background());
                            } else {
                                pen.set_background(col);
                            }
                        }
                        Sgr::UnderlineColor(col) => {
                            pen.set_underline_color(col);
                        }
                        Sgr::Font(_) => {}
                    },
                    _ => {}
                }
            }
            Action::OperatingSystemCommand(_)
            | Action::DeviceControl(_)
            | Action::Esc(_)
            | Action::KittyImage(_)
            | Action::XtGetTcap(_)
            | Action::Sixel(_) => {
                flush_print(&mut print_buffer, &mut cells, &pen);
            }
        }
    });
    flush_print(&mut print_buffer, &mut cells, &pen);
    Line::from_cells(cells, SEQ_ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;
    use wezterm_term::{BlockedKind, ProgramState, ProgramStatus};

    fn pane(id: usize) -> PaneInformation {
        PaneInformation {
            pane_id: id,
            pane_index: 0,
            is_active: true,
            is_zoomed: false,
            has_unseen_output: false,
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            pixel_width: 640,
            pixel_height: 384,
            title: format!("pane{id}"),
            user_vars: HashMap::new(),
            progress: Progress::None,
            program_status: None,
            has_unseen_command_result: false,
            spinner: ' ',
        }
    }

    fn title_text(title: &TitleText) -> String {
        title
            .items
            .iter()
            .filter_map(|i| match i {
                FormatItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    fn builtin(pane: PaneInformation) -> TitleText {
        config::use_test_configuration();
        let tab = TabInformation {
            tab_id: 0,
            tab_index: 0,
            is_active: false,
            is_last_active: false,
            active_pane: Some(pane),
            window_id: 0,
            tab_title: String::new(),
            parent_tab_id: None,
        };
        builtin_tab_title(&tab, &config::configuration())
    }

    /// Built-in title glyphs: blocked beats everything, then error, then a
    /// working spinner (which must schedule frames), then the finished-
    /// command marker for plain shells.
    #[test]
    fn builtin_title_shows_program_status() {
        let mut p = pane(1);
        p.program_status = Some(ProgramStatus {
            state: ProgramState::Blocked,
            id: vec![],
            kind: Some(BlockedKind::Question),
            progress: None,
            app: Some("claude".into()),
            title: None,
            msg: None,
        });
        p.has_unseen_command_result = true;
        let t = builtin(p.clone());
        // The test config shows tab indices first, hence `contains`.
        assert!(title_text(&t).contains("? pane1"), "{:?}", title_text(&t));
        assert!(!t.has_indeterminate);

        p.program_status.as_mut().unwrap().state = ProgramState::Working;
        p.program_status.as_mut().unwrap().kind = None;
        let t = builtin(p.clone());
        assert!(t.has_indeterminate, "working drives the spinner timer");

        p.program_status.as_mut().unwrap().state = ProgramState::Error;
        let t = builtin(p.clone());
        assert!(title_text(&t).contains("✗ pane1"), "{:?}", title_text(&t));

        p.program_status = None;
        let t = builtin(p.clone());
        assert!(title_text(&t).contains("▸ pane1"), "{:?}", title_text(&t));

        p.has_unseen_command_result = false;
        let t = builtin(p);
        assert!(title_text(&t).ends_with("pane1"), "{:?}", title_text(&t));
    }

    fn tabs(n: usize) -> (Vec<TabInformation>, Vec<PaneInformation>) {
        let tabs = (0..n)
            .map(|i| TabInformation {
                tab_id: i,
                tab_index: i,
                is_active: i == 0,
                is_last_active: false,
                active_pane: Some(pane(i)),
                window_id: 0,
                tab_title: format!("tab{i}"),
                parent_tab_id: None,
            })
            .collect();
        (tabs, (0..n).map(pane).collect())
    }

    /// Install a Lua config whose `format-tab-title` counts its invocations.
    fn install_counting_callback() -> mlua::Lua {
        config::use_test_configuration();
        let lua = config::lua::make_lua_context(Path::new("/nonexistent/wezterm.lua"))
            .expect("lua context");
        lua.load(
            r#"
            local wezterm = require 'wezterm'
            calls = 0
            wezterm.on('format-tab-title', function(tab, tabs, panes, config, hover, max_width)
                calls = calls + 1
                return tab.tab_title
            end)
            "#,
        )
        .exec()
        .expect("register callback");
        lua
    }

    fn calls() -> usize {
        config::run_immediate_with_lua_config(|lua| {
            Ok(lua
                .expect("lua installed")
                .globals()
                .get::<_, usize>("calls")?)
        })
        .unwrap()
    }

    fn build(mouse_x: Option<usize>, n: usize) -> TabBarState {
        let (tab_info, pane_info) = tabs(n);
        build_from(mouse_x, &tab_info, &pane_info, None)
    }

    fn build_from(
        mouse_x: Option<usize>,
        tab_info: &[TabInformation],
        pane_info: &[PaneInformation],
        reuse: Option<&TabBarState>,
    ) -> TabBarState {
        TabBarState::new(
            200,
            mouse_x,
            tab_info,
            pane_info,
            None,
            &config::configuration(),
            "",
            "",
            reuse,
        )
    }

    /// A spinner frame (10 Hz while an agent works) only re-asks Lua for
    /// the animated tabs; the rest reuse the previous build's titles.
    #[test]
    fn animation_frame_reuses_unanimated_titles() {
        config::use_this_lua_context_for_tests(install_counting_callback());
        let (mut tab_info, pane_info) = tabs(5);
        let working = ProgramStatus {
            state: ProgramState::Working,
            id: vec![],
            kind: None,
            progress: None,
            app: Some("claude".into()),
            title: None,
            msg: None,
        };
        tab_info[2].active_pane.as_mut().unwrap().program_status = Some(working);

        let first = build_from(None, &tab_info, &pane_info, None);
        assert_eq!(calls(), 5);
        assert!(first.next_progress_frame_due().is_some());

        let frame = build_from(None, &tab_info, &pane_info, Some(&first));
        assert_eq!(calls(), 6, "only the working tab was recomputed");
        assert_eq!(frame.items().len(), first.items().len());

        // A full rebuild still asks for everything.
        build_from(None, &tab_info, &pane_info, None);
        assert_eq!(calls(), 11);
    }

    /// One Lua round-trip per tab per rebuild. With 40 tabs the old
    /// two-pass layout made 80 calls, each serialising the whole Config.
    #[test]
    fn format_tab_title_runs_once_per_tab() {
        config::use_this_lua_context_for_tests(install_counting_callback());

        let n = 7;
        let state = build(None, n);
        assert_eq!(
            state
                .items()
                .iter()
                .filter(|i| matches!(i.item, TabBarItem::Tab { .. }))
                .count(),
            n
        );
        assert_eq!(calls(), n);
    }

    /// An absent program status reaches Lua as nil, not serde's `null`
    /// userdata, so `if pane.program_status then` works.
    #[test]
    fn absent_program_status_is_nil_in_lua() {
        config::use_test_configuration();
        let lua = config::lua::make_lua_context(Path::new("/nonexistent/wezterm.lua"))
            .expect("lua context");
        lua.load(
            r#"
            local wezterm = require 'wezterm'
            wezterm.on('format-tab-title', function(tab)
                return type(tab.active_pane.program_status)
            end)
            "#,
        )
        .exec()
        .expect("register callback");
        config::use_this_lua_context_for_tests(lua);

        let state = build(None, 1);
        let item = state
            .items()
            .iter()
            .find(|i| matches!(i.item, TabBarItem::Tab { .. }))
            .expect("tab present");
        assert_eq!(item.title.as_str(), "nil");
    }

    /// Hovering recomputes only the hovered tab, so N+1 calls, not 2N.
    #[test]
    fn hover_recomputes_only_the_hovered_tab() {
        config::use_this_lua_context_for_tests(install_counting_callback());

        let n = 7;
        // tab0 is active and occupies x 0..4 ("tab0"); hover tab1 at x=5.
        let state = build(Some(5), n);
        let hovered = state
            .items()
            .iter()
            .find(|i| matches!(i.item, TabBarItem::Tab { tab_idx: 1, .. }))
            .expect("tab1 present");
        assert!(
            hovered.x <= 5 && 5 < hovered.x + hovered.width,
            "hover lands on tab1"
        );
        assert_eq!(calls(), n + 1);
    }
}
