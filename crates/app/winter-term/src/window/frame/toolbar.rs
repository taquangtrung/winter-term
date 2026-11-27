//! The pane toolbars and breadcrumb rows, drawn by misty in the UI font:
//! a band matching the pane background with a subtle hairline below it,
//! rounded hover pills on clickable items, column names aligned over their
//! columns, and hit spans kept for pointer interaction and tooltips.

use misty::{
    BoxStyle, CellColor, Frame, IconSet, Point, Rect, Rgba, Span, StyledText, Svg, TextColor,
    Theme, ThemeToken, draw_styled_label, styled_label_width,
};

use crate::icons;
use crate::render::{ColumnAlign, ToolbarItem, ToolbarTone, ToolbarView};
use crate::window::App;
use crate::window::appearance::ui_theme;

// ========================================================================
// Constants
// ========================================================================

/// How much of the background the band's bottom hairline mixes in, so it
/// separates quietly without reading as another full-contrast divider.
const BORDER_FADE_SHARE: f32 = 0.72;

/// Characters kept when shortening a dot-prefixed ancestor segment (e.g.
/// `.cargo` -> `.c`).
const DOT_SHORT_CHARS: usize = 2;

/// Horizontal padding at the band's two ends, in logical pixels.
const EDGE_PAD: f32 = 10.0;

/// What a shortened item ends with.
const ELLIPSIS: char = '…';

/// The hairline under the band, in logical pixels.
const HAIRLINE: f32 = 1.0;

/// Horizontal gap between an item's icon and its label, in logical pixels.
const ICON_GAP: f32 = 4.0;

/// Icon side length as a share of the UI font's cell height.
const ICON_HEIGHT_SHARE: f32 = 0.85;

/// Horizontal gap between adjacent items, in logical pixels.
const ITEM_GAP: f32 = 2.0;

/// Minimum room kept for left items before right items encroach, in logical
/// pixels.
const MIN_LEFT_WIDTH: f32 = 40.0;

/// Horizontal padding inside a clickable item's hover pill, in logical pixels.
const PILL_H_PAD: f32 = 5.0;

/// Corner radius of a clickable item's hover pill, in logical pixels.
const PILL_RADIUS: f32 = 4.0;

/// Vertical inset of a hover pill from the band's top and hairline, in logical
/// pixels.
const PILL_V_INSET: f32 = 3.0;

/// Mix share of `theme.background` into `theme.muted` for non-button dim
/// separators (chevrons, vertical rules) so they sit quietly between items.
const SEPARATOR_FADE_SHARE: f32 = 0.45;

/// Prefix of bundled monochrome symbol icons, tinted to the text color.
const SYMBOL_PREFIX: &str = "symbols/";

/// Toolbar and breadcrumb font size as a share of `config.font_size`.
const TOOLBAR_FONT_RATIO: f32 = 0.95;

// ========================================================================
// Data Structures
// ========================================================================

/// Where a toolbar's buttons were drawn: whose bar, its band's top and
/// bottom, and each button's left and right edges with what it is told back
/// as.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct ToolbarHits {
    /// The band's top and bottom edges.
    band: (f32, f32),
    /// Each button's left and right edges, and what it is told back as.
    buttons: Vec<(f32, f32, u16)>,
    /// Whose bar it is.
    owner: u64,
}

/// Scaled pixel geometry for laying out one toolbar band.
#[derive(Clone, Copy, Debug)]
struct BandMetrics {
    edge_pad: f32,
    gap: f32,
    hairline_h: f32,
    icon_gap: f32,
    icon_size: f32,
    icon_top: f32,
    min_left: f32,
    pill_h: f32,
    pill_h_pad: f32,
    pill_radius: f32,
    pill_top: f32,
    text_top: f32,
}

// ========================================================================
// App: the toolbars misty draws
// ========================================================================

