//! Tests for inline image protocol handling

use super::*;
use wezterm_cell::image::ImageDataType;
use wezterm_surface::change::ImageData;

/// A tiny but valid 11x11 PNG, base64 encoded.
/// Taken from the reproduction in <https://github.com/wezterm/wezterm/issues/6344>.
const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAsAAAALCAYAAACprHcmAAAACXBIWXMAAAGKAAABigEzlzBYAAAAOUlEQVQYlZXOwQ0AMAzCQEdi7yaT0xWAN7JuDCac2PQKYxflycOoICOKtPIuqFCg4/LzKxiz6xjyAYh9DR1sLUN1AAAAAElFTkSuQmCC";

/// Feeding a Kitty graphics escape that requests a zero-sized placement (here `r=0,h=0`)
/// must not panic the terminal.
/// Prior to the fix for <https://github.com/wezterm/wezterm/issues/6344> this divided by zero
/// while computing the per-cell pixel deltas and took down the whole pane.
#[test]
fn kitty_zero_dimension_image_does_not_panic() {
    let mut term = TestTerm::new(3, 10, 0);

    // a=T: transmit and display, t=d: data is directly embedded,
    // f=100: PNG, r=0/h=0: zero rows / zero source height.
    let seq = format!("\x1b_Gr=0,h=0,a=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.print(seq.as_bytes());

    // The image is refused, so the cursor never moved;
    // Printing normal text and observing it confirms we recovered rather than crashing.
    term.print(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["ok", "", ""]);
}

/// A well-formed Kitty graphic with non-zero dimensions should continue to be accepted.
/// The test passes as long as processing the image does not panic and the terminal remains usable.
#[test]
fn kitty_valid_image_is_accepted() {
    let mut term = TestTerm::new(3, 10, 0);

    let seq = format!("\x1b_Ga=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.print(seq.as_bytes());

    // Printing normal text and observing it shifted confirms the terminal is usable.
    term.print(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["  ok", "", ""]);
}

/// When the pty has no pixel size, `cell_pixel_width`/`cell_pixel_height` are zero.
/// Displaying an image sized in cells (ie: without explicit `c=`/`r=`) must not divide by zero.
/// This is a distinct crash from the zero-dimension image above and is not caught by that guard.
/// See <https://github.com/wezterm/wezterm/issues/6344>.
#[test]
fn kitty_image_with_zero_pixel_dimensions_does_not_panic() {
    let mut term = Terminal::new(
        TerminalSize {
            rows: 3,
            cols: 80,
            // No pixel size!
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        },
        Arc::new(TestTermConfig { scrollback: 0 }),
        "WezTerm",
        "O_o",
        Box::new(Vec::new()),
    );

    // No `c=`/`r=`, so the placement is computed from the (zero) cell pixel
    // size, exercising the divide that previously panicked.
    let seq = format!("\x1b_Ga=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.advance_bytes(seq.as_bytes());

    // The image is refused, so the cursor never moved;
    // Printing normal text and observing it confirms we recovered rather than crashing.
    term.advance_bytes(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["ok", "", ""]);
}

/// A 2x2 RGBA image, base64 encoded.
const TINY_RGBA_BASE64: &str = "AQID/wQFBv8HCAn/CgsM/w==";

/// The image data attached to the first cell that carries one.
fn first_image(term: &TestTerm) -> Arc<ImageData> {
    for line in term.screen().visible_lines().iter() {
        for cell in line.visible_cells() {
            if let Some(im) = cell
                .attrs()
                .images()
                .and_then(|images| images.into_iter().next())
            {
                return Arc::clone(im.image_data());
            }
        }
    }
    panic!("no image was attached to the screen");
}

/// An Rgba8 stores the hash of its pixels and `compute_hash` reports it without
/// recomputing, so a kitty frame transmission that edits those pixels in place
/// must leave the stored hash describing what is now there.
#[test]
fn kitty_frame_edit_keeps_the_stored_hash_current() {
    let mut term = TestTerm::new(3, 10, 0);

    // a=T: transmit and display, f=32: RGBA, s/v: 2x2 pixels.
    let seq = format!("\x1b_Ga=T,t=d,f=32,s=2,v=2,i=1;{}\x1b\\", TINY_RGBA_BASE64);
    term.advance_bytes(seq.as_bytes());
    let transmitted_hash = first_image(&term).data().compute_hash();

    // a=f: transmit a frame, r=1: edit frame 1 in place, painting the top left
    // pixel opaque red over the 0x01,0x02,0x03 it arrived with.
    term.advance_bytes(b"\x1b_Ga=f,t=d,f=32,s=1,v=1,i=1,r=1,x=0,y=0;/wAA/w==\x1b\\");

    let image = first_image(&term);
    let image = image.data();
    let edited_hash = image.compute_hash();

    match &*image {
        ImageDataType::Rgba8 { data, .. } => {
            // Asserted on the pixels rather than on the hash, so a blit that
            // did nothing cannot be mistaken for a hash that went stale.
            k9::assert_equal!(&data[0..4], &[0xff, 0x00, 0x00, 0xff][..]);
            k9::assert_equal!(edited_hash, ImageDataType::hash_bytes(data));
        }
        other => panic!("expected Rgba8, got {:?}", other),
    }

    assert_ne!(edited_hash, transmitted_hash);
}

/// A pty writer the test can read back: what the terminal answers to an escape.
#[derive(Clone, Default)]
struct Answers(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Answers {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What a fresh terminal writes back after being fed `seq`.
fn answer_to(seq: &str) -> String {
    let answers = Answers::default();
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
        Box::new(answers.clone()),
    );
    // The terminal writes through a background thread, so a reply may land after
    // `advance_bytes` returns. A device status report asked for last is answered
    // after every earlier reply: wait for it, then strip it.
    const STATUS_OK: &str = "\x1b[0n";
    term.advance_bytes(seq.as_bytes());
    term.advance_bytes(b"\x1b[5n");
    for _ in 0..400 {
        let text = String::from_utf8(answers.0.lock().unwrap().clone()).unwrap();
        if let Some(before) = text.strip_suffix(STATUS_OK) {
            return before.to_string();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no status report reply: the terminal did not answer in 2s");
}

/// A 1x1 RGB pixel (3 bytes of data).
const PIXEL: &str = "f=24,t=d,s=1,v=1;AAAA";

/// Kitty replies `OK` to a transmit that names its image with `i=`, not only
/// `I=`; programs probe graphics support by transmitting a pixel and waiting.
#[test]
fn kitty_transmit_with_an_image_id_is_acknowledged() {
    let seq = format!("\x1b_Gi=4294111295,{PIXEL}\x1b\\");
    std::assert_eq!(answer_to(&seq), "\x1b_Gi=4294111295;OK\x1b\\");
}

/// `q=1` asks for no `OK`.
#[test]
fn kitty_quiet_transmit_is_not_acknowledged() {
    let seq = format!("\x1b_Gq=1,i=7,{PIXEL}\x1b\\");
    std::assert_eq!(answer_to(&seq), "");
}

/// With neither `i=` nor `I=` there is nothing to name in a reply.
#[test]
fn kitty_transmit_without_an_id_is_not_acknowledged() {
    let seq = format!("\x1b_G{PIXEL}\x1b\\");
    std::assert_eq!(answer_to(&seq), "");
}

/// Standard base64 (RFC 4648, with padding) of `bytes`; the test crate has no base64 dependency.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// macOS rounds a shared-memory object up to whole pages, so a 4x4 RGBA frame
/// (64 bytes) is read back as 16384. The terminal must use only the pixels the
/// transmit describes (`s` x `v` x bytes per pixel), as Kitty does, not the padding.
#[test]
#[cfg(target_os = "macos")]
fn kitty_shared_memory_frame_ignores_the_padding_macos_adds() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    const HELPER: &str = "import _posixshmem,os,mmap,sys\n\
        n='/wzt_%d'%os.getpid()\n\
        fd=_posixshmem.shm_open(n,os.O_CREAT|os.O_RDWR,0o600)\n\
        os.ftruncate(fd,64)\n\
        m=mmap.mmap(fd,64)\n\
        m[:64]=bytes(range(64))\n\
        print(n,flush=True)\n\
        sys.stdin.read()\n";
    let mut child = match Command::new("python3")
        .args(["-I", "-c", HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            eprintln!("skipped: python3 is needed to create shared memory: {err}");
            return;
        }
    };
    let mut name = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut name)
        .unwrap();
    let seq = format!(
        "\x1b_Gf=32,t=s,s=4,v=4,i=9;{}\x1b\\",
        base64(name.trim().as_bytes())
    );
    std::assert_eq!(answer_to(&seq), "\x1b_Gi=9;OK\x1b\\");
    drop(child.stdin.take());
    child.wait().unwrap();
}

/// The content hash of the one image a terminal holds after being fed an RGBA
/// transmit of `side` x `side` zero pixels.
fn hash_after_transmitting(side: usize) -> [u8; 32] {
    let mut term = TestTerm::new(3, 10, 0);
    let pixels = vec![0u8; side * side * 4];
    let seq = format!(
        "\x1b_Ga=T,t=d,f=32,s={side},v={side},i=1;{}\x1b\\",
        base64(&pixels)
    );
    term.advance_bytes(seq.as_bytes());
    first_image(&term).data().compute_hash()
}

/// Hashing every frame costs more than it saves for programs that stream big
/// frames (a browser in a pane): each one is new, so a content id never matches.
/// Frames of 1 MiB or more therefore get a cheap unique id instead.
#[test]
fn kitty_streamed_frames_are_not_content_hashed() {
    // 512 x 512 x 4 bytes = 1 MiB: identical pixels, yet distinct ids.
    std::assert_ne!(hash_after_transmitting(512), hash_after_transmitting(512));
}

/// Small images keep the content hash, so the same picture shares one texture.
#[test]
fn kitty_small_images_are_still_content_hashed() {
    std::assert_eq!(hash_after_transmitting(64), hash_after_transmitting(64));
}
