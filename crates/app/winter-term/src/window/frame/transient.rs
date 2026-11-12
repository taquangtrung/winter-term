//! The key menu a transient opens, drawn by misty: the card of switches and
//! commands centred in the focused pane, its row lit where Winter's keys
//! (or the pointer) put it. The card keeps where it was dragged and the
//! size it grew to while the menu is up; a click on a row types the row's
//! key, and a click outside puts the menu away.

use crate::render::TransientView;
use misty::{
    EventCx, Frame, Input, Rect, Transient, TransientCommand, TransientResponse, TransientRow,
    TransientSwitch, Widget,
};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the key menu misty draws
// ========================================================================

impl App {
    /// Draw the key menu the last frame worked out over `frame`: the card
    /// already up when it is the same menu, a new one otherwise.
    pub(in crate::window) fn draw_transient(&mut self, frame: &mut Frame) {
        let Some(view) = self.chrome.drawn_transient.clone() else {
            self.chrome.transient = None;
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let same = self.chrome.transient_title.as_deref() == Some(view.title.as_str())
            && self
                .chrome
                .transient
                .as_ref()
                .is_some_and(|card| card.switches().len() == view.switches.len());
        if !same {
            self.chrome.transient = Some(card_of(&view));
            self.chrome.transient_title = Some(view.title.clone());
        }
        let Some(card) = &mut self.chrome.transient else {
            return;
        };
        for (index, switch) in view.switches.iter().enumerate() {
            card.set_checked(index, switch.checked);
        }
        card.set_selected(row_of(view.selected, view.switches.len()));
        let area = match view.anchor_rect {
            Some((x, y, width, height)) => Rect::new(x, y, width, height),
            None => frame.bounds(),
        };
        card.draw(frame, area, &theme);
    }

    /// Hand pointer `input` to the key menu; whether it took it. A row
    /// clicked types its key; the row under the pointer is selected, as the
    /// arrows select it.
    pub(in crate::window) fn point_at_transient(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        let (Some(card), Some(view)) = (&mut self.chrome.transient, &self.chrome.drawn_transient)
        else {
            return false;
        };
        let answer = card.handle(input, cx);
        let selected = card
            .selected()
            .map(|row| index_of(row, view.switches.len()));
        let was = view.selected;
        let chosen = match answer {
            TransientResponse::Chosen(TransientRow::Command(at)) => {
                view.commands.get(at).map(|command| command.key.clone())
            }
            TransientResponse::Chosen(TransientRow::Switch(at)) => {
                view.switches.get(at).map(|switch| switch.key.clone())
            }
            TransientResponse::Closed | TransientResponse::Handled | TransientResponse::Ignored => {
                None
            }
        };
        match answer {
            TransientResponse::Chosen(_) => {
                if let Some(key) = chosen {
                    self.click_which_key(&key);
                }
            }
            TransientResponse::Closed => self.dismiss_which_key_popup(),
            TransientResponse::Handled => {
                let Some(row) = selected.filter(|row| *row != was) else {
                    return true;
                };
                let focused = self.layout().focused();
                if let Some(slot) = self.open_tabs.page_mut(&focused) {
                    slot.page.select_transient_row(row);
                }
            }
            TransientResponse::Ignored => return false,
        }
        self.dirty = true;
        true
    }

    /// Where the key menu's card was drawn last, as `(x, y, width,
    /// height)`, for the list a command opens to take its place.
    pub(in crate::window) fn transient_card_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let rect = self.chrome.transient.as_ref()?.card_rect()?;
        Some((rect.x, rect.y, rect.width, rect.height))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's card for Winter's menu `view`.
fn card_of(view: &TransientView) -> Transient {
    let switches = view
        .switches
        .iter()
        .map(|switch| {
            let shown = TransientSwitch::new(switch.key.as_str(), switch.label.as_str())
                .with_checked(switch.checked);
            let shown = match switch.flag.is_empty() {
                true => shown,
                false => shown.with_flag(switch.flag.as_str()),
            };
            match switch.kind {
                crate::render::SwitchKind::Checkbox => shown,
                crate::render::SwitchKind::Radio => shown.radio(),
            }
        })
        .collect();
    let commands = view
        .commands
        .iter()
        .map(|command| TransientCommand::new(command.key.as_str(), command.label.as_str()))
        .collect();
    Transient::new(view.title.as_str(), switches, commands)
}

/// The row at `index`, counting `switches` switches first.
fn row_of(index: usize, switches: usize) -> TransientRow {
    match index.checked_sub(switches) {
        Some(at) => TransientRow::Command(at),
        None => TransientRow::Switch(index),
    }
}

/// The index of `row`, counting `switches` switches first.
fn index_of(row: TransientRow, switches: usize) -> usize {
    match row {
        TransientRow::Command(at) => switches + at,
        TransientRow::Switch(at) => at,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rows_count_the_switches_first_both_ways() {
        assert_eq!(row_of(1, 2), TransientRow::Switch(1));
        assert_eq!(row_of(3, 2), TransientRow::Command(1));
        assert_eq!(index_of(TransientRow::Command(1), 2), 3);
        assert_eq!(index_of(row_of(0, 0), 0), 0);
    }
}
