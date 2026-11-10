//! The info tooltip: what a language server says of a name, the problems found
//! where the cursor or the pointer is, or the signature being typed, in the
//! editor's own font on a card hung over the word, or under it where the
//! window has no room above. What kind of thing the card is about (a
//! problem's kind and what found it) sits apart at its foot, dim, under a
//! hairline, as VS Code's hover says `rustc(E0308)` below the message.

use glyphon::cosmic_text::{Style, Weight};
use glyphon::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};

use super::view::{InfoSpan, InfoView};
use crate::renderer::glyphs::{base_family, effective_bold_weight, parse_weight, FontCtx};
use crate::renderer::raster::chrome::{draw_divider, Chrome};
use crate::renderer::raster::paint::buffer_width;
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::{Rgb, Theme};

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

/// One line a card draws: its text, and where that starts in the card's
/// text, as a line and a char of it, to find the colours it is drawn in.
#[derive(Debug, PartialEq)]
struct CardLine {
    /// The line and the char it starts at; `None` for the ellipsis ending a
    /// card cut short.
    from: Option<(usize, usize)>,
    text: String,
}

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
fn card_lines(text: &str) -> Vec<CardLine> {
    let mut lines: Vec<CardLine> = Vec::new();
    for (row, line) in text.lines().enumerate() {
        let mut rest: Vec<char> = line.trim_end().chars().collect();
        let mut from = 0;
        loop {
            if rest.len() <= MAX_LINE_CHARS {
                lines.push(CardLine {
                    from: Some((row, from)),
                    text: rest.iter().collect(),
                });
                break;
            }
            let cut = rest[..MAX_LINE_CHARS]
                .iter()
                .rposition(|c| *c == ' ')
                .filter(|at| *at > 0)
                .unwrap_or(MAX_LINE_CHARS);
            lines.push(CardLine {
                from: Some((row, from)),
                text: rest[..cut].iter().collect(),
            });
            let spaces = rest[cut..].iter().take_while(|c| **c == ' ').count();
            from += cut + spaces;
            rest.drain(..cut + spaces);
        }
    }
    while lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        lines.push(CardLine {
            from: None,
            text: MORE.to_string(),
        });
    }
    lines
}

/// `line` split into runs of one look each, as `spans`, the runs of the
/// whole line it was folded from, give them; plain, in `plain`, where they
/// say nothing.
fn line_runs(line: &CardLine, spans: &[Vec<InfoSpan>], plain: Rgb) -> Vec<InfoSpan> {
    let looks: Vec<(Rgb, bool, bool)> = match line.from {
        Some((row, from)) => spans
            .get(row)
            .into_iter()
            .flatten()
            .flat_map(|span| {
                let look = (span.color, span.bold, span.italic);
                span.text.chars().map(move |_| look)
            })
            .skip(from)
            .collect(),
        None => Vec::new(),
    };
    let mut runs: Vec<InfoSpan> = Vec::new();
    for (at, c) in line.text.chars().enumerate() {
        let (color, bold, italic) = looks.get(at).copied().unwrap_or((plain, false, false));
        match runs.last_mut() {
            Some(run) if (run.color, run.bold, run.italic) == (color, bold, italic) => {
                run.text.push(c)
            }
            Some(_) | None => runs.push(InfoSpan {
                bold,
                color,
                italic,
                text: c.to_string(),
            }),
        }
    }
    runs
}

