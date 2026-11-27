//! One frame's preparation: the popups and pane toolbars rasterized for the
//! host to draw ([`GpuRenderer::prepare`]), and the fills the paint path
//! draws each pane with (its background pass, the separators, the strips of
//! a gutter).

use super::GpuRenderer;
use super::background::{
    BgParams, CURSOR_HOLLOW_STROKE_WIDTH, LineFlash, PaneFills, SeparatorStyle, build_bg_fills,
    cursor_outline_quads, cursor_quad, fill_color, quad_fill, separator_fills, separators,
};
use super::{GridStrip, PaneView, grid_area};
use crate::render::image::ImagePlacement;
use misty::{CellHighlight, Rect, SolidFill};
use winter_emulator::CursorShape;
use winter_theme::Rgb;

/// Glyph color over the current search match's tint.
pub(super) const CURRENT_MATCH_FG: (u8, u8, u8) = (255, 255, 255);

/// Glyph color over every other search match's tint.
pub(super) const SEARCH_MATCH_FG: (u8, u8, u8) = (0, 0, 0);

// ========================================================================
// Data Structures
// ========================================================================

/// The whole cell columns and rows that fit inside one pane's rect.
#[derive(Clone, Copy)]
pub(super) struct PaneExtent {
    pub(super) cols: usize,
    pub(super) rows: usize,
}

// ========================================================================
// GpuRenderer: frame composition
// ========================================================================

impl GpuRenderer {
    /// Get one frame ready for a host drawing it with misty: how tall the
    /// status bar is drawn, `None` while it is hidden, and `images`, the
    /// pictures it draws from [`Self::image_texture`], recorded in
    /// [`Self::hits`]. The panes themselves the host draws from
    /// [`Self::paint_pane`].
    pub fn prepare(&mut self, status_bar: Option<f32>, images: &[ImagePlacement]) {
        self.window_layout.status_bar_height = status_bar;
        self.hits.pictures = images.to_vec();
    }

    // --------------------------------------------------------------------
    // Per-pane composition
    // --------------------------------------------------------------------

