//! What is drawn in a pane rather than painted as cells, by blocks and editors
//! alike: the content, from an encoded file to pixels already drawn, and the
//! geometry that places it and crops it to where it may show.

use std::sync::Arc;

// ========================================================================
// Constants
// ========================================================================

/// How much one zoom step scales by.
const ZOOM_STEP: f32 = 1.25;

/// The smallest and largest scale a zoom reaches.
const MIN_ZOOM: f32 = 0.01;
const MAX_ZOOM: f32 = 64.0;

/// The middle of the picture, where a fitted one is looked at from.
const MIDDLE: (f32, f32) = (0.5, 0.5);

// ========================================================================
// Data Structures
// ========================================================================

/// What an editor asks the host to draw over its pane.
#[derive(Clone, Debug)]
pub struct Picture {
    /// Names the contents, so the host decodes them once and again only
    /// when they change.
    pub key: u64,
    /// What is drawn.
    pub content: Content,
    /// How it is shown.
    pub view: PictureView,
}

/// Something drawn in a pane: what an inline block shows, and what an editor
/// shows over its pane.
#[derive(Clone, Debug)]
pub enum Content {
    /// Markdown, laid out to the width it is drawn at.
    Markdown(Arc<str>),
    /// Pixels already drawn.
    Pixels(Arc<Pixels>),
    /// Encoded pixels: PNG, JPEG, GIF, WebP, BMP, or ICO, sharpest at their
    /// own size.
    Raster(Arc<[u8]>),
    /// A Vello scene, drawn on the GPU at the size it carries.
    Scene(Arc<SizedScene>),
    /// Monospace text, laid out to the width it is drawn at.
    Text(Arc<str>),
    /// An SVG document, drawn again at whatever size it is shown.
    Vector(Arc<[u8]>),
}

/// Content an editor placed in its pane itself: a page of a document drawn at
/// the size it is shown, or a box over it. Past the pane on a side it
/// overflows, where the host crops it.
#[derive(Clone, Debug)]
pub struct Tile {
    /// How tall it is drawn, in pane pixels.
    pub height: f32,
    /// Names the content, so the host uploads it once and again only when it
    /// changes.
    pub key: u64,
    /// What is drawn.
    pub content: Content,
    /// How wide it is drawn, in pane pixels.
    pub width: f32,
    /// Its left edge, from the pane's.
    pub x: f32,
    /// Its top edge, from the pane's.
    pub y: f32,
}

/// Pixels an editor drew itself: straight RGBA, four bytes a pixel, row by row.
#[derive(Debug)]
pub struct Pixels {
    /// Rows.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
    /// Pixels a row.
    pub width: u32,
}

/// A scene an editor painted (part of a web page laid out by Blitz) and the
/// pixel size it is drawn at.
#[derive(Default)]
pub struct SizedScene {
    /// Rows.
    pub height: u32,
    /// The paint operations, in pixels from its top-left.
    pub scene: vello::Scene,
    /// Pixels a row.
    pub width: u32,
}

/// How a picture is shown: fitted to the pane or at a scale, and which point
/// of it is at the middle of the pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureView {
    /// The point at the middle of the pane, as fractions of the picture's
    /// width and height. Only matters along a side the picture overflows.
    pub center: (f32, f32),
    /// Pixels shown per pixel of the picture, or `None` to fit the pane.
    pub zoom: Option<f32>,
}

/// Where a picture lands, in pixels from the pane's top-left: past the pane
/// on a side it overflows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureFrame {
    /// How tall it is drawn.
    pub height: f32,
    /// Pixels shown per pixel of the picture.
    pub scale: f32,
    /// How wide it is drawn.
    pub width: f32,
    /// Its left edge.
    pub x: f32,
    /// Its top edge.
    pub y: f32,
}

/// The part of a [`PictureFrame`] inside the bounds it may show in: where it
/// is drawn, and which part of the picture fills it, as fractions of its
/// width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureClip {
    /// How tall the visible part is drawn.
    pub height: f32,
    /// Where the visible part ends across the picture.
    pub u_max: f32,
    /// Where it starts across the picture.
    pub u_min: f32,
    /// Where it ends down the picture.
    pub v_max: f32,
    /// Where it starts down the picture.
    pub v_min: f32,
    /// How wide the visible part is drawn.
    pub width: f32,
    /// Its left edge.
    pub x: f32,
    /// Its top edge.
    pub y: f32,
}

// ========================================================================
// Content
// ========================================================================

impl Content {
    /// Whether it stays sharp at any size, and so may be shown larger than
    /// its own.
    pub fn is_vector(&self) -> bool {
        matches!(self, Self::Vector(_))
    }

