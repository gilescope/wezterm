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
use crate::quad::QuadTrait;
use crate::quad::{TripleLayerQuadAllocator, TripleLayerQuadAllocatorTrait};
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
const DURATION_MS: f32 = 4200.0;
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
    /// `None` records a grapheme that has no drawable glyph (blank/unshaped).
    glyphs: RefCell<HashMap<String, Option<GlyphInfo>>>,
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
        });
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    /// Look up a real grapheme's glyph from the per-animation memo, shaping it
    /// on first sight (subject to a per-frame budget). Returns `None` for
    /// blanks, undrawable glyphs, or when the budget is spent this frame.
    fn memo_glyph(&self, anim: &TabSwitchAnim, s: &str, budget: &mut i32) -> Option<GlyphInfo> {
        if s.is_empty() || s == " " {
            return None;
        }
        if !anim.glyphs.borrow().contains_key(s) {
            if *budget <= 0 {
                return None;
            }
            let shaped = self.shape_grapheme(s);
            anim.glyphs.borrow_mut().insert(s.to_string(), shaped);
            *budget -= 1;
        }
        anim.glyphs.borrow().get(s).and_then(|o| o.clone())
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
        let tab_pos = self.resolved_tab_bar_position();
        let tab_bar_height = if self.show_tab_bar && !tab_pos.is_vertical() {
            self.tab_bar_pixel_height().unwrap_or(0.)
        } else {
            0.
        };
        let tab_bar_width = if self.show_tab_bar {
            self.tab_bar_pixel_width()
        } else {
            0.
        };
        let top_bar_height = if tab_pos == config::TabBarPosition::Top {
            tab_bar_height
        } else {
            0.
        };
        let left_bar_width = if tab_pos == config::TabBarPosition::Left {
            tab_bar_width
        } else {
            0.
        };
        let border = self.get_os_border();
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let x = left_bar_width + padding_left + border.left.get() as f32 + pane_left as f32 * cell_w;
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
        quad.set_position(gx - left_off, gy - top_off, gx + gw - left_off, gy + gh - top_off);
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
                    if let Some(info) = self.memo_glyph(anim, &s, &mut shape_budget) {
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
                if let Some(info) = self.memo_glyph(anim, &s, &mut shape_budget) {
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

        Ok(())
    }
}
