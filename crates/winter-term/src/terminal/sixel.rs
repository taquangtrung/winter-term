//! Sixel image decoding: turns a DCS Sixel payload (the bytes between
//! `DCS <params> q` and the String Terminator) into an RGBA raster. The terminal
//! emits the result as an inline image block, reusing the Kitty-graphics path.
//!
//! Sixel encodes six vertical pixels per character (`?`..`~`, low bit = top),
//! with a small command set: `#` color select/define, `!` run-length repeat,
//! `$` carriage return, `-` newline (next six-pixel band), and `"` raster
//! attributes (ignored here beyond skipping their parameters).

use std::collections::HashMap;

// ========================================================================
// Constants
// ========================================================================

/// Hard cap on Sixel image width/height, in pixels. Without it, a single
/// `!4294967295~` (13 bytes) drives an unbounded plot loop; the payload's
/// byte-size cap doesn't help since a compact command can expand far past it.
const MAX_SIXEL_DIM: usize = 4096;

/// The VT340 default Sixel color palette (indices 0..15), as RGB.
const DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [51, 51, 204],
    [204, 33, 33],
    [51, 204, 51],
    [204, 51, 204],
    [51, 204, 204],
    [204, 204, 51],
    [135, 135, 135],
    [66, 66, 66],
    [84, 84, 153],
    [153, 66, 66],
    [84, 153, 84],
    [153, 84, 153],
    [84, 153, 153],
    [153, 153, 84],
    [204, 204, 204],
];

// ========================================================================
// Decoding
// ========================================================================

