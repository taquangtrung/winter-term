//! The dialog, a misty [`Dialog`]: a question with the field it is answered
//! in and its OK and Cancel, an error, or a confirmation with its buttons.
//! It opens on the card of the transient or editor that asked, else centred
//! on the window. misty draws it, lights the button under the pointer, and
//! hands back the button clicked; the window answers it as the button's key
//! would, so a click and a key take the same path.

use misty::{
    BoxStyle, Dialog, DialogButton, DialogResponse, EventCx, Frame, Input, Paragraph, Point, Rect,
    Rgba, Severity, StyledText, TextColor, Theme, ThemeToken, Widget,
};
use winter_model::input::{Key, KeyCode};

use crate::render::{InputView, NoticeKind};
use crate::window::App;
use crate::window::appearance::ui_theme;

// ========================================================================
// Constants
// ========================================================================

/// The title a confirmation without one of its own takes.
const CONFIRM_TITLE: &str = "Confirmation";

/// The title a warning without one of its own takes.
const WARNING_TITLE: &str = "Warning";

/// The title an error without one of its own takes.
const ERROR_TITLE: &str = "Error";

/// What a question's button that answers it says.
const OK_LABEL: &str = "OK";

/// What a question's button that cancels it says.
const CANCEL_LABEL: &str = "Cancel";

/// The cell height the dialog's sizes below were chosen at; they scale
/// with the window's font from it.
const REFERENCE_CELL_HEIGHT: f32 = 17.0;

/// The room kept between an editor's card and its pane's edges, in pixels
/// at the reference cell height.
const EDGE_ROOM: f32 = 40.0;

/// An editor's card: its share of the pane and its least and most size, in
/// pixels at the reference cell height.
const EDITOR_CARD_WIDTH_SHARE: f32 = 0.6;
const EDITOR_CARD_MIN_W: f32 = 480.0;
const EDITOR_CARD_MAX_W: f32 = 720.0;
const EDITOR_CARD_HEIGHT_SHARE: f32 = 0.4;
const EDITOR_CARD_MIN_H: f32 = 240.0;
const EDITOR_CARD_MAX_H: f32 = 340.0;

/// How wide the caret is, in pixels.
const CARET_WIDTH: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The dialog up, and the view it was built from: rebuilt only when what it
/// asks changes, so the button under the pointer stays lit as text is typed.
pub(in crate::window) struct OpenDialog {
    /// What each button does, by its index.
    actions: Vec<DialogAction>,
    /// misty's dialog.
    dialog: Dialog<usize>,
    /// What it was built from, the typed text and caret left out.
    shape: InputView,
}

/// What a dialog's button does once chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DialogAction {
    /// Chooses the overlay's own button at this index.
    Button(usize),
    /// Presses this key in the overlay, as the keyboard would.
    Key(Key),
}

// ========================================================================
// App: the dialog
// ========================================================================

impl App {
    /// Draw the dialog the last frame worked out over `frame`, building it
    /// afresh when what it asks has changed.
    pub(in crate::window) fn draw_dialog(&mut self, frame: &mut Frame) {
        let Some(view) = self.parts.drawn_dialog.clone() else {
            self.parts.dialog = None;
            return;
        };
        let shape = shape_of(&view);
        if self
            .parts
            .dialog
            .as_ref()
            .is_none_or(|open| open.shape != shape)
        {
            self.parts.dialog = Some(open_dialog(&view, shape));
        }
        let theme = ui_theme(self.renderer.as_ref());
        let area = area_of(&view, frame);
        let Some(open) = &mut self.parts.dialog else {
            return;
        };
        open.dialog.draw(frame, area, theme);
        if let (Some(input), Some(field)) = (&view.input, open.dialog.content_rect()) {
            draw_field(frame, input, (view.caret, view.caret_visible), field, theme);
        }
    }

