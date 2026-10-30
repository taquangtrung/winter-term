//! Toast cards: the warnings and errors stacked in the window's top-right
//! corner. Minimal on purpose: one colored icon, the text, and who said it as
//! a quiet prefix, on the same surface and border as every other popup. The
//! close button shows only while the pointer is on the card.

use glyphon::cosmic_text::Wrap;
use glyphon::{Attrs, FontSystem, Shaping, SwashCache};

use crate::renderer::glyphs::{effective_bold_weight, parse_weight, FontCtx};
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line, ui_family};
use crate::renderer::raster::chrome::Chrome;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::renderer::view::{NoticeKind, ToastCard};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// Space inside a card left and right of its contents, in pixels.
const PAD_X: f32 = 14.0;

/// Space inside a card above and below its contents, in pixels.
const PAD_Y: f32 = 10.0;

/// Gap from the surface's right edge and from the bottom of the tab bar.
pub(in crate::renderer) const EDGE_GAP: f32 = 12.0;

/// Gap between one card and the next below it.
pub(in crate::renderer) const CARD_GAP: f32 = 6.0;

/// The most lines of text a card shows before it is cut short.
const MAX_LINES: usize = 3;

/// The narrowest and widest a card is, whatever the window's width.
const MIN_WIDTH: f32 = 240.0;
const MAX_WIDTH: f32 = 360.0;

/// The share of the window's width a card takes between those bounds.
const WIDTH_RATIO: f32 = 0.3;

/// Space between the icon and the text, in cells.
const ICON_GAP_CELLS: f32 = 1.0;

/// Cells kept clear at the right for the close button, shown or not, so the
/// text does not reflow when the pointer arrives.
const CLOSE_CELLS: f32 = 2.0;

/// What stands between the source and the text.
const SOURCE_SEPARATOR: &str = "  ";

/// The close button's glyph: one every font carries.
const CLOSE_GLYPH: &str = "\u{d7}";

// ========================================================================
// Free functions
// ========================================================================

/// How wide a card is on a surface `surface_w` pixels wide.
pub(in crate::renderer) fn card_width(surface_w: f32) -> f32 {
    (surface_w * WIDTH_RATIO)
        .clamp(MIN_WIDTH, MAX_WIDTH)
        .min(surface_w - 2.0 * EDGE_GAP)
        .max(0.0)
}

/// The close button of a card whose panel is `panel`, as `(x, y, w, h)`: the
/// cells kept clear at its top-right corner, as tall as the first line and
/// the padding above and below it.
pub(in crate::renderer) fn close_rect(
    panel: (f32, f32, f32, f32),
    cell_w: f32,
    line_height: f32,
) -> (f32, f32, f32, f32) {
    let (x, y, w, _) = panel;
    let side = cell_w * CLOSE_CELLS + PAD_X;
    (x + w - side, y, side, line_height + 2.0 * PAD_Y)
}

/// The color a notice of `kind` is marked in.
fn kind_color(theme: &Theme, kind: NoticeKind) -> Rgb {
    match kind {
        NoticeKind::Error => theme.ansi[1],
        NoticeKind::Info => theme.ansi[4],
        NoticeKind::Progress => theme.ansi[8],
        NoticeKind::Warning => theme.ansi[3],
    }
}

/// The codicon a notice of `kind` leads with.
fn kind_icon(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Error => "\u{ea87}",
        NoticeKind::Info | NoticeKind::Progress => "\u{ea74}",
        NoticeKind::Warning => "\u{ea6c}",
    }
}

