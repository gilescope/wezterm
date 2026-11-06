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
    pane_id: PaneId,
    cols: usize,
    rows: usize,
    /// New screen graphemes, row-major (`row * cols + col`).
    cells: Vec<String>,
    /// Outgoing screen graphemes, same layout. Empty when there was no
    /// comparable previous screen (then the dissolve phase is skipped).
    old_cells: Vec<String>,
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
        let mut depth = vec![-1i32; cols];
        let prev = prev_pane.as_ref().map(capture_screen);
        let old_cells = match prev {
            Some((pc, pr, pcells)) if pc == cols && pr == rows => {
                for c in 0..cols {
                    for r in 0..rows {
                        if pcells[r * cols + c] != cells[r * cols + c] {
                            depth[c] = r as i32;
                        }
                    }
                }
                pcells
            }
            _ => {
                // No comparable previous screen: rebuild every column fully and
                // skip the dissolve phase (no old grid to fall away).
                for d in depth.iter_mut() {
                    *d = rows as i32 - 1;
                }
                Vec::new()
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
            pane_id: new_pane.pane_id(),
            cols,
            rows,
            cells,
            old_cells,
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
            LinearRgba::with_components(0.0, 0.03, 0.01, 0.9 * strength),
        )?;

        self.paint_egg(
            layers,
            &mut glyph_cache,
            &card.glyphs,
            &mut shape_budget,
            egg,
            phase,
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
        let start = match &self.tab_switch_anim {
            Some(anim) => anim.start,
            None => return Ok(()),
        };

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
                        // Settled quote character in bright white-green.
                        let color = LinearRgba::with_components(0.85, 1.0, 0.88, 1.0);
                        let glyph = char::from_u32(code).unwrap_or('\u{2588}');
                        let mut buf = [0u8; 4];
                        let g = glyph.encode_utf8(&mut buf);
                        if let Some(info) = self.memo_glyph(memo, g, shape_budget) {
                            self.emit_matrix_glyph(
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
                                rr as f32,
                                color,
                                1.0,
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
