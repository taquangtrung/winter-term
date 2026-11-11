//! The which-key popup, drawn by misty: the keys that may follow a pending
//! prefix and what each does, along the bottom of the pane the keys go to.
//! Winter times the hint and names the keys itself; a click on an entry
//! types its key, and a click outside puts the popup away.

use misty::{EventCx, Frame, Input, Rect, WhichKeyEntry, WhichKeyResponse, Widget};
use winter_render::WhichKeyView;

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the which-key popup misty draws
// ========================================================================

impl App {
    /// Draw the which-key popup the last frame worked out over `frame`, or
    /// put it away when there is none.
    pub(in crate::window) fn draw_which_key(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let Some(view) = &self.chrome.drawn_which_key else {
            self.chrome.which_key.reset();
            return;
        };
        let entries = view
            .items
            .iter()
            .map(|(key, title)| WhichKeyEntry::written(key.as_str(), title.as_str(), false))
            .collect();
        self.chrome.which_key.show(view.title.as_str(), entries);
        let area = match view.anchor_rect {
            Some((x, y, width, height)) => Rect::new(x, y, width, height),
            None => {
                let pane = self.viewport_rect();
                Rect::new(pane.x, pane.y, pane.width, pane.height)
            }
        };
        self.chrome.which_key.draw(frame, area, &theme);
    }

    /// Hand pointer `input` to the which-key popup; whether it took it.
    pub(in crate::window) fn point_at_which_key(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        match self.chrome.which_key.handle(input, cx) {
            WhichKeyResponse::Chosen(index) => {
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
            WhichKeyResponse::Dismissed => {
                self.dismiss_which_key_popup();
                self.dirty = true;
                true
            }
            WhichKeyResponse::Handled => true,
            WhichKeyResponse::Ignored => false,
        }
    }
}
