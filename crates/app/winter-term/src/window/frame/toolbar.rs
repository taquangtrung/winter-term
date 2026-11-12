//! The pane toolbars, drawn by misty: a band over the top of a pane in the
//! window's own font, as the status bar is, its items laid out from the
//! left edge and against the right, the names of the pane's columns over
//! them, and every button's span kept so a press on the band is told which
//! button it landed on.

use misty::color::Rgba;
use misty::text::{width as text_width, Span, StyledText, TextColor};
use misty::theme::{Theme, ThemeToken};
use misty::{BoxStyle, Frame, Point, Rect};
use winter_render::{ColumnAlign, ToolbarItem, ToolbarTone, ToolbarView};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The room at the band's two ends, and between two items, in cells.
const EDGE_PAD_CELLS: usize = 1;
const ITEM_GAP_CELLS: usize = 1;

/// The least room the left items keep before they give up characters, in
/// cells.
const MIN_LEFT_CELLS: usize = 4;

/// What a shortened item ends with.
const ELLIPSIS: char = '…';

/// The hairline under the band, in pixels.
const HAIRLINE: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Where a toolbar's buttons were drawn: whose bar, its band's top and
/// bottom, and each button's left and right edges with what it is told back
/// as.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct ToolbarHits {
    /// Each button's left and right edges, and what it is told back as.
    buttons: Vec<(f32, f32, u16)>,
    /// The band's top and bottom edges.
    band: (f32, f32),
    /// Whose bar it is.
    owner: u64,
}

// ========================================================================
// App: the toolbars misty draws
// ========================================================================

impl App {
    /// Draw the toolbars the last frame worked out over `frame`, keeping
    /// where their buttons went.
    pub(in crate::window) fn draw_toolbars(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let bars = std::mem::take(&mut self.chrome.drawn_toolbars);
        self.chrome.toolbar_hits = bars
            .iter()
            .map(|bar| draw_toolbar(frame, bar, &theme))
            .collect();
        self.chrome.drawn_toolbars = bars;
    }

    /// The toolbar button at `(x, y)` as the bars were last drawn: whose bar
    /// it is, and what the button is told back as.
    pub(in crate::window) fn toolbar_button_at(&self, x: f32, y: f32) -> Option<(u64, u16)> {
        self.chrome
            .toolbar_hits
            .iter()
            .filter(|hits| y >= hits.band.0 && y < hits.band.1)
            .find_map(|hits| {
                hits.buttons
                    .iter()
                    .find(|(left, right, _)| x >= *left && x < *right)
                    .map(|(_, _, button)| (hits.owner, *button))
            })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Draw `bar` in `frame`: its band with a hairline under it, the right
/// items against its right edge, the column names over their columns, and
/// the left items from its left edge in what room is left, a shrinking item
/// cut short to fit. Where its buttons went.
fn draw_toolbar(frame: &mut Frame, bar: &ToolbarView, theme: &Theme) -> ToolbarHits {
    let (x, y, width, height) = bar.rect;
    let band = Rect::new(x, y, width, height);
    fill(frame, band, Rgba::from(theme.surface));
    fill(
        frame,
        Rect::new(x, y + height - HAIRLINE, width, HAIRLINE),
        Rgba::from(theme.border),
    );
    let cell = frame.cell_metrics().width;
    let cells = |count: usize| count as f32 * cell;
    let (pad, gap) = (cells(EDGE_PAD_CELLS), cells(ITEM_GAP_CELLS));
    let top = y + ((height - frame.cell_metrics().height) / 2.0).round();
    let mut buttons = Vec::new();
    let mut draw = |frame: &mut Frame, item: &ToolbarItem, text: &str, at: f32| {
        let drawn = cells(text_width(text));
        let span = Span::new(text.to_owned(), color(item.tone));
        let span = match item.tone == ToolbarTone::Strong {
            true => span.bold(),
            false => span,
        };
        frame.draw_text(
            &StyledText::from(span),
            Point::new(x + at, top),
            band,
            theme,
        );
        if let Some(button) = item.button {
            // A button answers across half the gaps beside it too, so the
            // spaces between buttons are no dead zones.
            buttons.push((x + at - gap / 2.0, x + at + drawn + gap / 2.0, button));
        }
        drawn
    };
    // The right items first: they stay whole, and the left ones fit into
    // what they leave.
    let right_total: f32 = bar
        .right
        .iter()
        .map(|item| cells(text_width(&item.text)))
        .sum::<f32>()
        + gap * bar.right.len().saturating_sub(1) as f32;
    let mut at = (width - pad - right_total).max(pad + cells(MIN_LEFT_CELLS));
    let right_start = at;
    for item in &bar.right {
        at += draw(frame, item, &item.text, at) + gap;
    }
    // The column names over their columns, as near their edge as the band
    // lets them, which the left items then stop short of.
    let mut left_end = right_start;
    for column in &bar.columns {
        let name = ToolbarItem {
            button: None,
            shrinks: false,
            text: column.text.clone(),
            tone: ToolbarTone::Dim,
        };
        let name_w = cells(text_width(&column.text));
        let start = match column.align {
            ColumnAlign::Left => column.left - x,
            ColumnAlign::Right => column.right - x - name_w,
        };
        let start = start.clamp(pad, (width - pad - name_w).max(pad));
        draw(frame, &name, &column.text, start);
        left_end = left_end.min(start);
    }
    // Where the left items run past their room, the ones that shrink give
    // up characters first to last, each down to its ellipsis before the next
    // gives any: the last items (where a path ends) stay whole the longest.
    let room = left_end - gap - pad;
    let gaps = gap * bar.left.len().saturating_sub(1) as f32;
    let mut texts: Vec<String> = bar.left.iter().map(|item| item.text.clone()).collect();
    for (index, item) in bar.left.iter().enumerate() {
        let chars: Vec<char> = item.text.chars().collect();
        let mut kept = chars.len();
        let total = |texts: &[String]| {
            texts
                .iter()
                .map(|text| cells(text_width(text)))
                .sum::<f32>()
        };
        while item.shrinks && kept > 0 && total(&texts) + gaps > room {
            kept -= 1;
            let head: String = chars[..kept].iter().collect();
            texts[index] = format!("{}{ELLIPSIS}", head.trim_end());
        }
    }
    let mut at = pad;
    for (item, text) in bar.left.iter().zip(&texts) {
        if left_end - gap - at <= 0.0 {
            break;
        }
        at += draw(frame, item, text, at) + gap;
    }
    ToolbarHits {
        buttons,
        band: (y, y + height),
        owner: bar.owner,
    }
}

/// The color an item of `tone` is drawn in.
fn color(tone: ToolbarTone) -> TextColor {
    match tone {
        ToolbarTone::Accent => TextColor::Token(ThemeToken::Accent),
        ToolbarTone::Dim => TextColor::Token(ThemeToken::Muted),
        ToolbarTone::Normal | ToolbarTone::Strong => TextColor::Token(ThemeToken::Foreground),
    }
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
