//! The frame every floating popup is drawn in, after VS Code's widget chrome:
//! one soft shadow, one hairline border, one surface color, and one title bar
//! shape, so a menu, a picker, a dialog, and a tooltip read as one kind of
//! thing. Each rasterizer lays out and paints only its own contents.

use glyphon::{Color, FontSystem, SwashCache};

use super::paint::{
    blend_px, buffer_width, composite_buffer, fill_rounded_rect, rounded_rect_sdf,
    shape_chrome_line,
};
use super::view::DropdownImage;
use super::PALETTE_ITEM_PAD_Y;
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

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

/// Width of the popup's hairline border, in pixels.
const BORDER_W: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A popup's pixels: a panel of a given size, inside a transparent margin
/// that holds its drop shadow. Coordinates passed to its drawing methods are
/// canvas coordinates, so the panel's top-left corner is `(margin, margin)`.
pub(in crate::renderer) struct PopupFrame {
    canvas: (u32, u32),
    margin: f32,
    panel_w: f32,
    rgba: Vec<u8>,
}

// ========================================================================
// PopupFrame Implementations
// ========================================================================

impl PopupFrame {
    /// Paint the empty popup: the shadow fading out across `margin`, the
    /// hairline border, and the surface inset one border width within it.
    pub(in crate::renderer) fn new(theme: &Theme, panel_w: f32, panel_h: f32, margin: f32) -> Self {
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

    /// The raw pixels and their `(width, height)`, for painters that work on
    /// the buffer directly.
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

    /// Draw the title bar along the panel's top edge: `title` centred across
    /// the full panel width, and `hint` (when not empty) right-aligned in the
    /// muted color, as VS Code's quick input puts its actions. Returns the
    /// canvas y where the content below the title bar begins.
    pub(in crate::renderer) fn draw_title_bar(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        ctx: &FontCtx,
        theme: &Theme,
        title: &str,
        hint: &str,
    ) -> f32 {
        let top = self.margin + TITLE_BAR_TOP_PAD;
        let text_y = (top + PALETTE_ITEM_PAD_Y) as i32;
        let foreground = theme.foreground.to_glyphon();
        let mut title_buf = shape_chrome_line(font_system, ctx, title, foreground, false, true);
        let title_w = buffer_width(&title_buf).ceil();
        let title_x = self.margin + ((self.panel_w - title_w) / 2.0).max(0.0);
        self.draw_text(
            font_system,
            swash_cache,
            &mut title_buf,
            (title_x as i32, text_y),
            foreground,
        );
        if !hint.is_empty() {
            let muted = theme.ansi[8].to_glyphon();
            let mut hint_buf = shape_chrome_line(font_system, ctx, hint, muted, false, true);
            let hint_w = buffer_width(&hint_buf).ceil();
            let hint_x = self.margin + self.panel_w - ctx.cell_w - hint_w;
            self.draw_text(
                font_system,
                swash_cache,
                &mut hint_buf,
                (hint_x as i32, text_y),
                muted,
            );
        }
        top + title_bar_height(ctx)
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

/// The top-left corner that centres a `panel_w` x `panel_h` panel on the
/// surface, where a popup that interrupts the view belongs.
pub(in crate::renderer) fn centered(
    surface_w: f32,
    surface_h: f32,
    panel_w: f32,
    panel_h: f32,
) -> (f32, f32) {
    (
        ((surface_w - panel_w) / 2.0).max(0.0),
        ((surface_h - panel_h) / 2.0).max(0.0),
    )
}

// ========================================================================
// Fields and dividers
// ========================================================================

/// The space above and below the text inside the field, in cell heights.
pub(in crate::renderer) const INPUT_FIELD_PAD_RATIO: f32 = 0.3;

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
pub(in crate::renderer) fn draw_divider(
    frame: &mut PopupFrame,
    theme: &Theme,
    y: f32,
    panel_w: f32,
) {
    let margin = frame.margin();
    frame.fill((margin, y.round(), panel_w, 1.0), 0.0, theme.divider, 0.5);
}

/// The well a field is typed in, over `field` as `(x, y, width, height)`: a
/// rounded box a shade off the card, ringed in the accent color since it
/// holds the keyboard.
pub(in crate::renderer) fn draw_field_well(
    frame: &mut PopupFrame,
    theme: &Theme,
    field: (f32, f32, f32, f32),
) {
    frame.fill(field, INPUT_FIELD_RADIUS, theme.active_indicator, 1.0);
    let well = mix_rgb(theme.menu_bg, theme.background, INPUT_WELL_MIX);
    let inset = (
        field.0 + INPUT_FIELD_BORDER,
        field.1 + INPUT_FIELD_BORDER,
        field.2 - INPUT_FIELD_BORDER * 2.0,
        field.3 - INPUT_FIELD_BORDER * 2.0,
    );
    frame.fill(inset, INPUT_FIELD_RADIUS - INPUT_FIELD_BORDER, well, 1.0);
}

/// The caret of an input line: a thin bar with its top-left at `pos`, drawn
/// directly rather than as a glyph, which would sit centred in a cell box and
/// leave a gap after the last character typed.
pub(in crate::renderer) fn draw_caret(
    frame: &mut PopupFrame,
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
    use crate::renderer::test_support::*;

    /// The leftmost and rightmost canvas columns in `rows` whose pixel differs
    /// from the panel surface, which is where text was drawn. Only the
    /// panel's interior is scanned: the border runs down both sides of every
    /// row and would make any line of text look centred.
    fn ink_columns(frame: &mut PopupFrame, theme: &Theme, rows: (u32, u32)) -> (u32, u32) {
        let inside = (frame.margin + 2.0 * BORDER_W) as u32;
        let (rgba, (width, _)) = frame.pixels();
        let surface = theme.menu_bg;
        let (mut left, mut right) = (u32::MAX, 0);
        for y in rows.0..rows.1 {
            for x in inside..width - inside {
                let idx = ((y * width + x) * 4) as usize;
                let differs = rgba[idx] != surface.r
                    || rgba[idx + 1] != surface.g
                    || rgba[idx + 2] != surface.b;
                if differs {
                    left = left.min(x);
                    right = right.max(x);
                }
            }
        }
        (left, right)
    }

    #[test]
    fn test_the_title_is_centred_on_the_panel_not_the_canvas_edge() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (panel_w, panel_h, margin) = (400.0, 120.0, 8.0);
        let mut frame = PopupFrame::new(&theme, panel_w, panel_h, margin);
        let bottom = frame.draw_title_bar(&mut font_system, &mut swash, &ctx, &theme, "Branch", "");
        let (left, right) = ink_columns(
            &mut frame,
            &theme,
            ((margin + TITLE_BAR_TOP_PAD) as u32 + 1, bottom as u32),
        );
        assert!(left < right, "the title drew something");
        let ink_centre = (left + right) as f32 / 2.0;
        let panel_centre = margin + panel_w / 2.0;
        assert!(
            (ink_centre - panel_centre).abs() <= ctx.cell_w,
            "title centred at {ink_centre}, panel centre {panel_centre}"
        );
    }

    #[test]
    fn test_the_image_puts_the_panel_corner_at_whole_pixels() {
        let theme = Theme::dark();
        let frame = PopupFrame::new(&theme, 100.0, 50.0, 8.0);
        let image = frame.into_image(40.4, 20.6);
        assert_eq!((image.x, image.y), (32.0, 13.0));
        assert_eq!((image.width, image.height), (116, 66));
    }
}
