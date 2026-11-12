//! The drawing machinery the window's parts and widgets share: painting
//! text and artwork into RGBA buffers ([`paint`]), the elevated card a
//! floating widget is drawn on ([`popup`](crate::renderer::widgets::popup)), the images they hand back
//! ([`view`]), and the one way any of them reaches the GPU ([`upload`]),
//! which caches each raster against what it was painted from.

pub(in crate::renderer) mod chrome;
pub(in crate::renderer) mod paint;
pub(in crate::renderer) mod upload;
pub(in crate::renderer) mod view;

pub(super) use upload::OverlayCaches;

// ========================================================================
// Constants
// ========================================================================

/// Vertical padding above and below the text in each palette result row, in
/// pixels. The row height is one line of text plus this on each side, so the
/// inter-row gap is a small fixed amount rather than a multiple of the line.
pub(in crate::renderer) const PALETTE_ITEM_PAD_Y: f32 = 4.0;

/// Width of the soft drop shadow cast around the dropdown panel, in pixels.
pub(in crate::renderer) const DROPDOWN_SHADOW: f32 = 8.0;

/// The room a floating card leaves between itself and the window's edges,
/// both sides together, in pixels: every centred card stops this short.
pub const POPUP_EDGE_ROOM: f32 = 40.0;

/// The narrowest a dialog or picker card is at the reference font size, in
/// pixels, so a short question is still a target.
pub(in crate::renderer) const DIALOG_MIN_WIDTH: f32 = 300.0;

/// The font size the cards' pixel sizes are given at; a larger font scales
/// them up by the same share.
pub(in crate::renderer) const REFERENCE_FONT_PX: f32 = 13.0;

/// How much the cards' pixel sizes grow at the font `ctx` draws with.
pub(in crate::renderer) fn font_scale(ctx: &super::glyphs::FontCtx) -> f32 {
    ctx.font_size / REFERENCE_FONT_PX
}

/// The editor card's share of its pane, across and down: less of its height
/// than its width, so a tall pane does not draw a short menu in a tall card.
const EDITOR_CARD_WIDTH_SHARE: f32 = 0.6;
const EDITOR_CARD_HEIGHT_SHARE: f32 = 0.4;

/// The editor card's smallest and largest size at the reference font, in
/// pixels: tall enough for a menu of about ten entries, or a list of about
/// eight, and no taller, so a short menu is not drawn in a tall card.
const EDITOR_CARD_MIN_W: f32 = 480.0;
const EDITOR_CARD_MAX_W: f32 = 720.0;
const EDITOR_CARD_MIN_H: f32 = 240.0;
const EDITOR_CARD_MAX_H: f32 = 340.0;

/// The narrowest and shortest an editor card shrinks to in a pane too small
/// for it, in pixels.
const EDITOR_CARD_FLOOR: f32 = 100.0;

/// The size `(w, h)` every popup an editor opens is drawn at in a pane
/// `pane_w` by `pane_h`: its menus, the lists they ask for, and its
/// questions' width, so a menu turning into a list keeps its size. The
/// window's own palette is sized apart, to the window.
pub(in crate::renderer) fn editor_card_size(
    ctx: &super::glyphs::FontCtx,
    pane: (f32, f32),
) -> (f32, f32) {
    let (pane_w, pane_h) = pane;
    let s = font_scale(ctx);
    let length = |pane: f32, share: f32, low: f32, high: f32| {
        (pane * share)
            .clamp(low * s, high * s)
            .min((pane - POPUP_EDGE_ROOM).max(EDITOR_CARD_FLOOR))
            .floor()
    };
    (
        length(
            pane_w,
            EDITOR_CARD_WIDTH_SHARE,
            EDITOR_CARD_MIN_W,
            EDITOR_CARD_MAX_W,
        ),
        length(
            pane_h,
            EDITOR_CARD_HEIGHT_SHARE,
            EDITOR_CARD_MIN_H,
            EDITOR_CARD_MAX_H,
        ),
    )
}

pub(in crate::renderer) use crate::DIM_FACTOR;
