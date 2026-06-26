//! Rasterizing SVG documents with resvg, shared by the GPU image pass and the
//! chrome painted on the CPU, so an icon comes out at the same size and in the
//! same place in a tool's rows, a pane strip, and a popup.

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

// ========================================================================
// Constants
// ========================================================================

/// The widest and tallest an SVG is rasterized, whatever it or its box asks.
pub(super) const MAX_SVG_DIM: u32 = 4096;

// ========================================================================
// Data Structures
// ========================================================================

/// An SVG rasterized to straight-alpha RGBA, row by row.
pub(super) struct SvgRaster {
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
    pub(super) width: u32,
}

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize `svg` to fill a `box_w` x `box_h` pixel box, keeping its aspect
/// ratio and centering it in whatever room is left over. `None` when the
/// document does not parse or has no size.
///
/// An icon has to be sharp at one specific size, which means rasterizing to
/// that size rather than sampling a larger raster down to it.
pub(super) fn rasterize_in_box(
    svg: &[u8],
    options: &Options,
    box_w: u32,
    box_h: u32,
) -> Option<SvgRaster> {
    let tree = Tree::from_data(svg, options).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let width = box_w.clamp(1, MAX_SVG_DIM);
    let height = box_h.clamp(1, MAX_SVG_DIM);
    let mut pixmap = Pixmap::new(width, height)?;
    let scale = (width as f32 / size.width()).min(height as f32 / size.height());
    let transform = Transform::from_translate(
        (width as f32 - size.width() * scale) / 2.0,
        (height as f32 - size.height() * scale) / 2.0,
    )
    .pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(SvgRaster {
        height,
        rgba: straight_rgba(&pixmap),
        width,
    })
}

/// A pixmap's pixels as straight-alpha RGBA. tiny-skia stores premultiplied
/// alpha, and both the image pass and the chrome's blending want straight.
pub(super) fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    rgba
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2:1 document, solid red, so where it lands in a square box shows.
    const WIDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#ff0000"/></svg>"##;

    fn alpha_at(raster: &SvgRaster, x: u32, y: u32) -> u8 {
        raster.rgba[((y * raster.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn test_a_wide_icon_is_centered_in_a_square_box() {
        // The CPU path used to pin the artwork to the box's top edge while the
        // GPU path centered it, so the same icon sat lower in a tool's rows
        // than in a pane strip. One routine for both is what this pins.
        let raster = rasterize_in_box(WIDE_SVG, &Options::default(), 20, 20).expect("rasterizes");
        assert_eq!((raster.width, raster.height), (20, 20));
        assert_eq!(alpha_at(&raster, 10, 2), 0, "empty band above");
        assert_eq!(alpha_at(&raster, 10, 10), 255, "the artwork in the middle");
        assert_eq!(alpha_at(&raster, 10, 17), 0, "empty band below");
    }

    #[test]
    fn test_an_unparsable_document_rasterizes_to_nothing() {
        assert!(rasterize_in_box(b"not svg", &Options::default(), 8, 8).is_none());
    }
}