    /// Whether it is laid out to the width it is drawn at, rather than
    /// having a size of its own.
    pub fn wraps(&self) -> bool {
        matches!(self, Self::Markdown(_) | Self::Text(_))
    }
}

// ========================================================================
// SizedScene
// ========================================================================

impl std::fmt::Debug for SizedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A scene's encoding is opaque: its size is what tells one apart.
        f.debug_struct("SizedScene")
            .field("height", &self.height)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// PictureFrame
// ========================================================================

impl From<&Tile> for PictureFrame {
    fn from(tile: &Tile) -> Self {
        Self {
            height: tile.height,
            scale: 1.0,
            width: tile.width,
            x: tile.x,
            y: tile.y,
        }
    }
}

impl PictureFrame {
    /// The frame moved onto whole pixels of the window, for a pane whose
    /// top-left sits at `origin` there. Drawn at its own size, each of the
    /// picture's pixels then lands on one of the screen's; at a fractional
    /// offset the sampler blends every pixel with its neighbour, and the
    /// whole picture blurs.
    pub fn snapped(self, origin: (f32, f32)) -> Self {
        Self {
            x: (origin.0 + self.x).round() - origin.0,
            y: (origin.1 + self.y).round() - origin.1,
            ..self
        }
    }

    /// The part of the frame inside `bounds`, `(left, top, width, height)`
    /// in the frame's own pixels, or `None` when none of it is.
    pub fn clip(&self, bounds: (f32, f32, f32, f32)) -> Option<PictureClip> {
        let left = self.x.max(bounds.0);
        let top = self.y.max(bounds.1);
        let right = (self.x + self.width).min(bounds.0 + bounds.2);
        let bottom = (self.y + self.height).min(bounds.1 + bounds.3);
        if right <= left || bottom <= top {
            return None;
        }
        Some(PictureClip {
            height: bottom - top,
            u_max: (right - self.x) / self.width,
            u_min: (left - self.x) / self.width,
            v_max: (bottom - self.y) / self.height,
            v_min: (top - self.y) / self.height,
            width: right - left,
            x: left,
            y: top,
        })
    }
}

// ========================================================================
// PictureView
// ========================================================================

impl PictureView {
    /// The whole picture, fitted to the pane.
    pub fn fitted() -> Self {
        Self {
            center: MIDDLE,
            zoom: None,
        }
    }

    /// The scale the picture is shown at: its zoom, or the one that fits a
    /// picture of size `natural` into `pane`. A raster picture is never
    /// fitted past its own size, where it would only blur.
    pub fn scale(&self, natural: (f32, f32), pane: (f32, f32), vector: bool) -> f32 {
        if let Some(zoom) = self.zoom {
            return zoom;
        }
        let fit = (pane.0 / natural.0).min(pane.1 / natural.1);
        match vector {
            true => fit,
            false => fit.min(1.0),
        }
    }

    /// Where a picture of size `natural` lands in `pane`: in the middle along
    /// a side it fits, and with [`Self::center`] at the middle of the pane
    /// along one it overflows, never leaving a gap at an edge.
    pub fn frame(&self, natural: (f32, f32), pane: (f32, f32), vector: bool) -> PictureFrame {
        let scale = self.scale(natural, pane, vector);
        let (width, height) = (natural.0 * scale, natural.1 * scale);
        PictureFrame {
            height,
            scale,
            width,
            x: place(width, pane.0, self.center.0),
            y: place(height, pane.1, self.center.1),
        }
    }

    /// Zoom by `steps` from the scale shown now, keeping the middle of the
    /// pane on the same point of the picture.
    pub fn zoom_by(&mut self, steps: i32, natural: (f32, f32), pane: (f32, f32), vector: bool) {
        let scale = self.scale(natural, pane, vector) * ZOOM_STEP.powi(steps);
        self.zoom = Some(scale.clamp(MIN_ZOOM, MAX_ZOOM));
    }

    /// Show the picture at its own size.
    pub fn actual_size(&mut self) {
        self.zoom = Some(1.0);
    }

    /// Fit the whole picture to the pane again.
    pub fn fit(&mut self) {
        *self = Self::fitted();
    }

