//! The drawing machinery the window's parts and widgets share: painting
//! text and artwork into RGBA buffers ([`paint`]), the elevated card a
//! floating widget is drawn on ([`popup`](crate::renderer::widgets::popup)), the images they hand back
//! ([`view`]), and the one way any of them reaches the GPU ([`upload`]),
//! which caches each raster against what it was painted from.

pub(in crate::renderer) mod paint;
pub(in crate::renderer) mod chrome;
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
