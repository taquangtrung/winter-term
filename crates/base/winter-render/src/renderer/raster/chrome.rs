//! The chrome every floating window the renderer still draws is drawn in,
//! after VS Code's widget chrome: one soft shadow, one hairline border, one
//! surface color, and one title bar shape, so every kind reads as one
//! family. Each rasterizer lays out and paints only its own contents. The
//! picker, the key hints, the completion list, and the info card are
//! misty's widgets, drawn by the app.
//!
//! - A **dialog** (a question typed or confirmed) is headed by a
//!   [`TitleBar::Decorated`] bar, the title centred across and down it.
//! - A **popup** (the fixes) and a **tooltip** (a link's address, the scopes
//!   under the cursor) have no title bar, and hang from the cell they are
//!   about, placed by [`anchored`].

use std::cell::RefCell;
use std::collections::VecDeque;

use glyphon::{Color, FontSystem, SwashCache};

use super::paint::{
    blend_px, buffer_width, composite_buffer, fill_rounded_rect, rounded_rect_sdf,
    shape_chrome_line, truncate_label,
};
use super::view::DropdownImage;
use super::PALETTE_ITEM_PAD_Y;
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// How many empty windows [`Chrome::new`] keeps painted: one for each
/// overlay up at once (a picker and its action menu, a dialog, a toast).
const EMPTY_WINDOWS_KEPT: usize = 8;

thread_local! {
    /// The empty windows painted lately, by their size and colors: the shadow
    /// and the rounded surface cost a pass over every pixel, the same each
    /// time a picker is drawn again, so it is copied rather than painted.
    static EMPTY_WINDOWS: RefCell<VecDeque<(EmptyWindow, Vec<u8>)>> =
        const { RefCell::new(VecDeque::new()) };
}

/// Corner radius of the dropdown menu panel and its hover highlight, in pixels.
const DROPDOWN_RADIUS: f32 = 4.0;

/// Peak opacity of the dropdown drop shadow, fading to zero at its outer edge.
const DROPDOWN_SHADOW_ALPHA: f32 = 0.28;

/// Color of the dropdown drop shadow (alpha is applied per pixel).
const SHADOW_COLOR: Rgb = Rgb::new(0, 0, 0);

/// Space between a popup's top edge and its title bar, in pixels. Small, as
/// in VS Code's quick input: the title bar carries its own
/// [`PALETTE_ITEM_PAD_Y`] above the text, so more would leave the title
/// floating well below the edge.
pub(in crate::renderer) const TITLE_BAR_TOP_PAD: f32 = 2.0;

/// How much of the divider color to mix into the menu background for the
/// popup title header bar, elevating it from the card body.
const HEADER_BG_MIX: f32 = 0.40;

/// Width of the popup's hairline border, in pixels.
const BORDER_W: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A floating window's pixels: a panel of a given size, inside a transparent margin
/// that holds its drop shadow. Coordinates passed to its drawing methods are
/// canvas coordinates, so the panel's top-left corner is `(margin, margin)`.
pub(in crate::renderer) struct Chrome {
    canvas: (u32, u32),
    margin: f32,
    panel_w: f32,
    rgba: Vec<u8>,
}

/// What an empty window looks like: its size, and the colors of its border
/// and surface.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EmptyWindow {
    border: Rgb,
    margin: f32,
    panel_h: f32,
    panel_w: f32,
    surface: Rgb,
}

/// How a centred window's title bar is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::renderer) enum TitleBar {
    /// A tinted bar, the title bold and centred across it and down it: a
    /// picker's or a dialog's, which ask something.
    Decorated,
}

// ========================================================================
// Chrome Implementations
// ========================================================================

impl Chrome {
    /// Paint the empty window: the shadow fading out across `margin`, the
    /// hairline border, and the surface inset one border width within it.
    pub(in crate::renderer) fn new(theme: &Theme, panel_w: f32, panel_h: f32, margin: f32) -> Self {
        let width = (panel_w + 2.0 * margin) as u32;
        let height = (panel_h + 2.0 * margin) as u32;
        let empty = EmptyWindow {
            border: theme.divider,
            margin,
            panel_h,
            panel_w,
            surface: theme.menu_bg,
        };
        let kept = EMPTY_WINDOWS.with(|kept| {
            let kept = kept.borrow();
            let (_, rgba) = kept.iter().find(|(window, _)| *window == empty)?;
            Some(rgba.clone())
        });
        if let Some(rgba) = kept {
            return Self {
                canvas: (width, height),
                margin,
                panel_w,
                rgba,
            };
        }
        let frame = Self::paint_empty(theme, panel_w, panel_h, margin);
        EMPTY_WINDOWS.with(|kept| {
            let mut kept = kept.borrow_mut();
            if kept.len() == EMPTY_WINDOWS_KEPT {
                kept.pop_back();
            }
            kept.push_front((empty, frame.rgba.clone()));
        });
        frame
    }