    /// Move what is shown by `delta` pane pixels across and down, stopping
    /// where an edge of the picture meets the pane's.
    pub fn pan(&mut self, delta: (f32, f32), natural: (f32, f32), pane: (f32, f32), vector: bool) {
        let frame = self.frame(natural, pane, vector);
        self.center = (
            pan_axis(self.center.0, delta.0, frame.width, pane.0),
            pan_axis(self.center.1, delta.1, frame.height, pane.1),
        );
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Where along one side a picture `extent` long starts in `room`, with the
/// fraction `center` of it at the middle of the room.
fn place(extent: f32, room: f32, center: f32) -> f32 {
    if extent <= room {
        return (room - extent) / 2.0;
    }
    room / 2.0 - clamp_center(center, extent, room) * extent
}

/// `center` moved by `delta` pixels along a side `extent` long shown in
/// `room`, kept where no gap opens at either edge.
fn pan_axis(center: f32, delta: f32, extent: f32, room: f32) -> f32 {
    if extent <= room {
        return MIDDLE.0;
    }
    clamp_center(
        clamp_center(center, extent, room) + delta / extent,
        extent,
        room,
    )
}

/// The nearest point to `center` that can be at the middle of `room`
/// without the picture's edge coming inside it.
fn clamp_center(center: f32, extent: f32, room: f32) -> f32 {
    let half = room / 2.0 / extent;
    center.clamp(half, 1.0 - half)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: (f32, f32) = (800.0, 600.0);

    #[test]
    fn test_a_snapped_frame_starts_on_a_whole_window_pixel() {
        // Regression: a PDF page centered in its pane, in a pane itself at a
        // fractional place, sat between pixels and every line of it blurred.
        let frame = PictureFrame {
            height: 792.0,
            scale: 1.0,
            width: 612.0,
            x: 94.5,
            y: 12.25,
        };
        let origin = (300.4, 40.6);
        let snapped = frame.snapped(origin);
        assert_eq!((origin.0 + snapped.x).fract(), 0.0);
        assert_eq!((origin.1 + snapped.y).fract(), 0.0);
        assert!((snapped.x - frame.x).abs() <= 0.5 && (snapped.y - frame.y).abs() <= 0.5);
        assert_eq!(
            (snapped.width, snapped.height),
            (612.0, 792.0),
            "never resized"
        );
    }

    #[test]
    fn test_a_small_photo_fits_at_its_own_size_in_the_middle() {
        let frame = PictureView::fitted().frame((200.0, 100.0), PANE, false);
        assert_eq!(frame.scale, 1.0, "not blown up past its pixels");
        assert_eq!((frame.x, frame.y), (300.0, 250.0));
    }

    #[test]
    fn test_a_small_drawing_is_fitted_up_to_the_pane() {
        let frame = PictureView::fitted().frame((200.0, 100.0), PANE, true);
        assert_eq!(frame.scale, 4.0, "the width is what bounds it");
        assert_eq!((frame.width, frame.height), (800.0, 400.0));
        assert_eq!((frame.x, frame.y), (0.0, 100.0));
    }

    #[test]
    fn test_a_zoom_from_fitted_starts_at_the_fitted_scale() {
        let natural = (1600.0, 1200.0);
        let mut view = PictureView::fitted();
        view.zoom_by(1, natural, PANE, false);
        assert_eq!(view.zoom, Some(0.5 * ZOOM_STEP), "one step past the fit");
    }

    #[test]
    fn test_a_picture_overflowing_the_pane_is_cropped_to_it() {
        let mut view = PictureView::fitted();
        view.actual_size();
        let frame = view.frame((1600.0, 300.0), PANE, false);
        assert_eq!(
            (frame.x, frame.y),
            (-400.0, 150.0),
            "its middle in the middle"
        );
        let clip = frame.clip((0.0, 0.0, PANE.0, PANE.1)).expect("on screen");
        assert_eq!((clip.x, clip.width), (0.0, 800.0));
        assert_eq!((clip.u_min, clip.u_max), (0.25, 0.75));
        assert_eq!((clip.v_min, clip.v_max), (0.0, 1.0), "fits top to bottom");
    }

    #[test]
    fn test_panning_stops_where_an_edge_meets_the_pane() {
        let natural = (1600.0, 1200.0);
        let mut view = PictureView::fitted();
        view.actual_size();
        view.pan((-10_000.0, 0.0), natural, PANE, false);
        assert_eq!(
            view.frame(natural, PANE, false).x,
            0.0,
            "left edge at the left"
        );
        view.pan((10_000.0, 10_000.0), natural, PANE, false);
        let frame = view.frame(natural, PANE, false);
        assert_eq!(frame.x + frame.width, PANE.0, "right edge at the right");
        assert_eq!(frame.y + frame.height, PANE.1, "bottom edge at the bottom");
        // Back one step from the far edge moves at once, rather than first
        // unwinding the overshoot.
        view.pan((-100.0, 0.0), natural, PANE, false);
        let frame = view.frame(natural, PANE, false);
        assert_eq!(frame.x + frame.width, PANE.0 + 100.0);
    }

    #[test]
    fn test_a_frame_wholly_off_the_pane_draws_nothing() {
        let frame = PictureFrame {
            height: 10.0,
            scale: 1.0,
            width: 10.0,
            x: PANE.0 + 5.0,
            y: 0.0,
        };
        assert_eq!(frame.clip((0.0, 0.0, PANE.0, PANE.1)), None);
    }
}
