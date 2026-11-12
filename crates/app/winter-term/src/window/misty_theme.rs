//! Winter's theme as misty's: the colors misty's widgets draw in, named
//! from Winter's own (a menu's background is misty's raised surface, the
//! tab bar its chrome surface), so a misty widget over a pane matches the
//! chrome Winter draws around it.

use crate::render::{Theme, ThemeRgb};
use misty::color::Rgb;
use misty::theme::Theme as MistyTheme;

// ========================================================================
// Constants
// ========================================================================

/// The ANSI color misty's danger color takes: red.
const ANSI_RED: usize = 1;

/// The ANSI color misty's warning color takes: yellow.
const ANSI_YELLOW: usize = 3;

/// How much of the foreground muted text keeps, the rest the background's.
const MUTED_SHARE: f32 = 0.6;

/// How light a background is, out of 255, from which the theme counts as
/// light.
const LIGHT_BACKGROUND: u32 = 128;

// ========================================================================
// Functions
// ========================================================================

/// Misty's theme for Winter's `theme`: misty's light or dark preset,
/// whichever the background is, with Winter's colors put in.
pub(in crate::window) fn misty_theme(theme: &Theme) -> MistyTheme {
    let mut misty = match is_light(theme) {
        true => MistyTheme::light(),
        false => MistyTheme::dark(),
    };
    let palette = &mut misty.palette;
    palette.background = rgb(theme.background);
    palette.background_alpha = theme.background_alpha;
    palette.cursor = rgb(theme.cursor_bg);
    palette.foreground = rgb(theme.foreground);
    for (slot, color) in palette.colors.iter_mut().zip(theme.ansi) {
        *slot = rgb(color);
    }
    for &(index, color) in &theme.indexed {
        palette.colors[usize::from(index)] = rgb(color);
    }
    misty.accent = rgb(theme.active_indicator);
    misty.border = rgb(theme.divider);
    misty.cursor_line = rgb(theme.cursor_line_bg);
    misty.danger = rgb(theme.ansi[ANSI_RED]);
    misty.hover = rgb(theme.menu_hover_bg);
    misty.muted = mix(theme.foreground, theme.background, MUTED_SHARE);
    misty.raised = rgb(theme.menu_bg);
    misty.scrollbar = rgb(theme.scrollbar);
    misty.search_current = rgb(theme.search_current_bg);
    misty.search_match = rgb(theme.search_match_bg);
    misty.selection = rgb(theme.selection_bg);
    misty.surface = rgb(theme.tabbar_bg);
    misty.warning = rgb(theme.ansi[ANSI_YELLOW]);
    misty
}

/// Whether `theme`'s background is light.
fn is_light(theme: &Theme) -> bool {
    let background = theme.background;
    let sum = u32::from(background.r) + u32::from(background.g) + u32::from(background.b);
    sum / 3 >= LIGHT_BACKGROUND
}

/// `share` of `front` over the rest of `back`, in misty's colors.
fn mix(front: ThemeRgb, back: ThemeRgb, share: f32) -> Rgb {
    let blend = |a: u8, b: u8| (f32::from(a) * share + f32::from(b) * (1.0 - share)).round() as u8;
    Rgb::new(
        blend(front.r, back.r),
        blend(front.g, back.g),
        blend(front.b, back.b),
    )
}

/// Misty's color for Winter's `color`, its alpha left behind.
fn rgb(color: ThemeRgb) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_winters_colors_land_in_misty_and_the_preset_follows_the_background() {
        let dark = Theme::dark();
        let misty = misty_theme(&dark);
        assert_eq!(misty.raised, rgb(dark.menu_bg));
        assert_eq!(misty.palette.colors[ANSI_RED], rgb(dark.ansi[ANSI_RED]));
        assert_eq!(misty.palette.background, rgb(dark.background));
        assert_eq!(misty.palette.foreground, rgb(dark.foreground));
        let light = misty_theme(&Theme::light());
        assert!(is_light(&Theme::light()));
        assert!(!is_light(&dark));
        assert_eq!(light.surface, rgb(Theme::light().tabbar_bg));
    }
}
