//! Matrix "digital rain" transition played over the active pane when the user
//! switches tabs. Each column of the freshly-activated screen is *built up*
//! from the top down: a stream of green katakana falls ahead of each real
//! character, a one-or-two cell gap follows, then the real glyph slides into
//! place and stays. Columns whose content is unchanged from the previous tab
//! are skipped, and each column only rebuilds down to its deepest changed row.
//!
//! The animation rides the existing per-frame repaint scheduler
//! (`update_next_frame_time`), the same mechanism the visual bell uses.

use crate::color::LinearRgba;
use crate::customglyph::BlockKey;
use crate::quad::{QuadTrait, TripleLayerQuadAllocator, TripleLayerQuadAllocatorTrait};
use mux::pane::{Pane, PaneId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use termwiz::cell::CellAttributes;
use termwiz::image::{ImageCell, TextureCoordinate};
use wezterm_bidi::Direction;
use wezterm_font::GlyphInfo;
use window::WindowOps;

/// Total length of the transition.
const DURATION_MS: f32 = 2100.0;
/// Per-column matrix-glyph run length (the stream that leads each real char),
/// in cells. Each column picks a value in this inclusive range.
const MATRIX_MIN: i32 = 3;
const MATRIX_MAX: i32 = 12;
/// Largest per-column head-start, as a fraction of the transition, so columns
/// finish building at slightly different times.
const PHASE_MAX: f32 = 0.25;
/// Vertical gap (in cells) between the bottom of a column's matrix stream and
/// the top of the still-falling outgoing text. Kept small so the dissolving and
/// incoming screens flow into one another. Each column picks 1 or 2.
fn bridge_for(c: usize) -> i32 {
    1 + (hash3(c as u32, 5, 0) % 2) as i32
}
/// Rain quads sit on the top quad layer so they cover the pane's text.
const LAYER: usize = 2;
/// The effect's glyphs start this much larger than the cell and settle to 1.0×
/// over `ZOOM_SETTLE` of the transition. 1.21 = two Ctrl+Plus steps (×1.1 each).
const START_ZOOM: f32 = 1.21;
const ZOOM_SETTLE: f32 = 0.35;
/// Font family used for the falling stream glyphs. Its glyphs (the film's
/// mirrored katakana) are mapped onto ASCII, so we feed it ASCII below. Falls
/// back to the user's font if it isn't installed.
const MATRIX_FONT_FAMILY: &str = "Matrix Code NFI";

/// A switch within this long of the previous one is part of a scroll through
/// tabs: show plain rain instead of building each tab for a split second.
const SCRUB_GAP_MS: u64 = 350;
/// The rain holds until no switch has come for this long, then resolves into
/// the tab you landed on.
const SETTLE_MS: u64 = 300;

/// Once you stop on a tab, the rain you were scrolling through is wiped away
/// top to bottom by a front that reveals that tab; this long for the slowest column.
const RESOLVE_MS: f32 = 1000.0;

/// Where a column's reveal front is, `dt_ms` after landing: in rows from the
/// top, `-1` meaning nothing revealed yet (every row still rain) and `rows - 1`
/// meaning the whole column revealed. `phase` (0..PHASE_MAX) staggers columns.
fn resolve_front(dt_ms: f32, phase: f32, rows: usize) -> f32 {
    let fp = ((dt_ms / RESOLVE_MS - phase) / (1.0 - PHASE_MAX)).clamp(0.0, 1.0);
    fp * rows as f32 - 1.0
}

/// Is a switch this long after the previous one (`None`: there was none) part of a scroll?
fn is_scrub(since_last: Option<Duration>) -> bool {
    since_last.map_or(false, |d| d < Duration::from_millis(SCRUB_GAP_MS))
}

/// Has the hold ending at `until` run out at `now`?
fn hold_released(now: Instant, until: Instant) -> bool {
    now >= until
}

/// A pane sets this user variable (to anything non-empty) while it has nothing to
/// show yet: a browser tab unloaded to save memory, about to be reloaded. A switch
/// to it holds the rain until its picture arrives, hiding the load.
pub const HOLD_RAIN_VAR: &str = "wezterm_hold_rain_for_picture";
/// How long the rain waits for such a pane's picture before resolving anyway.
const PICTURE_WAIT_MS: u64 = 2500;
/// The share of a pane's cells that must hold a picture for it to count as arrived.
const PICTURE_SHARE: f32 = 0.5;

/// Is the hold over? At `until` always; sooner when it was waiting for the
/// pane's picture and the picture is there.
fn hold_over(now: Instant, until: Instant, wait_picture: bool, has_picture: bool) -> bool {
    hold_released(now, until) || (wait_picture && has_picture)
}

/// Chance that a tab switch decodes an easter-egg figure into the rain.
const EGG_PROBABILITY: f32 = 0.5;
/// Centre of the egg within the transition. The rain NEVER pauses — the figure
/// matrixes in and out *within* the still-streaming code.
const EGG_MIDPOINT: f32 = 0.5;
/// Egg phase durations as fractions of the (now 2.1s) transition: matrix the
/// figure in, linger at full, then matrix it out. The hold is ~0.40 × 2100 ≈
/// 840 ms — kept the same readable linger even though the rain is 2× faster.
const EGG_RISE: f32 = 0.22;
const EGG_HOLD: f32 = 0.40;
const EGG_FALL: f32 = 0.22;

/// Resume card: decode in, hold until dismissed, dissolve out. Its own
/// clock, since it outlives the transition.
const CARD_RISE_MS: f32 = 600.0;
const CARD_FALL_MS: f32 = 450.0;

/// Settled text colours: the film quote flashes white-green; the card is
/// read for seconds, so it gets a muted green on an off-black backing.
/// Bright-on-pure-black halates on an LCD, and poor vision feels it most.
const QUOTE_TEXT: LinearRgba = LinearRgba::with_components(0.85, 1.0, 0.88, 1.0);
const CARD_TEXT: LinearRgba = LinearRgba::with_components(0.45, 0.78, 0.50, 1.0);
const CARD_BACKING: LinearRgba = LinearRgba::with_components(0.07, 0.09, 0.07, 1.0);

/// Where an egg is in its life; the fraction runs 0 → 1 within the phase.
#[derive(Clone, Copy)]
enum EggPhase {
    Rise(f32),
    Hold,
    Fall(f32),
}

/// Iconic film lines, hidden in plain sight in the rain.
const QUOTES: &[&str] = &[
    "Wake up, Neo...",
    "Follow the white rabbit.",
    "There is no spoon.",
    "Knock, knock, Neo.",
    "The Matrix has you...",
    "Free your mind.",
    "I know kung fu.",
    "Welcome to the real world.",
    "Dodge this.",
    "Mr. Anderson...",
    "Know thyself.",
    "Guns. Lots of guns.",
    "What is real?",
    "He is the One.",
    "Deja vu.",
    "Welcome to the desert of the real.",
    "I'm trying to free your mind, Neo.\nBut I can only show you the door.",
    "Never send a human\nto do a machine's job.",
    "Fate, it seems, is not\nwithout a sense of irony.",
    "The Matrix is everywhere.",
    "Choice is an illusion created\nbetween those with power\nand those without.",
    "There is a difference between\nknowing the path\nand walking the path.",
    "Do you think that's air\nyou're breathing now?",
    "What you know\nyou can't explain,\nbut you feel it.",
    "Don't think you are.\nKnow you are.",
    "Don't worry about the vase.",
    "Everything that has a\nbeginning has an end.",
    "The body cannot live\nwithout the mind.",
    "You have to let it all go, Neo.",
    "He's beginning to believe.",
    "What good is a phone call\nif you're unable to speak?",
];

/// A decoded easter egg: a centred grid of a film quote's characters. The live
/// rain decodes it in, holds, then dissolves it back into the stream.
struct Egg {
    x0: i32,
    y0: i32,
    fw: usize,
    fh: usize,
    /// Per-cell glyph codepoint (0 = empty).
    glyphs: Vec<u32>,
}

impl Egg {
    /// Lay a (possibly multi-line) quote out centred in the pane grid.
    fn from_quote(text: &str, cols: usize, rows: usize) -> Egg {
        Self::from_lines(&text.split('\n').collect::<Vec<_>>(), cols, rows, true)
    }

    /// A block of lines centred in the grid; each line centred within the
    /// block (`centre`) or left-aligned, as a card wants.
    fn from_lines(lines: &[&str], cols: usize, rows: usize, centre: bool) -> Egg {
        let fw = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0)
            .max(1);
        let fh = lines.len().max(1);
        let mut glyphs = vec![0u32; fw * fh];
        for (r, line) in lines.iter().enumerate() {
            // Centre each line within the block so multi-line quotes look tidy.
            let off = if centre {
                (fw - line.chars().count()) / 2
            } else {
                0
            };
            for (c, ch) in line.chars().enumerate() {
                if ch != ' ' {
                    glyphs[r * fw + off + c] = ch as u32;
                }
            }
        }
        Egg {
            x0: (cols as i32 - fw as i32) / 2,
            y0: (rows as i32 - fh as i32) / 2,
            fw,
            fh,
            glyphs,
        }
    }
}