    /// Offer pointer `input` to the dialog up: it lights the button under
    /// the pointer and answers a click on one. Every pointer event is the
    /// dialog's while it is up, but for a press on the window's edges, which
    /// still resizes the window (the title bar is offered the pointer first).
    /// Whether the dialog took it.
    pub(in crate::window) fn point_at_dialog(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        if self.parts.dialog.is_none() || !self.parts.dialog_open() {
            return false;
        }
        if let Input::PointerButton(button) = input
            && button.pressed
            && self
                .edge_resize_direction(button.position.x, button.position.y)
                .is_some()
        {
            return false;
        }
        let Some(open) = &mut self.parts.dialog else {
            return false;
        };
        let response = open.dialog.handle(input, cx);
        let chosen = match response {
            DialogResponse::Activated(index) => open.actions.get(index).copied(),
            _ => None,
        };
        let over_field = input
            .position()
            .zip(open.dialog.content_rect())
            .is_some_and(|(point, field)| field.contains(point));
        cx.set_cursor(match over_field {
            true => misty::CursorIcon::Text,
            false => misty::CursorIcon::Default,
        });
        match chosen {
            Some(DialogAction::Button(index)) => self.click_dialog_button(index),
            Some(DialogAction::Key(key)) => {
                self.dialog_key(&key);
            }
            None => {}
        }
        self.dirty = true;
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `view` without what is typed into it, which changes the field and not
/// the dialog.
fn shape_of(view: &InputView) -> InputView {
    InputView {
        caret: 0,
        caret_visible: false,
        input: view.input.as_ref().map(|_| String::new()),
        ..view.clone()
    }
}

/// misty's dialog for `view`: its title, its message, its kind's mark, and
/// its buttons, or an OK and a Cancel for a question typed into.
fn open_dialog(view: &InputView, shape: InputView) -> OpenDialog {
    let (title, message) = match &view.title {
        Some(title) => (title.clone(), view.label.clone()),
        None if view.input.is_some() => (view.label.clone(), String::new()),
        None => (default_title(view.kind).to_string(), view.label.clone()),
    };
    let body = message
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| Paragraph::plain(paragraph, ThemeToken::Foreground.into()))
        .collect();
    let mut dialog = Dialog::new(title, body);
    if let Some(severity) = view.kind.and_then(severity_of) {
        dialog = dialog.with_severity(severity);
    }
    if view.input.is_some() {
        dialog = dialog.with_content(1);
    }
    let mut actions = Vec::new();
    match view.buttons.is_empty() {
        false => {
            for (index, button) in view.buttons.iter().enumerate() {
                let mut shown = DialogButton::new(index, button.text.clone());
                if button.primary {
                    shown = shown.primary();
                }
                if let Some(letter) = button.shortcut.as_deref().and_then(single_letter) {
                    shown = shown.with_shortcut(letter);
                }
                dialog = dialog.with_button(shown);
                actions.push(DialogAction::Button(index));
            }
        }
        true if view.input.is_some() => {
            dialog = dialog
                .with_button(DialogButton::new(0, CANCEL_LABEL))
                .with_button(DialogButton::new(1, OK_LABEL).primary());
            actions.push(DialogAction::Key(Key::plain(KeyCode::Escape)));
            actions.push(DialogAction::Key(Key::plain(KeyCode::Enter)));
        }
        true => {}
    }
    if view.input.is_none()
        && let Some(focused) = view.buttons.iter().position(|button| button.focused)
    {
        dialog.set_focus(focused);
    }
    OpenDialog {
        actions,
        dialog,
        shape,
    }
}

/// The title a dialog of `kind` takes without one of its own.
fn default_title(kind: Option<NoticeKind>) -> &'static str {
    match kind {
        Some(NoticeKind::Error) => ERROR_TITLE,
        Some(NoticeKind::Warning) => WARNING_TITLE,
        Some(NoticeKind::Info | NoticeKind::Progress | NoticeKind::Question) | None => {
            CONFIRM_TITLE
        }
    }
}