    /// Paint the empty window [`Chrome::new`] gives, pixel by pixel.
    fn paint_empty(theme: &Theme, panel_w: f32, panel_h: f32, margin: f32) -> Self {
        let width = (panel_w + 2.0 * margin) as u32;
        let height = (panel_h + 2.0 * margin) as u32;
        let mut frame = Self {
            canvas: (width, height),
            margin,
            panel_w,
            rgba: vec![0u8; (width * height * 4) as usize],
        };
        let panel_rect = (margin, margin, panel_w, panel_h);
        for py in 0..height {
            for px in 0..width {
                let sdf = rounded_rect_sdf(
                    px as f32 + 0.5,
                    py as f32 + 0.5,
                    panel_rect,
                    DROPDOWN_RADIUS,
                );
                let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
                if sdf <= 0.0 || falloff <= 0.0 {
                    continue;
                }
                let idx = ((py * width + px) * 4) as usize;
                blend_px(
                    &mut frame.rgba,
                    idx,
                    SHADOW_COLOR,
                    falloff * falloff * DROPDOWN_SHADOW_ALPHA,
                );
            }
        }
        frame.fill(panel_rect, DROPDOWN_RADIUS, theme.divider, 1.0);
        frame.fill(
            (
                margin + BORDER_W,
                margin + BORDER_W,
                panel_w - 2.0 * BORDER_W,
                panel_h - 2.0 * BORDER_W,
            ),
            DROPDOWN_RADIUS - BORDER_W,
            theme.menu_bg,
            1.0,
        );
        frame
    }

    /// The width of the transparent shadow margin around the panel.
    pub(in crate::renderer) fn margin(&self) -> f32 {
        self.margin
    }

    /// The raw pixels and their `(width, height)`, for tests that look at
    /// what was painted.
    #[cfg(test)]
    pub(in crate::renderer) fn pixels(&mut self) -> (&mut [u8], (u32, u32)) {
        (&mut self.rgba, self.canvas)
    }

    /// Fill a rounded rectangle, `rect` as `(x, y, w, h)` in canvas pixels.
    pub(in crate::renderer) fn fill(
        &mut self,
        rect: (f32, f32, f32, f32),
        radius: f32,
        color: Rgb,
        alpha: f32,
    ) {
        fill_rounded_rect(&mut self.rgba, self.canvas, rect, radius, color, alpha);
    }

    /// Fill the header background across the panel's top: rounded on the top
    /// corners to match the panel's corner radius, and flat on the bottom edge
    /// where the divider sits.
    pub(in crate::renderer) fn draw_header_background(&mut self, theme: &Theme, header_h: f32) {
        let panel_x = self.margin + BORDER_W;
        let panel_y = self.margin + BORDER_W;
        let panel_w_inner = self.panel_w - 2.0 * BORDER_W;
        let radius = (DROPDOWN_RADIUS - BORDER_W).max(1.0);
        let header_bg = mix_rgb(theme.menu_bg, theme.divider, HEADER_BG_MIX);

        let top_h = radius.min(header_h);
        self.fill(
            (panel_x, panel_y, panel_w_inner, top_h * 2.0),
            radius,
            header_bg,
            1.0,
        );
        if header_h > top_h {
            self.fill(
                (panel_x, panel_y + top_h, panel_w_inner, header_h - top_h),
                0.0,
                header_bg,
                1.0,
            );
        }
    }

    /// Composite a shaped line of text with its top-left corner at `pos`.
    pub(in crate::renderer) fn draw_text(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        buffer: &mut glyphon::Buffer,
        pos: (i32, i32),
        color: Color,
    ) {
        composite_buffer(
            font_system,
            swash_cache,
            &mut self.rgba,
            self.canvas,
            buffer,
            pos,
            color,
        );
    }