/// Shape `runs` as one line in the editor's font, each in its look: in one
/// buffer, so every glyph takes the same fallback font as it would alone.
fn shape_runs(font_system: &mut FontSystem, ctx: &FontCtx, runs: &[InfoSpan]) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(ctx.font_size, ctx.line_height));
    buffer.set_size(Some(f32::MAX), Some(ctx.line_height));
    let family = base_family(ctx.family);
    let normal = parse_weight(ctx.normal_weight, Weight::NORMAL);
    let heavy = effective_bold_weight(ctx.bold_weight, ctx.font_has_bold);
    let plain = Attrs::new().family(family).weight(normal);
    let attrs = |run: &InfoSpan| {
        let attrs = Attrs::new()
            .family(family)
            .color(run.color.to_glyphon())
            .weight(match run.bold {
                true => heavy,
                false => normal,
            });
        match run.italic {
            true => attrs.style(Style::Italic),
            false => attrs,
        }
    };
    buffer.set_rich_text(
        runs.iter().map(|run| (run.text.as_str(), attrs(run))),
        &plain,
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
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
    let muted = theme.ansi[FOOTER_ANSI];
    let mut shape = |lines: &[CardLine], spans: &[Vec<InfoSpan>], plain: Rgb| -> Vec<Buffer> {
        lines
            .iter()
            .map(|line| shape_runs(font_system, ctx, &line_runs(line, spans, plain)))
            .collect()
    };
    let mut body = shape(&lines, &view.spans, theme.foreground);
    let mut foot = shape(&footer, &[], muted);
    let width = body
        .iter()
        .chain(&foot)
        .map(|buffer| buffer_width(buffer).ceil())
        .fold(0.0, f32::max);
    let line_h = ctx.line_height.round();
    let layout = card_layout(body.len(), foot.len(), line_h);
    let panel_w = (width + 2.0 * PAD_X).min(surface_w - 2.0 * margin).ceil();
    let panel_h = layout.height.ceil();

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);
    let foreground = theme.foreground.to_glyphon();
    let mut draw = |frame: &mut Chrome, buffer: &mut Buffer, top: f32| {
        let y = (margin + top).round() as i32;
        let x = (margin + PAD_X) as i32;
        frame.draw_text(font_system, swash_cache, buffer, (x, y), foreground);
    };
    for (at, buffer) in body.iter_mut().enumerate() {
        draw(&mut frame, buffer, PAD_Y + line_h * at as f32);
    }
    if let Some(divider) = layout.divider {
        draw_divider(&mut frame, theme, margin + divider, panel_w);
    }
    for (at, buffer) in foot.iter_mut().enumerate() {
        draw(&mut frame, buffer, layout.footer_top + line_h * at as f32);
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
        assert!(lines
            .iter()
            .all(|line| line.text.chars().count() <= MAX_LINE_CHARS));
        assert!(
            lines[0].text.ends_with("word"),
            "folded at a space: {:?}",
            lines[0].text
        );
        let many = "line\n".repeat(40);
        let lines = card_lines(&many);
        assert_eq!(lines.len(), MAX_LINES + 1);
        assert_eq!(lines.last().map(|line| line.text.as_str()), Some(MORE));
        assert!(card_lines("\n\n").is_empty());
    }

    #[test]
    fn test_a_folded_line_keeps_the_colours_of_its_own_chars() {
        let (red, blue, plain) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255), Rgb::new(9, 9, 9));
        let head = "a".repeat(MAX_LINE_CHARS - 1);
        let text = format!("{head} bb");
        let spans = vec![vec![
            InfoSpan {
                bold: false,
                color: red,
                italic: false,
                text: format!("{head} "),
            },
            InfoSpan {
                bold: true,
                color: blue,
                italic: false,
                text: "b".to_string(),
            },
        ]];
        let lines = card_lines(&text);
        assert_eq!(lines[1].from, Some((0, MAX_LINE_CHARS)));
        let runs: Vec<(String, Rgb, bool)> = line_runs(&lines[1], &spans, plain)
            .into_iter()
            .map(|run| (run.text, run.color, run.bold))
            .collect();
        assert_eq!(
            runs,
            vec![
                ("b".to_string(), blue, true),
                ("b".to_string(), plain, false)
            ],
            "past the fold, and past the runs"
        );
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
            spans: Vec::new(),
            text: "fn f() -> u8\n\nDoes a thing.".to_string(),
        };
        let over = info_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view(400.0),
            1000.0,
            800.0,
        )
        .expect("a card");
        assert!(
            over.y + over.height as f32 <= 400.0 + DROPDOWN_SHADOW,
            "over it: {}",
            over.y
        );
        let under = info_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view(5.0),
            1000.0,
            800.0,
        )
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
            spans: Vec::new(),
            text: String::new(),
        };
        let card = info_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            1000.0,
            800.0,
        );
        assert!(card.is_some(), "a foot alone is still a card");
    }
}
