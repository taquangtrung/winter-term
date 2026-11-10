//! A pane's scrollbar: a recessed track along its right edge and a thumb
//! sized and placed by how much history there is and where the view is.

use super::{quad_vertices, BgVertex};
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

const SCROLLBAR_WIDTH: f32 = 5.0;
/// How far the scrollbar track is mixed from the pane background toward the
/// divider color, so the track reads as a recess without becoming a hard line.
const SCROLLBAR_TRACK_DIVIDER_MIX: f32 = 0.3;

/// Saturation multiplier for the scrollbar thumb (>1 pushes channels away from
/// gray for a more vivid color); applied in every pane.
const SCROLLBAR_SATURATE: f32 = 1.3;
/// How far the scrollbar thumb is lerped toward pure blue, applied in every pane.
const SCROLLBAR_BLUE: f32 = 0.10;
/// How far the scrollbar thumb is lerped toward white, lifting its brightness a
/// touch in every pane.
const SCROLLBAR_BRIGHTEN: f32 = 0.05;
/// How far an unfocused pane's scrollbar is lerped toward the background
/// (0 = unchanged, 1 = fully background). Stronger than the content dim so the
/// active pane's scrollbar clearly stands out.
const SCROLLBAR_DIM_FACTOR: f32 = 0.5;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;

// ========================================================================
// Scrollbar
// ========================================================================

/// Where and how to draw one pane's scrollbar.
pub(super) struct ScrollbarParams<'a> {
    /// Whether this scrollbar is active (undimmed). Modeled independently
    /// from pane focus so at most one scrollbar is active at a time.
    pub(super) active: bool,
    /// The pane's background in linear space, dimmed toward when unfocused.
    pub(super) bg_lin: (f32, f32, f32),
    pub(super) cell_h: f32,
    /// Right edge of the pane's content area, which the bar hugs.
    pub(super) content_right: f32,
    pub(super) dim: bool,
    pub(super) hovered: bool,
    pub(super) offset_y: f32,
    /// Visible grid rows, i.e. the height of the scroll window.
    pub(super) rows: usize,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) surface_h: f32,
    pub(super) surface_w: f32,
    pub(super) theme: &'a Theme,
}