/// The card's words in the UI font, wrapped to `width` pixels and cut off
/// after [`MAX_LINES`] lines: the source first, quiet and set apart, then
/// the text.
fn shape_message(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    theme: &Theme,
    card: &ToastCard,
    width: f32,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(
        Some(width.max(ctx.cell_w)),
        Some(ctx.line_height * MAX_LINES as f32),
    );
    let family = ui_family(font_system);
    let normal = parse_weight(ctx.normal_weight, glyphon::cosmic_text::Weight::NORMAL);
    let text = Attrs::new()
        .family(family)
        .weight(normal)
        .color(theme.foreground.to_glyphon());
    let source = Attrs::new()
        .family(family)
        .weight(effective_bold_weight(ctx.bold_weight, true))
        .color(theme.ansi[8].to_glyphon());
    let mut spans = Vec::new();
    if let Some(name) = card.source.as_deref() {
        spans.push((name, source.clone()));
        spans.push((SOURCE_SEPARATOR, text.clone()));
    }
    spans.push((card.text.as_str(), text.clone()));
    buffer.set_rich_text(spans, &text, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Paint `card` as a panel `panel_w` pixels wide, as tall as its text needs.
/// Returns the frame, to be placed by the caller, and the panel's height.
pub(in crate::renderer) fn toast_card(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    card: &ToastCard,
    panel_w: f32,
) -> (Chrome, f32) {
    let accent = kind_color(theme, card.kind);
    let mut icon = shape_chrome_line(
        font_system,
        ctx,
        kind_icon(card.kind),
        accent.to_glyphon(),
        false,
        false,
    );
    let icon_w = buffer_width(&icon).ceil().max(ctx.cell_w);
    let text_x = PAD_X + icon_w + ctx.cell_w * ICON_GAP_CELLS;
    let text_w = panel_w - text_x - ctx.cell_w * CLOSE_CELLS - PAD_X;
    let mut message = shape_message(font_system, ctx, theme, card, text_w);
    let lines = message.layout_runs().count().clamp(1, MAX_LINES);
    let panel_h = 2.0 * PAD_Y + lines as f32 * ctx.line_height;

    let mut frame = Chrome::new(theme, panel_w, panel_h, DROPDOWN_SHADOW);
    let margin = frame.margin();
    let top = (margin + PAD_Y) as i32;
    frame.draw_text(
        font_system,
        swash_cache,
        &mut icon,
        ((margin + PAD_X) as i32, top),
        accent.to_glyphon(),
    );
    frame.draw_text(
        font_system,
        swash_cache,
        &mut message,
        ((margin + text_x) as i32, top),
        theme.foreground.to_glyphon(),
    );
    if card.hovered {
        let muted = theme.ansi[8].to_glyphon();
        let mut close = shape_chrome_line(font_system, ctx, CLOSE_GLYPH, muted, false, true);
        let close_x = margin + panel_w - PAD_X - buffer_width(&close).ceil();
        frame.draw_text(
            font_system,
            swash_cache,
            &mut close,
            (close_x as i32, top),
            muted,
        );
    }
    (frame, panel_h)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::sample_font_ctx;

    fn card(text: &str, source: Option<&str>) -> ToastCard {
        ToastCard {
            hovered: false,
            id: 1,
            kind: NoticeKind::Error,
            source: source.map(str::to_string),
            text: text.to_string(),
        }
    }

    fn height(text: &str, source: Option<&str>) -> f32 {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let (_, height) = toast_card(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &card(text, source),
            MIN_WIDTH,
        );
        height
    }

    #[test]
    fn test_a_card_grows_with_its_text_up_to_a_limit() {
        let one = height("pushed", None);
        let long = "a failed command said a great deal about why ".repeat(4);
        assert!(height(&long, None) > one, "long text wraps onto more lines");
        let ctx = sample_font_ctx();
        let endless = height(&long.repeat(10), None);
        assert_eq!(endless, 2.0 * PAD_Y + MAX_LINES as f32 * ctx.line_height);
    }

    #[test]
    fn test_a_short_source_shares_the_first_line() {
        assert_eq!(height("pushed", Some("Git")), height("pushed", None));
    }

    #[test]
    fn test_a_card_is_a_share_of_the_window_within_its_bounds() {
        assert_eq!(
            card_width(250.0),
            250.0 - 2.0 * EDGE_GAP,
            "never past the window"
        );
        assert_eq!(card_width(400.0), MIN_WIDTH);
        assert_eq!(card_width(1000.0), 300.0);
        assert_eq!(card_width(4000.0), MAX_WIDTH);
    }
}
