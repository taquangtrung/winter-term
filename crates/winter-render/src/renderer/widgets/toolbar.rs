//! A pane's toolbar: a band over the top of the pane, drawn in the window's
//! own font like the status bar, with its items laid out from the left edge
//! and against the right, and every button's span kept so a press on the
//! band can be told which button it landed on.

use glyphon::{FontSystem, SwashCache};

use super::view::{ToolbarItem, ToolbarTone, ToolbarView};
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, composite_buffer, fill_rounded_rect, shape_chrome_line};
use crate::renderer::raster::view::DropdownImage;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// The room at the band's two ends, and between two items, in cells.
const EDGE_PAD_CELLS: f32 = 0.6;
const ITEM_GAP_CELLS: f32 = 0.6;

/// The least room the left items keep before they give up characters.
const MIN_LEFT_CELLS: f32 = 4.0;

/// What a shortened item ends with.
const ELLIPSIS: char = '…';

// ========================================================================
// Data Structures
// ========================================================================

/// Where a button was drawn: its left and right edges in surface pixels,
/// and what it is told back as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct ToolbarHit {
    pub(in crate::renderer) button: u16,
    pub(in crate::renderer) left: f32,
    pub(in crate::renderer) right: f32,
}

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize `view`: the band in the title bar's shade with a hairline under
/// it, the right items against its right edge, and the left items from its
/// left edge in what room they leave, a shrinking item cut short to fit.
/// The image, and where each button was drawn.
pub(in crate::renderer) fn toolbar_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &ToolbarView,
) -> (DropdownImage, Vec<ToolbarHit>) {
    let (x, y, width, height) = view.rect;
    let canvas = (width.max(1.0).round() as u32, height.max(1.0).round() as u32);
    let mut rgba = vec![0u8; (canvas.0 * canvas.1 * 4) as usize];
    let (w, h) = (canvas.0 as f32, canvas.1 as f32);
    fill_rounded_rect(&mut rgba, canvas, (0.0, 0.0, w, h), 0.0, theme.tabbar_bg, 1.0);
    fill_rounded_rect(&mut rgba, canvas, (0.0, h - 1.0, w, 1.0), 0.0, theme.divider, 1.0);

    let pad = (ctx.cell_w * EDGE_PAD_CELLS).round();
    let gap = (ctx.cell_w * ITEM_GAP_CELLS).round();
    let text_y = ((h - ctx.line_height) / 2.0).round() as i32;
    let width_of = |font_system: &mut FontSystem, item: &ToolbarItem, text: &str| {
        buffer_width(&shape_chrome_line(
            font_system,
            ctx,
            text,
            color(theme, item.tone),
            item.tone == ToolbarTone::Strong,
            true,
        ))
        .ceil()
    };

    let mut hits = Vec::new();
    let mut draw = |font_system: &mut FontSystem, item: &ToolbarItem, text: &str, at: f32| {
        let mut buffer = shape_chrome_line(
            font_system,
            ctx,
            text,
            color(theme, item.tone),
            item.tone == ToolbarTone::Strong,
            true,
        );
        let drawn = buffer_width(&buffer).ceil();
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut buffer,
            (at.round() as i32, text_y),
            color(theme, item.tone),
        );
        if let Some(button) = item.button {
            // A button answers across half the gaps beside it too, so the
            // spaces between buttons are no dead zones.
            hits.push(ToolbarHit {
                button,
                left: x + at - gap / 2.0,
                right: x + at + drawn + gap / 2.0,
            });
        }
        drawn
    };

    // The right items first, since they stay whole and the left ones fit
    // into what they leave.
    let right_widths: Vec<f32> = view
        .right
        .iter()
        .map(|item| width_of(font_system, item, &item.text))
        .collect();
    let right_total: f32 =
        right_widths.iter().sum::<f32>() + gap * view.right.len().saturating_sub(1) as f32;
    let mut at = (w - pad - right_total).max(pad + ctx.cell_w * MIN_LEFT_CELLS);
    let right_start = at;
    for (item, item_w) in view.right.iter().zip(&right_widths) {
        draw(font_system, item, &item.text, at);
        at += item_w + gap;
    }

    let mut at = pad;
    for item in &view.left {
        let room = right_start - gap - at;
        if room <= 0.0 {
            break;
        }
        let mut text = item.text.clone();
        if item.shrinks {
            let chars: Vec<char> = item.text.chars().collect();
            let mut kept = chars.len();
            while kept > 0 && width_of(font_system, item, &text) > room {
                kept -= 1;
                let head: String = chars[..kept].iter().collect();
                text = format!("{}{ELLIPSIS}", head.trim_end());
            }
        }
        let drawn = draw(font_system, item, &text, at);
        at += drawn + gap;
    }

    let image = DropdownImage {
        height: canvas.1,
        rgba,
        width: canvas.0,
        x: x.round(),
        y: y.round(),
    };
    (image, hits)
}

/// The color an item of `tone` is drawn in.
fn color(theme: &Theme, tone: ToolbarTone) -> glyphon::Color {
    match tone {
        ToolbarTone::Accent => theme.cursor_bg.to_glyphon(),
        ToolbarTone::Dim => theme.ansi[8].to_glyphon(),
        ToolbarTone::Normal | ToolbarTone::Strong => theme.foreground.to_glyphon(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    fn item(text: &str, button: Option<u16>, shrinks: bool) -> ToolbarItem {
        ToolbarItem {
            button,
            shrinks,
            text: text.to_string(),
            tone: ToolbarTone::Strong,
        }
    }

    fn bar(width: f32) -> ToolbarView {
        ToolbarView {
            left: vec![item(&"A very long section title ".repeat(6), Some(7), true)],
            owner: 3,
            rect: (100.0, 40.0, width, 20.0),
            right: vec![item("−", Some(1), false), item("125%", Some(2), false)],
        }
    }

    #[test]
    fn test_the_bar_covers_its_band_and_keeps_its_buttons_apart_and_inside() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let (image, hits) = toolbar_rgba(
            &mut font_system,
            &mut swash,
            &sample_font_ctx(),
            &Theme::dark(),
            &bar(600.0),
        );
        assert_eq!((image.x, image.y, image.width, image.height), (100.0, 40.0, 600, 20));
        let buttons: Vec<u16> = hits.iter().map(|hit| hit.button).collect();
        assert_eq!(buttons, [1, 2, 7], "the right items, then the left");
        let title = hits[2];
        assert!(title.left >= 100.0, "{title:?}");
        assert!(title.right <= hits[0].left + 1.0, "the long title gives way: {hits:?}");
        assert!(hits[1].right <= 700.0 + 1.0, "inside the band: {hits:?}");
        assert!(hits[0].right <= hits[1].left + 1.0, "side by side");
    }
}
