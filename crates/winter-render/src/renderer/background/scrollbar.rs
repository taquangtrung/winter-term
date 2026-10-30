//! A pane's scrollbar: a recessed track along its right edge and a thumb
//! sized and placed by how much history there is and where the view is.

use super::{quad_vertices, BgVertex};
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

const SCROLLBAR_WIDTH: f32 = 2.0;
/// How far the scrollbar track is mixed from the pane background toward the
/// divider color, so the track reads as a recess without becoming a hard line.
const SCROLLBAR_TRACK_DIVIDER_MIX: f32 = 0.3;

/// Saturation multiplier for the scrollbar thumb (>1 pushes channels away from
/// gray for a more vivid color); applied in every pane.
const SCROLLBAR_SATURATE: f32 = 1.8;
/// How far the scrollbar thumb is lerped toward pure blue, applied in every pane.
const SCROLLBAR_BLUE: f32 = 0.18;
/// How far the scrollbar thumb is lerped toward white, lifting its brightness a
/// touch in every pane.
const SCROLLBAR_BRIGHTEN: f32 = 0.15;
/// How far an unfocused pane's scrollbar is lerped toward the background
/// (0 = unchanged, 1 = fully background). Stronger than the content dim so the
/// active pane's scrollbar clearly stands out.
const SCROLLBAR_DIM_FACTOR: f32 = 0.5;
const SCROLLBAR_MIN_THUMB: f32 = 6.0;

// ========================================================================
// Scrollbar
// ========================================================================

/// Where and how to draw one pane's scrollbar.
pub(super) struct ScrollbarParams<'a> {
    /// The pane's background in linear space, dimmed toward when unfocused.
    pub(super) bg_lin: (f32, f32, f32),
    pub(super) cell_h: f32,
    /// Right edge of the pane's content area, which the bar hugs.
    pub(super) content_right: f32,
    pub(super) dim: bool,
    pub(super) focused: bool,
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
        let c = p.theme.scrollbar.as_linear();
        let avg = (c.0 + c.1 + c.2) / 3.0;
        let sat = |x: f32| (avg + (x - avg) * SCROLLBAR_SATURATE).clamp(0.0, 1.0);
        let (r, g, b) = (sat(c.0), sat(c.1), sat(c.2));
        let (r, g, b) = (
            r + (0.0 - r) * SCROLLBAR_BLUE,
            g + (0.0 - g) * SCROLLBAR_BLUE,
            b + (1.0 - b) * SCROLLBAR_BLUE,
        );
        // Lift it a touch toward white so both panes read a bit brighter.
        (
            r + (1.0 - r) * SCROLLBAR_BRIGHTEN,
            g + (1.0 - g) * SCROLLBAR_BRIGHTEN,
            b + (1.0 - b) * SCROLLBAR_BRIGHTEN,
        )
    };
    // An unfocused pane's scrollbar is then dimmed toward the background so
    // the active pane's still stands out (independent of the content dim).
    if !p.focused || p.dim {
        let dim_to_bg = |c: (f32, f32, f32)| {
            (
                c.0 + (p.bg_lin.0 - c.0) * SCROLLBAR_DIM_FACTOR,
                c.1 + (p.bg_lin.1 - c.1) * SCROLLBAR_DIM_FACTOR,
                c.2 + (p.bg_lin.2 - c.2) * SCROLLBAR_DIM_FACTOR,
            )
        };
        track_color = dim_to_bg(track_color);
        thumb_color = dim_to_bg(thumb_color);
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
            bg_lin: (0.0, 0.0, 0.0),
            cell_h: 10.0,
            content_right: 100.0,
            dim: false,
            focused: true,
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
}
