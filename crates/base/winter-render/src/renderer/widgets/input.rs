//! The input dialog: a question, the field it is answered in, and the keys
//! that answer it, or a confirmation answered by one key.

use glyphon::cosmic_text::Wrap;
use glyphon::{Attrs, FontSystem, Shaping, SwashCache};

use crate::renderer::glyphs::{base_family, parse_weight, FontCtx};
use crate::renderer::raster::chrome::{
    centered, draw_caret, draw_field_well, title_bar_height, Chrome, TitleBar,
    INPUT_FIELD_TEXT_INSET, TITLE_BAR_TOP_PAD,
};
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line, ui_family};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::{
    editor_card_size, font_scale, DIALOG_MIN_WIDTH, DROPDOWN_SHADOW, POPUP_EDGE_ROOM,
};
use crate::theme::Theme;

use super::super::colors::mix_rgb;
use super::button::{draw_buttons_row, measure_button, measure_buttons_row, BUTTON_GAP_RATIO};
use super::view::{ButtonView, InputView};

// ========================================================================
// Constants
// ========================================================================

/// The default title for a confirmation dialog.
const CONFIRM_DEFAULT_TITLE: &str = "Confirmation";

/// How much larger than the text a confirmation's icon is drawn, beside its
/// message: about two lines tall, so the kind of question reads first.
const CONFIRM_ICON_SCALE: f32 = 2.0;

/// The space between a confirmation's icon and its message, in cell widths.
const CONFIRM_ICON_GAP_RATIO: f32 = 1.0;

/// The minimum and maximum width for a confirmation popup card.
const CONFIRM_MAX_WIDTH: f32 = 520.0;

/// The input dialog's width as a fraction of the surface.
///
/// Narrower than the palette: it holds one line of an answer rather than a
/// list of choices, and a box the width of the window for a filename reads as
/// a panel rather than as a question.
const INPUT_WIDTH_RATIO: f32 = 0.5;

/// The input dialog's padding inside its card, in cell heights.
const INPUT_PAD_RATIO: f32 = 0.9;

/// The space between the dialog's question, field, and keys, in cell heights.
const INPUT_GAP_RATIO: f32 = 0.6;

/// A keycap's padding around its key: across in cell widths, down in pixels.
const KEYCAP_PAD_X_RATIO: f32 = 0.5;
const KEYCAP_PAD_Y: f32 = 2.0;

/// A keycap's corner radius, in pixels.
const KEYCAP_RADIUS: f32 = 4.0;

/// How far a keycap leans from the card's color to the text's.
const KEYCAP_MIX: f32 = 0.14;

