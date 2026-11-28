//! The text tabs, drawn by Cherry's `EditorView` in their panes in
//! the terminal's font: the frame leaves a text tab's body to it, the
//! highlighter's styles resolved against Winter's theme. The pointer over
//! a text tab and an input method's text while one has the focus go to
//! its view straight from misty; keys come through the window's dispatch.

use std::time::Instant;

use misty::grid::Attributes;
use misty::{EventCx, Frame, Input, Rect, Rgb};
use winter_emulator::{Color, Style};
use winter_model::layout::TabId;
use winter_text::{HighlightStyle, TextEditor};

use crate::render::{PANE_H_PAD, SCROLLBAR_WIDTH, Theme};
use crate::window::App;
use crate::window::frame::frame_grid::span_style;

// ========================================================================
// App: the text tabs Cherry draws
// ========================================================================

impl App {
    /// Draw every text tab a pane shows over `frame`, in its pane's body.
    pub(in crate::window) fn draw_text_editors(&mut self, frame: &mut Frame) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let theme = renderer.misty_theme();
        let styles = highlight_styles(renderer.theme());
        let font = renderer.grid_font();
        // The cursor is solid only in the focused pane of a focused window.
        let keyboard = self.active_pane().filter(|_| self.input.focus.window);
        for (tab, body) in self.shown_text_editors() {
            let focused = keyboard == Some(tab);
            // The window drew the pane's scrollbar under this frame, so the
            // view stops short of it rather than painting over it.
            let bar = PANE_H_PAD + SCROLLBAR_WIDTH;
            let body = Rect::new(body.x, body.y, (body.width - bar).max(0.0), body.height);
            let Some(slot) = self.open_tabs.page_mut(&tab) else {
                continue;
            };
            slot.page.with_kind_mut(|text: &mut TextEditor| {
                text.draw(frame, body, theme, &font, styles.clone(), focused);
            });
        }
    }

    /// When a text tab shown next needs drawing for its scroll's glide.
    pub(in crate::window) fn text_editors_deadline(&self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.shown_text_editors()
            .into_iter()
            .filter_map(|(tab, _)| {
                let slot = self.open_tabs.page(&tab)?;
                slot.page.with_kind(|text: &TextEditor| text.glide(cx))?
            })
            .min()
    }

    /// Hand `input` to a text tab's view: the pointer while over its body,
    /// an input method's text while it has the focus. Whether it took it;
    /// what it asks of the window is carried out at once.
    pub(in crate::window) fn offer_to_text_editors(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        let focused = self.active_pane();
        let shown = self.shown_text_editors();
        if let Input::PointerMoved(_) | Input::PointerLeft = input {
            self.hover_text_editors(&shown, focused, input, cx);
        }
        // Scrollbar and divider drags belong to the window, never to the text.
        let dragging = self.input.pointer.scrollbar_drag.is_some()
            || self.input.pointer.divider_drag.is_some();
        let rects = self.pane_rects();
        let viewport = self.viewport_rect();
        let on_scrollbar =
            |tab: TabId, x: f32, y: f32| self.hovered_scrollbar_pane_at(x, y, &rects) == Some(tab);
        let target = shown.into_iter().find(|(tab, body)| match input {
            _ if dragging && !matches!(input, Input::Ime(_)) => false,
            Input::Ime(_) => focused == Some(*tab),
            Input::PointerButton(button) => {
                let at = button.position;
                let on_divider =
                    button.pressed && self.layout().divider_at(at.x, at.y, viewport).is_some();
                body.contains(at) && !on_scrollbar(*tab, at.x, at.y) && !on_divider
            }
            Input::Wheel(wheel) => body.contains(wheel.position),
            Input::PointerMoved(_) => focused == Some(*tab),
            _ => false,
        });
        let Some((tab, _)) = target else {
            return false;
        };
        let outcome = self.open_tabs.page_mut(&tab).and_then(|slot| {
            slot.page
                .with_kind_mut(|text: &mut TextEditor| text.handle_input(input, cx))
        });
        let Some(outcome) = outcome else {
            return false;
        };
        if outcome == winter_model::editor::EditorOutcome::Ignored {
            return false;
        }
        if matches!(input, Input::PointerButton(button) if button.pressed) {
            self.set_focus(tab);
        }
        self.act_on_editor_outcome(tab, outcome);
        self.dirty = true;
        // The pointer moving is still Winter's, for its hovers.
        !matches!(input, Input::PointerMoved(_))
    }

    /// Hand the pointer moving, or leaving the window, to a text tab it is
    /// over that has no focus (the focused one hears every move, for its
    /// drags), for its gutter's tooltips; and tell the one it left.
    fn hover_text_editors(
        &mut self,
        shown: &[(TabId, Rect)],
        focused: Option<TabId>,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) {
        let over = match input {
            Input::PointerMoved(point) => shown
                .iter()
                .find(|(_, body)| body.contains(*point))
                .map(|(tab, _)| *tab),
            _ => None,
        };
        let left = std::mem::replace(&mut self.input.pointer.over_text, over);
        let told = |app: &mut Self, tab: TabId, input: &Input, cx: &mut EventCx<'_>| {
            if let Some(slot) = app.open_tabs.page_mut(&tab) {
                slot.page
                    .with_kind_mut(|text: &mut TextEditor| text.handle_input(input, cx));
            }
        };
        if let Some(left) = left.filter(|left| Some(*left) != over && Some(*left) != focused) {
            told(self, left, &Input::PointerLeft, cx);
        }
        if let Some(over) = over.filter(|over| Some(*over) != focused) {
            told(self, over, input, cx);
        }
    }

    /// The text tabs a pane shows, each with the body it is drawn in.
    fn shown_text_editors(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects()
            .into_iter()
            .filter(|(tab, _)| {
                self.open_tabs
                    .page(tab)
                    .is_some_and(|slot| slot.page.with_kind(|_: &TextEditor| ()).is_some())
            })
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How each style the highlighter numbered is drawn under `theme`, by its
/// number: its colors and weight as the window colors a run of source.
fn highlight_styles(theme: &Theme) -> Vec<HighlightStyle> {
    winter_text::syntax::highlight_styles()
        .into_iter()
        .map(|style| highlight_style(&span_style(style, theme), theme))
        .collect()
}

/// `style`, a cell's as the window resolves a run, as a highlight style.
fn highlight_style(style: &Style, theme: &Theme) -> HighlightStyle {
    let flags = [
        (style.bold, Attributes::BOLD),
        (style.italic, Attributes::ITALIC),
        (style.faint, Attributes::DIM),
        (style.strikethrough, Attributes::STRIKETHROUGH),
    ];
    let attributes = flags
        .into_iter()
        .filter(|(set, _)| *set)
        .fold(Attributes::NONE, |all, (_, flag)| all | flag);
    HighlightStyle {
        attributes,
        background: rgb_of(style.background, theme),
        foreground: rgb_of(style.foreground, theme),
    }
}

/// `color` as an RGB color, an indexed one from `theme`'s palette; `None`
/// for the default.
fn rgb_of(color: Color, theme: &Theme) -> Option<Rgb> {
    match color {
        Color::Default => None,
        Color::Indexed(index) => theme
            .ansi
            .get(usize::from(index))
            .map(|c| Rgb::new(c.r, c.g, c.b)),
        Color::Rgb(c) => Some(Rgb::new(c.r, c.g, c.b)),
    }
}
