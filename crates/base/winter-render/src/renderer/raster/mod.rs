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

/// Texture slots for the status bar's icons, from the left: left-segment
/// icons (project, branch), quota logos, and the editor's icon. No more icons
/// are drawn than there are slots.
pub(in crate::renderer) const STATUS_ICON_TEXTURE_IDS: [u64; 8] = [
    u64::MAX - 12,
    u64::MAX - 13,
    u64::MAX - 14,
    u64::MAX - 15,
    u64::MAX - 16,
    u64::MAX - 17,
    u64::MAX - 18,
    u64::MAX - 19,
];

/// Vertical padding above and below the text in each palette result row, in
/// pixels. The row height is one line of text plus this on each side, so the
/// inter-row gap is a small fixed amount rather than a multiple of the line.
pub(in crate::renderer) const PALETTE_ITEM_PAD_Y: f32 = 4.0;

/// Side of an icon's square beside a line of chrome text (a tab title, a
/// palette entry, the status bar's editor), as a fraction of cell height. One
/// size for all of them, so a file reads the same wherever it is named.
pub(in crate::renderer) const ICON_HEIGHT_RATIO: f32 = 0.85;

/// Gap between an icon and the text it heads, in cells.
pub(in crate::renderer) const ICON_GAP_CELLS: f32 = 0.5;

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

// ========================================================================
// Test support
// ========================================================================

/// Helpers the parts' and widgets' tests share.
#[cfg(test)]
pub(in crate::renderer) mod test_support {
    use super::view::StripImage;

    pub(in crate::renderer) const CELL_W: f32 = 9.0;
    pub(in crate::renderer) const CELL_H: f32 = 18.0;

    /// The RGBA pixel at `(x, y)`.
    pub(in crate::renderer) fn pixel(image: &StripImage, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * image.width + x) * 4) as usize;
        (
            image.rgba[i],
            image.rgba[i + 1],
            image.rgba[i + 2],
            image.rgba[i + 3],
        )
    }

    /// The `(x, y, width, height)` box around every pixel where `after`
    /// differs from `before`, over a `width`-wide RGBA image.
    pub(in crate::renderer) fn changed_box(
        before: &[u8],
        after: &[u8],
        width: u32,
    ) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, (b, a)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
            if b != a {
                let (x, y) = (i as u32 % width, i as u32 / width);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        assert!(x0 != u32::MAX, "nothing changed");
        (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }
}