    /// Like [`Self::draw_title`], but with an optional icon glyph and color
    /// preceding the title text.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::renderer) fn draw_title_with_icon(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        ctx: &FontCtx,
        theme: &Theme,
        bar: TitleBar,
        title: &str,
        hint: &str,
        icon: Option<(&str, crate::theme::Rgb)>,
    ) -> f32 {
        let header_h = TITLE_BAR_TOP_PAD + title_bar_height(ctx);
        let text_y = (self.margin + (header_h - ctx.line_height) / 2.0).round() as i32;
        let foreground = theme.foreground.to_glyphon();
        let muted = theme.ansi[8].to_glyphon();

        let mut hint_buf = shape_chrome_line(font_system, ctx, hint, muted, false, true);
        let hint_w = match hint.is_empty() {
            true => 0.0,
            false => buffer_width(&hint_buf).ceil() + ctx.cell_w,
        };
        let (icon_buf, icon_w, icon_gap) = match icon {
            Some((icon_str, icon_color)) => {
                let buf = shape_chrome_line(
                    font_system,
                    ctx,
                    icon_str,
                    icon_color.to_glyphon(),
                    false,
                    false,
                );
                let w = buffer_width(&buf).ceil();
                let gap = (ctx.cell_w * 0.6).round();
                (Some(buf), w, gap)
            }
            None => (None, 0.0, 0.0),
        };
        let icon_total_w = if icon_buf.is_some() {
            icon_w + icon_gap
        } else {
            0.0
        };
        let room = (self.panel_w - 2.0 * ctx.cell_w - hint_w - icon_total_w).max(0.0);
        let (bold, color) = match bar {
            TitleBar::Decorated => (true, foreground),
        };
        let mut title_buf = shape_fitted(font_system, ctx, title, color, bold, true, room);
        let title_w = buffer_width(&title_buf).ceil();
        let total_content_w = icon_total_w + title_w;
        let start_x = match bar {
            TitleBar::Decorated => {
                self.draw_header_background(theme, header_h);
                self.margin + ((self.panel_w - total_content_w) / 2.0).max(0.0)
            }
        };
        draw_divider(self, theme, self.margin + header_h, self.panel_w);
        if let (Some(mut i_buf), Some((_, icon_color))) = (icon_buf, icon) {
            self.draw_text(
                font_system,
                swash_cache,
                &mut i_buf,
                (start_x.round() as i32, text_y),
                icon_color.to_glyphon(),
            );
        }
        let title_x = start_x + icon_total_w;
        self.draw_text(
            font_system,
            swash_cache,
            &mut title_buf,
            (title_x.round() as i32, text_y),
            color,
        );
        if !hint.is_empty() {
            let hint_x = self.margin + self.panel_w - ctx.cell_w - buffer_width(&hint_buf).ceil();
            self.draw_text(
                font_system,
                swash_cache,
                &mut hint_buf,
                (hint_x as i32, text_y),
                muted,
            );
        }
        self.margin + header_h
    }

    /// The finished image, placed so the panel's top-left corner lands at
    /// `(panel_x, panel_y)` on the surface. Snapped to whole pixels: the
    /// texture is sampled 1:1, and a fractional placement would smear every
    /// baked glyph.
    pub(in crate::renderer) fn into_image(self, panel_x: f32, panel_y: f32) -> DropdownImage {
        DropdownImage {
            height: self.canvas.1,
            rgba: self.rgba,
            width: self.canvas.0,
            x: (panel_x - self.margin).round(),
            y: (panel_y - self.margin).round(),
        }
    }
}

// ========================================================================
// Layout helpers
// ========================================================================

/// The height of a popup's title bar, not counting [`TITLE_BAR_TOP_PAD`]
/// above it.
pub(in crate::renderer) fn title_bar_height(ctx: &FontCtx) -> f32 {
    ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0
}

/// Shape `text` in `color`, cut short with an ellipsis until it fits
/// `max_w` pixels.
pub(in crate::renderer) fn shape_fitted(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: Color,
    bold: bool,
    proportional: bool,
    max_w: f32,
) -> glyphon::Buffer {
    let mut chars = text.chars().count();
    loop {
        let fitted = truncate_label(text, chars);
        let buffer = shape_chrome_line(font_system, ctx, &fitted, color, bold, proportional);
        if chars <= 1 || buffer_width(&buffer) <= max_w {
            return buffer;
        }
        chars -= 1;
    }
}

/// The top-left corner that centres a `panel_w` x `panel_h` panel on the
/// surface, where a popup that interrupts the view belongs.
pub(in crate::renderer) fn centered(
    surface_w: f32,
    surface_h: f32,
    panel_w: f32,
    panel_h: f32,
) -> (f32, f32) {
    (
        ((surface_w - panel_w) / 2.0).max(0.0).floor(),
        ((surface_h - panel_h) / 2.0).max(0.0).floor(),
    )
}