/// The space after a keycap before what it does, and between two keys, in
/// cell widths.
const KEYCAP_LABEL_GAP_RATIO: f32 = 0.5;
const KEYCAP_PAIR_GAP_RATIO: f32 = 1.5;

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the input dialog: a centered floating card holding one
/// question in bold, the field it is answered in, and the keys that answer
/// it as keycaps along the foot, right-aligned the way a modern dialog puts
/// its buttons. A question answered by one key, with no field, is a
/// confirmation, drawn as one (see [`confirm_rgba`]).
pub(in crate::renderer) fn input_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InputView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let has_buttons = !view.buttons.is_empty();
    if (view.input.is_none() && !view.keys.is_empty()) || has_buttons {
        return confirm_rgba(
            font_system,
            swash_cache,
            ctx,
            theme,
            view,
            surface_w,
            surface_h,
        );
    }
    let margin = DROPDOWN_SHADOW;
    let pad = (ctx.cell_h * INPUT_PAD_RATIO).round();
    let gap = (ctx.cell_h * INPUT_GAP_RATIO).round();
    let header_h = TITLE_BAR_TOP_PAD + title_bar_height(ctx);

    let field_h = match view.input {
        Some(_) => ctx.field_h,
        None => 0.0,
    };
    let keycap_h = ctx.line_height + KEYCAP_PAD_Y * 2.0;
    let keys_h = match view.keys.is_empty() {
        true => 0.0,
        false => keycap_h,
    };
    let has_subtitle = view.title.is_some();
    let subtitle_h = if has_subtitle {
        ctx.line_height + gap
    } else {
        0.0
    };
    // Each part below the question stands a gap below the one before it.
    let below: f32 = [field_h, keys_h]
        .iter()
        .filter(|&&h| h > 0.0)
        .map(|h| h + gap)
        .sum::<f32>()
        + subtitle_h;

    // On a card, the dialog is as wide as the card it replaces.
    let panel_w = match dialog_card(ctx, view) {
        Some((_, _, card_w, _)) => card_w,
        None => (surface_w * INPUT_WIDTH_RATIO).min(surface_w - POPUP_EDGE_ROOM),
    }
    .floor()
    .max(DIALOG_MIN_WIDTH * font_scale(ctx));
    let panel_h = (header_h + pad + below + pad).ceil();

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let left = margin + pad;
    let right = margin + panel_w - pad;

    // A dialog's decorated title bar.
    let title_text = view.title.as_deref().unwrap_or(&view.label);
    let icon = view.kind.map(|k| (k.icon(), k.color(theme)));
    let below_title = frame.draw_title_with_icon(
        font_system,
        swash_cache,
        ctx,
        theme,
        TitleBar::Decorated,
        title_text,
        "",
        icon,
    );

    let mut y = below_title + pad;
    if has_subtitle {
        let mut sub_buf = shape_chrome_line(font_system, ctx, &view.label, foreground, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
            &mut sub_buf,
            (left as i32, y as i32),
            foreground,
        );
        y += ctx.line_height + gap;
    }

    // The field answered in.
    if let Some(input) = &view.input {
        draw_field_well(&mut frame, theme, (left, y, right - left, field_h));

        let text_x = (left + ctx.cell_w * INPUT_FIELD_TEXT_INSET) as i32;
        let text_y = (y + ((field_h - ctx.line_height) / 2.0).max(0.0)) as i32;
        let mut input_buf = shape_chrome_line(font_system, ctx, input, foreground, false, true);
        let input_w = buffer_width(&input_buf).ceil() as i32;
        frame.draw_text(
            font_system,
            swash_cache,
            &mut input_buf,
            (text_x, text_y),
            foreground,
        );

        // A bar drawn directly rather than a glyph, which would sit centered
        // in a cell box and leave a gap after the last character typed. One
        // inside the text sits after the characters before it, measured by
        // shaping them alone.
        let before: String = input.chars().take(view.caret).collect();
        let caret_w = match before.len() == input.len() {
            true => input_w,
            false => {
                let before_buf =
                    shape_chrome_line(font_system, ctx, &before, foreground, false, true);
                buffer_width(&before_buf).ceil() as i32
            }
        };
        if view.caret_visible {
            let caret = ((text_x + caret_w) as f32 + 1.0, text_y as f32 + 1.0);
            draw_caret(&mut frame, ctx, theme, caret);
        }
        y += field_h + gap;
    }

    // The keys that answer, each a keycap with what it does beside it,
    // laid out from the right edge in.
    if !view.keys.is_empty() {
        let keycap_bg = mix_rgb(theme.menu_bg, theme.foreground, KEYCAP_MIX);
        let mut shaped: Vec<(glyphon::Buffer, f32, glyphon::Buffer, f32)> = view
            .keys
            .iter()
            .map(|(key, does)| {
                let key_buf = shape_chrome_line(font_system, ctx, key, foreground, true, true);
                let does_buf = shape_chrome_line(font_system, ctx, does, muted, false, true);
                let key_w = buffer_width(&key_buf).ceil();
                let does_w = buffer_width(&does_buf).ceil();
                (key_buf, key_w, does_buf, does_w)
            })
            .collect();
        let keycap_pad_x = (ctx.cell_w * KEYCAP_PAD_X_RATIO).round();
        let label_gap = (ctx.cell_w * KEYCAP_LABEL_GAP_RATIO).round();
        let pair_gap = (ctx.cell_w * KEYCAP_PAIR_GAP_RATIO).round();
        let widths: Vec<f32> = shaped
            .iter()
            .map(|(_, key_w, _, does_w)| key_w + keycap_pad_x * 2.0 + label_gap + does_w)
            .collect();
        let total = widths.iter().sum::<f32>() + pair_gap * (widths.len() - 1) as f32;
        let mut x = (right - total).max(left);
        for ((key_buf, key_w, does_buf, _), width) in shaped.iter_mut().zip(&widths) {
            let keycap_w = *key_w + keycap_pad_x * 2.0;
            frame.fill((x, y, keycap_w, keycap_h), KEYCAP_RADIUS, keycap_bg, 1.0);
            frame.draw_text(
                font_system,
                swash_cache,
                key_buf,
                ((x + keycap_pad_x) as i32, (y + KEYCAP_PAD_Y) as i32),
                foreground,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                does_buf,
                ((x + keycap_w + label_gap) as i32, (y + KEYCAP_PAD_Y) as i32),
                muted,
            );
            x += width + pair_gap;
        }
    }

    // Centred both ways, where the palette and the key-hint card sit: a
    // question interrupts whatever was being read, so it belongs where the
    // eye already is rather than at an edge. One a transient asked opens on
    // its card, where the eye already is.
    let (x, y) = dialog_origin(ctx, view, (panel_w, panel_h), (surface_w, surface_h));
    frame.into_image(x, y)
}

