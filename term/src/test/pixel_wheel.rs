//! Pixel wheel reports: `PIXEL_WHEEL_MODE`.

use super::*;

const WHEEL: PixelWheel = PixelWheel {
    x: 120,
    y: 45,
    dx: 0.,
    dy: -12.5,
    ticks_x: 0,
    ticks_y: -1,
};

/// A pty writer the test can read back.
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sent {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Feeds `setup` to a fresh terminal, offers it `WHEEL`, and returns whether it
/// took it and what it wrote to the program.
fn offer(setup: &str) -> (bool, String) {
    let sent = Sent::default();
    let mut term = Terminal::new(
        TerminalSize {
            rows: 3,
            cols: 10,
            pixel_width: 80,
            pixel_height: 48,
            dpi: 0,
        },
        Arc::new(TestTermConfig { scrollback: 0 }),
        "WezTerm",
        "O_o",
        Box::new(sent.clone()),
    );
    term.advance_bytes(setup.as_bytes());
    let taken = term.pixel_wheel(WHEEL).unwrap();
    // Replies go through a background thread: a status report asked for last is
    // answered after everything before it.
    const STATUS_OK: &str = "\x1b[0n";
    term.advance_bytes(b"\x1b[5n");
    for _ in 0..400 {
        let text = String::from_utf8(sent.0.lock().unwrap().clone()).unwrap();
        if let Some(before) = text.strip_suffix(STATUS_OK) {
            return (taken, before.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no status report reply: the terminal did not answer in 2s");
}

const GRAB: &str = "\x1b[?1003h\x1b[?1016h";

#[test]
fn a_program_that_asked_gets_the_distance_in_pixels() {
    let (taken, sent) = offer(&format!("{GRAB}\x1b[?9278h"));
    assert!(taken);
    k9::assert_equal!(sent, "\x1b_pixel-wheel;120;45;0.00;-12.50;0;-1\x1b\\");
}

#[test]
fn a_program_that_did_not_ask_gets_nothing() {
    k9::assert_equal!(offer(GRAB), (false, String::new()));
}

#[test]
fn a_program_not_taking_the_mouse_gets_nothing() {
    k9::assert_equal!(offer("\x1b[?9278h"), (false, String::new()));
}

#[test]
fn the_mode_can_be_turned_off() {
    let setup = format!("{GRAB}\x1b[?9278h\x1b[?9278l");
    k9::assert_equal!(offer(&setup), (false, String::new()));
}

/// A program that died without turning it off must not leave the next one
/// without wheel notches: a full reset clears it with the mouse modes.
#[test]
fn a_full_reset_turns_the_mode_off() {
    let setup = format!("{GRAB}\x1b[?9278h\x1bc{GRAB}");
    k9::assert_equal!(offer(&setup), (false, String::new()));
}
