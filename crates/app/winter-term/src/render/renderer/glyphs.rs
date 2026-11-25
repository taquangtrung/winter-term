//! The grid's font: its configuration, the sizes it is held to, the
//! weights the user names, and the font scan started before the window.

use super::GpuRenderer;

// ========================================================================
// Constants
// ========================================================================

pub(super) const DEFAULT_FONT_SIZE: f32 = 15.0;

/// The smallest and largest logical font size the renderer takes. Below the
/// floor the line height rounds to zero, which the text shaper asserts
/// against, so a size from config is held inside these before any use.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;

pub(super) const DEFAULT_LINE_HEIGHT: f32 = 20.0;

/// The weight regular text is drawn in when the user has set no
/// `font-weight`, as CSS numbers it.
const DEFAULT_WEIGHT: u16 = 400;

/// The weight bold text is drawn in when the user has set no
/// `font-weight-bold`.
const DEFAULT_BOLD_WEIGHT: u16 = 700;

/// `size` held to the font sizes the renderer can draw: inside
/// [`MIN_FONT_SIZE`]..=[`MAX_FONT_SIZE`], and the default for a size that is
/// no number at all (NaN, infinite), which no clamp can place.
pub fn clamp_font_size(size: f32) -> f32 {
    match size.is_finite() {
        true => size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE),
        false => DEFAULT_FONT_SIZE,
    }
}

// ========================================================================
// Implementation
// ========================================================================

/// Font selection for the renderer. `family` is the primary family name (e.g.
/// "JetBrains Mono"); `None` falls back to the system monospace font.
/// Glyphs missing from the primary font are filled in from the system font
/// database automatically.
#[derive(Clone, Debug)]
pub struct FontConfig {
    /// Primary font family, or `None` for the system monospace font.
    pub family: Option<String>,
    /// Size in points.
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
        }
    }
}

/// Start the system-font scan misty measures the grid's cell and renders
/// rich blocks with, on a background thread. Scanning takes ~150ms and needs
/// no GPU, so call this as early as possible (before misty sets up the GPU)
/// so the scan overlaps that instead of adding to it; the first measurement
/// waits for it.
pub fn start_font_load() {
    std::thread::spawn(|| misty::Font::default().cell_metrics(1.0));
}

impl GpuRenderer {
    /// Change the logical font size and recompute cell dimensions. Returns
    /// `Some((cols, rows))` if the size changed, `None` if already at that size.
    pub fn set_font_size(&mut self, logical_size: f32) -> Option<(usize, usize)> {
        let logical_size = clamp_font_size(logical_size);
        if (self.fonts.logical_font_size - logical_size).abs() < 0.01 {
            return None;
        }
        self.fonts.logical_font_size = logical_size;
        self.measure_grid();
        Some((self.cols, self.rows))
    }

    /// Enable or disable OpenType ligatures in the grid's text.
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.fonts.ligatures = enabled;
    }
}

/// The weight, as CSS numbers it, the user's `font-weight` asks the grid's
/// regular text in: regular when unset or unreadable.
pub fn regular_weight(weight: Option<&str>) -> u16 {
    parse_weight(weight).unwrap_or(DEFAULT_WEIGHT)
}

/// The weight the user's `font-weight-bold` asks the grid's bold text in:
/// [`DEFAULT_BOLD_WEIGHT`] when unset or unreadable.
pub fn bold_weight(weight: Option<&str>) -> u16 {
    parse_weight(weight).unwrap_or(DEFAULT_BOLD_WEIGHT)
}

/// The weight `weight` names, by its CSS name (`"medium"`) or number
/// (`"500"`); `None` when unset or unreadable.
fn parse_weight(weight: Option<&str>) -> Option<u16> {
    let weight = weight?.to_lowercase();
    match weight.as_str() {
        "thin" => Some(100),
        "extra-light" | "extralight" => Some(200),
        "light" => Some(300),
        "normal" | "regular" => Some(400),
        "medium" => Some(500),
        "semibold" | "semi-bold" => Some(600),
        "bold" => Some(700),
        "extra-bold" | "extrabold" => Some(800),
        "black" | "heavy" => Some(900),
        number => number.parse().ok(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_weight_is_read_by_name_or_number_and_defaults_otherwise() {
        assert_eq!(regular_weight(Some("Medium")), 500);
        assert_eq!(regular_weight(Some("350")), 350);
        assert_eq!(regular_weight(Some("heavyish")), DEFAULT_WEIGHT);
        assert_eq!(regular_weight(None), DEFAULT_WEIGHT);
        assert_eq!(bold_weight(Some("extra-bold")), 800);
        assert_eq!(bold_weight(None), DEFAULT_BOLD_WEIGHT);
    }

    #[test]
    fn test_a_font_size_is_held_to_what_the_shaper_can_lay_out() {
        assert_eq!(
            clamp_font_size(0.0),
            MIN_FONT_SIZE,
            "a zero line height panics"
        );
        assert_eq!(clamp_font_size(-4.0), MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(500.0), MAX_FONT_SIZE);
        assert_eq!(
            clamp_font_size(f32::NAN),
            DEFAULT_FONT_SIZE,
            "no clamp places NaN"
        );
        assert_eq!(clamp_font_size(13.0), 13.0);
    }
}
