//! The dialog, drawn by misty: a question with the field it is answered in
//! and the keys that answer it as keycaps along its foot, or a confirmation
//! with its icon, its message, and buttons along its foot. It opens on the
//! card of the transient or editor that asked, else centred on the window,
//! and keeps where it and its buttons went for the pointer.

use misty::color::{Rgb, Rgba};
use misty::text::{width as text_width, Span, StyledText, TextColor};
use misty::theme::{Theme, ThemeToken};
use misty::{BoxStyle, Frame, Point, Popup, PopupSide, Rect};
use winter_render::{ButtonView, InputView, NoticeKind};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The title a confirmation without one of its own takes.
const CONFIRM_TITLE: &str = "Confirmation";
const WARNING_TITLE: &str = "Warning";
const ERROR_TITLE: &str = "Error";

/// A question's width as a share of the window's, and a confirmation's
/// widest, in pixels at the reference font.
const INPUT_WIDTH_SHARE: f32 = 0.5;
const CONFIRM_MAX_WIDTH: f32 = 520.0;

/// The narrowest a dialog is, in pixels at the reference font.
const DIALOG_MIN_WIDTH: f32 = 300.0;

/// The cell height the pixel sizes above are given at; a larger font scales
/// them by the same share.
const REFERENCE_CELL_HEIGHT: f32 = 17.0;

/// The room a floating card leaves between itself and the window's edges,
/// both sides together, in pixels.
const EDGE_ROOM: f32 = 40.0;

/// The room inside the card and between its parts, in cells (across) and
/// lines (down).
const PAD_CELLS: f32 = 1.0;
const GAP_LINES: f32 = 0.6;

/// The room around a keycap's key and a button's label, in cells, and
/// between two of them.
const KEYCAP_PAD_CELLS: usize = 1;
const PAIR_GAP_CELLS: usize = 2;

/// The editor card's share of its pane, and its smallest and largest width
/// at the reference font, which a dialog an editor asks opens as wide as.
const EDITOR_CARD_WIDTH_SHARE: f32 = 0.6;
const EDITOR_CARD_MIN_W: f32 = 480.0;
const EDITOR_CARD_MAX_W: f32 = 720.0;
const EDITOR_CARD_HEIGHT_SHARE: f32 = 0.4;
const EDITOR_CARD_MIN_H: f32 = 240.0;
const EDITOR_CARD_MAX_H: f32 = 340.0;

/// The caret's width, and a focused button's outline, in pixels.
const LINE: f32 = 1.0;

// ========================================================================
// App: the dialog misty draws
// ========================================================================