/// Shaped glyphs per grapheme, filled lazily; `None` is an undrawable one.
type GlyphMemo = RefCell<HashMap<String, Option<GlyphInfo>>>;

/// A resume card drawn as an egg on its own clock: matrixes in, stays
/// until dismissed, then matrixes out. A dark backing keeps it readable
/// over the pane, since no rain is streaming behind it.
pub struct ResumeCardAnim {
    start: Instant,
    dismissed: Option<Instant>,
    pane_id: PaneId,
    cols: usize,
    rows: usize,
    egg: Egg,
    glyphs: GlyphMemo,
}

impl ResumeCardAnim {
    fn phase(&self) -> Option<EggPhase> {
        if let Some(at) = self.dismissed {
            let f = at.elapsed().as_secs_f32() * 1000.0 / CARD_FALL_MS;
            return (f < 1.0).then_some(EggPhase::Fall(f));
        }
        let f = self.start.elapsed().as_secs_f32() * 1000.0 / CARD_RISE_MS;
        Some(if f < 1.0 {
            EggPhase::Rise(f)
        } else {
            EggPhase::Hold
        })
    }
}

/// Captured state for one tab-switch transition.
pub struct TabSwitchAnim {
    start: Instant,
    /// While in the future, this is a scroll through tabs: plain rain over the
    /// pane, no tab built. When it passes, the active tab resolves out of the rain.
    hold_until: Option<Instant>,
    /// Set once a scroll has stopped: the rain, which has run on one time base
    /// since the first switch, now resolves into the tab (`cells`) behind a front.
    landed_at: Option<Instant>,
    /// The hold ends early once the pane shows its picture (see `HOLD_RAIN_VAR`).
    wait_picture: bool,
    pane_id: PaneId,
    cols: usize,
    rows: usize,
    /// New screen graphemes, row-major (`row * cols + col`).
    cells: Vec<String>,
    /// Outgoing screen graphemes, same layout. Empty when there was no
    /// comparable previous screen (then the dissolve phase is skipped).
    old_cells: Vec<String>,
    /// Outgoing screen pictures, same layout; empty when it had none.
    old_images: Vec<Vec<ImageCell>>,
    /// Deepest changed row per column; `-1` means the column is unchanged and
    /// is skipped entirely.
    depth: Vec<i32>,
    /// Memoised shaping of each real grapheme, filled lazily as cells resolve.
    glyphs: GlyphMemo,
    /// Easter-egg film quote to decode mid-transition, if this switch rolled
    /// one. It plays within the normal duration; the rain never pauses.
    egg: Option<Egg>,
}