/// Shape a confirmation's icon, [`CONFIRM_ICON_SCALE`] times the text's
/// size, in the terminal family that carries the icon glyphs.
fn shape_icon(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    icon: &str,
    color: glyphon::Color,
) -> glyphon::Buffer {
    let size = ctx.font_size * CONFIRM_ICON_SCALE;
    let line_height = ctx.line_height * CONFIRM_ICON_SCALE;
    let mut buffer = glyphon::Buffer::new(font_system, glyphon::Metrics::new(size, line_height));
    buffer.set_size(Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new().family(base_family(ctx.family)).color(color);
    buffer.set_text(icon, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Shape multi-line wrapped text inside a dialog.
fn shape_wrapped_message(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: glyphon::Color,
    width: f32,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(width.max(ctx.cell_w)), None);
    let family = ui_family(font_system);
    let normal = parse_weight(ctx.normal_weight, glyphon::cosmic_text::Weight::NORMAL);
    let attrs = Attrs::new().family(family).weight(normal).color(color);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Resolve the button views for a confirmation dialog: either its explicit
/// buttons or synthetic buttons derived from its key hints.
pub(in crate::renderer) fn resolve_confirm_buttons(view: &InputView) -> Vec<ButtonView> {
    if !view.buttons.is_empty() {
        view.buttons.clone()
    } else {
        view.keys
            .iter()
            .enumerate()
            .map(|(i, (k, does))| ButtonView {
                focused: i == 0,
                primary: i == 0,
                shortcut: Some(k.clone()),
                text: does.clone(),
            })
            .collect()
    }
}

/// Compute the bounding boxes of confirmation buttons in screen coordinates.
/// Returns a vector of `(x, y, w, h)` matching the order of buttons in `buttons`.
pub(in crate::renderer) fn confirm_button_rects(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    buttons: &[ButtonView],
    card_rect: (f32, f32, f32, f32),
) -> Vec<(f32, f32, f32, f32)> {
    if buttons.is_empty() {
        return Vec::new();
    }
    let (card_x, card_y, panel_w, panel_h) = card_rect;
    let pad = (ctx.cell_h * INPUT_PAD_RATIO).round();
    let gap = (ctx.cell_w * BUTTON_GAP_RATIO).round();

    let measured: Vec<(f32, f32)> = buttons
        .iter()
        .map(|b| measure_button(font_system, ctx, b))
        .collect();

    let button_h = measured.iter().map(|(_, h)| *h).fold(0.0f32, f32::max);
    let right_x = card_x + panel_w - pad;
    let y = card_y + panel_h - pad - button_h;

    let mut rects = vec![(0.0, 0.0, 0.0, 0.0); buttons.len()];
    let mut current_right = right_x;

    for (i, (w, h)) in measured.into_iter().enumerate().rev() {
        let x = current_right - w;
        rects[i] = (x, y, w, h);
        current_right = x - gap;
    }

    rects
}

/// Rasterize a confirmation: a centered floating card holding a centered title,
/// the message below it (wrapping across multiple lines if long), with its width
/// adjusted to the content, and interactive buttons along the foot.
fn confirm_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InputView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let pad = (ctx.cell_h * INPUT_PAD_RATIO).round();
    let gap = (ctx.cell_h * INPUT_GAP_RATIO).round();
    let header_h = TITLE_BAR_TOP_PAD + title_bar_height(ctx);

    let buttons = resolve_confirm_buttons(view);

    let (buttons_w, button_h) = measure_buttons_row(font_system, ctx, &buttons);
    let foreground = theme.foreground.to_glyphon();

    // The title, measured so the dialog is wide enough for it.
    let title_text = view.title.as_deref().unwrap_or(match view.kind {
        Some(crate::NoticeKind::Warning) => "Warning",
        Some(crate::NoticeKind::Error) => "Error",
        _ => CONFIRM_DEFAULT_TITLE,
    });
    let title_buf = shape_chrome_line(font_system, ctx, title_text, foreground, true, true);
    let title_w = buffer_width(&title_buf).ceil();

    // The icon goes in the content, left of the message, as GNOME, macOS,
    // and Windows draw an alert's: it qualifies the message, and the title
    // bar is the dialog's name.
    let mut icon = view.kind.map(|kind| {
        let buf = shape_icon(
            font_system,
            ctx,
            kind.icon(),
            kind.color(theme).to_glyphon(),
        );
        let w = buffer_width(&buf).ceil();
        (buf, w)
    });
    let icon_col = icon.as_ref().map_or(0.0, |(_, w)| {
        w + (ctx.cell_w * CONFIRM_ICON_GAP_RATIO).round()
    });
    let icon_h = match icon {
        Some(_) => (ctx.line_height * CONFIRM_ICON_SCALE).ceil(),
        None => 0.0,
    };

    // Measure the raw message width if on a single line
    let single_line_buf = shape_chrome_line(font_system, ctx, &view.label, foreground, false, true);
    let raw_message_w = buffer_width(&single_line_buf).ceil();

    // Window width adjusts with content between minimum and maximum bounds
    let scale = font_scale(ctx);
    let min_panel_w = (buttons_w + pad * 2.0)
        .max(title_w + pad * 2.0)
        .max(DIALOG_MIN_WIDTH * scale);
    let max_panel_w = (surface_w - POPUP_EDGE_ROOM)
        .min(CONFIRM_MAX_WIDTH * scale)
        .max(min_panel_w);

    let content_w = raw_message_w + icon_col + pad * 2.0;
    let panel_w = if content_w <= max_panel_w {
        content_w.max(min_panel_w)
    } else {
        max_panel_w
    };

    let text_w = panel_w - pad * 2.0 - icon_col;
    let mut message_buf = shape_wrapped_message(font_system, ctx, &view.label, foreground, text_w);
    let line_count = message_buf.layout_runs().count().max(1);
    let message_h = line_count as f32 * ctx.line_height;
    let content_h = message_h.max(icon_h);

    let panel_h = (header_h + pad + content_h + gap + button_h + pad).ceil();
    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    let left = margin + pad;
    let right = margin + panel_w - pad;

    // 1. A dialog's decorated title bar, holding its title alone.
    let below_title = frame.draw_title_with_icon(
        font_system,
        swash_cache,
        ctx,
        theme,
        TitleBar::Decorated,
        title_text,
        "",
        None,
    );

    // 2. The icon, then the message beside it, a line of it centered on
    // the icon; with no icon, one line is centered in the card.
    let mut y = below_title + pad;
    if let Some((buf, _)) = icon.as_mut() {
        let color = view
            .kind
            .map_or(foreground, |kind| kind.color(theme).to_glyphon());
        frame.draw_text(
            font_system,
            swash_cache,
            buf,
            (left as i32, y as i32),
            color,
        );
    }
    let msg_x = match (icon.is_some(), line_count) {
        (true, _) => left + icon_col,
        (false, 1) => margin + ((panel_w - raw_message_w) / 2.0).floor(),
        (false, _) => left,
    };
    let msg_y = y + ((content_h - message_h) / 2.0).floor();
    frame.draw_text(
        font_system,
        swash_cache,
        &mut message_buf,
        (msg_x as i32, msg_y as i32),
        foreground,
    );
    y += content_h + gap;

    // 3. Buttons along the foot, right-aligned
    draw_buttons_row(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        theme,
        &buttons,
        (right, y),
    );

    let (x, y) = dialog_origin(ctx, view, (panel_w, panel_h), (surface_w, surface_h));
    frame.into_image(x, y)
}

/// The card a dialog for `view` opens on: the transient card that asked,
/// else the editor card in the asking editor's pane; `None` for the
/// window's own questions.
fn dialog_card(ctx: &FontCtx, view: &InputView) -> Option<(f32, f32, f32, f32)> {
    view.card.or_else(|| {
        view.pane.map(|(pane_x, pane_y, pane_w, pane_h)| {
            let (card_w, card_h) = editor_card_size(ctx, (pane_w, pane_h));
            (
                (pane_x + (pane_w - card_w) / 2.0).floor(),
                (pane_y + (pane_h - card_h) / 2.0).floor(),
                card_w,
                card_h,
            )
        })
    })
}

/// Where a `size` dialog for `view` goes in a `surface` window: on its card
/// (see [`dialog_card`]), at its top and across its middle, kept on the
/// window; else centred on the window.
fn dialog_origin(
    ctx: &FontCtx,
    view: &InputView,
    size: (f32, f32),
    surface: (f32, f32),
) -> (f32, f32) {
    let ((panel_w, panel_h), (surface_w, surface_h)) = (size, surface);
    match dialog_card(ctx, view) {
        Some((card_x, card_y, card_w, _)) => (
            (card_x + (card_w - panel_w) / 2.0)
                .clamp(0.0, (surface_w - panel_w).max(0.0))
                .floor(),
            card_y.clamp(0.0, (surface_h - panel_h).max(0.0)).floor(),
        ),
        None => centered(surface_w, surface_h, panel_w, panel_h),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;
    use crate::theme::Rgb;

    #[test]
    fn test_the_input_dialog_is_centered_and_only_as_tall_as_it_needs() {
        // It holds one question, so it is sized to the question: the palette's
        // fixed height would put a filename prompt in a panel most of which is
        // the empty list it does not have.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);

        let typed = InputView {
            pane: None,
            card: None,
            buttons: Vec::new(),
            caret: 8,
            caret_visible: true,
            input: Some("notes.md".to_string()),
            keys: vec![
                ("Enter".to_string(), "accept".to_string()),
                ("Esc".to_string(), "cancel".to_string()),
            ],
            label: "New file".to_string(),
            title: None,
            kind: None,
        };
        let image = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &typed,
            surface_w,
            surface_h,
        );
        let center_x = image.x + image.width as f32 / 2.0;
        let center_y = image.y + image.height as f32 / 2.0;
        assert!((center_x - surface_w / 2.0).abs() < 2.0, "centered across");
        assert!((center_y - surface_h / 2.0).abs() < 2.0, "and down");
        assert!(
            (image.height as f32) < surface_h * 0.3,
            "a question is not a panel: {} of {surface_h}",
            image.height
        );
        let middle = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[middle + 3], 255, "the panel interior is opaque");

        // A question answered by one key has no line to type on, so it is
        // shorter than one that does.
        let confirmed = InputView {
            pane: None,
            card: None,
            buttons: Vec::new(),
            caret: 0,
            caret_visible: true,
            input: None,
            keys: vec![
                ("y".to_string(), "Yes".to_string()),
                ("any key".to_string(), "No".to_string()),
            ],
            label: "Delete 3 entries?".to_string(),
            title: None,
            kind: None,
        };
        let shorter = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &confirmed,
            surface_w,
            surface_h,
        );
        assert!(
            shorter.height < image.height,
            "{} is not shorter than {}",
            shorter.height,
            image.height
        );
    }

    #[test]
    fn test_a_question_a_transient_asked_opens_on_its_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let card = (260.0, 180.0, 480.0, 360.0);
        let view = InputView {
            pane: None,
            buttons: Vec::new(),
            caret: 0,
            caret_visible: true,
            card: Some(card),
            input: Some(String::new()),
            keys: Vec::new(),
            kind: None,
            label: "Create and checkout".to_string(),
            title: None,
        };
        let image = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            1000.0,
            800.0,
        );
        assert_eq!(image.x + DROPDOWN_SHADOW, 260.0, "at the card's left edge");
        assert_eq!(image.y + DROPDOWN_SHADOW, 180.0, "and its top");
        assert_eq!(image.width as f32, 480.0 + 2.0 * DROPDOWN_SHADOW, "as wide");
    }

    #[test]
    fn test_an_editor_s_question_opens_on_the_editor_card_in_its_pane() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let pane = (500.0, 0.0, 500.0, 800.0);
        let view = InputView {
            buttons: Vec::new(),
            caret: 0,
            caret_visible: true,
            card: None,
            input: Some(String::new()),
            keys: Vec::new(),
            kind: None,
            label: "Rename to".to_string(),
            pane: Some(pane),
            title: None,
        };
        let image = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            1000.0,
            800.0,
        );
        let (card_w, card_h) = editor_card_size(&ctx, (500.0, 800.0));
        assert_eq!(
            image.width as f32,
            card_w + 2.0 * DROPDOWN_SHADOW,
            "the card's width"
        );
        let card_x = (500.0 + (500.0 - card_w) / 2.0).floor();
        let card_y = ((800.0 - card_h) / 2.0).floor();
        assert_eq!(image.x + DROPDOWN_SHADOW, card_x, "on the card in the pane");
        assert_eq!(image.y + DROPDOWN_SHADOW, card_y, "at its top");
    }

    #[test]
    fn test_a_confirmation_has_no_left_stripe_and_focused_button_is_filled() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let view = |input: Option<&str>| InputView {
            pane: None,
            card: None,
            buttons: vec![
                ButtonView {
                    focused: true,
                    primary: true,
                    shortcut: Some("y".to_string()),
                    text: "Yes".to_string(),
                },
                ButtonView {
                    focused: false,
                    primary: false,
                    shortcut: Some("n".to_string()),
                    text: "No".to_string(),
                },
            ],
            caret: 0,
            caret_visible: false,
            input: input.map(String::from),
            keys: Vec::new(),
            label: "Delete 3 entries?".to_string(),
            title: None,
            kind: None,
        };
        let draw = |font_system: &mut FontSystem, swash: &mut SwashCache, view: &InputView| {
            input_rgba(font_system, swash, &ctx, &theme, view, 1000.0, 800.0)
        };
        let has = |image: &DropdownImage, color: Rgb| {
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[3] == 255 && (px[0], px[1], px[2]) == (color.r, color.g, color.b))
        };
        let edge = |image: &DropdownImage| {
            let (x, y) = (DROPDOWN_SHADOW as u32 + 1, image.height / 2);
            let i = ((y * image.width + x) * 4) as usize;
            Rgb::new(image.rgba[i], image.rgba[i + 1], image.rgba[i + 2])
        };

        let confirm = draw(&mut font_system, &mut swash, &view(None));
        let typed = draw(&mut font_system, &mut swash, &view(Some("x")));
        let warning = theme.ansi[3];
        assert_ne!(edge(&confirm), warning, "no left stripe on confirmation");
        assert_ne!(edge(&typed), warning, "a typed question is not marked");
        assert!(
            has(&confirm, theme.active_indicator),
            "the focused button is filled with accent"
        );
    }

    #[test]
    fn test_confirmation_width_adjusts_and_wraps_long_message() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();

        let short_view = InputView {
            pane: None,
            card: None,
            buttons: vec![ButtonView {
                focused: true,
                primary: true,
                shortcut: Some("y".to_string()),
                text: "Yes".to_string(),
            }],
            caret: 0,
            caret_visible: false,
            input: None,
            keys: Vec::new(),
            label: "Save?".to_string(),
            title: Some("Confirm".to_string()),
            kind: None,
        };
        let short_img = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &short_view,
            1000.0,
            800.0,
        );

        let long_view = InputView {
            pane: None,
            card: None,
            buttons: vec![ButtonView {
                focused: true,
                primary: true,
                shortcut: Some("y".to_string()),
                text: "Yes".to_string(),
            }],
            caret: 0,
            caret_visible: false,
            input: None,
            keys: Vec::new(),
            label: "This is a very long confirmation message explaining that multiple files have unsaved changes and closing without saving will permanently lose all modifications made during this session.".to_string(),
            title: Some("Discard Changes".to_string()),
            kind: None,
        };
        let long_img = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &long_view,
            1000.0,
            800.0,
        );

        // The short confirmation width is compact (close to min bound)
        assert!(
            short_img.width < long_img.width,
            "short {} should be narrower than long {}",
            short_img.width,
            long_img.width
        );
        // The long confirmation wraps and is taller
        assert!(
            long_img.height > short_img.height,
            "long {} should be taller than short {}",
            long_img.height,
            short_img.height
        );
        // The long confirmation width is bounded by max width
        // The widest a confirmation grows, scaled with the font as every card is.
        let max_w = CONFIRM_MAX_WIDTH * font_scale(&ctx);
        assert!(long_img.width as f32 <= max_w + DROPDOWN_SHADOW * 2.0);
    }

    #[test]
    fn test_confirm_button_rects_computation_and_hit_bounds() {
        let mut font_system = FontSystem::new();
        let ctx = sample_font_ctx();
        let buttons = vec![
            ButtonView {
                focused: true,
                primary: true,
                shortcut: Some("y".to_string()),
                text: "Yes".to_string(),
            },
            ButtonView {
                focused: false,
                primary: false,
                shortcut: Some("n".to_string()),
                text: "No".to_string(),
            },
        ];
        let card_rect = (100.0, 100.0, 400.0, 200.0);
        let rects = confirm_button_rects(&mut font_system, &ctx, &buttons, card_rect);
        assert_eq!(rects.len(), 2);

        let (x0, y0, w0, h0) = rects[0];
        let (x1, y1, w1, h1) = rects[1];

        // Button 0 (Yes) is to the left of button 1 (No)
        assert!(x0 + w0 < x1, "button 0 ends before button 1 starts");
        // Both buttons share the same Y coordinate and height
        assert_eq!(y0, y1);
        assert_eq!(h0, h1);
        // Both buttons are inside the card bounds
        assert!(x0 >= card_rect.0);
        assert!(x1 + w1 <= card_rect.0 + card_rect.2);
        assert!(y0 + h0 <= card_rect.1 + card_rect.3);
    }

    #[test]
    fn test_a_confirmation_draws_its_warning_or_error_icon() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();

        let warning_view = InputView {
            pane: None,
            card: None,
            buttons: vec![ButtonView {
                focused: true,
                primary: true,
                shortcut: Some("y".to_string()),
                text: "Yes".to_string(),
            }],
            caret: 0,
            caret_visible: false,
            input: None,
            keys: Vec::new(),
            label: "Close without saving?".to_string(),
            title: Some("Warning".to_string()),
            kind: Some(crate::NoticeKind::Warning),
        };
        let warning_img = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &warning_view,
            1000.0,
            800.0,
        );
        let found_warning_pixels = warning_img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 140 && px[1] > 80 && px[2] < 60)
            .count();
        assert!(found_warning_pixels > 0, "warning icon was rasterized");

        let error_view = InputView {
            pane: None,
            card: None,
            buttons: vec![ButtonView {
                focused: true,
                primary: true,
                shortcut: Some("y".to_string()),
                text: "Yes".to_string(),
            }],
            caret: 0,
            caret_visible: false,
            input: None,
            keys: Vec::new(),
            label: "Permanent data loss will occur.".to_string(),
            title: Some("Error".to_string()),
            kind: Some(crate::NoticeKind::Error),
        };
        let error_img = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &error_view,
            1000.0,
            800.0,
        );
        let found_error_pixels = error_img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 140 && px[1] < 80 && px[2] < 80)
            .count();
        assert!(found_error_pixels > 0, "error icon was rasterized");
    }
}
