//! The textures of the pictures a window shows: decoded images, rasterized
//! popups and toolbars, and scenes Vello renders, each cached by id on
//! misty's GPU for the host to draw.

use std::collections::HashMap;

use wgpu::{
    Extent3d, Origin3d, Queue, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureView, TextureViewDescriptor,
};

// ========================================================================
// Data Structures
// ========================================================================

/// Where to draw a cached image this frame, in pixels from the surface
/// top-left. `id` keys the uploaded texture. `u_min`/`u_max` and
/// `v_min`/`v_max` bound the sampled texture columns and rows (`0.0`/`1.0` =
/// whole image); narrowing them crops the image
/// rather than squashing it, which is how a block is kept inside the content
/// area as it scrolls past either edge. `alpha` multiplies the sampled color
/// (1.0 = full opacity), used to dim a closed live block.
#[derive(Clone, Copy, Debug)]
pub struct ImagePlacement {
    /// Opacity multiplier applied to the sampled color, `1.0` being fully
    /// opaque. Used to dim a closed live block.
    pub alpha: f32,
    /// Destination height in physical pixels.
    pub height: f32,
    /// Which uploaded texture to sample.
    pub id: u64,
    /// Right bound of the sampled `u` texture coordinate, `1.0` for a whole
    /// image. Below `1.0` the image is cropped from the right, which is how
    /// a picture zoomed wider than its pane is kept inside it.
    pub u_max: f32,
    /// Left bound of the sampled `u` texture coordinate, `0.0` for a whole
    /// image. Above `0.0` the image is cropped from the left.
    pub u_min: f32,
    /// Upper bound of the sampled `v` texture coordinate, `1.0` for a whole
    /// image. Below `1.0` the image is cropped from the bottom, which is how a
    /// block taller than its reserved band, or one running past the bottom of
    /// the content area, is clipped.
    pub v_max: f32,
    /// Lower bound of the sampled `v` texture coordinate, `0.0` for a whole
    /// image. Above `0.0` the image is cropped from the top, which is how a
    /// block whose band has scrolled partly above the content area keeps its
    /// remaining rows aligned with the grid instead of disappearing.
    pub v_min: f32,
    /// Destination width in physical pixels.
    pub width: f32,
    /// Destination left edge in physical pixels, from the window's left.
    pub x: f32,
    /// Destination top edge in physical pixels, from the window's top.
    pub y: f32,
}

/// The textures cached by id.
#[derive(Default)]
pub(crate) struct ImagePass {
    textures: HashMap<u64, ImageTexture>,
}

/// One cached image: its texture and size, so a same-sized upload can
/// rewrite the pixels in place rather than allocate a fresh texture.
struct ImageTexture {
    height: u32,
    /// The texture, misty's so a host drawing with misty draws it.
    texture: misty::GpuTexture,
    width: u32,
}

// ========================================================================
// ImagePass
// ========================================================================

impl ImagePass {
    /// Drop the texture cached under `id`, releasing its GPU memory. The
    /// bind group owns the texture view, which owns the texture, so removing
    /// it is what actually frees the memory; without this the cache only
    /// ever grows, keeping every decoded image block alive for the life of
    /// the process.
    pub fn remove(&mut self, id: u64) {
        self.textures.remove(&id);
    }
    /// Upload `rgba` (tightly packed, `width * height * 4` bytes) as a texture
    /// cached under `id`, replacing any previous one. A replacement of the
    /// same size is written into the texture already there: an overlay
    /// repainted while it stays open costs a copy, not an allocation.
    ///
    /// An image the GPU cannot hold (no pixels, more than its largest texture
    /// side, or bytes that do not match the size) is not uploaded, and `id`
    /// keeps what it had: handed to wgpu, it failed every frame with an error
    /// on the log and nothing drawn. Says whether it was uploaded.
    pub fn upload(
        &mut self,
        gpu: &misty::Gpu,
        id: u64,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> bool {
        if !fits_texture(
            width,
            height,
            rgba.len(),
            gpu.device().limits().max_texture_dimension_2d,
        ) {
            return false;
        }
        let Some(cached) = self.ensure_texture(gpu, id, width, height) else {
            return false;
        };
        write_rgba(gpu.queue(), cached.texture.texture(), rgba, width, height);
        true
    }
    /// The texture cached under `id`, for a host drawing it itself.
    pub fn texture(&self, id: u64) -> Option<&misty::GpuTexture> {
        self.textures.get(&id).map(|cached| &cached.texture)
    }
    /// A view of the `width` x `height` texture cached under `id`, for a
    /// compute pass to draw into (a scene rendered by Vello), made or
    /// reused as [`Self::upload`] does.
    pub fn target(
        &mut self,
        gpu: &misty::Gpu,
        id: u64,
        width: u32,
        height: u32,
    ) -> Option<TextureView> {
        let cached = self.ensure_texture(gpu, id, width, height)?;
        Some(
            cached
                .texture
                .texture()
                .create_view(&TextureViewDescriptor::default()),
        )
    }
    /// The texture cached under `id`, made anew unless the one there is
    /// already `width` x `height`.
    fn ensure_texture(
        &mut self,
        gpu: &misty::Gpu,
        id: u64,
        width: u32,
        height: u32,
    ) -> Option<&ImageTexture> {
        let reusable = self
            .textures
            .get(&id)
            .is_some_and(|cached| (cached.width, cached.height) == (width, height));
        if !reusable {
            let image = Self::create_texture(gpu, width, height)?;
            self.textures.insert(id, image);
        }
        self.textures.get(&id)
    }

    /// A `width` x `height` texture, or `None` for a size the GPU cannot
    /// hold. It holds sRGB-encoded bytes, as Vello writes them too.
    fn create_texture(gpu: &misty::Gpu, width: u32, height: u32) -> Option<ImageTexture> {
        let texture = misty::GpuTexture::new(gpu, width, height).ok()?;
        Some(ImageTexture {
            height,
            texture,
            width,
        })
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Write `rgba` (`width` x `height`, tightly packed) over all of `texture`.
fn write_rgba(queue: &Queue, texture: &Texture, rgba: &[u8], width: u32, height: u32) {
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        rgba,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Whether a `width` x `height` image of `bytes` RGBA bytes can be a
/// texture on a device whose largest side is `max_side`.
fn fits_texture(width: u32, height: u32, bytes: usize, max_side: u32) -> bool {
    let expected = width as usize * height as usize * 4;
    width > 0 && height > 0 && width <= max_side && height <= max_side && bytes == expected
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_image_the_gpu_cannot_hold_is_not_uploaded() {
        // A zero-sized strip or an outline taller than the largest texture
        // went to wgpu as it was, and failed there every frame.
        assert!(fits_texture(4, 2, 32, 8192));
        assert!(!fits_texture(0, 2, 0, 8192), "no pixels");
        assert!(
            !fits_texture(4, 9000, 4 * 9000 * 4, 8192),
            "taller than a texture"
        );
        assert!(!fits_texture(4, 2, 31, 8192), "bytes not matching the size");
    }
}