    /// One pane's cell backgrounds, highlights, cursors, lines, and
    /// scrollbar.
    pub(super) fn pane_fills(&self, pane: &PaneView) -> PaneFills {
        let grid = pane.grid;
        // Rows are placed in the grid area, centred in the pane.
        let rect = grid_area(pane.rect, self.fonts.cell_height);
        let extent = pane_cell_extent(
            rect,
            (self.fonts.cell_width, self.fonts.cell_height),
            self.pane_pad(),
        );
        let pane_cols = extent.cols;
        let pane_rows = extent.rows;

        let mut fills = build_bg_fills(
            grid,
            BgParams {
                ch: self.fonts.cell_height,
                content_right: rect.x + rect.width - self.pane_pad(),
                cursor_unfocused: pane.cursor_unfocused,
                cursor_shape: pane.cursor_shape,
                cw: self.fonts.cell_width,
                dim: pane.dim,
                hide_cursor: !pane.cursor_visible || pane.nav_cursor.is_some(),
                hovered_link: pane.hovered_link,
                labels: pane.labels,
                room: (pane_cols, pane_rows),
                overlay: pane.overlay,
                presentation: pane.presentation(),
                find_labels: pane.find_labels,
                offset_x: rect.x + self.pane_pad(),
                offset_y: rect.y,
                // The cursor's row gets a `cursorline` wash while the pane is
                // being navigated; it doesn't blink with the cursor itself.
                cursor_line: pane
                    .cursor_line_row
                    .map(|row| match pane.cursor_line_alone {
                        true => (row, row),
                        false => grid.wrapped_row_span(row),
                    }),
                jump_line: pane.jump_target_row.map(|row| grid.wrapped_row_span(row)),
                line_flash: pane.line_flash.map(|(row, strength)| LineFlash {
                    rows: grid.wrapped_row_span(row),
                    strength,
                }),
                scroll_offset: pane.scroll_offset,
                scrollback_len: pane.scrollback_len,
                scrollbar_active: pane.scrollbar_active,
                scrollbar_hover: pane.scrollbar_hover,
                scrollbar_opacity: pane.scrollbar_opacity,
                search_current: pane.search_current,
                search_matches: pane.search_matches,
                sentence_spans: pane.sentence_spans,
                outlines: pane.outlines,
                selection: pane.selection,
                selection_block: pane.selection_block,
                theme: self.themes.winter(),
                url_underline: pane.url_underline,
            },
        );

        // The block-as-cursor: while the traversal cursor sits inside a rich
        // block's reserved band, the band itself is the cursor: an accent
        // outline around the whole band replaces the 1-cell block, which
        // would otherwise sit invisible in the blank space behind the image.
        // Steady, not blinked: it is a band-sized affordance, and a rectangle
        // that big flashing would be noise rather than feedback.
        if let Some((band_top, band_rows)) = pane.block_band {
            let px0 = rect.x + self.pane_pad();
            let py0 = rect.y + band_top as f32 * self.fonts.cell_height;
            let px1 = px0 + pane_cols as f32 * self.fonts.cell_width;
            let py1 = py0 + band_rows as f32 * self.fonts.cell_height;
            for (qx0, qy0, qx1, qy1) in
                cursor_outline_quads(px0, py0, px1 - px0, py1 - py0, CURSOR_HOLLOW_STROKE_WIDTH)
            {
                fills.fills.push(quad_fill(
                    qx0,
                    qy0,
                    qx1,
                    qy1,
                    self.themes.winter().cursor_bg.as_linear(),
                ));
            }
        } else if let Some((nav_row, nav_col)) = pane.nav_cursor.filter(|_| pane.nav_cursor_visible)
            && nav_row < pane_rows
            && nav_col < pane_cols
        {
            let px0 = rect.x + self.pane_pad() + nav_col as f32 * self.fonts.cell_width;
            let py0 = rect.y + nav_row as f32 * self.fonts.cell_height;
            // The traversal cursor takes the same unfocused form as the
            // shell's (see `build_bg_fills`): a hollow outline
            // of the cell, whatever its shape.
            if pane.cursor_unfocused {
                for (qx0, qy0, qx1, qy1) in cursor_outline_quads(
                    px0,
                    py0,
                    self.fonts.cell_width,
                    self.fonts.cell_height,
                    CURSOR_HOLLOW_STROKE_WIDTH,
                ) {
                    fills.fills.push(quad_fill(
                        qx0,
                        qy0,
                        qx1,
                        qy1,
                        self.themes.winter().cursor_bg.as_linear(),
                    ));
                }
            } else if pane.cursor_shape == CursorShape::Block {
                // Its cell's background, under the glyph, as the shell's
                // block cursor is.
                fills.highlights.push(CellHighlight {
                    color: fill_color(self.themes.winter().cursor_bg.as_linear()),
                    cols: nav_col..nav_col + 1,
                    row: nav_row,
                });
            } else {
                let (qx0, qy0, qx1, qy1) = cursor_quad(
                    pane.cursor_shape,
                    px0,
                    py0,
                    self.fonts.cell_width,
                    self.fonts.cell_height,
                    false,
                );
                fills.fills.push(quad_fill(
                    qx0,
                    qy0,
                    qx1,
                    qy1,
                    self.themes.winter().cursor_bg.as_linear(),
                ));
            }
        }
        fills
    }

    // --------------------------------------------------------------------
    // WindowParts around the panes
    // --------------------------------------------------------------------

    /// The color split dividers are drawn in: the theme's divider at full
    /// strength, so the line between panes stands out from the fainter lines
    /// inside a pane's tab strip. The pane strips draw their share of a
    /// divider with this too, so the two always meet in one color.
    pub(super) fn split_divider_color(&self) -> Rgb {
        self.themes.winter().divider
    }