impl App {
    /// Draw the toolbars the last frame worked out over `frame`, keeping
    /// where their buttons went.
    pub(in crate::window) fn draw_toolbars(&mut self, frame: &mut Frame) {
        let theme = ui_theme(self.renderer.as_ref());
        let hovered = self.input.pointer.hovered_toolbar_button;
        let font = self.misty_font_at(self.config.font_size * TOOLBAR_FONT_RATIO);
        let bars = std::mem::take(&mut self.parts.drawn_toolbars);
        load_toolbar_icons(&mut self.parts.toolbar_icons, &bars);
        let icons = &self.parts.toolbar_icons;
        let mut hits = Vec::with_capacity(bars.len());
        frame.with_font(&font, |frame| {
            hits.extend(
                bars.iter()
                    .map(|bar| draw_toolbar(frame, bar, theme, icons, hovered)),
            );
        });
        self.parts.toolbar_hits = hits;
        self.parts.drawn_toolbars = bars;
    }

    /// Where button `button` of `owner`'s bar was last drawn, for its
    /// tooltip to hang under.
    pub(in crate::window) fn toolbar_button_rect(&self, owner: u64, button: u16) -> Option<Rect> {
        let hits = self
            .parts
            .toolbar_hits
            .iter()
            .find(|hits| hits.owner == owner)?;
        let (left, right, _) = hits.buttons.iter().find(|(_, _, id)| *id == button)?;
        let (top, bottom) = hits.band;
        Some(Rect::new(*left, top, right - left, bottom - top))
    }