impl App {
    /// Draw the dialog the last frame worked out over `frame`, keeping where
    /// it and its buttons went.
    pub(in crate::window) fn draw_dialog(&mut self, frame: &mut Frame) {
        let Some(view) = self.chrome.drawn_dialog.clone() else {
            self.chrome.dialog_rect = None;
            self.chrome.dialog_buttons.clear();
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let kind_color = self
            .renderer
            .as_ref()
            .zip(view.kind)
            .map(|(renderer, kind)| {
                let color = kind.color(renderer.theme());
                Rgb::new(color.r, color.g, color.b)
            });
        let confirm = (view.input.is_none() && !view.keys.is_empty()) || !view.buttons.is_empty();
        let (rect, buttons) = match confirm {
            true => draw_confirm(frame, &view, &theme, kind_color),
            false => draw_question(frame, &view, &theme, kind_color),
        };
        self.chrome.dialog_rect = Some(rect);
        self.chrome.dialog_buttons = buttons;
    }

    /// Whether `(x, y)` is on the dialog as last drawn.
    pub(in crate::window) fn dialog_at(&self, x: f32, y: f32) -> bool {
        self.chrome
            .dialog_rect
            .is_some_and(|rect| rect.contains(Point::new(x, y)))
    }

    /// The dialog's button at `(x, y)`, by its place, as last drawn.
    pub(in crate::window) fn dialog_button_at(&self, x: f32, y: f32) -> Option<usize> {
        self.chrome
            .dialog_buttons
            .iter()
            .position(|rect| rect.contains(Point::new(x, y)))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The buttons a confirmation shows: its own, else one for each key that
/// answers it, the first the primary one.
pub(in crate::window) fn confirm_buttons(view: &InputView) -> Vec<ButtonView> {
    if !view.buttons.is_empty() {
        return view.buttons.clone();
    }
    view.keys
        .iter()
        .enumerate()
        .map(|(index, (key, does))| ButtonView {
            focused: index == 0,
            primary: index == 0,
            shortcut: Some(key.clone()),
            text: does.clone(),
        })
        .collect()
}

/// Draw a question: its title bar, the question under it when a title
/// names the dialog, the field with its caret, and the keycaps.
fn draw_question(
    frame: &mut Frame,
    view: &InputView,
    theme: &Theme,
    kind_color: Option<Rgb>,
) -> (Rect, Vec<Rect>) {
    let metrics = frame.cell_metrics();
    let (pad, gap) = room(metrics);
    let scale = scale(metrics);
    let bounds = frame.bounds();
    let width = match card_of(view, metrics) {
        Some(card) => card.width,
        None => (bounds.width * INPUT_WIDTH_SHARE).min(bounds.width - EDGE_ROOM),
    }
    .floor()
    .max(DIALOG_MIN_WIDTH * scale);
    let line = metrics.height;
    let mut parts = 0.0;
    if view.title.is_some() {
        parts += line + gap;
    }
    if view.input.is_some() {
        parts += line + gap;
    }
    if !view.keys.is_empty() {
        parts += line + gap;
    }
    let height = (line + pad + pad + parts).ceil();
    let (popup, content) = open_card(frame, view, (width, height), theme);
    let title = view.title.as_deref().unwrap_or(&view.label);
    let mut y = draw_title_bar(frame, content, title, view.kind, kind_color, theme) + pad;
    let left = content.x + pad;
    let right = content.x + content.width - pad;
    let field = view.input.as_ref().map(|input| {
        let field_top = y + view.title.as_ref().map_or(0.0, |_| line + gap);
        (
            input.clone(),
            Rect::new(left, field_top, (right - left).max(metrics.width), line),
        )
    });
    let label = view.title.as_ref().map(|_| view.label.clone());
    let keys = view.keys.clone();
    let caret = view.caret;
    let caret_visible = view.caret_visible;
    frame.overlay(|frame| {
        if let Some(label) = label {
            draw_line(
                frame,
                &label,
                TextColor::Token(ThemeToken::Foreground),
                Point::new(left, y),
                content,
                theme,
            );
            y += line + gap;
        }
        if let Some((input, field)) = field {
            fill(frame, field, Rgba::from(theme.palette.background));
            let fits = metrics.cols_in(field.width).max(1);
            let chars: Vec<char> = input.chars().collect();
            let caret = caret.min(chars.len());
            let first = caret.saturating_sub(fits.saturating_sub(1));
            let shown: String = chars[first..].iter().take(fits).collect();
            draw_line(
                frame,
                &shown,
                TextColor::Token(ThemeToken::Foreground),
                Point::new(field.x, field.y),
                field,
                theme,
            );
            if caret_visible {
                let x = field.x + (caret - first) as f32 * metrics.width;
                fill(
                    frame,
                    Rect::new(x, field.y, LINE, line),
                    Rgba::from(theme.palette.cursor),
                );
            }
            y = field.y + line + gap;
        }
        if !keys.is_empty() {
            draw_keycaps(frame, &keys, (left, right, y), content, theme);
        }
    });
    (popup, Vec::new())
}

/// Draw a confirmation: its title bar, its icon and message, and its
/// buttons along the foot, right-aligned. Where each button went.
fn draw_confirm(
    frame: &mut Frame,
    view: &InputView,
    theme: &Theme,
    kind_color: Option<Rgb>,
) -> (Rect, Vec<Rect>) {
    let metrics = frame.cell_metrics();
    let (pad, gap) = room(metrics);
    let scale = scale(metrics);
    let bounds = frame.bounds();
    let buttons = confirm_buttons(view);
    let cell = metrics.width;
    let button_w =
        |button: &ButtonView| (text_width(&button.text) + 2 * KEYCAP_PAD_CELLS) as f32 * cell;
    let buttons_w = buttons.iter().map(button_w).sum::<f32>()
        + (buttons.len().saturating_sub(1) * PAIR_GAP_CELLS) as f32 * cell;
    let title = view.title.clone().unwrap_or_else(|| {
        match view.kind {
            Some(NoticeKind::Warning) => WARNING_TITLE,
            Some(NoticeKind::Error) => ERROR_TITLE,
            _ => CONFIRM_TITLE,
        }
        .to_owned()
    });
    let icon = view.kind.map(|kind| kind.icon());
    let icon_col = icon.map_or(0.0, |_| 3.0 * cell);
    let least = (buttons_w + 2.0 * pad)
        .max(text_width(&title) as f32 * cell + 2.0 * pad)
        .max(DIALOG_MIN_WIDTH * scale);
    let most = (bounds.width - EDGE_ROOM)
        .min(CONFIRM_MAX_WIDTH * scale)
        .max(least);
    let message_w = text_width(&view.label) as f32 * cell;
    let width = (message_w + icon_col + 2.0 * pad)
        .clamp(least, most)
        .floor();
    let text_cells = metrics.cols_in(width - 2.0 * pad - icon_col).max(1);
    let lines = wrap(&view.label, text_cells);
    let line = metrics.height;
    let message_h = lines.len().max(1) as f32 * line;
    let height = (line + pad + message_h + gap + line + pad).ceil();
    let (popup, content) = open_card(frame, view, (width, height), theme);
    let top = draw_title_bar(frame, content, &title, None, None, theme) + pad;
    let left = content.x + pad;
    let right = content.x + content.width - pad;
    let foot = top + message_h + gap;
    let mut rects = vec![Rect::default(); buttons.len()];
    let mut at = right;
    for (index, button) in buttons.iter().enumerate().rev() {
        let w = button_w(button);
        at -= w;
        rects[index] = Rect::new(at, foot, w, line);
        at -= PAIR_GAP_CELLS as f32 * cell;
    }
    let drawn = rects.clone();
    frame.overlay(|frame| {
        if let (Some(icon), Some(color)) = (icon, kind_color) {
            let glyph =
                StyledText::from(Span::new(icon, TextColor::Rgba(Rgba::from(color))).bold());
            frame.draw_text(&glyph, Point::new(left, top), content, theme);
        }
        let single = lines.len() == 1 && icon.is_none();
        for (index, text) in lines.iter().enumerate() {
            let x = match single {
                true => {
                    content.x + ((content.width - text_width(text) as f32 * cell) / 2.0).floor()
                }
                false => left + icon_col,
            };
            let y = top + index as f32 * line;
            draw_line(
                frame,
                text,
                TextColor::Token(ThemeToken::Foreground),
                Point::new(x, y),
                content,
                theme,
            );
        }
        for (button, rect) in buttons.iter().zip(&drawn) {
            draw_button(frame, button, *rect, theme);
        }
    });
    (popup, rects)
}

/// Open the card for `view` at its `(width, height)`: on the card that
/// asked, at its top and across its middle, else centred on the window.
/// Where the card went, and the room inside it.
fn open_card(
    frame: &mut Frame,
    view: &InputView,
    (width, height): (f32, f32),
    theme: &Theme,
) -> (Rect, Rect) {
    let metrics = frame.cell_metrics();
    let bounds = frame.bounds();
    let inset = Popup::inset(&theme.shape, frame.scale_factor());
    let (width, height) = (width + 2.0 * inset, height + 2.0 * inset);
    let (x, y) = match card_of(view, metrics) {
        Some(card) => (
            (card.x + (card.width - width) / 2.0).clamp(0.0, (bounds.width - width).max(0.0)),
            card.y.clamp(0.0, (bounds.height - height).max(0.0)),
        ),
        None => ((bounds.width - width) / 2.0, (bounds.height - height) / 2.0),
    };
    let anchor = Rect::new(x.floor(), y.floor(), width, 0.0);
    let popup = Popup::new(anchor, width, height, PopupSide::Below, bounds);
    popup.draw(frame, theme);
    (
        popup.rect(),
        popup.content(&theme.shape, frame.scale_factor()),
    )
}

/// Draw the dialog's title bar across the top of `content`: its title in
/// bold, after its icon when it has one. Where the bar ends.
fn draw_title_bar(
    frame: &mut Frame,
    content: Rect,
    title: &str,
    kind: Option<NoticeKind>,
    kind_color: Option<Rgb>,
    theme: &Theme,
) -> f32 {
    let metrics = frame.cell_metrics();
    let (pad, _) = room(metrics);
    let bar = Rect::new(content.x, content.y, content.width, metrics.height + pad);
    let top = content.y + (pad / 2.0).floor();
    let mut x = content.x + pad;
    let icon = kind.zip(kind_color);
    let title = title.to_owned();
    frame.overlay(|frame| {
        fill(frame, bar, Rgba::from(theme.surface));
        fill(
            frame,
            Rect::new(bar.x, bar.y + bar.height - LINE, bar.width, LINE),
            Rgba::from(theme.border),
        );
        if let Some((kind, color)) = icon {
            let glyph =
                StyledText::from(Span::new(kind.icon(), TextColor::Rgba(Rgba::from(color))));
            frame.draw_text(&glyph, Point::new(x, top), bar, theme);
            x += 2.0 * metrics.width;
        }
        let title =
            StyledText::from(Span::new(title, TextColor::Token(ThemeToken::Foreground)).bold());
        frame.draw_text(&title, Point::new(x, top), bar, theme);
    });
    bar.y + bar.height
}

/// Draw `keys` as keycaps, each with what it does beside it, right-aligned
/// between `left` and `right` on the line at `y`.
fn draw_keycaps(
    frame: &mut Frame,
    keys: &[(String, String)],
    (left, right, y): (f32, f32, f32),
    clip: Rect,
    theme: &Theme,
) {
    let cell = frame.cell_metrics().width;
    let line = frame.cell_metrics().height;
    let pad = KEYCAP_PAD_CELLS as f32 * cell;
    let pair = |(key, does): &(String, String)| {
        (text_width(key) + text_width(does) + 1) as f32 * cell + 2.0 * pad
    };
    let total = keys.iter().map(pair).sum::<f32>()
        + (keys.len().saturating_sub(1) * PAIR_GAP_CELLS) as f32 * cell;
    let mut x = (right - total).max(left);
    for entry in keys {
        let (key, does) = entry;
        let cap_w = text_width(key) as f32 * cell + 2.0 * pad;
        fill(
            frame,
            Rect::new(x, y, cap_w, line),
            Rgba::from(theme.raised),
        );
        let key_text = StyledText::from(
            Span::new(key.as_str(), TextColor::Token(ThemeToken::Foreground)).bold(),
        );
        frame.draw_text(&key_text, Point::new(x + pad, y), clip, theme);
        draw_line(
            frame,
            does,
            TextColor::Token(ThemeToken::Muted),
            Point::new(x + cap_w + cell, y),
            clip,
            theme,
        );
        x += pair(entry) + PAIR_GAP_CELLS as f32 * cell;
    }
}

/// Draw `button` in `rect`: the primary one filled in the accent, the
/// focused one outlined in it, its shortcut's letter underlined.
fn draw_button(frame: &mut Frame, button: &ButtonView, rect: Rect, theme: &Theme) {
    let cell = frame.cell_metrics().width;
    let (fill_color, text) = match button.primary {
        true => (
            Rgba::from(theme.accent),
            TextColor::Token(ThemeToken::Background),
        ),
        false => (
            Rgba::from(theme.raised),
            TextColor::Token(ThemeToken::Foreground),
        ),
    };
    fill(frame, rect, fill_color);
    if button.focused {
        let ring = BoxStyle {
            border: Some(misty::Border {
                color: Rgba::from(theme.accent),
                width: LINE,
            }),
            corner_radius: 0.0,
            fill: Rgba::new(0, 0, 0, 0),
            shadow: None,
        };
        frame.draw_box(
            Rect::new(
                rect.x - 2.0,
                rect.y - 2.0,
                rect.width + 4.0,
                rect.height + 4.0,
            ),
            &ring,
        );
    }
    let x = rect.x + KEYCAP_PAD_CELLS as f32 * cell;
    let label = StyledText::from(Span::new(button.text.as_str(), text));
    frame.draw_text(&label, Point::new(x, rect.y), rect, theme);
    let underlined = button.shortcut.as_deref().and_then(|shortcut| {
        let wanted = shortcut.chars().next()?.to_lowercase().next()?;
        button
            .text
            .chars()
            .position(|ch| ch.to_lowercase().next() == Some(wanted))
    });
    if let Some(at) = underlined {
        let under = Rect::new(
            x + at as f32 * cell,
            rect.y + rect.height - 2.0 * LINE,
            cell,
            LINE,
        );
        let color = match button.primary {
            true => Rgba::from(theme.palette.background),
            false => Rgba::from(theme.accent),
        };
        fill(frame, under, color);
    }
}

/// Draw `text` on one line at `origin` in `color`, clipped to `clip`.
fn draw_line(
    frame: &mut Frame,
    text: &str,
    color: TextColor,
    origin: Point,
    clip: Rect,
    theme: &Theme,
) {
    frame.draw_text(&StyledText::plain(text, color), origin, clip, theme);
}

/// `rect` filled with `color`.
fn fill(frame: &mut Frame, rect: Rect, color: Rgba) {
    let style = BoxStyle {
        border: None,
        corner_radius: 0.0,
        fill: color,
        shadow: None,
    };
    frame.draw_box(rect, &style);
}

/// The room inside a card and between its parts, in pixels.
fn room(metrics: misty::CellMetrics) -> (f32, f32) {
    (
        (metrics.width * PAD_CELLS).round(),
        (metrics.height * GAP_LINES).round(),
    )
}

/// How much the dialog's pixel sizes grow with the font.
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

/// `text` folded into lines of at most `cells` cells, at spaces where it
/// has them.
fn wrap(text: &str, cells: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let wanted = text_width(&current) + usize::from(!current.is_empty()) + text_width(word);
        if !current.is_empty() && wanted > cells {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_message_folds_at_spaces_within_its_width() {
        assert_eq!(
            wrap("stop the running program now", 12),
            ["stop the", "running", "program now"]
        );
        assert_eq!(wrap("", 12), [""]);
    }

    #[test]
    fn test_keys_alone_become_buttons_the_first_primary() {
        let view = InputView {
            buttons: Vec::new(),
            card: None,
            caret: 0,
            caret_visible: false,
            input: None,
            keys: vec![("y".into(), "Yes".into()), ("n".into(), "No".into())],
            kind: None,
            label: "Quit?".into(),
            pane: None,
            title: None,
        };
        let buttons = confirm_buttons(&view);
        assert_eq!(buttons.len(), 2);
        assert!(buttons[0].primary && !buttons[1].primary);
        assert_eq!(buttons[1].shortcut.as_deref(), Some("n"));
    }
}
