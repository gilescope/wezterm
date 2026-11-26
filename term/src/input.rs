// clippy hates bitflags
#![allow(clippy::suspicious_arithmetic_impl, clippy::redundant_field_names)]

use super::VisibleRowIndex;
#[cfg(feature = "use_serde")]
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use wezterm_dynamic::{FromDynamic, ToDynamic};

pub use termwiz::input::{KeyCode, Modifiers as KeyModifiers};

#[cfg_attr(feature = "use_serde", derive(Deserialize, Serialize))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash, FromDynamic, ToDynamic)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp(usize),
    WheelDown(usize),
    WheelLeft(usize),
    WheelRight(usize),
    None,
}

#[cfg_attr(feature = "use_serde", derive(Deserialize, Serialize))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Move,
}

#[cfg_attr(feature = "use_serde", derive(Deserialize, Serialize))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub x: usize,
    pub y: VisibleRowIndex,
    pub x_pixel_offset: isize,
    pub y_pixel_offset: isize,
    pub button: MouseButton,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ClickPosition {
    pub column: usize,
    pub row: i64,
    pub x_pixel_offset: isize,
    pub y_pixel_offset: isize,
}

/// This is a little helper that keeps track of the "click streak",
/// which is the number of successive clicks of the same mouse button
/// within the `CLICK_INTERVAL`.  The streak is reset to 1 each time
/// the mouse button differs from the last click, or when the elapsed
/// time exceeds `CLICK_INTERVAL`, or when the cursor position
/// changes to a different character cell.
#[derive(Debug, Clone)]
pub struct LastMouseClick {
    pub button: MouseButton,
    pub position: ClickPosition,
    time: Instant,
    pub streak: usize,
}

/// The multi-click interval, measured in milliseconds
const CLICK_INTERVAL: u64 = 500;

impl LastMouseClick {
    pub fn new(button: MouseButton, position: ClickPosition) -> Self {
        Self {
            button,
            position,
            time: Instant::now(),
            streak: 1,
        }
    }

    pub fn add(&self, button: MouseButton, position: ClickPosition) -> Self {
        let now = Instant::now();
        let streak = if button == self.button
            && position.column == self.position.column
            && position.row == self.position.row
            && now.duration_since(self.time) <= Duration::from_millis(CLICK_INTERVAL)
        {
            self.streak + 1
        } else {
            1
        };
        Self {
            button,
            position,
            time: now,
            streak,
        }
    }
}

/// DEC private mode by which a program asks for wheel movement as pixel distances
/// (`CSI ? 9278 h`), instead of one report per notch. Private to wezterm.
pub const PIXEL_WHEEL_MODE: u16 = 9278;

/// Wheel or trackpad movement in pixels, for a program that set `PIXEL_WHEEL_MODE`.
/// It is reported as `APC pixel-wheel ; x ; y ; dx ; dy ; ticks_x ; ticks_y ST`.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PixelWheel {
    /// The pointer, in pixels from the top left of the pane.
    pub x: usize,
    pub y: usize,
    /// Distance in pixels; positive is what a wheel-left or wheel-up notch reports.
    pub dx: f32,
    pub dy: f32,
    /// The whole notches the same movement makes without this mode, so a program
    /// can keep notch-counting behaviour (a swipe) alongside smooth scrolling.
    pub ticks_x: i16,
    pub ticks_y: i16,
}