// ========================================================================
// Fields and dividers
// ========================================================================

/// How tall the field typed into is in pixels, `rows` of the font's cells
/// tall, on whole pixels, and never shorter than a line of its text: every
/// popup's field is this tall.
pub(in crate::renderer) fn field_height(cell_h: f32, line_height: f32, rows: f32) -> f32 {
    (cell_h * rows).round().max(line_height.ceil())
}

/// How far into the field its text starts, in cell widths.
pub(in crate::renderer) const INPUT_FIELD_TEXT_INSET: f32 = 0.7;

/// The field's corner radius and its accent ring's thickness, in pixels.
const INPUT_FIELD_RADIUS: f32 = 6.0;
const INPUT_FIELD_BORDER: f32 = 1.5;

/// How far the field's well leans from the card's color to the terminal's
/// background, so it reads as somewhere to type into.
const INPUT_WELL_MIX: f32 = 0.6;

/// The hairline that separates two parts of a popup (its title, its input
/// line, its content), across the panel's full width at canvas row `y`.
pub(in crate::renderer) fn draw_divider(frame: &mut Chrome, theme: &Theme, y: f32, panel_w: f32) {
    let margin = frame.margin();
    frame.fill((margin, y.round(), panel_w, 1.0), 0.0, theme.divider, 0.5);
}

/// The well a field is typed in, over `field` as `(x, y, width, height)`:
/// an outer soft focus halo, crisp accent border, and a rich sunken well
/// with a subtle inner bevel.
pub(in crate::renderer) fn draw_field_well(
    frame: &mut Chrome,
    theme: &Theme,
    field: (f32, f32, f32, f32),
) {
    // 1. Soft focus halo / outer glow
    frame.fill(
        (field.0 - 1.5, field.1 - 1.5, field.2 + 3.0, field.3 + 3.0),
        INPUT_FIELD_RADIUS + 1.5,
        theme.active_indicator,
        0.20,
    );
    // 2. Crisp accent border
    frame.fill(field, INPUT_FIELD_RADIUS, theme.active_indicator, 1.0);
    // 3. Deep sunken interior
    let well = mix_rgb(theme.menu_bg, theme.background, INPUT_WELL_MIX);
    let inset = (
        field.0 + INPUT_FIELD_BORDER,
        field.1 + INPUT_FIELD_BORDER,
        field.2 - INPUT_FIELD_BORDER * 2.0,
        field.3 - INPUT_FIELD_BORDER * 2.0,
    );
    frame.fill(
        inset,
        (INPUT_FIELD_RADIUS - INPUT_FIELD_BORDER).max(0.0),
        well,
        1.0,
    );
    // 4. Subtle top inner bevel / shadow for inset depth
    let shadow = mix_rgb(well, Rgb::new(0, 0, 0), 0.45);
    frame.fill(
        (inset.0 + 1.0, inset.1, inset.2 - 2.0, 1.0),
        0.0,
        shadow,
        0.6,
    );
}

/// The caret of an input line: a thin bar with its top-left at `pos`, drawn
/// directly rather than as a glyph, which would sit centred in a cell box and
/// leave a gap after the last character typed.
pub(in crate::renderer) fn draw_caret(
    frame: &mut Chrome,
    ctx: &FontCtx,
    theme: &Theme,
    pos: (f32, f32),
) {
    frame.fill(
        (pos.0, pos.1, 2.0, (ctx.line_height - 2.0).max(1.0)),
        1.0,
        theme.cursor_bg,
        1.0,
    );
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_field_is_its_rows_of_the_font_tall_and_never_less_than_a_line() {
        assert_eq!(field_height(18.0, 18.0, 1.5), 27.0);
        assert_eq!(field_height(18.0, 18.0, 2.0), 36.0);
        // A field too short for its text is a line tall.
        assert_eq!(field_height(18.0, 21.2, 1.0), 22.0);
    }

    #[test]
    fn test_the_image_puts_the_panel_corner_at_whole_pixels() {
        let theme = Theme::dark();
        let frame = Chrome::new(&theme, 100.0, 50.0, 8.0);
        let image = frame.into_image(40.4, 20.6);
        assert_eq!((image.x, image.y), (32.0, 13.0));
        assert_eq!((image.width, image.height), (116, 66));
    }
}