/// The mark a dialog of `kind` shows before its title; `None` for a plain
/// question.
fn severity_of(kind: NoticeKind) -> Option<Severity> {
    match kind {
        NoticeKind::Error => Some(Severity::Error),
        NoticeKind::Warning => Some(Severity::Warning),
        NoticeKind::Info | NoticeKind::Question => Some(Severity::Info),
        NoticeKind::Progress => None,
    }
}

/// The letter a shortcut written as one character is, which also chooses
/// its button; `None` for a key named in words (`Esc`, `Enter`).
fn single_letter(shortcut: &str) -> Option<char> {
    let mut chars = shortcut.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) => Some(letter),
        _ => None,
    }
}

/// Where the dialog centres: on the transient or editor card that asked,
/// else the whole window.
fn area_of(view: &InputView, frame: &Frame) -> Rect {
    card_of(view, frame.cell_metrics()).unwrap_or_else(|| frame.bounds())
}

/// The field a question is typed into, in `field`: its well, the text that
/// fits with the caret in sight, and the caret in its shown phase.
fn draw_field(
    frame: &mut Frame,
    input: &str,
    (caret, caret_visible): (usize, bool),
    field: Rect,
    theme: &Theme,
) {
    let metrics = frame.cell_metrics();
    let scale_factor = frame.scale_factor();
    frame.overlay(|frame| {
        let well = BoxStyle {
            border: None,
            corner_radius: f32::from(theme.shape.control_radius) * scale_factor as f32,
            fill: Rgba::from(theme.background),
            shadow: None,
        };
        frame.draw_box(field, &well);
        let fits = metrics.cols_in(field.width).max(1);
        let chars: Vec<char> = input.chars().collect();
        let caret = caret.min(chars.len());
        let first = caret.saturating_sub(fits.saturating_sub(1));
        let shown: String = chars[first..].iter().take(fits).collect();
        // Typed text stays on the cell grid, where the caret counts it.
        let typed = StyledText::plain(shown, TextColor::Token(ThemeToken::Foreground));
        frame.draw_text(&typed, Point::new(field.x, field.y), field, theme);
        if caret_visible {
            let x = field.x + (caret - first) as f32 * metrics.width;
            let style = BoxStyle {
                border: None,
                corner_radius: 0.0,
                fill: Rgba::from(theme.cursor),
                shadow: None,
            };
            frame.draw_box(Rect::new(x, field.y, CARET_WIDTH, metrics.height), &style);
        }
    });
}

/// How much bigger than at the reference cell height the window's font is.
fn scale(metrics: misty::CellMetrics) -> f32 {
    metrics.height / REFERENCE_CELL_HEIGHT
}

/// The card `view` opens on: the transient card that asked, else the editor
/// card in the asking editor's pane; `None` for the window's own questions.
fn card_of(view: &InputView, metrics: misty::CellMetrics) -> Option<Rect> {
    if let Some((x, y, width, height)) = view.card {
        return Some(Rect::new(x, y, width, height));
    }
    let (pane_x, pane_y, pane_w, pane_h) = view.pane?;
    let scale = scale(metrics);
    let length = |pane: f32, share: f32, low: f32, high: f32| {
        (pane * share)
            .clamp(low * scale, high * scale)
            .min((pane - EDGE_ROOM).max(0.0))
            .floor()
    };
    let width = length(
        pane_w,
        EDITOR_CARD_WIDTH_SHARE,
        EDITOR_CARD_MIN_W,
        EDITOR_CARD_MAX_W,
    );
    let height = length(
        pane_h,
        EDITOR_CARD_HEIGHT_SHARE,
        EDITOR_CARD_MIN_H,
        EDITOR_CARD_MAX_H,
    );
    Some(Rect::new(
        (pane_x + (pane_w - width) / 2.0).floor(),
        (pane_y + (pane_h - height) / 2.0).floor(),
        width,
        height,
    ))
}