/// The scrollbar's track and thumb quads for one pane. Empty when there is no
/// scrollback to navigate, which is when the bar is not shown at all.
pub(super) fn scrollbar_quads(p: ScrollbarParams) -> Vec<BgVertex> {
    let mut verts = Vec::new();
    if p.scrollback_len == 0 {
        return verts;
    }
    let total = (p.rows + p.scrollback_len) as f32;
    let visible = p.rows as f32;
    let thumb_h_frac = (visible / total).max(0.0);
    let top_virtual = p.scrollback_len.saturating_sub(p.scroll_offset) as f32;
    let thumb_top_frac = (top_virtual / total).clamp(0.0, 1.0 - thumb_h_frac);

    // Hug the right content edge of the pane (snapped to whole pixels so
    // every pane's scrollbar renders at the same crisp width, instead of
    // landing on the last grid column with a variable sub-column gap).
    let sb_x1 = p.content_right.round();
    let sb_x0 = sb_x1 - SCROLLBAR_WIDTH;
    let track_y0 = p.offset_y;
    let track_y1 = p.offset_y + p.rows as f32 * p.cell_h;
    let track_h = track_y1 - track_y0;

    // Size the thumb first, then place it. A deep scrollback drives the
    // proportional height toward zero, so [`SCROLLBAR_MIN_THUMB`] keeps it
    // grabbable; on a track too short to hold even that, the thumb fills the
    // track instead of overflowing it.
    let thumb_h = (thumb_h_frac * track_h)
        .max(SCROLLBAR_MIN_THUMB)
        .min(track_h);
    // Keeping the thumb inside the track has to move it, not shrink it.
    // Clamping the bottom edge instead would cancel the minimum height exactly
    // when it matters most: a pane sitting at its live bottom (the resting
    // state) parks the thumb against the track's end, where a bottom clamp
    // collapses it back to a hairline.
    let thumb_y0 = (track_y0 + thumb_top_frac * track_h).min(track_y1 - thumb_h);
    let thumb_y1 = thumb_y0 + thumb_h;

    let (bg_r, bg_g, bg_b) = p.theme.background.as_linear();
    let (dv_r, dv_g, dv_b) = p.theme.divider.as_linear();
    let mix = |bg: f32, dv: f32| {
        bg * (1.0 - SCROLLBAR_TRACK_DIVIDER_MIX) + dv * SCROLLBAR_TRACK_DIVIDER_MIX
    };
    let mut track_color = (mix(bg_r, dv_r), mix(bg_g, dv_g), mix(bg_b, dv_b));
    // Make the thumb more saturated and bluer than the configured color, in
    // every pane: push the channels away from gray, then shift toward pure
    // blue.
    let mut thumb_color = {
        let base = if p.hovered {
            p.theme.scrollbar_hover()
        } else {
            p.theme.scrollbar
        };
        let c = base.as_linear();
        let avg = (c.0 + c.1 + c.2) / 3.0;
        let sat = |x: f32| (avg + (x - avg) * SCROLLBAR_SATURATE).clamp(0.0, 1.0);
        let (r, g, b) = (sat(c.0), sat(c.1), sat(c.2));
        let (r, g, b) = (
            r + (0.0 - r) * SCROLLBAR_BLUE,
            g + (0.0 - g) * SCROLLBAR_BLUE,
            b + (1.0 - b) * SCROLLBAR_BLUE,
        );
        // Lift it a touch toward white so both panes read cleanly.
        (
            r + (1.0 - r) * SCROLLBAR_BRIGHTEN,
            g + (1.0 - g) * SCROLLBAR_BRIGHTEN,
            b + (1.0 - b) * SCROLLBAR_BRIGHTEN,
        )
    };
    // An inactive scrollbar is dimmed toward the background so the active
    // scrollbar stands out (independent of the content dim).
    // An active hover lifts the thumb to its full hover accent even when inactive.
    if !p.active || p.dim {
        let dim_to_bg = |c: (f32, f32, f32)| {
            (
                c.0 + (p.bg_lin.0 - c.0) * SCROLLBAR_DIM_FACTOR,
                c.1 + (p.bg_lin.1 - c.1) * SCROLLBAR_DIM_FACTOR,
                c.2 + (p.bg_lin.2 - c.2) * SCROLLBAR_DIM_FACTOR,
            )
        };
        track_color = dim_to_bg(track_color);
        if !p.hovered {
            thumb_color = dim_to_bg(thumb_color);
        }
    }

    verts.extend_from_slice(&quad_vertices(
        sb_x0,
        track_y0,
        sb_x1,
        track_y1,
        track_color,
        p.surface_w,
        p.surface_h,
    ));
    verts.extend_from_slice(&quad_vertices(
        sb_x0,
        thumb_y0,
        sb_x1,
        thumb_y1,
        thumb_color,
        p.surface_w,
        p.surface_h,
    ));
    verts
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every vertex's y coordinate, in surface pixels rather than NDC.
    fn pixel_ys(verts: &[BgVertex], surface_h: f32) -> Vec<f32> {
        verts
            .iter()
            .map(|v| (1.0 - v.y) * surface_h / 2.0)
            .collect()
    }

    fn scrollbar_params(scrollback_len: usize, scroll_offset: usize) -> ScrollbarParams<'static> {
        static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        ScrollbarParams {
            active: true,
            bg_lin: (0.0, 0.0, 0.0),
            cell_h: 10.0,
            content_right: 100.0,
            dim: false,
            hovered: false,
            offset_y: 0.0,
            rows: 10,
            scroll_offset,
            scrollback_len,
            surface_h: 100.0,
            surface_w: 100.0,
            theme: THEME.get_or_init(Theme::dark),
        }
    }

    #[test]
    fn test_scrollbar_is_absent_without_scrollback() {
        // Nothing to navigate means no bar at all, not a full-height thumb.
        assert!(scrollbar_quads(scrollbar_params(0, 0)).is_empty());
    }

    #[test]
    fn test_scrollbar_thumb_stays_inside_its_track() {
        // A huge scrollback drives the proportional thumb toward zero height,
        // and `scroll_offset: 0` (the resting state, sitting at the live
        // bottom) parks it against the track's end. That is the case where a
        // bottom-edge clamp used to cancel the minimum height.
        let verts = scrollbar_quads(scrollbar_params(100_000, 0));
        let ys = pixel_ys(&verts, 100.0);
        let track_bottom = 10.0 * 10.0;
        let thumb_ys = &ys[6..];
        let (top, bottom) = thumb_ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(top >= -f32::EPSILON, "thumb top {top} is above the track");
        assert!(
            bottom <= track_bottom + f32::EPSILON,
            "thumb bottom {bottom} overruns the track at {track_bottom}"
        );
        assert!(
            bottom - top >= SCROLLBAR_MIN_THUMB - f32::EPSILON,
            "thumb {top}..{bottom} is thinner than the {SCROLLBAR_MIN_THUMB}px minimum"
        );
    }

    #[test]
    fn test_scrollbar_thumb_fills_a_track_shorter_than_the_minimum() {
        // A pane only a couple of rows tall has a track shorter than
        // `SCROLLBAR_MIN_THUMB`; the minimum must not push the thumb past the
        // track's end there.
        let mut params = scrollbar_params(500, 0);
        params.rows = 1;
        params.cell_h = 4.0;
        let verts = scrollbar_quads(params);
        let ys = pixel_ys(&verts, 100.0);
        let thumb_ys = &ys[6..];
        let (top, bottom) = thumb_ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(top >= -f32::EPSILON, "thumb top {top} is above the track");
        assert!(
            bottom <= 4.0 + f32::EPSILON,
            "thumb bottom {bottom} overruns the 4px track"
        );
    }

    #[test]
    fn test_scrollbar_hover_uses_theme_hover_color() {
        let normal_verts = scrollbar_quads(scrollbar_params(100, 0));
        let mut hovered_params = scrollbar_params(100, 0);
        hovered_params.hovered = true;
        let hovered_verts = scrollbar_quads(hovered_params);

        // Track quad (vertices 0..6) color must remain unchanged on hover.
        assert_eq!(normal_verts[0].r, hovered_verts[0].r);
        assert_eq!(normal_verts[0].g, hovered_verts[0].g);
        assert_eq!(normal_verts[0].b, hovered_verts[0].b);

        // Thumb quads start after track quad (6 vertices per quad).
        let normal_thumb = &normal_verts[6..12];
        let hovered_thumb = &hovered_verts[6..12];

        // Hovered thumb brightness (RGB sum) must be greater than normal thumb.
        let normal_sum = normal_thumb[0].r + normal_thumb[0].g + normal_thumb[0].b;
        let hovered_sum = hovered_thumb[0].r + hovered_thumb[0].g + hovered_thumb[0].b;
        assert!(
            hovered_sum > normal_sum,
            "hovered thumb ({hovered_sum}) should be brighter than normal ({normal_sum})"
        );

        // Hovered thumb must have a higher blue channel and blue must be dominant.
        assert!(
            hovered_thumb[0].b > normal_thumb[0].b,
            "hovered thumb blue ({}) should exceed normal blue ({})",
            hovered_thumb[0].b,
            normal_thumb[0].b
        );
        assert!(
            hovered_thumb[0].b > hovered_thumb[0].r && hovered_thumb[0].b > hovered_thumb[0].g,
            "blue must remain the dominant channel for a blue appearance"
        );
    }

    #[test]
    fn test_scrollbar_hover_applies_full_color_even_when_inactive() {
        let mut active_hovered = scrollbar_params(100, 0);
        active_hovered.active = true;
        active_hovered.hovered = true;
        let active_verts = scrollbar_quads(active_hovered);

        let mut inactive_hovered = scrollbar_params(100, 0);
        inactive_hovered.active = false;
        inactive_hovered.hovered = true;
        let inactive_verts = scrollbar_quads(inactive_hovered);

        // Hovered thumb color (vertices 6..12) must match whether active or inactive.
        let active_thumb = &active_verts[6..12];
        let inactive_thumb = &inactive_verts[6..12];
        assert_eq!(active_thumb[0].r, inactive_thumb[0].r);
        assert_eq!(active_thumb[0].g, inactive_thumb[0].g);
        assert_eq!(active_thumb[0].b, inactive_thumb[0].b);
    }

    #[test]
    fn test_scrollbar_active_vs_inactive() {
        let active_params = scrollbar_params(100, 0);
        let active_verts = scrollbar_quads(active_params);

        let mut inactive_params = scrollbar_params(100, 0);
        inactive_params.active = false;
        let inactive_verts = scrollbar_quads(inactive_params);

        // Inactive thumb is dimmed toward background (different color than active thumb).
        let thumb_active = &active_verts[6];
        let thumb_inactive = &inactive_verts[6];
        assert_ne!(thumb_active.r, thumb_inactive.r);
    }
}
