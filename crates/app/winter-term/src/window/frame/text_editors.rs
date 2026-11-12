//! The text tabs, drawn by misty-viento's `EditorView` in their panes in
//! the terminal's font: the frame leaves a text tab's body to it, the
//! highlighter's styles resolved against Winter's theme. The pointer over
//! a text tab and an input method's text while one has the focus go to
//! its view straight from misty; keys come through the window's dispatch.

use misty::cell::Attributes;
use misty::color::Rgb;
use misty::{EventCx, Font, Frame, Input, Rect};
use winter_emulator::{Color, Style};
use winter_model::layout::TabId;
use winter_text::{HighlightStyle, TextEditor};

use crate::render::Theme;
use crate::window::frame::frame_grid::span_style;
use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the text tabs misty-viento draws
// ========================================================================

impl App {
    /// Draw every text tab a pane shows over `frame`, in its pane's body.
    pub(in crate::window) fn draw_text_editors(&mut self, frame: &mut Frame) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let theme = misty_theme(renderer.theme());
        let styles = highlight_styles(renderer.theme());
        let (size, family, line_height, ligatures) = renderer.grid_font();
        let font = Font::new(size)
            .with_line_height(line_height)
            .with_ligatures(ligatures);
        let font = match family {
            Some(family) => font.with_family(family),
            None => font,
        };
        for (tab, body) in self.shown_text_editors() {
            let Some(slot) = self.open_tabs.page_mut(&tab) else {
                continue;
            };
            slot.page.with_kind_mut(|text: &mut TextEditor| {
                text.draw(frame, body, &theme, &font, styles.clone());
            });
        }
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
        let target = self
            .shown_text_editors()
            .into_iter()
            .find(|(tab, body)| match input {
                Input::Ime(_) => focused == Some(*tab),
                Input::PointerButton(button) => body.contains(button.position),
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

    /// The text tabs a pane shows, each with the body it is drawn in.
    fn shown_text_editors(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects()
            .into_iter()
            .filter(|(tab, _)| {
                self.open_tabs
                    .page(tab)
                    .is_some_and(|slot| slot.page.with_kind(|_: &TextEditor| ()).is_some())
            })
            .map(|(tab, body)| (tab, Rect::new(body.x, body.y, body.width, body.height)))
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How each style the highlighter numbered is drawn under `theme`, by its
/// number: its colors and weight as the window colors a run of source.
fn highlight_styles(theme: &Theme) -> Vec<HighlightStyle> {
    winter_syntax::highlight_styles()
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
