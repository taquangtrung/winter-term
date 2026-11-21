//! The which-key popup, drawn by misty: the keys that may follow a pending
//! prefix and what each does, centred on the pane the keys go to, an
//! option's `[x]` or `[ ]` drawn as a check box.
//! Winter times the hint and names the keys itself; a click on an entry
//! types its key, and a click outside puts the popup away.

use crate::render::WhichKeyView;
use misty::{EventCx, Frame, Input, Rect, WhichKeyEntry, WhichKeyResponse, Widget};

use crate::window::App;
use crate::window::misty_theme::misty_theme;

// ========================================================================
// App: the which-key popup misty draws
// ========================================================================

impl App {
    /// Draw the which-key popup the last frame worked out over `frame`, or
    /// put it away when there is none.
    pub(in crate::window) fn draw_which_key(&mut self, frame: &mut Frame) {
        let theme = misty_theme(self.renderer.as_ref());
        let Some(view) = &self.chrome.drawn_which_key else {
            self.chrome.which_key.reset();
            return;
        };
        let entries = view
            .items
            .iter()
            .map(|(key, title)| which_key_entry(key, title))
            .collect();
        self.chrome.which_key.show(view.title.as_str(), entries);
        let area = match view.anchor_rect {
            Some((x, y, width, height)) => Rect::new(x, y, width, height),
            None => self.viewport_rect(),
        };
        self.chrome.which_key.draw(frame, area, theme);
    }

    /// Hand pointer `input` to the which-key popup; whether it took it.
    pub(in crate::window) fn point_at_which_key(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        match self.chrome.which_key.handle(input, cx) {
            WhichKeyResponse::Activated(index) => {
                let key = self
                    .chrome
                    .drawn_which_key
                    .as_ref()
                    .and_then(|view: &WhichKeyView| view.items.get(index))
                    .map(|(key, _)| key.clone());
                if let Some(key) = key {
                    self.click_which_key(&key);
                }
                self.dirty = true;
                true
            }
            WhichKeyResponse::Closed => {
                self.dismiss_which_key_popup();
                self.dirty = true;
                true
            }
            WhichKeyResponse::Handled => true,
            WhichKeyResponse::Ignored => false,
            // A response misty adds later is none the popup acts on.
            _ => false,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The entry for `key` titled `title`: a check box for an option whose
/// title starts `[x] ` (on) or `[ ] ` (off), the mark taken off the title.
fn which_key_entry(key: &str, title: &str) -> WhichKeyEntry {
    let (checked, title) = match (title.strip_prefix("[x] "), title.strip_prefix("[ ] ")) {
        (Some(rest), _) => (Some(true), rest),
        (None, Some(rest)) => (Some(false), rest),
        (None, None) => (None, title),
    };
    let entry = WhichKeyEntry::written(key, title, false);
    match checked {
        Some(checked) => entry.with_checked(checked),
        None => entry,
    }
}
