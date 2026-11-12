//! The find card, drawn by misty: the line a search is typed on, hung from
//! the top-right corner of the pane being searched, the way an editor's
//! find widget is. How far the search has got stays in the status bar.

use misty::text::{width as text_width, Span, StyledText, TextColor};
use misty::theme::ThemeToken;
use misty::{BoxStyle, Frame, Point, Popup, PopupSide, Rect};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// How wide the card is, in cells, and the least it shrinks to in a narrow
/// pane before the pane clips it instead.
const FIND_WIDTH_CELLS: f32 = 34.0;
const FIND_MIN_WIDTH_CELLS: f32 = 14.0;

/// How far in from the pane's top and right edges the card hangs, in
/// pixels.
const FIND_EDGE_GAP: f32 = 6.0;

/// The room around the card's contents, and between the label and the
/// field, in cells.
const FIND_PAD_CELLS: f32 = 0.6;
const FIND_LABEL_GAP_CELLS: f32 = 0.5;

/// How wide the caret is, in pixels.
const CARET_WIDTH: f32 = 1.0;

// ========================================================================
// App: the find card misty draws
// ========================================================================

impl App {
    /// Draw the find card the last frame worked out over `frame`, and keep
    /// where it went for the pointer.
    pub(in crate::window) fn draw_find(&mut self, frame: &mut Frame) {
        let Some(view) = self.chrome.drawn_find.clone() else {
            self.chrome.find_rect = None;
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let (pane_x, pane_y, pane_w) = view.pane;
        let pad = (metrics.width * FIND_PAD_CELLS).round();
        let inset = Popup::inset(&theme.shape, scale_factor);
        let room = (pane_w - 2.0 * FIND_EDGE_GAP).max(metrics.width * FIND_MIN_WIDTH_CELLS);
        let width = (metrics.width * FIND_WIDTH_CELLS).min(room).floor();
        let height = (metrics.height + 2.0 * (pad + inset)).ceil();
        let anchor = Rect::new(
            pane_x + pane_w - FIND_EDGE_GAP - width,
            pane_y + FIND_EDGE_GAP,
            width,
            0.0,
        );
        let popup = Popup::new(anchor, width, height, PopupSide::Below, frame.bounds());
        popup.draw(frame, &theme);
        self.chrome.find_rect = Some(popup.rect());
        let content = popup.content(&theme.shape, scale_factor);
        let label = StyledText::from(
            Span::new(view.label.as_str(), TextColor::Token(ThemeToken::Muted)).bold(),
        );
        let label_x = content.x + pad;
        let top = content.y + pad;
        let field_x = label_x
            + text_width(&view.label) as f32 * metrics.width
            + (metrics.width * FIND_LABEL_GAP_CELLS).round();
        let field = Rect::new(
            field_x,
            top,
            (content.x + content.width - pad - field_x).max(metrics.width),
            metrics.height,
        );
        // Only the part of the query that fits is drawn, starting late
        // enough that the caret is inside the field.
        let fits = metrics.cols_in(field.width).max(1);
        let chars: Vec<char> = view.query.chars().collect();
        let caret = view.caret.min(chars.len());
        let first = caret.saturating_sub(fits.saturating_sub(1));
        let shown: String = chars[first..].iter().take(fits).collect();
        let well = BoxStyle {
            border: None,
            corner_radius: 0.0,
            fill: misty::color::Rgba::from(theme.palette.background),
            shadow: None,
        };
        let caret_x = field.x + (caret - first) as f32 * metrics.width;
        let caret_visible = view.caret_visible;
        frame.overlay(|frame| {
            frame.draw_text(&label, Point::new(label_x, top), content, &theme);
            frame.draw_box(field, &well);
            frame.draw_text(
                &StyledText::plain(shown, TextColor::Token(ThemeToken::Foreground)),
                Point::new(field.x, top),
                field,
                &theme,
            );
            if caret_visible {
                let caret = Rect::new(caret_x, top, CARET_WIDTH, metrics.height);
                frame.draw_box(
                    caret,
                    &BoxStyle {
                        fill: misty::color::Rgba::from(theme.palette.cursor),
                        ..well
                    },
                );
            }
        });
    }

    /// Whether `(x, y)` is on the find card as last drawn.
    pub(in crate::window) fn find_card_at(&self, x: f32, y: f32) -> bool {
        self.chrome
            .find_rect
            .is_some_and(|rect| rect.contains(Point::new(x, y)))
    }
}
