//! The info tooltip: what a language server says of a name, the problems found
//! where the cursor or the pointer is, or the signature being typed, in the
//! editor's own font on a card hung over the word, or under it where the
//! window has no room above. What kind of thing the card is about (a
//! problem's kind and what found it) sits apart at its foot, dim, under a
//! hairline, as VS Code's hover says `rustc(E0308)` below the message.

use glyphon::{FontSystem, SwashCache};

use super::view::InfoView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::chrome::{draw_divider, Chrome};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// The most lines shown; past them the card ends in an ellipsis.
const MAX_LINES: usize = 18;

/// The widest a line is drawn, in characters, before it is folded.
const MAX_LINE_CHARS: usize = 90;

/// The room around the text, in pixels.
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 5.0;

/// What ends a card cut short.
const MORE: &str = "…";

/// The ANSI slot the foot is drawn in: bright black, the dim gray the
/// completion popup says a candidate's detail in.
const FOOTER_ANSI: usize = 8;

/// The height of the hairline over the foot, in pixels.
const DIVIDER_H: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Where a card's parts go, down from the panel's top edge, in pixels.
#[derive(Debug, PartialEq)]
struct CardLayout {
    /// The hairline over the foot, for a card with one.
    divider: Option<f32>,
    /// The foot's first line.
    footer_top: f32,
    /// The whole panel's height.
    height: f32,
}

// ========================================================================
// Free functions
// ========================================================================

/// `text` as the card's lines: each folded at a space before
/// [`MAX_LINE_CHARS`] where it can be, else at the limit, and the whole cut
/// to [`MAX_LINES`].
pub(in crate::renderer) fn card_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut rest: Vec<char> = line.trim_end().chars().collect();
        loop {
            if rest.len() <= MAX_LINE_CHARS {
                lines.push(rest.iter().collect());
                break;
            }
            let cut = rest[..MAX_LINE_CHARS]
                .iter()
                .rposition(|c| *c == ' ')
                .filter(|at| *at > 0)
                .unwrap_or(MAX_LINE_CHARS);
            lines.push(rest[..cut].iter().collect());
            rest = rest[cut..].iter().skip_while(|c| **c == ' ').copied().collect();
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        lines.push(MORE.to_string());
    }
    lines
}

/// Where the parts of a card of `body` lines and `footer` lines of `line_h`
/// pixels each go: the body padded round, then, for a foot, a hairline and
/// the foot padded round in its turn.
fn card_layout(body: usize, footer: usize, line_h: f32) -> CardLayout {
    let body_bottom = PAD_Y + line_h * body as f32 + PAD_Y;
    match footer {
        0 => CardLayout {
            divider: None,
            footer_top: body_bottom,
            height: body_bottom,
        },
        _ => {
            let footer_top = body_bottom + DIVIDER_H + PAD_Y;
            CardLayout {
                divider: Some(body_bottom),
                footer_top,
                height: footer_top + line_h * footer as f32 + PAD_Y,
            }
        }
    }
}

/// Rasterize the card `view` describes, inside a `surface_w` by `surface_h`
/// window.
pub(in crate::renderer) fn info_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InfoView,
    surface_w: f32,
    surface_h: f32,
) -> Option<DropdownImage> {
    let lines = card_lines(&view.text);
    let footer = view.footer.as_deref().map(card_lines).unwrap_or_default();
    if lines.is_empty() && footer.is_empty() {
        return None;
    }
    let margin = DROPDOWN_SHADOW;
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[FOOTER_ANSI].to_glyphon();
    let width = lines
        .iter()
        .chain(&footer)
        .map(|line| buffer_width(&shape_chrome_line(font_system, ctx, line, foreground, false, false)).ceil())
        .fold(0.0, f32::max);
    let line_h = ctx.line_height.round();
    let layout = card_layout(lines.len(), footer.len(), line_h);
    let panel_w = (width + 2.0 * PAD_X).min(surface_w - 2.0 * margin).ceil();
    let panel_h = layout.height.ceil();

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);
    let mut draw = |frame: &mut Chrome, line: &str, top: f32, color| {
        let y = (margin + top).round() as i32;
        let mut shaped = shape_chrome_line(font_system, ctx, line, color, false, false);
        frame.draw_text(font_system, swash_cache, &mut shaped, ((margin + PAD_X) as i32, y), color);
    };
    for (at, line) in lines.iter().enumerate() {
        draw(&mut frame, line, PAD_Y + line_h * at as f32, foreground);
    }
    if let Some(divider) = layout.divider {
        draw_divider(&mut frame, theme, margin + divider, panel_w);
    }
    for (at, line) in footer.iter().enumerate() {
        draw(&mut frame, line, layout.footer_top + line_h * at as f32, muted);
    }

    let (left, top, bottom) = view.anchor;
    let x = left.min(surface_w - panel_w - margin).max(0.0);
    let above = top - panel_h - 2.0;
    let y = match above >= 0.0 {
        true => above,
        false => (bottom + 2.0).min((surface_h - panel_h).max(0.0)),
    };
    Some(frame.into_image(x, y))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_a_long_line_folds_at_a_space_and_a_long_card_is_cut_short() {
        let long = format!("{} tail", "word ".repeat(20));
        let lines = card_lines(&long);
        assert!(lines.iter().all(|line| line.chars().count() <= MAX_LINE_CHARS));
        assert!(lines[0].ends_with("word"), "folded at a space: {:?}", lines[0]);
        let many = "line\n".repeat(40);
        let lines = card_lines(&many);
        assert_eq!(lines.len(), MAX_LINES + 1);
        assert_eq!(lines.last().map(String::as_str), Some(MORE));
        assert!(card_lines("\n\n").is_empty());
    }

    #[test]
    fn test_the_card_hangs_over_the_word_or_under_it_near_the_top() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let theme = Theme::dark();
        let view = |top: f32| InfoView {
            anchor: (100.0, top, top + 18.0),
            footer: None,
            text: "fn f() -> u8\n\nDoes a thing.".to_string(),
        };
        let over = info_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(400.0), 1000.0, 800.0)
            .expect("a card");
        assert!(over.y + over.height as f32 <= 400.0 + DROPDOWN_SHADOW, "over it: {}", over.y);
        let under = info_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(5.0), 1000.0, 800.0)
            .expect("a card");
        assert!(under.y >= 23.0 - DROPDOWN_SHADOW, "under it: {}", under.y);
    }

    #[test]
    fn test_the_foot_sits_under_a_hairline_below_every_line_of_the_body() {
        let line_h = 18.0;
        let plain = card_layout(3, 0, line_h);
        assert_eq!(plain.divider, None);
        let footed = card_layout(3, 2, line_h);
        let divider = footed.divider.expect("a hairline over the foot");
        assert!(divider >= PAD_Y + 3.0 * line_h, "under the body: {divider}");
        assert!(footed.footer_top > divider, "the foot under the hairline");
        assert_eq!(footed.height, footed.footer_top + 2.0 * line_h + PAD_Y);
        assert!(footed.height > plain.height);

        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let view = InfoView {
            anchor: (100.0, 400.0, 418.0),
            footer: Some("error rustc(E0308)".to_string()),
            text: String::new(),
        };
        let card = info_rgba(&mut font_system, &mut swash, &ctx, &Theme::dark(), &view, 1000.0, 800.0);
        assert!(card.is_some(), "a foot alone is still a card");
    }
}