/// Cheap deterministic 3-input hash (FNV-1a flavoured). Used to give each
/// column an independent stream length / gap / phase without pulling in an RNG
/// (keeps the effect reproducible frame-to-frame).
fn hash3(a: u32, b: u32, c: u32) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for v in [a, b, c] {
        h ^= v;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Snapshot the visible viewport of `pane` as a row-major grid of graphemes.
fn capture_screen(pane: &Arc<dyn Pane>) -> (usize, usize, Vec<String>) {
    let dims = pane.get_dimensions();
    let cols = dims.cols;
    let rows = dims.viewport_rows;
    let top = dims.physical_top;
    let (_first, lines) = pane.get_lines(top..top + rows as isize);
    let mut cells = vec![String::new(); cols * rows];
    for (r, line) in lines.iter().enumerate() {
        if r >= rows {
            break;
        }
        for c in 0..cols {
            if let Some(cell) = line.get_cell(c) {
                cells[r * cols + c] = cell.str().to_string();
            }
        }
    }
    (cols, rows, cells)
}

/// The picture (if any) in each cell of `pane`'s viewport, row-major like
/// `capture_screen`. Empty when the screen holds no picture at all.
fn capture_images(pane: &Arc<dyn Pane>, cols: usize, rows: usize) -> Vec<Vec<ImageCell>> {
    let dims = pane.get_dimensions();
    let top = dims.physical_top;
    let (_first, lines) = pane.get_lines(top..top + rows as isize);
    let mut images = vec![Vec::new(); cols * rows];
    let mut any = false;
    for (r, line) in lines.iter().enumerate().take(rows) {
        for c in 0..cols {
            // Every picture in the cell, bottom to top: a browser pane stacks a
            // mostly transparent toolbar layer over its page.
            let mut stack = line
                .get_cell(c)
                .and_then(|cell| cell.attrs().images())
                .unwrap_or_default();
            stack.sort_by_key(|image| image.z_index());
            any |= !stack.is_empty();
            images[r * cols + c] = stack;
        }
    }
    if any {
        images
    } else {
        Vec::new()
    }
}

/// Per column, the deepest row that differs between two screens of the same
/// size (`-1`: the column is unchanged and is left alone). A cell differs when
/// its text does, or when either screen has a picture there: a pane showing a
/// picture (a browser) is blank text, yet all of it must leave or arrive.
fn changed_depth(
    cols: usize,
    rows: usize,
    old: &[String],
    new: &[String],
    old_picture: &[bool],
    new_picture: &[bool],
) -> Vec<i32> {
    let mut depth = vec![-1i32; cols];
    for c in 0..cols {
        for r in 0..rows {
            let i = r * cols + c;
            if old[i] != new[i] || old_picture[i] || new_picture[i] {
                depth[c] = r as i32;
            }
        }
    }
    depth
}

/// The share (0..=1) of `pane`'s viewport cells that hold a picture.
fn picture_share(pane: &Arc<dyn Pane>) -> f32 {
    let dims = pane.get_dimensions();
    let (cols, rows) = (dims.cols, dims.viewport_rows);
    if cols == 0 || rows == 0 {
        return 0.0;
    }
    let with = capture_images(pane, cols, rows)
        .iter()
        .filter(|stack| !stack.is_empty())
        .count();
    with as f32 / (cols * rows) as f32
}

/// Which cells of a captured picture grid hold a picture (all false when the grid is empty).
fn picture_flags(images: &[Vec<ImageCell>], cells: usize) -> Vec<bool> {
    if images.is_empty() {
        vec![false; cells]
    } else {
        images.iter().map(|stack| !stack.is_empty()).collect()
    }
}

impl crate::TermWindow {
    /// The pane the active transition is animating, if any.
    pub fn matrix_rain_pane_id(&self) -> Option<PaneId> {
        self.tab_switch_anim.as_ref().map(|a| a.pane_id)
    }

    /// Begin the matrix-rain transition revealing `new_pane`. Diffs against
    /// `prev_pane` so unchanged columns are skipped. No-op when nothing changed.
    pub fn start_matrix_rain(
        &mut self,
        prev_pane: Option<Arc<dyn Pane>>,
        new_pane: &Arc<dyn Pane>,
    ) {
        let (cols, rows, cells) = capture_screen(new_pane);
        if cols == 0 || rows == 0 {
            return;
        }

        // Per-column deepest changed row. When the previous screen has the same
        // dimensions we diff cell-by-cell; otherwise treat everything as changed.
        let prev = prev_pane.as_ref().map(capture_screen);
        let (depth, old_cells, old_images) = match (prev, prev_pane.as_ref()) {
            (Some((pc, pr, pcells)), Some(prev_pane)) if pc == cols && pr == rows => {
                // A picture pane (a browser) falls away, or arrives, like text does.
                let old_images = capture_images(prev_pane, cols, rows);
                let new_images = capture_images(new_pane, cols, rows);
                let depth = changed_depth(
                    cols,
                    rows,
                    &pcells,
                    &cells,
                    &picture_flags(&old_images, cols * rows),
                    &picture_flags(&new_images, cols * rows),
                );
                (depth, pcells, old_images)
            }
            _ => {
                // No comparable previous screen: rebuild every column fully and
                // skip the dissolve phase (no old grid to fall away).
                (vec![rows as i32 - 1; cols], Vec::new(), Vec::new())
            }
        };

        if depth.iter().all(|d| *d < 0) {
            // Identical screen: nothing to animate.
            return;
        }

        // Roll for an easter-egg film quote, decoded into the rain.
        let egg = if fastrand::f32() < EGG_PROBABILITY {
            let quote = QUOTES[fastrand::usize(0..QUOTES.len())];
            Some(Egg::from_quote(quote, cols, rows))
        } else {
            None
        };

        self.tab_switch_anim = Some(TabSwitchAnim {
            start: Instant::now(),
            hold_until: None,
            landed_at: None,
            wait_picture: false,
            pane_id: new_pane.pane_id(),
            cols,
            rows,
            cells,
            old_cells,
            old_images,
            depth,
            // Real glyphs are shaped lazily, on demand, as cells resolve — so
            // we never pay for the whole screen up front (which could stall).
            glyphs: RefCell::new(HashMap::new()),
            egg,
        });
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    /// A tab switch arrived: a first one builds the new tab as usual; one in
    /// quick succession (scrolling through tabs) holds plain rain instead,
    /// renewed by each further switch, and resolves into whichever tab you
    /// stop on.
    pub fn on_tab_switched(&mut self, prev_pane: Option<Arc<dyn Pane>>, new_pane: &Arc<dyn Pane>) {
        let now = Instant::now();
        let since = self.last_tab_switch_at.map(|t| now.duration_since(t));
        self.last_tab_switch_at = Some(now);
        // A scroll in progress (or still resolving) carries on, whatever the gap.
        let in_scroll = self
            .tab_switch_anim
            .as_ref()
            .map_or(false, |a| a.hold_until.is_some());
        let wait_picture = new_pane
            .copy_user_vars()
            .get(HOLD_RAIN_VAR)
            .map_or(false, |v| !v.is_empty());
        if !is_scrub(since) && !in_scroll && !wait_picture {
            self.start_matrix_rain(prev_pane, new_pane);
            return;
        }
        let wait_ms = if wait_picture {
            PICTURE_WAIT_MS
        } else {
            SETTLE_MS
        };
        let until = now + Duration::from_millis(wait_ms);
        match self.tab_switch_anim.as_mut() {
            // Already holding: keep the rain flowing, just push the end out.
            Some(a) if a.hold_until.is_some() => {
                a.hold_until = Some(until);
                a.landed_at = None; // scrolling again: the rain has no destination
                a.wait_picture = wait_picture;
                a.pane_id = new_pane.pane_id();
            }
            _ => {
                self.tab_switch_anim = Some(TabSwitchAnim {
                    start: now,
                    hold_until: Some(until),
                    landed_at: None,
                    wait_picture,
                    pane_id: new_pane.pane_id(),
                    cols: 0,
                    rows: 0,
                    cells: Vec::new(),
                    old_cells: Vec::new(),
                    old_images: Vec::new(),
                    depth: Vec::new(),
                    glyphs: RefCell::new(HashMap::new()),
                    egg: None,
                });
            }
        }
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    /// One frame of a scroll through tabs: two rain streams per column over a
    /// black pane, on the animation's single time base. Before landing nothing is
    /// revealed; after, each column's front sweeps down, wiping the rain away and
    /// revealing the landed tab (the glyph at the front slides in, as in the
    /// normal build). The streams never restart, so the rain is one effect.
    #[allow(clippy::too_many_arguments)]
    fn paint_rain_hold(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        anim: &TabSwitchAnim,
        palette: &[GlyphInfo],
        pane_left: usize,
        pane_top: usize,
        cols: usize,
        rows: usize,
    ) -> anyhow::Result<()> {
        if cols == 0 || rows == 0 || palette.is_empty() {
            return Ok(());
        }
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let (ox, oy) = self.matrix_pane_origin(pane_left, pane_top);
        let left_off = self.dimensions.pixel_width as f32 / 2.0;
        let top_off = self.dimensions.pixel_height as f32 / 2.0;
        let secs = anim.start.elapsed().as_secs_f32();
        let landed_ms = anim.landed_at.map(|t| t.elapsed().as_secs_f32() * 1000.0);
        let quantum = (secs * 1000.0 / 60.0) as u32;
        let config = self.config.clone();
        let style = self
            .fonts
            .match_style(&config, &CellAttributes::default())
            .clone();
        let font = self.fonts.default_font()?;
        let matrix_style = Self::matrix_text_style();
        let matrix_font = self.matrix_font()?;
        let metrics = self.render_metrics.clone();
        let gl_state = self.render_state.as_ref().unwrap();
        let mut glyph_cache = gl_state.glyph_cache.borrow_mut();
        let mut shape_budget = 96i32;
        let hide = LinearRgba::with_components(0.0, 0.0, 0.0, 1.0);

        for c in 0..cols {
            let phase = (hash3(c as u32, 3, 0) % 1000) as f32 / 1000.0 * PHASE_MAX;
            let front = landed_ms.map_or(-1.0, |ms| resolve_front(ms, phase, rows));
            // First row still under rain; rows above it show the landed tab.
            let first_rain = ((front.floor() as i32) + 1).clamp(0, rows as i32);

            if first_rain < rows as i32 {
                self.filled_rectangle(
                    layers,
                    LAYER,
                    euclid::rect(
                        ox + c as f32 * cell_w,
                        oy + first_rain as f32 * cell_h,
                        cell_w,
                        (rows as i32 - first_rain) as f32 * cell_h,
                    ),
                    hide,
                )?;
            }

            // The tab's own glyph, sliding into the row the front is locking in.
            if front >= 0.0
                && first_rain < rows as i32
                && c < anim.cols
                && (first_rain as usize) < anim.rows
            {
                let cell = anim.cells[first_rain as usize * anim.cols + c].clone();
                if let Some(info) = self.memo_glyph(&anim.glyphs, &cell, &mut shape_budget) {
                    self.emit_matrix_glyph(
                        layers,
                        &mut glyph_cache,
                        &info,
                        &style,
                        &font,
                        &metrics,
                        ox,
                        oy,
                        cell_w,
                        cell_h,
                        c,
                        front,
                        LinearRgba::with_components(0.80, 1.0, 0.85, 1.0),
                        1.0,
                        left_off,
                        top_off,
                    )?;
                }
            }

            for stream in 0..2u32 {
                let trail = 5 + (hash3(c as u32, 11, stream) % 9) as i32;
                let speed = 9.0 + (hash3(c as u32, 12, stream) % 12) as f32; // rows per second
                let span = (rows as i32 + trail) as f32;
                let offset = (hash3(c as u32, 13, stream) % 1000) as f32 / 1000.0 * span;
                let head = ((secs * speed + offset) % span) as i32;
                for j in 0..trail {
                    let row = head - j;
                    if row < first_rain || row >= rows as i32 {
                        continue;
                    }
                    let idx =
                        hash3(c as u32, row as u32, quantum + stream) as usize % palette.len();
                    let b = 1.0 - j as f32 / trail as f32;
                    let color = if j == 0 {
                        LinearRgba::with_components(0.75, 1.0, 0.80, 1.0)
                    } else {
                        let g = 0.25 + 0.75 * b;
                        LinearRgba::with_components(0.0, g, 0.10 * g, 0.35 + 0.65 * b)
                    };
                    self.emit_matrix_glyph(
                        layers,
                        &mut glyph_cache,
                        &palette[idx],
                        &matrix_style,
                        &matrix_font,
                        &metrics,
                        ox,
                        oy,
                        cell_w,
                        cell_h,
                        c,
                        row as f32,
                        color,
                        1.0,
                        left_off,
                        top_off,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Look up a real grapheme's glyph from the per-animation memo, shaping it
    /// on first sight (subject to a per-frame budget). Returns `None` for
    /// blanks, undrawable glyphs, or when the budget is spent this frame.
    fn memo_glyph(&self, memo: &GlyphMemo, s: &str, budget: &mut i32) -> Option<GlyphInfo> {
        if s.is_empty() || s == " " {
            return None;
        }
        if !memo.borrow().contains_key(s) {
            if *budget <= 0 {
                return None;
            }
            let shaped = self.shape_grapheme(s);
            memo.borrow_mut().insert(s.to_string(), shaped);
            *budget -= 1;
        }
        memo.borrow().get(s).and_then(|o| o.clone())
    }

    pub fn resume_card_pane_id(&self) -> Option<PaneId> {
        self.resume_card.as_ref().map(|a| a.pane_id)
    }

    /// Show `lines` as a card over the active pane, matrixing in. Replaces
    /// any card already up.
    pub fn show_resume_card(&mut self, lines: Vec<String>) {
        let pane = match self.get_active_pane_or_overlay() {
            Some(p) => p,
            None => return,
        };
        let dims = pane.get_dimensions();
        let (cols, rows) = (dims.cols, dims.viewport_rows);
        if cols == 0 || rows == 0 || lines.is_empty() {
            return;
        }
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        self.resume_card = Some(ResumeCardAnim {
            start: Instant::now(),
            dismissed: None,
            pane_id: pane.pane_id(),
            cols,
            rows,
            egg: Egg::from_lines(&lines, cols, rows, false),
            glyphs: RefCell::new(HashMap::new()),
        });
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    /// Start the card dissolving. A no-op without a card, or if it already is.
    pub fn dismiss_resume_card(&mut self) {
        if let Some(card) = self.resume_card.as_mut() {
            if card.dismissed.is_none() {
                card.dismissed = Some(Instant::now());
                if let Some(window) = self.window.as_ref() {
                    window.invalidate();
                }
            }
        }
    }

    /// Drop the card outright, as a tab change does.
    pub fn clear_resume_card(&mut self) {
        self.resume_card = None;
    }

    /// Paint one frame of the resume card over the active pane. Frames are
    /// only scheduled while it is rising or falling; the hold is static.
    pub fn paint_resume_card(
        &mut self,
        layers: &mut TripleLayerQuadAllocator,
        pane_left: usize,
        pane_top: usize,
        pane_cols: usize,
        pane_rows: usize,
    ) -> anyhow::Result<()> {
        let phase = match self.resume_card.as_ref().and_then(|c| c.phase()) {
            Some(phase) => phase,
            None => {
                if self.resume_card.take().is_some() {
                    if let Some(window) = self.window.as_ref() {
                        window.invalidate();
                    }
                }
                return Ok(());
            }
        };
        if !matches!(phase, EggPhase::Hold) {
            self.update_next_frame_time(Some(Instant::now() + Duration::from_millis(16)));
        }
        let card = self.resume_card.take().unwrap();
        let palette = match self.matrix_rain_palette.take() {
            Some(p) => p,
            None => self.shape_matrix_palette().unwrap_or_default(),
        };
        let result = self.paint_resume_card_inner(
            layers, &card, &palette, phase, pane_left, pane_top, pane_cols, pane_rows,
        );
        self.matrix_rain_palette = Some(palette);
        self.resume_card = Some(card);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_resume_card_inner(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        card: &ResumeCardAnim,
        palette: &[GlyphInfo],
        phase: EggPhase,
        pane_left: usize,
        pane_top: usize,
        pane_cols: usize,
        pane_rows: usize,
    ) -> anyhow::Result<()> {
        let cols = card.cols.min(pane_cols);
        let rows = card.rows.min(pane_rows);
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let (ox, oy) = self.matrix_pane_origin(pane_left, pane_top);
        let left_off = self.dimensions.pixel_width as f32 / 2.0;
        let top_off = self.dimensions.pixel_height as f32 / 2.0;
        let quantum = (card.start.elapsed().as_secs_f32() * 1000.0 / 60.0) as u32;

        let config = self.config.clone();
        let attrs = CellAttributes::default();
        let style = self.fonts.match_style(&config, &attrs).clone();
        let font = self.fonts.default_font()?;
        let matrix_style = Self::matrix_text_style();
        let matrix_font = self.matrix_font()?;
        let gl_state = self.render_state.as_ref().unwrap();
        let mut glyph_cache = gl_state.glyph_cache.borrow_mut();
        let mut shape_budget = 96i32;

        // Backing: a cell of margin round the block, fading with the phase.
        let strength = match phase {
            EggPhase::Rise(f) => f,
            EggPhase::Hold => 1.0,
            EggPhase::Fall(f) => 1.0 - f,
        };
        let egg = &card.egg;
        let x0 = (egg.x0 - 1).max(0) as f32;
        let x1 = (egg.x0 + egg.fw as i32 + 1).min(cols as i32) as f32;
        let y0 = (egg.y0 - 1).max(0) as f32;
        let y1 = (egg.y0 + egg.fh as i32 + 1).min(rows as i32) as f32;
        self.filled_rectangle(
            layers,
            LAYER,
            euclid::rect(
                ox + x0 * cell_w - left_off,
                oy + y0 * cell_h - top_off,
                (x1 - x0) * cell_w,
                (y1 - y0) * cell_h,
            ),
            CARD_BACKING.mul_alpha(0.96 * strength),
        )?;

        self.paint_egg(
            layers,
            &mut glyph_cache,
            &card.glyphs,
            &mut shape_budget,
            egg,
            phase,
            CARD_TEXT,
            palette,
            quantum,
            &style,
            &font,
            &matrix_style,
            &matrix_font,
            ox,
            oy,
            cols,
            rows,
            cell_w,
            cell_h,
            left_off,
            top_off,
        )
    }

    /// Shape a single grapheme to its leading glyph, or `None` if it has no
    /// drawable glyph. One `font.shape` call; callers memoise the result.
    fn shape_grapheme(&self, s: &str) -> Option<GlyphInfo> {
        let font = self.fonts.default_font().ok()?;
        let window = self.window.as_ref().map(|w| w.clone());
        let infos = font
            .shape(
                s,
                move || {
                    if let Some(w) = window {
                        w.notify(crate::termwindow::TermWindowNotif::InvalidateShapeCache);
                    }
                },
                BlockKey::filter_out_synthetic,
                None,
                Direction::LeftToRight,
                None,
                None,
            )
            .ok()?;
        infos.into_iter().next()
    }

    /// Text style selecting the matrix stream font.
    fn matrix_text_style() -> config::TextStyle {
        config::TextStyle {
            font: vec![config::FontAttributes::new(MATRIX_FONT_FAMILY)],
            foreground: None,
        }
    }

    /// Resolve the matrix stream font, falling back to the user's font if the
    /// Matrix Code NFI family isn't installed.
    fn matrix_font(&self) -> anyhow::Result<std::rc::Rc<wezterm_font::LoadedFont>> {
        self.fonts
            .resolve_font(&Self::matrix_text_style())
            .or_else(|_| self.fonts.default_font())
    }

    /// Lazily shape the glyph palette for the falling stream, in the matrix
    /// font. Matrix Code NFI maps its rain glyphs onto ASCII letters/digits, so
    /// that's what we feed it. `.notdef` glyphs are dropped so a missing font
    /// degrades to plain characters rather than tofu boxes.
    fn shape_matrix_palette(&self) -> anyhow::Result<Vec<GlyphInfo>> {
        let font = self.matrix_font()?;
        let s = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let window = self.window.as_ref().unwrap().clone();
        let infos = font.shape(
            s,
            move || window.notify(crate::termwindow::TermWindowNotif::InvalidateShapeCache),
            BlockKey::filter_out_synthetic,
            None,
            Direction::LeftToRight,
            None,
            None,
        )?;
        Ok(infos
            .into_iter()
            .filter(|i| i.glyph_pos != 0 && !i.is_space)
            .collect())
    }

    /// Top-left pixel of the active pane's cell grid. Mirrors the origin maths
    /// in `paint_pane` so the rain lines up with the real cells.
    fn matrix_pane_origin(&self, pane_left: usize, pane_top: usize) -> (f32, f32) {
        let (padding_left, padding_top) = self.padding_left_top();
        let (top_bar_height, _) = self.horizontal_bar_heights();
        let left_bar_width = self.left_bar_width();
        let border = self.get_os_border();
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let x =
            left_bar_width + padding_left + border.left.get() as f32 + pane_left as f32 * cell_w;
        let y = top_bar_height + padding_top + border.top.get() as f32 + pane_top as f32 * cell_h;
        (x, y)
    }

    /// Emit one glyph quad at cell (col, fractional-row) tinted `color`.
    #[allow(clippy::too_many_arguments)]
    fn emit_matrix_glyph(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        glyph_cache: &mut crate::glyphcache::GlyphCache,
        info: &GlyphInfo,
        style: &config::TextStyle,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        metrics: &crate::utilsprites::RenderMetrics,
        ox: f32,
        oy: f32,
        cell_w: f32,
        cell_h: f32,
        col: usize,
        row_f: f32,
        color: LinearRgba,
        zoom: f32,
        left_off: f32,
        top_off: f32,
    ) -> anyhow::Result<()> {
        let glyph = glyph_cache.cached_glyph(info, style, false, font, metrics, 1)?;
        let texture = match glyph.texture.as_ref() {
            Some(t) => t,
            None => return Ok(()),
        };
        // `zoom` scales the glyph about its cell centre, so the effect can start
        // a little larger and settle to the normal cell size.
        let gw = texture.coords.size.width as f32 * glyph.scale as f32 * zoom;
        let gh = texture.coords.size.height as f32 * glyph.scale as f32 * zoom;
        let gx = ox + col as f32 * cell_w + (cell_w - gw) / 2.0;
        let gy = oy + row_f * cell_h + (cell_h - gh) / 2.0;
        let mut quad = layers.allocate(LAYER)?;
        quad.set_position(
            gx - left_off,
            gy - top_off,
            gx + gw - left_off,
            gy + gh - top_off,
        );
        quad.set_fg_color(color);
        quad.set_texture(texture.texture_coords());
        quad.set_has_color(glyph.has_color);
        quad.set_hsv(None);
        Ok(())
    }

    /// Emit the slice of a picture one cell holds, at cell (col, row_f): the
    /// outgoing screen's picture falls with its column, as its text does.
    #[allow(clippy::too_many_arguments)]
    fn emit_matrix_image(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        glyph_cache: &mut crate::glyphcache::GlyphCache,
        image: &ImageCell,
        ox: f32,
        oy: f32,
        cell_w: f32,
        cell_h: f32,
        col: usize,
        row_f: f32,
        left_off: f32,
        top_off: f32,
    ) -> anyhow::Result<()> {
        // Same lookup as the pane renderer, so the texture is the one already cached.
        let padding = (cell_h.max(cell_w) as usize).next_power_of_two();
        let (sprite, _next_due, _load_state) =
            glyph_cache.cached_image(image.image_data(), Some(padding), self.allow_images)?;
        let width = sprite.coords.size.width as f32;
        let height = sprite.coords.size.height as f32;
        let texture_width = sprite.texture.width() as f32;
        let texture_height = sprite.texture.height() as f32;
        let (top_left, bottom_right): (TextureCoordinate, TextureCoordinate) =
            (image.top_left(), image.bottom_right());
        let texture_rect = window::bitmaps::TextureRect::new(
            window::bitmaps::TextureCoord::new(
                (sprite.coords.origin.x as f32 + *top_left.x * width) / texture_width,
                (sprite.coords.origin.y as f32 + *top_left.y * height) / texture_height,
            ),
            window::bitmaps::TextureSize::new(
                (*bottom_right.x - *top_left.x) * width / texture_width,
                (*bottom_right.y - *top_left.y) * height / texture_height,
            ),
        );
        let x = ox + col as f32 * cell_w - left_off;
        let y = oy + row_f * cell_h - top_off;
        let mut quad = layers.allocate(LAYER)?;
        quad.set_position(x, y, x + cell_w, y + cell_h);
        quad.set_hsv(None);
        quad.set_fg_color(LinearRgba::with_components(1.0, 1.0, 1.0, 1.0));
        quad.set_texture(texture_rect);
        quad.set_has_color(true);
        Ok(())
    }

    /// Emit one glyph quad at cell (col, row) placed on the text baseline, as
    /// the pane renderer does, so a line of prose reads as a line of prose.
    /// (`emit_matrix_glyph` centres each bitmap in its cell, which is right
    /// for the stream and wrong for text: letters of different heights hop.)
    #[allow(clippy::too_many_arguments)]
    fn emit_text_glyph(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        glyph_cache: &mut crate::glyphcache::GlyphCache,
        info: &GlyphInfo,
        style: &config::TextStyle,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        metrics: &crate::utilsprites::RenderMetrics,
        ox: f32,
        oy: f32,
        cell_w: f32,
        cell_h: f32,
        col: usize,
        row: usize,
        color: LinearRgba,
        left_off: f32,
        top_off: f32,
    ) -> anyhow::Result<()> {
        let glyph = glyph_cache.cached_glyph(info, style, false, font, metrics, 1)?;
        let texture = match glyph.texture.as_ref() {
            Some(t) => t,
            None => return Ok(()),
        };
        let gw = texture.coords.size.width as f32 * glyph.scale as f32;
        let gh = texture.coords.size.height as f32 * glyph.scale as f32;
        let gx = ox + col as f32 * cell_w + (glyph.x_offset + glyph.bearing_x).get() as f32;
        let gy = oy + row as f32 * cell_h + cell_h + metrics.descender.get() as f32
            - (glyph.y_offset + glyph.bearing_y).get() as f32;
        let mut quad = layers.allocate(LAYER)?;
        quad.set_position(
            gx - left_off,
            gy - top_off,
            gx + gw - left_off,
            gy + gh - top_off,
        );
        quad.set_fg_color(color);
        quad.set_texture(texture.texture_coords());
        quad.set_has_color(glyph.has_color);
        quad.set_hsv(None);
        Ok(())
    }

    /// Paint one frame of the transition over the active pane and schedule the
    /// next frame. Clears the animation state once it has run its course.
    pub fn paint_matrix_rain(
        &mut self,
        layers: &mut TripleLayerQuadAllocator,
        pane_left: usize,
        pane_top: usize,
        pane_cols: usize,
        pane_rows: usize,
    ) -> anyhow::Result<()> {
        let (start, hold_until) = match &self.tab_switch_anim {
            Some(anim) => (anim.start, anim.hold_until),
            None => return Ok(()),
        };

        // A scroll through tabs: one rain on one time base. It runs with no
        // destination until you stop; then the same rain is wiped away by a
        // front revealing the tab you landed on.
        if let Some(until) = hold_until {
            let landed = self.tab_switch_anim.as_ref().and_then(|a| a.landed_at);
            let wait_picture = self
                .tab_switch_anim
                .as_ref()
                .map_or(false, |a| a.wait_picture);
            let has_picture = landed.is_none()
                && wait_picture
                && self
                    .get_active_pane_or_overlay()
                    .map_or(false, |pane| picture_share(&pane) >= PICTURE_SHARE);
            if landed.is_none() && hold_over(Instant::now(), until, wait_picture, has_picture) {
                if let Some(pane) = self.get_active_pane_or_overlay() {
                    let (cols, rows, cells) = capture_screen(&pane);
                    if let Some(a) = self.tab_switch_anim.as_mut() {
                        a.cols = cols;
                        a.rows = rows;
                        a.cells = cells;
                        a.pane_id = pane.pane_id();
                        a.glyphs = RefCell::new(HashMap::new());
                        a.landed_at = Some(Instant::now());
                    }
                } else {
                    self.tab_switch_anim = None;
                    return Ok(());
                }
            }
            let landed = self.tab_switch_anim.as_ref().and_then(|a| a.landed_at);
            if landed.map_or(false, |t| t.elapsed().as_secs_f32() * 1000.0 >= RESOLVE_MS) {
                self.tab_switch_anim = None;
                if let Some(window) = self.window.as_ref() {
                    window.invalidate();
                }
                return Ok(());
            }
            self.update_next_frame_time(Some(Instant::now() + Duration::from_millis(16)));
            let anim = self.tab_switch_anim.take().unwrap();
            let palette = match self.matrix_rain_palette.take() {
                Some(p) => p,
                None => self.shape_matrix_palette().unwrap_or_default(),
            };
            let result = self.paint_rain_hold(
                layers, &anim, &palette, pane_left, pane_top, pane_cols, pane_rows,
            );
            self.matrix_rain_palette = Some(palette);
            self.tab_switch_anim = Some(anim);
            return result;
        }

        let elapsed = start.elapsed().as_secs_f32() * 1000.0;
        if elapsed >= DURATION_MS {
            self.tab_switch_anim = None;
            if let Some(window) = self.window.as_ref() {
                window.invalidate();
            }
            return Ok(());
        }

        // Keep the repaint loop spinning while the effect runs.
        self.update_next_frame_time(Some(Instant::now() + Duration::from_millis(16)));

        // Take the animation + palette out so we can borrow `self` immutably
        // for rendering; both are put back before returning.
        let anim = self.tab_switch_anim.take().unwrap();
        let palette = match self.matrix_rain_palette.take() {
            Some(p) => p,
            None => self.shape_matrix_palette().unwrap_or_default(),
        };

        let result = self.paint_matrix_rain_inner(
            layers, &anim, &palette, elapsed, pane_left, pane_top, pane_cols, pane_rows,
        );

        self.matrix_rain_palette = Some(palette);
        self.tab_switch_anim = Some(anim);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_matrix_rain_inner(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        anim: &TabSwitchAnim,
        palette: &[GlyphInfo],
        elapsed: f32,
        pane_left: usize,
        pane_top: usize,
        pane_cols: usize,
        pane_rows: usize,
    ) -> anyhow::Result<()> {
        // Geometry from the live pane; content/diff from the snapshot. If a
        // resize changed the grid mid-flight, clamp to the smaller extent.
        let cols = anim.cols.min(pane_cols);
        let rows = anim.rows.min(pane_rows);
        if cols == 0 || rows == 0 {
            return Ok(());
        }

        let p = (elapsed / DURATION_MS).clamp(0.0, 1.0);
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let (ox, oy) = self.matrix_pane_origin(pane_left, pane_top);
        let left_off = self.dimensions.pixel_width as f32 / 2.0;
        let top_off = self.dimensions.pixel_height as f32 / 2.0;
        let rows_i = rows as i32;
        let quantum = (elapsed / 60.0) as u32;
        // Start a touch larger and settle to the normal cell size. The ease
        // finishes well before any row locks in, so the hand-off to the normal
        // (1.0×) terminal render is seamless.
        let zoom = {
            let t = (p / ZOOM_SETTLE).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            START_ZOOM + (1.0 - START_ZOOM) * t
        };

        // Fonts shared by both phases: real chars in the user's font, the
        // falling stream in the matrix font.
        let config = self.config.clone();
        let attrs = CellAttributes::default();
        let style = self.fonts.match_style(&config, &attrs).clone();
        let font = self.fonts.default_font()?;
        let matrix_style = Self::matrix_text_style();
        let matrix_font = self.matrix_font()?;
        let metrics = self.render_metrics.clone();
        let gl_state = self.render_state.as_ref().unwrap();
        let mut glyph_cache = gl_state.glyph_cache.borrow_mut();
        // Cap new shaping per frame so a dense screen can never stall a frame.
        let mut shape_budget = 96i32;

        let hide = LinearRgba::with_components(0.0, 0.0, 0.0, 1.0);
        let has_old = !anim.old_cells.is_empty();

        struct Col {
            fall: f32,
            settle: f32,
            front_row: i32,
            gap: i32,
            matrix: i32,
            depth: i32,
        }
        // One descending coordinate ties the dissolve and the build together.
        // `fall` is the outgoing text's downward shift; the matrix stream sits
        // just above it and the locked-in new text above that. So as the old
        // text pours off the bottom, the new screen flows in right behind it
        // with only a `bridge`-row gap — no big black band between them.
        let col_desc = |c: usize| -> Col {
            let depth = anim.depth[c].min(rows_i - 1);
            let span = (MATRIX_MAX - MATRIX_MIN + 1) as u32;
            let matrix = MATRIX_MIN + (hash3(c as u32, 1, 0) % span) as i32;
            let gap = 1 + (hash3(c as u32, 2, 0) % 2) as i32; // new char ↔ stream
            let bridge = bridge_for(c); // stream ↔ falling old text
            let phase = (hash3(c as u32, 3, 0) % 1000) as f32 / 1000.0 * PHASE_MAX;
            let fp = ((p - phase) / (1.0 - PHASE_MAX)).clamp(0.0, 1.0);
            let fp = fp * fp * (3.0 - 2.0 * fp); // smoothstep
            let total_lead = (gap + matrix + bridge) as f32;
            // Both finish at fp == 1: old fully off the bottom, all rows built.
            let fall = fp * (depth as f32 + 1.0 + total_lead);
            let settle = fall - total_lead;
            Col {
                fall,
                settle,
                front_row: settle.floor() as i32,
                gap,
                matrix,
                depth,
            }
        };

        // One unified pass per column. Draw order within a column: black mask →
        // falling old text → building new text, so the incoming build layers
        // over the outgoing text where they meet.
        for c in 0..cols {
            let cd = col_desc(c);
            if cd.depth < 0 {
                continue; // unchanged column: its text just stays put
            }

            // Mask the region below the build front (down to the deepest
            // change). Above the front, the locked-in new text shows through.
            let mask_top = (cd.front_row + 1).max(0);
            if mask_top <= cd.depth {
                let y0 = oy + mask_top as f32 * cell_h;
                let y1 = oy + (cd.depth + 1) as f32 * cell_h;
                self.filled_rectangle(
                    layers,
                    LAYER,
                    euclid::rect(ox + c as f32 * cell_w, y0, cell_w, y1 - y0),
                    hide,
                )?;
            }

            // Falling old text: rides `bridge` rows below the matrix stream and
            // pours off the bottom, so the outgoing screen flows straight into
            // the incoming build above it.
            if has_old {
                for r in 0..=cd.depth {
                    let yrow = r as f32 + cd.fall;
                    if yrow > cd.depth as f32 + 0.999 {
                        continue; // off the bottom of the changed region
                    }
                    let stack = anim.old_images.get(r as usize * anim.cols + c);
                    for image in stack.into_iter().flatten() {
                        self.emit_matrix_image(
                            layers,
                            &mut glyph_cache,
                            image,
                            ox,
                            oy,
                            cell_w,
                            cell_h,
                            c,
                            yrow,
                            left_off,
                            top_off,
                        )?;
                    }
                    let s = anim.old_cells[r as usize * anim.cols + c].clone();
                    if let Some(info) = self.memo_glyph(&anim.glyphs, &s, &mut shape_budget) {
                        let g = 0.40 + 0.55 * (yrow / (cd.depth as f32 + 1.0)).clamp(0.0, 1.0);
                        let color = LinearRgba::with_components(0.0, g, 0.12 * g, 1.0);
                        self.emit_matrix_glyph(
                            layers,
                            &mut glyph_cache,
                            &info,
                            &style,
                            &font,
                            &metrics,
                            ox,
                            oy,
                            cell_w,
                            cell_h,
                            c,
                            yrow,
                            color,
                            zoom,
                            left_off,
                            top_off,
                        )?;
                    }
                }
            }

            let r = cd.front_row + 1; // row currently locking in

            // The new real glyph slides down sub-cell, riding the fractional
            // front. Skip while the front is still above the top edge.
            if r >= 0 && r <= cd.depth && cd.settle >= 0.0 {
                let s = anim.cells[r as usize * anim.cols + c].clone();
                if let Some(info) = self.memo_glyph(&anim.glyphs, &s, &mut shape_budget) {
                    let color = LinearRgba::with_components(0.80, 1.0, 0.85, 1.0);
                    self.emit_matrix_glyph(
                        layers,
                        &mut glyph_cache,
                        &info,
                        &style,
                        &font,
                        &metrics,
                        ox,
                        oy,
                        cell_w,
                        cell_h,
                        c,
                        cd.settle, // fractional row → sub-cell slide
                        color,
                        zoom,
                        left_off,
                        top_off,
                    )?;
                }
            }

            // Matrix stream: below the new char, separated by `gap` blanks.
            if !palette.is_empty() {
                for j in 0..cd.matrix {
                    let row = r + cd.gap + 1 + j;
                    if row < 0 || row > cd.depth || row >= rows_i {
                        continue;
                    }
                    let idx = hash3(c as u32, row as u32, quantum) as usize % palette.len();
                    let b = (j + 1) as f32 / cd.matrix as f32;
                    let color = if j == cd.matrix - 1 {
                        LinearRgba::with_components(0.75, 1.0, 0.80, 1.0)
                    } else {
                        let g = 0.25 + 0.75 * b;
                        LinearRgba::with_components(0.0, g, 0.10 * g, 0.35 + 0.65 * b)
                    };
                    self.emit_matrix_glyph(
                        layers,
                        &mut glyph_cache,
                        &palette[idx],
                        &matrix_style,
                        &matrix_font,
                        &metrics,
                        ox,
                        oy,
                        cell_w,
                        cell_h,
                        c,
                        row as f32,
                        color,
                        zoom,
                        left_off,
                        top_off,
                    )?;
                }
            }
        }

        // Easter egg: the quote matrixes ON (rain glyphs sweep down and settle
        // into the text) then OFF, within the live rain, which never pauses.
        let egg_total = EGG_RISE + EGG_HOLD + EGG_FALL;
        let egg_t0 = EGG_MIDPOINT - egg_total / 2.0;
        if let Some(egg) = anim.egg.as_ref() {
            if p >= egg_t0 && p <= egg_t0 + egg_total {
                let local = p - egg_t0;
                let phase = if local < EGG_RISE {
                    EggPhase::Rise(local / EGG_RISE)
                } else if local < EGG_RISE + EGG_HOLD {
                    EggPhase::Hold
                } else {
                    EggPhase::Fall((local - EGG_RISE - EGG_HOLD) / EGG_FALL)
                };
                self.paint_egg(
                    layers,
                    &mut glyph_cache,
                    &anim.glyphs,
                    &mut shape_budget,
                    egg,
                    phase,
                    QUOTE_TEXT,
                    palette,
                    quantum,
                    &style,
                    &font,
                    &matrix_style,
                    &matrix_font,
                    ox,
                    oy,
                    cols,
                    rows,
                    cell_w,
                    cell_h,
                    left_off,
                    top_off,
                )?;
            }
        }

        Ok(())
    }

    /// Draw one frame of the egg figure *within* the live, streaming rain. The
    /// figure matrixes ON then OFF: a front sweeps top→bottom; ahead of it the
    /// figure cells flicker as green matrix glyphs, behind it they settle into
    /// the white-green text. Past the peak, a second front dissolves it
    /// the same way. The rain underneath never pauses.
    #[allow(clippy::too_many_arguments)]
    fn paint_egg(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        glyph_cache: &mut crate::glyphcache::GlyphCache,
        memo: &GlyphMemo,
        shape_budget: &mut i32,
        egg: &Egg,
        phase: EggPhase,
        text: LinearRgba,
        palette: &[GlyphInfo],
        quantum: u32,
        style: &config::TextStyle,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        matrix_style: &config::TextStyle,
        matrix_font: &std::rc::Rc<wezterm_font::LoadedFont>,
        ox: f32,
        oy: f32,
        cols: usize,
        rows: usize,
        cell_w: f32,
        cell_h: f32,
        left_off: f32,
        top_off: f32,
    ) -> anyhow::Result<()> {
        // Length (in figure rows) of the flickering decode band at each front.
        const LEAD: f32 = 5.0;
        let span = egg.fh as f32 + LEAD;
        let (building, front) = match phase {
            EggPhase::Rise(f) => (true, f.clamp(0.0, 1.0) * span),
            EggPhase::Hold => (true, span), // fully settled
            EggPhase::Fall(f) => (false, f.clamp(0.0, 1.0) * span),
        };

        for fr in 0..egg.fh {
            let frf = fr as f32;
            // Per-row state relative to the sweeping front.
            // building: settled below front-LEAD, flickering in the band, unseen above front.
            // dissolving: gone below front-LEAD, flickering in the band, settled above front.
            enum Cell {
                Settled,
                Flicker,
                Skip,
            }
            let state = if building {
                if frf < front - LEAD {
                    Cell::Settled
                } else if frf <= front {
                    Cell::Flicker
                } else {
                    Cell::Skip
                }
            } else if frf < front - LEAD {
                Cell::Skip
            } else if frf <= front {
                Cell::Flicker
            } else {
                Cell::Settled
            };
            if matches!(state, Cell::Skip) {
                continue;
            }

            for fc in 0..egg.fw {
                let code = egg.glyphs[fr * egg.fw + fc];
                if code == 0 {
                    continue;
                }
                let cc = egg.x0 + fc as i32;
                let rr = egg.y0 + fr as i32;
                if cc < 0 || cc >= cols as i32 || rr < 0 || rr >= rows as i32 {
                    continue;
                }

                match state {
                    Cell::Settled => {
                        let color = text;
                        let glyph = char::from_u32(code).unwrap_or('\u{2588}');
                        let mut buf = [0u8; 4];
                        let g = glyph.encode_utf8(&mut buf);
                        if let Some(info) = self.memo_glyph(memo, g, shape_budget) {
                            self.emit_text_glyph(
                                layers,
                                glyph_cache,
                                &info,
                                style,
                                font,
                                &self.render_metrics,
                                ox,
                                oy,
                                cell_w,
                                cell_h,
                                cc as usize,
                                rr as usize,
                                color,
                                left_off,
                                top_off,
                            )?;
                        }
                    }
                    Cell::Flicker if !palette.is_empty() => {
                        // Green matrix glyph, flickering as the figure decodes.
                        let idx = hash3(cc as u32, rr as u32, quantum) as usize % palette.len();
                        let color = LinearRgba::with_components(0.2, 1.0, 0.3, 1.0);
                        self.emit_matrix_glyph(
                            layers,
                            glyph_cache,
                            &palette[idx],
                            matrix_style,
                            matrix_font,
                            &self.render_metrics,
                            ox,
                            oy,
                            cell_w,
                            cell_h,
                            cc as usize,
                            rr as f32,
                            color,
                            1.0,
                            left_off,
                            top_off,
                        )?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod scrub_test {
    use super::*;

    fn text(rows: &[&str]) -> Vec<String> {
        rows.iter()
            .flat_map(|r| r.chars().map(|c| c.to_string()))
            .collect()
    }

    #[test]
    fn a_column_is_changed_down_to_its_deepest_differing_row() {
        let old = text(&["ab", "cd", "ef"]);
        let new = text(&["ab", "cX", "ef"]);
        let none = vec![false; 6];
        assert_eq!(changed_depth(2, 3, &old, &new, &none, &none), vec![-1, 1]);
    }

    /// A browser pane is a picture over blank cells: its text never differs from
    /// another blank screen, yet the whole picture must leave (or arrive).
    #[test]
    fn a_picture_counts_as_changed_wherever_it_is() {
        let blank = text(&["  ", "  ", "  "]);
        let none = vec![false; 6];
        // Old screen has a picture in column 0, rows 0 and 1.
        let old_img = vec![true, false, true, false, false, false];
        assert_eq!(
            changed_depth(2, 3, &blank, &blank, &old_img, &none),
            vec![1, -1]
        );
        // The same picture arriving counts too.
        assert_eq!(
            changed_depth(2, 3, &blank, &blank, &none, &old_img),
            vec![1, -1]
        );
    }

    /// A pane that is still loading its picture (a browser tab being reloaded)
    /// keeps the rain until the picture is there, however soon that is.
    #[test]
    fn a_hold_waiting_for_a_picture_ends_when_the_picture_arrives() {
        let t0 = Instant::now();
        let until = t0 + Duration::from_millis(PICTURE_WAIT_MS);
        assert!(!hold_over(t0, until, true, false));
        assert!(hold_over(t0, until, true, true));
    }

    #[test]
    fn a_hold_waiting_for_a_picture_gives_up_at_its_limit() {
        let t0 = Instant::now();
        let until = t0 + Duration::from_millis(PICTURE_WAIT_MS);
        assert!(hold_over(until, until, true, false));
    }

    #[test]
    fn a_picture_does_not_cut_an_ordinary_scroll_short() {
        let t0 = Instant::now();
        let until = t0 + Duration::from_millis(SETTLE_MS);
        assert!(!hold_over(t0, until, false, true));
        assert!(hold_over(until, until, false, true));
    }

    #[test]
    fn a_first_switch_is_not_a_scrub() {
        assert!(!is_scrub(None));
    }

    #[test]
    fn a_quick_second_switch_is_a_scrub() {
        assert!(is_scrub(Some(Duration::from_millis(120))));
        assert!(is_scrub(Some(Duration::from_millis(SCRUB_GAP_MS - 1))));
    }

    #[test]
    fn a_pause_ends_the_scrub() {
        assert!(!is_scrub(Some(Duration::from_millis(SCRUB_GAP_MS))));
        assert!(!is_scrub(Some(Duration::from_secs(5))));
    }

    #[test]
    fn the_front_starts_above_the_pane_and_ends_below_it() {
        // Nothing revealed at landing (row -1: every row still rain), all of it
        // revealed once the resolve has run (row `rows - 1`: none masked).
        assert_eq!(resolve_front(0.0, 0.0, 40), -1.0);
        assert_eq!(resolve_front(RESOLVE_MS, 0.0, 40), 39.0);
        assert_eq!(resolve_front(RESOLVE_MS * 2.0, 0.0, 40), 39.0);
    }

    #[test]
    fn a_later_phase_lags_but_still_finishes() {
        let early = resolve_front(RESOLVE_MS * 0.5, 0.0, 40);
        let late = resolve_front(RESOLVE_MS * 0.5, 0.2, 40);
        assert!(late < early);
        assert_eq!(resolve_front(RESOLVE_MS, 0.2, 40), 39.0);
    }

    #[test]
    fn the_front_only_ever_moves_down() {
        let mut prev = f32::MIN;
        for ms in (0..=1200).step_by(25) {
            let f = resolve_front(ms as f32, 0.1, 30);
            assert!(f >= prev);
            prev = f;
        }
    }

    #[test]
    fn the_hold_releases_once_the_settle_time_has_passed() {
        let t0 = Instant::now();
        let until = t0 + Duration::from_millis(SETTLE_MS);
        assert!(!hold_released(t0, until));
        assert!(hold_released(until, until));
        assert!(hold_released(until + Duration::from_millis(1), until));
    }
}