/// Decode a Sixel payload into an RGBA image, or `None` if it contains no
/// pixels. Written pixels are opaque; never-touched pixels stay transparent.
pub fn decode(data: &[u8]) -> Option<image::RgbaImage> {
    let mut palette: HashMap<u16, [u8; 3]> = HashMap::new();
    for (i, rgb) in DEFAULT_PALETTE.iter().enumerate() {
        palette.insert(i as u16, *rgb);
    }
    let mut color = DEFAULT_PALETTE[0];
    let mut x = 0usize;
    let mut band_top = 0usize;
    let mut width = 0usize;
    let mut height = 0usize;
    let mut pixels: HashMap<(usize, usize), [u8; 3]> = HashMap::new();

    let mut i = 0;
    while i < data.len() {
        if x >= MAX_SIXEL_DIM || band_top >= MAX_SIXEL_DIM {
            break;
        }
        match data[i] {
            b'#' => {
                let (nums, next) = parse_params(data, i + 1);
                i = next;
                if let Some(&pc) = nums.first() {
                    if nums.len() >= 5 {
                        let rgb = match nums[1] {
                            2 => [scale(nums[2]), scale(nums[3]), scale(nums[4])],
                            1 => hls_to_rgb(nums[2], nums[3], nums[4]),
                            _ => color,
                        };
                        palette.insert(pc as u16, rgb);
                    }
                    color = *palette.get(&(pc as u16)).unwrap_or(&color);
                }
            }
            b'!' => {
                let (nums, next) = parse_params(data, i + 1);
                i = next;
                let requested = nums.first().copied().unwrap_or(1).max(1) as usize;
                let count = requested.min(MAX_SIXEL_DIM.saturating_sub(x));
                if i < data.len() && is_sixel(data[i]) {
                    let bits = data[i] - 0x3f;
                    i += 1;
                    for _ in 0..count {
                        plot(
                            &mut pixels,
                            &mut width,
                            &mut height,
                            x,
                            band_top,
                            bits,
                            color,
                        );
                        x += 1;
                    }
                }
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                x = 0;
                band_top += 6;
                i += 1;
            }
            b'"' => {
                let (_nums, next) = parse_params(data, i + 1);
                i = next;
            }
            b if is_sixel(b) => {
                plot(
                    &mut pixels,
                    &mut width,
                    &mut height,
                    x,
                    band_top,
                    b - 0x3f,
                    color,
                );
                x += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }

    if width == 0 || height == 0 {
        return None;
    }
    let mut img = image::RgbaImage::new(width as u32, height as u32);
    for ((px, py), rgb) in pixels {
        img.put_pixel(
            px as u32,
            py as u32,
            image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
        );
    }
    Some(img)
}

/// Set the pixels of one six-pixel column from `bits` (low bit = topmost), and
/// extend the image bounds to include this column even when no bit is set.
fn plot(
    pixels: &mut HashMap<(usize, usize), [u8; 3]>,
    width: &mut usize,
    height: &mut usize,
    x: usize,
    band_top: usize,
    bits: u8,
    color: [u8; 3],
) {
    *width = (*width).max(x + 1);
    for bit in 0..6 {
        if bits & (1 << bit) != 0 {
            let y = band_top + bit;
            pixels.insert((x, y), color);
            *height = (*height).max(y + 1);
        }
    }
}

/// Whether `b` is a Sixel data character (`?`..`~`).
fn is_sixel(b: u8) -> bool {
    (0x3f..=0x7e).contains(&b)
}

/// Scale a 0..100 Sixel color component to 0..255.
fn scale(v: u32) -> u8 {
    (v.min(100) * 255 / 100) as u8
}

/// Parse a run of `;`-separated decimal parameters starting at `start`, stopping
/// at the first non-digit/non-`;` byte. Returns the values and the index after.
fn parse_params(data: &[u8], start: usize) -> (Vec<u32>, usize) {
    let mut nums = Vec::new();
    let mut cur: u32 = 0;
    let mut seen = false;
    let mut i = start;
    while i < data.len() {
        match data[i] {
            b'0'..=b'9' => {
                cur = cur
                    .saturating_mul(10)
                    .saturating_add((data[i] - b'0') as u32);
                seen = true;
            }
            b';' => {
                nums.push(cur);
                cur = 0;
                seen = true;
            }
            _ => break,
        }
        i += 1;
    }
    if seen {
        nums.push(cur);
    }
    (nums, i)
}

/// Convert Sixel HLS (hue 0..360, lightness/saturation 0..100) to RGB. Sixel's
/// hue origin is blue, so it is rotated to the standard red origin first.
fn hls_to_rgb(h: u32, l: u32, s: u32) -> [u8; 3] {
    let h = ((h % 360) as f32 + 120.0) % 360.0 / 360.0;
    let l = (l.min(100) as f32) / 100.0;
    let s = (s.min(100) as f32) / 100.0;
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return [v, v, v];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let to_u8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        to_u8(hue(h + 1.0 / 3.0)),
        to_u8(hue(h)),
        to_u8(hue(h - 1.0 / 3.0)),
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_payload_decodes_to_none() {
        assert!(decode(b"").is_none());
        assert!(decode(b"$-").is_none());
    }

    #[test]
    fn test_single_full_column_is_six_pixels_tall() {
        // '~' (0x7e) sets all six bits in one column.
        let img = decode(b"~").expect("image");
        assert_eq!(img.dimensions(), (1, 6));
        assert_eq!(img.get_pixel(0, 0).0[3], 255);
        assert_eq!(img.get_pixel(0, 5).0[3], 255);
    }

    #[test]
    fn test_repeat_extends_width() {
        // Repeat a full column 4 times -> 4 wide, 6 tall.
        let img = decode(b"!4~").expect("image");
        assert_eq!(img.dimensions(), (4, 6));
    }

    #[test]
    fn test_huge_repeat_count_is_clamped_instead_of_hanging() {
        // Regression: an unclamped `!<count>~` could declare billions of
        // repeats from a few bytes, driving an unbounded plot loop. The
        // clamp must cap the resulting width at MAX_SIXEL_DIM, not the
        // requested count (this test would hang without it).
        let img = decode(b"!4294967295~").expect("image");
        assert_eq!(img.dimensions(), (MAX_SIXEL_DIM as u32, 6));
    }

    #[test]
    fn test_many_newlines_cap_the_height_instead_of_growing_unbounded() {
        // Regression: chaining many `-` (newline) commands grows `band_top`
        // with no cap of its own; height must stop growing once the total
        // reaches MAX_SIXEL_DIM, not keep expanding with every newline.
        let mut payload = Vec::new();
        for _ in 0..(MAX_SIXEL_DIM / 6 + 100) {
            payload.push(b'@');
            payload.push(b'-');
        }
        let img = decode(&payload).expect("image");
        assert!(img.height() as usize <= MAX_SIXEL_DIM);
    }

    #[test]
    fn test_color_definition_applies_rgb() {
        // Define color 1 as pure red (RGB 100;0;0), select it, plot top pixel.
        // '@' (0x40) sets only bit 0 (topmost).
        let img = decode(b"#1;2;100;0;0@").expect("image");
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn test_newline_starts_next_band() {
        // One column, carriage return + band advance, then another column:
        // the second pixel lands six rows down.
        let img = decode(b"@-@").expect("image");
        assert_eq!(img.dimensions(), (1, 7));
        assert_eq!(img.get_pixel(0, 0).0[3], 255);
        assert_eq!(img.get_pixel(0, 6).0[3], 255);
    }
}
