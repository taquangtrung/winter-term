//! The pages misty draws as views in their panes ([`Page::is_view`]): the
//! frame leaves a view page's body to it, and it takes the pointer over
//! its body and the keys while its tab has the focus before Winter's own
//! dispatch does, which gets what it leaves. What it asks of the window
//! goes the way every page's requests go.
//!
//! [`Page::is_view`]: winter_model::editor::Page::is_view

use std::time::Instant;

use misty::{AccessAction, EventCx, Frame, Input, Rect};

use winter_model::layout::TabId;

use crate::window::App;
use crate::window::appearance::ui_theme;

// ========================================================================
// App: the pages misty draws
// ========================================================================

impl App {
    /// Do what a screen reader's `action` asks of the page it names among
    /// those drawn as views, carrying out what came of it as a press would,
    /// a click putting the keyboard in that pane; whether a page took it.
    pub(in crate::window) fn page_views_access_action(&mut self, action: AccessAction) -> bool {
        for (tab, _) in self.shown_page_views() {
            let outcome = self
                .open_tabs
                .page_mut(&tab)
                .and_then(|slot| slot.page.view_access_action(action));
            if let Some(outcome) = outcome {
                if let AccessAction::Click(_) = action {
                    self.set_focus(tab);
                }
                self.act_on_editor_outcome(tab, outcome);
                return true;
            }
        }
        false
    }

    /// Each tab a pane shows that is a view, and the body it is drawn in.
    fn shown_page_views(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects()
            .into_iter()
            .filter(|(tab, _)| {
                self.open_tabs
                    .page(tab)
                    .is_some_and(|slot| slot.page.is_view())
            })
            .collect()
    }

    /// Draw every view page a pane shows over `frame`, in its pane's body.
    pub(in crate::window) fn draw_page_views(&mut self, frame: &mut Frame) {
        let theme = ui_theme(self.renderer.as_ref());
        for (tab, body) in self.shown_page_views() {
            if let Some(slot) = self.open_tabs.page_mut(&tab) {
                slot.page.draw_view(frame, body, theme);
            }
        }
    }

    /// Hand `input` to the view page it is for: a key while its tab has
    /// the focus, the pointer while over its body. Whether it took it; what
    /// it asks of the window is carried out at once.
    pub(in crate::window) fn offer_to_page_views(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        let pointer = matches!(
            input,
            Input::PointerButton(_) | Input::PointerMoved(_) | Input::PointerLeft | Input::Wheel(_)
        );
        if pointer && self.input.pointer.divider_drag.is_some() {
            return false;
        }
        if let Input::PointerButton(button) = input
            && button.pressed
            && self
                .layout()
                .divider_at(button.position.x, button.position.y, self.viewport_rect())
                .is_some()
        {
            return false;
        }
        let focused = self.active_pane();
        let shown = self.shown_page_views();
        // A page the pointer left hears so, for its hovers and tooltip.
        if let Input::PointerMoved(_) | Input::PointerLeft = input {
            let over = match input {
                Input::PointerMoved(point) => shown
                    .iter()
                    .find(|(_, body)| body.contains(*point))
                    .map(|(tab, _)| *tab),
                _ => None,
            };
            let left = std::mem::replace(&mut self.input.pointer.over_page, over);
            if let Some(left) = left.filter(|left| Some(*left) != over)
                && let Some(slot) = self.open_tabs.page_mut(&left)
            {
                slot.page.view_input(&Input::PointerLeft, cx);
                self.dirty = true;
            }
        }
        let target = shown.into_iter().find(|(tab, body)| match input {
            Input::Key(_) | Input::KeyReleased(_) | Input::Ime(_) => focused == Some(*tab),
            Input::PointerButton(button) => body.contains(button.position),
            Input::Wheel(wheel) => body.contains(wheel.position),
            Input::PointerMoved(point) => body.contains(*point),
            _ => false,
        });
        let Some((tab, _)) = target else {
            return false;
        };
        let outcome = self
            .open_tabs
            .page_mut(&tab)
            .and_then(|slot| slot.page.view_input(input, cx));
        let Some(outcome) = outcome else {
            return false;
        };
        if matches!(input, Input::PointerButton(button) if button.pressed) {
            self.set_focus(tab);
        }
        self.act_on_editor_outcome(tab, outcome);
        self.dirty = true;
        // The pointer moving is still Winter's, for its hovers.
        !matches!(input, Input::PointerMoved(_))
    }

    /// When the view pages shown next need drawing for an animation or a
    /// timer of theirs.
    pub(in crate::window) fn page_views_deadline(
        &mut self,
        cx: &mut EventCx<'_>,
    ) -> Option<Instant> {
        self.shown_page_views()
            .into_iter()
            .filter_map(|(tab, _)| self.open_tabs.page_mut(&tab)?.page.view_about_to_wait(cx))
            .min()
    }
}
