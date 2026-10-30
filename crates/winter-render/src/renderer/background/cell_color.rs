//! Resolving a cell's background color from the overlays that claim it:
//! labels, the selection, search matches, the cursor, the cursor line, and
//! sentence bands.

use super::super::colors::{grid_color_to_rgb, resolve_fg_linear};
use super::super::raster::DIM_FACTOR;
use crate::theme::Theme;
use winter_core::Color as GridColor;

// ========================================================================
// Constants
// ========================================================================

/// Opacity of the `/` search highlights over the cell's own background, so
/// highlighted text keeps its foreground color instead of being washed out by an
/// opaque block of tint. The current match is painted more strongly than the
/// others, which sit further back.
pub(super) const SEARCH_CURRENT_ALPHA: f32 = 0.8;

pub(super) const SEARCH_MATCH_ALPHA: f32 = 0.6;

/// Opacities of the two alternating sentence-highlight tones, blended over the
/// cell's own background like the search tints. Kept low and close together so
/// the bands read as a subtle rhythm, not a zebra stripe; tone parity is the
/// signal, not brightness.
const SENTENCE_TINT_ALPHA_EVEN: f32 = 0.28;

const SENTENCE_TINT_ALPHA_ODD: f32 = 0.14;

/// How opaque a focus flash is the moment it lights, over whatever the row's
/// cells show: strong enough to catch the eye across a split window, and gone
/// again before it gets in the way of reading.
pub(super) const FLASH_PEAK_ALPHA: f32 = 0.55;

/// The quick-select label box, amber in linear space.
pub(super) const QUICK_SELECT_BG: (f32, f32, f32) = (0.6, 0.45, 0.1);

// ========================================================================
// Data Structures
// ========================================================================

/// Which overlays claim one cell this frame, in the order the background color
/// resolution consults them. At most one wins; the rest are ignored.
#[derive(Clone, Copy)]
pub(super) struct CellBgOverlays {
    /// The cell holds a filled (non-outline) cursor.
    pub(super) cursor_filled: bool,
    /// The whole pane is dimmed because it is not the focused one.
    pub(super) dim: bool,
    /// How opaque the focus flash is over the cell, while it lasts: drawn in
    /// the active tab's color over whatever else the cell shows.
    pub(super) flash: Option<f32>,
    pub(super) is_current_match: bool,
    pub(super) is_cursor_line: bool,
    pub(super) is_find_label: bool,
    pub(super) is_label: bool,
    pub(super) is_search_match: bool,
    pub(super) is_selected: bool,
    /// SGR 7: the cell's foreground and background are swapped.
    pub(super) reversed: bool,
    /// The sentence band's alternating tone, when the cell sits inside one.
    pub(super) sentence_tone: Option<u8>,
}

// ========================================================================
// Cell background color
// ========================================================================

impl CellBgOverlays {
    /// Whether a cell with background `bg` needs a quad of its own: it has
    /// a color of its own, or an overlay claims it.
    pub(super) fn paints(&self, bg: GridColor) -> bool {
        self.cursor_filled
            || self.reversed
            || !matches!(bg, GridColor::Default)
            || self.is_selected
            || self.is_label
            || self.is_find_label
            || self.is_search_match
            || self.is_current_match
            || self.is_cursor_line
            || self.sentence_tone.is_some()
            || self.flash.is_some()
    }
}