    /// The toolbar button at `(x, y)` as the bars were last drawn: whose bar
    /// it is, and what the button is told back as.
    pub(in crate::window) fn toolbar_button_at(&self, x: f32, y: f32) -> Option<(u64, u16)> {
        self.parts
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

/// Draw `bar` in `frame`: a pane-matching band with a subtle hairline under
/// it, the right items against its right edge, the column names over their
/// columns, and the left items from its left edge in what room is left, a
/// shrinking item cut short to fit. Returns where its buttons landed.
fn draw_toolbar(
    frame: &mut Frame,
    bar: &ToolbarView,
    theme: &Theme,
    icons: &IconSet,
    hovered: Option<(u64, u16)>,
) -> ToolbarHits {
    let (x, y, width, height) = bar.rect;
    let band = Rect::new(x, y, width, height);
    let metrics = band_metrics(frame, y, height);

    fill(frame, band, Rgba::from(theme.background), 0.0);
    let hairline = theme.border.mix(theme.background, BORDER_FADE_SHARE);
    if width > metrics.edge_pad * 2.0 {
        fill(
            frame,
            Rect::new(
                x + metrics.edge_pad,
                y + height - metrics.hairline_h,
                width - metrics.edge_pad * 2.0,
                metrics.hairline_h,
            ),
            Rgba::from(hairline),
            0.0,
        );
    }

    let mut buttons = Vec::new();
    let mut draw_item = |frame: &mut Frame, item: &ToolbarItem, text: &str, at: f32, clip: Rect| {
        let is_hovered = item
            .button
            .is_some_and(|button| hovered == Some((bar.owner, button)));
        let styled = item_styled(item, text, is_hovered, theme);
        let text_w = styled_label_width(frame, theme, &styled);
        let icon_name = item_icon_name(item, icons);
        let icon_advance = match icon_name.is_some() {
            true => metrics.icon_size + metrics.icon_gap,
            false => 0.0,
        };
        let h_pad = match item.button.is_some() {
            true => metrics.pill_h_pad,
            false => 0.0,
        };
        let slot_w = (icon_advance + text_w + 2.0 * h_pad).ceil();
        let pill = Rect::new(x + at, metrics.pill_top, slot_w, metrics.pill_h);
        if is_hovered {
            frame.clipped(clip, |frame| {
                fill(frame, pill, Rgba::from(theme.hover), metrics.pill_radius);
            });
        }
        if let Some(name) = icon_name {
            let icon_rect = Rect::new(
                x + at + h_pad,
                metrics.icon_top,
                metrics.icon_size,
                metrics.icon_size,
            );
            frame.clipped(clip, |frame| {
                frame.draw_named_icon(icons, &name, icon_rect, theme.foreground);
            });
        }
        draw_styled_label(
            frame,
            theme,
            &styled,
            Point::new(x + at + h_pad + icon_advance, metrics.text_top),
            clip,
        );
        if let Some(button) = item.button {
            let hit_right = (x + at + slot_w).min(clip.x + clip.width);
            if hit_right > x + at {
                buttons.push((x + at, hit_right, button));
            }
        }
        slot_w
    };

    // The right items first: they stay whole, and the left ones fit into
    // what they leave.
    let right_total: f32 = bar
        .right
        .iter()
        .map(|item| slot_width(frame, theme, icons, item, &item.text, &metrics))
        .sum::<f32>()
        + metrics.gap * bar.right.len().saturating_sub(1) as f32;
    let right_edge_pad = match bar.right.last().is_some_and(|item| item.button.is_some()) {
        true => (metrics.edge_pad - metrics.pill_h_pad).max(metrics.gap),
        false => metrics.edge_pad,
    };
    let right_start =
        (width - right_edge_pad - right_total).max(metrics.edge_pad + metrics.min_left);
    let mut at = right_start;
    for item in &bar.right {
        at += draw_item(frame, item, &item.text, at, band) + metrics.gap;
    }

    // The column names over their columns, as near their edge as the band
    // lets them, which the left items then stop short of.
    let mut left_end = right_start;
    for column in &bar.columns {
        let styled = StyledText::plain(column.text.as_str(), TextColor::Token(ThemeToken::Muted));
        let name_w = styled_label_width(frame, theme, &styled);
        let start = match column.align {
            ColumnAlign::Left => column.left - x,
            ColumnAlign::Right => column.right - x - name_w,
        };
        let start = start.clamp(
            metrics.edge_pad,
            (width - metrics.edge_pad - name_w).max(metrics.edge_pad),
        );
        draw_styled_label(
            frame,
            theme,
            &styled,
            Point::new(x + start, metrics.text_top),
            band,
        );
        left_end = left_end.min(start);
    }

    // Align the first crumb's text with `edge_pad` even when its hover pill
    // pads to the left of the text.
    let left_start = match bar.left.first().is_some_and(|item| item.button.is_some()) {
        true => (metrics.edge_pad - metrics.pill_h_pad).max(metrics.gap),
        false => metrics.edge_pad,
    };
    let room = (left_end - metrics.gap - left_start).max(0.0);
    let gaps = metrics.gap * bar.left.len().saturating_sub(1) as f32;
    let texts = fit_left_texts(&bar.left, room, gaps, |item, text| {
        slot_width(frame, theme, icons, item, text, &metrics)
    });

    let left_clip_right = x + (left_end - metrics.gap).max(left_start);
    let mut at = left_start;
    for (item, text) in bar.left.iter().zip(&texts) {
        let remaining = left_clip_right - (x + at);
        if remaining <= 0.0 {
            break;
        }
        let clip = Rect::new(x + at, y, remaining, height);
        at += draw_item(frame, item, text, at, clip) + metrics.gap;
    }

    ToolbarHits {
        band: (y, y + height),
        buttons,
        owner: bar.owner,
    }
}

/// Shorten shrinking items left-to-right until their widths plus `gaps` fit in
/// `room`. Leading shrinking items collapse to their short form; the last
/// shrinking item truncates with an ellipsis.
fn fit_left_texts(
    items: &[ToolbarItem],
    room: f32,
    gaps: f32,
    measure: impl Fn(&ToolbarItem, &str) -> f32,
) -> Vec<String> {
    let total = |texts: &[String]| {
        items
            .iter()
            .zip(texts)
            .map(|(item, text)| measure(item, text))
            .sum::<f32>()
    };
    let last_shrinking = items.iter().rposition(|item| item.shrinks);
    let mut texts: Vec<String> = items.iter().map(|item| item.text.clone()).collect();

    for (index, item) in items.iter().enumerate() {
        if !item.shrinks || total(&texts) + gaps <= room {
            continue;
        }
        if Some(index) != last_shrinking {
            texts[index] = short_crumb(&item.text);
            continue;
        }
        let chars: Vec<char> = item.text.chars().collect();
        let mut kept = chars.len();
        while kept > 0 && total(&texts) + gaps > room {
            kept -= 1;
            let head: String = chars[..kept].iter().collect();
            texts[index] = format!("{}{ELLIPSIS}", head.trim_end());
        }
    }
    texts
}

/// Short form of an ancestor segment: its first character, or two characters
/// when dot-prefixed so hidden folders stay distinguishable.
fn short_crumb(text: &str) -> String {
    let keep = match text.starts_with('.') {
        true => DOT_SHORT_CHARS,
        false => 1,
    };
    text.chars().take(keep).collect()
}

/// Scaled layout measurements for a toolbar band at `(y, height)`.
fn band_metrics(frame: &Frame, y: f32, height: f32) -> BandMetrics {
    let scale = frame.scale_factor() as f32;
    let hairline_h = scale_px(HAIRLINE, scale).max(1.0);
    let inner_h = (height - hairline_h).max(0.0);
    let text_h = frame.cell_metrics().height;
    let icon_size = (text_h * ICON_HEIGHT_SHARE).round().max(1.0);
    let pill_v_inset = scale_px(PILL_V_INSET, scale);
    let pill_h = (inner_h - 2.0 * pill_v_inset).max(text_h).min(inner_h);
    BandMetrics {
        edge_pad: scale_px(EDGE_PAD, scale),
        gap: scale_px(ITEM_GAP, scale),
        hairline_h,
        icon_gap: scale_px(ICON_GAP, scale),
        icon_size,
        icon_top: y + ((inner_h - icon_size) / 2.0).round(),
        min_left: scale_px(MIN_LEFT_WIDTH, scale),
        pill_h,
        pill_h_pad: scale_px(PILL_H_PAD, scale),
        pill_radius: scale_px(PILL_RADIUS, scale),
        pill_top: y + ((inner_h - pill_h) / 2.0).round(),
        text_top: y + ((inner_h - text_h) / 2.0).round(),
    }
}

/// Ensure every icon referenced by `bars` is loaded into `icons`.
fn load_toolbar_icons(icons: &mut IconSet, bars: &[ToolbarView]) {
    for item in bars
        .iter()
        .flat_map(|bar| bar.left.iter().chain(&bar.right))
    {
        let Some(kind) = item.icon.as_ref() else {
            continue;
        };
        let Some(name) = icons::name_for(kind) else {
            continue;
        };
        if icons.get(&name).is_some() {
            continue;
        }
        let Some(svg) = icons::named_svg(&name).and_then(|bytes| Svg::parse(&bytes).ok()) else {
            continue;
        };
        match name.starts_with(SYMBOL_PREFIX) {
            true => icons.insert(name, svg),
            false => icons.insert_colored(name, svg),
        }
    }
}

/// Bundled icon name for `item`, when it has one loaded in `icons`.
fn item_icon_name(item: &ToolbarItem, icons: &IconSet) -> Option<String> {
    let kind = item.icon.as_ref()?;
    icons::name_for(kind).filter(|name| icons.get(name).is_some())
}

/// Styled label for `item` showing `text`, lifting dim buttons to foreground
/// when `hovered` and softening non-button dim separators.
fn item_styled(item: &ToolbarItem, text: &str, hovered: bool, theme: &Theme) -> StyledText {
    let text_color = match (item.button.is_some(), item.tone, hovered) {
        (false, ToolbarTone::Dim, _) => TextColor::Palette(CellColor::Rgb(
            theme.muted.mix(theme.background, SEPARATOR_FADE_SHARE),
        )),
        (true, ToolbarTone::Dim, true) => TextColor::Token(ThemeToken::Foreground),
        (_, tone, _) => color(tone),
    };
    let span = Span::new(text.to_owned(), text_color);
    let span = match item.tone == ToolbarTone::Strong {
        true => span.bold(),
        false => span,
    };
    StyledText::from(span)
}

/// Measured pixel width of `item` showing `text`, including any icon and
/// pill padding for clickable buttons.
fn slot_width(
    frame: &Frame,
    theme: &Theme,
    icons: &IconSet,
    item: &ToolbarItem,
    text: &str,
    metrics: &BandMetrics,
) -> f32 {
    let styled = item_styled(item, text, false, theme);
    let text_w = styled_label_width(frame, theme, &styled);
    let icon_advance = match item_icon_name(item, icons).is_some() {
        true => metrics.icon_size + metrics.icon_gap,
        false => 0.0,
    };
    let h_pad = match item.button.is_some() {
        true => metrics.pill_h_pad,
        false => 0.0,
    };
    (icon_advance + text_w + 2.0 * h_pad).ceil()
}

/// The color an item of `tone` is drawn in.
fn color(tone: ToolbarTone) -> TextColor {
    match tone {
        ToolbarTone::Accent => TextColor::Token(ThemeToken::Accent),
        ToolbarTone::Dim => TextColor::Token(ThemeToken::Muted),
        ToolbarTone::Normal | ToolbarTone::Strong => TextColor::Token(ThemeToken::Foreground),
    }
}

/// `rect` filled with `color` and rounded by `corner_radius`.
fn fill(frame: &mut Frame, rect: Rect, color: Rgba, corner_radius: f32) {
    let style = BoxStyle {
        border: None,
        corner_radius,
        fill: color,
        shadow: None,
    };
    frame.draw_box(rect, &style);
}

/// Scale a logical pixel measurement to surface pixels, rounded to whole
/// pixels for crisp edges.
fn scale_px(logical: f32, scale: f32) -> f32 {
    (logical * scale).round()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_item_styled_lifts_hovered_dim_button_and_softens_separator() {
        let theme = Theme::dark();
        let ancestor = ToolbarItem {
            button: Some(0x8000),
            icon: None,
            shrinks: true,
            text: "src".to_string(),
            tone: ToolbarTone::Dim,
        };
        let separator = ToolbarItem {
            button: None,
            icon: None,
            shrinks: false,
            text: "›".to_string(),
            tone: ToolbarTone::Dim,
        };

        let at_rest = item_styled(&ancestor, "src", false, &theme);
        assert_eq!(
            at_rest.spans()[0].color,
            TextColor::Token(ThemeToken::Muted)
        );

        let hovered = item_styled(&ancestor, "src", true, &theme);
        assert_eq!(
            hovered.spans()[0].color,
            TextColor::Token(ThemeToken::Foreground)
        );

        let sep = item_styled(&separator, "›", false, &theme);
        assert_eq!(
            sep.spans()[0].color,
            TextColor::Palette(CellColor::Rgb(
                theme.muted.mix(theme.background, SEPARATOR_FADE_SHARE)
            ))
        );
    }

    fn crumb(text: &str) -> ToolbarItem {
        ToolbarItem {
            button: Some(0x8000),
            icon: None,
            shrinks: true,
            text: text.to_string(),
            tone: ToolbarTone::Dim,
        }
    }

    fn sep() -> ToolbarItem {
        ToolbarItem {
            button: None,
            icon: None,
            shrinks: false,
            text: "›".to_string(),
            tone: ToolbarTone::Dim,
        }
    }

    fn char_width(_item: &ToolbarItem, text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn test_fit_left_texts_shortens_ancestors_to_initials_and_keeps_dot_prefix() {
        let items = vec![
            crumb("home"),
            sep(),
            crumb(".cargo"),
            sep(),
            crumb("bands.rs"),
        ];
        // Full width is 4 + 1 + 6 + 1 + 8 = 20; room 15 fits `h › .c › bands.rs` (13).
        let fitted = fit_left_texts(&items, 15.0, 0.0, char_width);
        assert_eq!(fitted, ["h", "›", ".c", "›", "bands.rs"]);
    }

    #[test]
    fn test_fit_left_texts_truncates_last_segment_with_ellipsis_when_still_tight() {
        let items = vec![crumb("home"), sep(), crumb("bands.rs")];
        // Shortened ancestor gives `h › bands.rs` (10); room 7 truncates the last segment.
        let fitted = fit_left_texts(&items, 7.0, 0.0, char_width);
        assert_eq!(fitted, ["h", "›", "band…"]);
    }

    #[test]
    fn test_fit_left_texts_truncates_single_shrinking_item_with_ellipsis() {
        let items = vec![crumb("Introduction")];
        let fitted = fit_left_texts(&items, 6.0, 0.0, char_width);
        assert_eq!(fitted, ["Intro…"]);
    }
}
