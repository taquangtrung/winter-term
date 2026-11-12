//! The Settings page, drawn by misty in its tab's pane: the frame leaves
//! the pane's body to it, and it takes the keys while its tab has the
//! focus and the pointer over it. What it asks of the window goes the way
//! every page's requests go.

use misty::{EventCx, Frame, Input, Rect};

use winter_model::editor::EditorOutcome;
use winter_model::layout::TabId;

use crate::editors::settings::SettingsEditor;
use crate::window::editor::EditorKind;
use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the settings page misty draws
// ========================================================================

impl App {
    /// The Settings tab while a pane shows it, and the body it is drawn in.
    pub(in crate::window) fn shown_settings(&self) -> Option<(TabId, Rect)> {
        let tab = self.page_tab(EditorKind::Settings)?;
        let (_, body) = self
            .pane_rects()
            .into_iter()
            .find(|(shown, _)| *shown == tab)?;
        Some((tab, Rect::new(body.x, body.y, body.width, body.height)))
    }

    /// Draw the Settings page over `frame`, where a pane shows it.
    pub(in crate::window) fn draw_settings(&mut self, frame: &mut Frame) {
        let Some((tab, body)) = self.shown_settings() else {
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        if let Some(slot) = self.open_tabs.page_mut(&tab) {
            slot.page
                .with_kind_mut(|page: &mut SettingsEditor| page.draw(frame, body, &theme));
        }
    }

    /// Hand `input` to the Settings page: a key while its tab has the
    /// focus, the pointer while over it. Whether it took it; what it asks
    /// of the window is carried out at once.
    pub(in crate::window) fn offer_to_settings(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        let Some((tab, body)) = self.shown_settings() else {
            return false;
        };
        let wanted = match input {
            Input::Key(_) | Input::Ime(_) => self.active_pane() == Some(tab),
            Input::PointerButton(button) => body.contains(button.position),
            Input::Wheel(wheel) => body.contains(wheel.position),
            Input::PointerMoved(point) => body.contains(*point),
            _ => false,
        };
        if !wanted {
            return false;
        }
        let took = self.open_tabs.page_mut(&tab).and_then(|slot| {
            slot.page
                .with_kind_mut(|page: &mut SettingsEditor| page.handle(input, cx))
        });
        if took != Some(true) {
            return false;
        }
        if matches!(input, Input::PointerButton(button) if button.pressed) {
            self.set_focus(tab);
        }
        self.act_on_editor_outcome(tab, EditorOutcome::Consumed);
        self.dirty = true;
        // The pointer moving is still Winter's, for its hovers.
        !matches!(input, Input::PointerMoved(_))
    }
}