/// Resolve one cell's background color in linear space: the highest-priority
/// overlay claiming the cell wins, and the result fades toward the pane
/// background when the pane (or an unfocused cursor) is dimmed.
pub(super) fn cell_bg_color(
    overlays: CellBgOverlays,
    bg: GridColor,
    fg: GridColor,
    theme: &Theme,
    bg_lin: (f32, f32, f32),
) -> (f32, f32, f32) {
    let c = if overlays.is_find_label {
        theme.find_label_bg.as_linear()
    } else if overlays.is_label {
        QUICK_SELECT_BG
    } else if overlays.is_selected {
        theme.selection_bg.as_linear()
    } else if overlays.is_search_match || overlays.is_current_match {
        // Search highlights are translucent: the tint blends into
        // whatever the cell's own background is, leaving the
        // glyph's foreground color legible on top of it (the text
        // pass draws matches in their normal color).
        let under = if overlays.cursor_filled {
            theme.cursor_bg.as_linear()
        } else {
            grid_color_to_rgb(&bg, theme)
        };
        let (tint, alpha) = if overlays.is_current_match {
            (theme.search_current_bg, SEARCH_CURRENT_ALPHA)
        } else {
            (theme.search_match_bg, SEARCH_MATCH_ALPHA)
        };
        blend_over(tint.as_linear(), under, alpha)
    } else if overlays.cursor_filled {
        theme.cursor_bg.as_linear()
    } else if overlays.reversed {
        // SGR 7: the highlight quad paints the resolved foreground,
        // swapped with the glyph color in the text pass below.
        resolve_fg_linear(fg, theme)
    } else if overlays.is_cursor_line {
        theme.cursor_line_bg.as_linear()
    } else if let Some(tone) = overlays.sentence_tone {
        // The sentence band shares the cursor-line hue at a
        // lower, tone-dependent opacity, so the two reading
        // aids read as one family.
        let alpha = if tone == 0 {
            SENTENCE_TINT_ALPHA_EVEN
        } else {
            SENTENCE_TINT_ALPHA_ODD
        };
        blend_over(
            theme.cursor_line_bg.as_linear(),
            grid_color_to_rgb(&bg, theme),
            alpha,
        )
    } else {
        grid_color_to_rgb(&bg, theme)
    };
    let c = match overlays.flash {
        Some(alpha) => blend_over(theme.active_indicator.as_linear(), c, alpha),
        None => c,
    };
    if overlays.dim {
        lerp_to_bg(c, bg_lin)
    } else {
        c
    }
}

pub(super) fn lerp_to_bg(c: (f32, f32, f32), bg: (f32, f32, f32)) -> (f32, f32, f32) {
    (
        c.0 + (bg.0 - c.0) * DIM_FACTOR,
        c.1 + (bg.1 - c.1) * DIM_FACTOR,
        c.2 + (bg.2 - c.2) * DIM_FACTOR,
    )
}

/// `over` composited on `under` at `alpha` (0 = fully `under`, 1 = fully `over`).
/// The background pass draws opaque quads, so translucent highlights are flattened
/// here rather than blended by the GPU: same result, no pipeline state to order.
pub(super) fn blend_over(
    over: (f32, f32, f32),
    under: (f32, f32, f32),
    alpha: f32,
) -> (f32, f32, f32) {
    let a = alpha.clamp(0.0, 1.0);
    (
        under.0 + (over.0 - under.0) * a,
        under.1 + (over.1 - under.1) * a,
        under.2 + (over.2 - under.2) * a,
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_label_outranks_selection_in_cell_background() {
        // Both overlays claim the cell; the `f`/`t` jump label has to win, or
        // the label the user is reading disappears into the selection wash.
        let theme = Theme::dark();
        let overlays = CellBgOverlays {
            cursor_filled: false,
            dim: false,
            flash: None,
            is_current_match: false,
            is_cursor_line: false,
            is_find_label: true,
            is_label: false,
            is_search_match: false,
            is_selected: true,
            reversed: false,
            sentence_tone: None,
        };
        let color = cell_bg_color(
            overlays,
            GridColor::Default,
            GridColor::Default,
            &theme,
            theme.background.as_linear(),
        );
        assert_eq!(color, theme.find_label_bg.as_linear());
    }

    #[test]
    fn test_blend_over_composites_at_the_given_alpha() {
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 1.0),
            (1.0, 1.0, 1.0)
        );
        let (r, g, b) = blend_over((1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.5);
        assert!((r - 0.5).abs() < 1e-6 && g.abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_a_flashed_cell_is_painted_in_the_active_tab_s_color() {
        let theme = Theme::dark();
        let plain = CellBgOverlays {
            cursor_filled: false,
            dim: false,
            flash: None,
            is_current_match: false,
            is_cursor_line: false,
            is_find_label: false,
            is_label: false,
            is_search_match: false,
            is_selected: false,
            reversed: false,
            sentence_tone: None,
        };
        let bg_lin = theme.background.as_linear();
        assert!(!plain.paints(GridColor::Default));
        let full = CellBgOverlays {
            flash: Some(1.0),
            ..plain
        };
        assert!(full.paints(GridColor::Default), "a blank cell lights too");
        let color = |overlays| {
            cell_bg_color(
                overlays,
                GridColor::Default,
                GridColor::Default,
                &theme,
                bg_lin,
            )
        };
        let (r, g, b) = color(full);
        let (ar, ag, ab) = theme.active_indicator.as_linear();
        assert!((r - ar).abs() < 1e-6 && (g - ag).abs() < 1e-6 && (b - ab).abs() < 1e-6);
        let faint = color(CellBgOverlays {
            flash: Some(0.0),
            ..plain
        });
        assert_eq!(
            faint,
            color(plain),
            "a faded flash leaves the cell as it was"
        );
    }
}
