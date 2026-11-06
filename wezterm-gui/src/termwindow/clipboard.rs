use crate::termwindow::TermWindowNotif;
use crate::TermWindow;
use config::keyassignment::{ClipboardCopyDestination, ClipboardPasteSource};
use mux::pane::Pane;
use mux::Mux;
use std::sync::Arc;
use window::{Clipboard, WindowOps};

/// CSI u encoding of a key held with super (cmd). The legacy encoding has
/// no way to say super and would type a bare `c`; termwiz's CSI u drops it
/// too (`Modifiers::encode_xterm` stops at ctrl). 9 is 1 + the super bit.
fn csi_u_super(c: char) -> String {
    format!("\x1b[{};9u", u32::from(c))
}

impl TermWindow {
    /// Copy with nothing selected while the app has grabbed the mouse: the
    /// selection the user sees is the app's (claude code keeps its own and
    /// copies it on cmd-c), so hand it the keystroke rather than wiping the
    /// clipboard with an empty string.
    pub fn copy_selection_or_forward(&self, pane: &Arc<dyn Pane>, dest: ClipboardCopyDestination) {
        let text = self.selection_text(pane);
        if text.is_empty() && pane.is_mouse_grabbed() {
            pane.writer().write_all(csi_u_super('c').as_bytes()).ok();
        } else {
            self.copy_to_clipboard(dest, text);
        }
    }

    pub fn copy_to_clipboard(&self, clipboard: ClipboardCopyDestination, text: String) {
        let clipboard = match clipboard {
            ClipboardCopyDestination::Clipboard => [Some(Clipboard::Clipboard), None],
            ClipboardCopyDestination::PrimarySelection => [Some(Clipboard::PrimarySelection), None],
            ClipboardCopyDestination::ClipboardAndPrimarySelection => [
                Some(Clipboard::Clipboard),
                Some(Clipboard::PrimarySelection),
            ],
        };
        for &c in &clipboard {
            if let Some(c) = c {
                self.window.as_ref().unwrap().set_clipboard(c, text.clone());
            }
        }
    }

    pub fn paste_from_clipboard(&mut self, pane: &Arc<dyn Pane>, clipboard: ClipboardPasteSource) {
        let pane_id = pane.pane_id();
        log::trace!(
            "paste_from_clipboard in pane {} {:?}",
            pane.pane_id(),
            clipboard
        );
        let window = self.window.as_ref().unwrap().clone();
        let clipboard = match clipboard {
            ClipboardPasteSource::Clipboard => Clipboard::Clipboard,
            ClipboardPasteSource::PrimarySelection => Clipboard::PrimarySelection,
        };
        let future = window.get_clipboard(clipboard);
        promise::spawn::spawn(async move {
            if let Ok(clip) = future.await {
                window.notify(TermWindowNotif::Apply(Box::new(move |myself| {
                    if let Some(pane) = myself
                        .pane_state(pane_id)
                        .overlay
                        .as_ref()
                        .map(|overlay| overlay.pane.clone())
                        .or_else(|| {
                            let mux = Mux::get();
                            mux.get_pane(pane_id)
                        })
                    {
                        pane.send_paste(&clip).ok();
                    }
                })));
            }
        })
        .detach();
        self.maybe_scroll_to_bottom_for_input(&pane);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn super_c_is_csi_u_with_the_super_bit() {
        // What claude code's key parser decodes as `cmd+c`.
        assert_eq!(csi_u_super('c'), "\x1b[99;9u");
    }
}