    /// The separators around the panes, the status bar's top edge among them,
    /// each drawn once, over their strips: the focused pane's bottom border in
    /// the focus color (see `background::divider`).
    pub fn paint_separators(&self, panes: &[PaneView]) -> Vec<SolidFill> {
        let areas: Vec<Rect> = panes.iter().map(|pane| pane.area).collect();
        let style = SeparatorStyle {
            focus: self.themes.winter().active_pane_border.as_linear(),
            plain: self.split_divider_color().as_linear(),
            status: self.themes.winter().status_bar_border.as_linear(),
            width: self.window_layout.divider_width,
        };
        // The focused pane is marked only among others: alone, it is plainly
        // the one in use.
        let focused = panes
            .iter()
            .position(|pane| pane.focused)
            .filter(|_| panes.len() > 1);
        // The status bar's top edge is a separator too, a bottom border.
        // Flush with the window's bottom, so the slack of a height no whole
        // number of rows collects above it.
        let status_top = self
            .window_layout
            .status_bar_height
            .map(|height| (self.size.height as f32 - height).round());
        let found = separators(&areas, style.width, status_top);
        separator_fills(&found, focused, style)
    }

    // --------------------------------------------------------------------
    // Overlay passes
    // --------------------------------------------------------------------
}

// ========================================================================
// Geometry and color, free of the GPU
// ========================================================================

/// The whole cell columns and rows that fit inside `rect`, inset by `pad`
/// on each side.
pub(super) fn pane_cell_extent(rect: Rect, (cell_w, cell_h): (f32, f32), pad: f32) -> PaneExtent {
    PaneExtent {
        cols: ((rect.width - pad * 2.0) / cell_w).floor() as usize,
        rows: (rect.height / cell_h).floor() as usize,
    }
}

// ========================================================================
// Gutters
// ========================================================================

/// What `pane`'s strips draw as: each lane of its gutter, its grid in its
/// strip.
pub(super) fn strip_panes<'a>(pane: &PaneView<'a>) -> Vec<PaneView<'a>> {
    pane.gutter
        .iter()
        .map(|strip| strip_pane(pane, *strip))
        .collect()
}

/// What `strip` of `pane` draws as: its grid in its strip, dimmed with the
/// pane, and none of the pane's cursor, marks, scrollbar, or popups.
fn strip_pane<'a>(pane: &PaneView<'a>, strip: GridStrip<'a>) -> PaneView<'a> {
    PaneView {
        area: pane.area,
        blink_visible: true,
        block_band: None,
        bracket_colors: &[],
        cursor_line_alone: false,
        cursor_line_row: None,
        cursor_shape: pane.cursor_shape,
        cursor_unfocused: false,
        cursor_visible: false,
        dim: pane.dim,
        find_labels: &[],
        focused: false,
        grid: strip.grid,
        gutter: Vec::new(),
        hovered_link: 0,
        jump_target_row: None,
        labels: None,
        line_flash: None,
        nav_cursor: None,
        nav_cursor_visible: false,
        outlines: &[],
        overlay: None,
        rect: strip.rect,
        scroll_offset: 0,
        scrollback_len: 0,
        scrollbar_active: false,
        scrollbar_hover: false,
        scrollbar_opacity: 1.0,
        search_current: &[],
        search_matches: &[],
        selection: None,
        selection_block: false,
        sentence_spans: &[],
        toolbar: None,
        url_underline: false,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: f32, height: f32) -> Rect {
        Rect {
            height,
            width,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn test_pane_narrower_than_its_padding_yields_no_columns() {
        // The padding is subtracted from both edges before the division, so a
        // pane thinner than that goes negative. It has to land on zero columns
        // rather than wrapping into a huge count that the row loop would then
        // try to walk.
        let extent = pane_cell_extent(
            rect(crate::render::PANE_H_PAD, 100.0),
            (9.0, 18.0),
            crate::render::PANE_H_PAD,
        );
        assert_eq!(extent.cols, 0);
    }

    #[test]
    fn test_pane_extent_floors_to_whole_cells() {
        // Partial cells are not drawable, so both axes floor rather than round:
        // 95px of 9px cells is ten whole columns once the padding is removed.
        let extent = pane_cell_extent(rect(95.0 + 4.0 * 2.0, 98.0), (9.0, 18.0), 4.0);
        assert_eq!((extent.cols, extent.rows), (10, 5));
    }
}
