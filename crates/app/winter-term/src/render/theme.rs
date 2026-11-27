//! Winter's theme as misty's: the colors misty's widgets draw in, named
//! from Winter's own (a menu's background is misty's raised surface, the
//! tab bar its chrome surface), so a misty widget over a pane matches the
//! chrome Winter draws around it. [`Themes`] keeps the two together, built
//! once each time Winter's theme changes, so no draw builds misty's again.

use std::borrow::Cow;

use crate::render::{PANE_H_PAD, Theme, ThemeRgb};
use misty::Theme as MistyTheme;
use misty::{LabelFont, Rgb, Rgba, Shape, ShapeShadow, Spacing};

// ========================================================================
// Constants
// ========================================================================

/// The ANSI color misty's danger color takes: red.
const ANSI_RED: usize = 1;

/// The ANSI color misty's data color takes: green, apart from the accent
/// that marks what is active.
const ANSI_GREEN: usize = 2;

/// The ANSI color misty's warning color takes: yellow.
const ANSI_YELLOW: usize = 3;

/// How much of the foreground muted text keeps, the rest the background's.
const MUTED_SHARE: f32 = 0.6;

/// Share of the foreground mixed into `menu_hover_bg` for `misty.hover`, so
/// hover highlights remain visible over a selected row.
const HOVER_LIFT_SHARE: f32 = 0.08;

/// Minimum WCAG contrast ratio for fuzzy-matched text on a popup surface.
const MATCH_MIN_CONTRAST: f64 = 4.5;

/// How light a background is, out of 255, from which the theme counts as
/// light.
const LIGHT_BACKGROUND: u32 = 128;

/// What a picker's hints line calls the keys that show a row's actions,
/// show or hide the preview, and save the rows.
const PICKER_ACTIONS: &str = "actions";
const PICKER_PREVIEW: &str = "Toggle Preview";
const PICKER_SAVE: &str = "save";

/// The shadow a popup casts: even on every side, not dropped below it, a
/// little darker close to the box.
const POPUP_SHADOW: ShapeShadow = ShapeShadow {
    blur: 8,
    color: Rgba::new(0, 0, 0, 71),
    offset_x: 0,
    offset_y: 0,
};

// ========================================================================
// Data Structures
// ========================================================================

/// Winter's theme and misty's built from it, changed only together through
/// [`Themes::set`], so misty's can never fall behind Winter's.
pub struct Themes {
    /// Misty's theme, built from `winter`.
    misty: MistyTheme,
    /// When scrollbars show, kept across a change of theme.
    scrollbar_show: misty::ScrollbarShow,
    /// Winter's theme.
    winter: Theme,
}

// ========================================================================
// Themes
// ========================================================================

impl Themes {
    /// `theme` with misty's built from it.
    pub fn new(theme: Theme) -> Self {
        Self {
            misty: misty_theme(&theme),
            scrollbar_show: misty::ScrollbarShow::default(),
            winter: theme,
        }
    }

    /// Make `theme` Winter's theme, and build misty's from it again.
    pub fn set(&mut self, theme: Theme) {
        let show = self.scrollbar_show;
        *self = Self::new(theme);
        self.set_scrollbar_show(show);
    }

    /// Show scrollbars as `show` says, in misty's theme and Winter's own.
    pub fn set_scrollbar_show(&mut self, show: misty::ScrollbarShow) {
        self.scrollbar_show = show;
        self.misty.scrollbar_show = show;
    }

    /// Misty's theme, built from Winter's.
    pub fn misty(&self) -> &MistyTheme {
        &self.misty
    }

    /// Winter's theme.
    pub fn winter(&self) -> &Theme {
        &self.winter
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's theme for Winter's `theme`: misty's light or dark preset,
/// whichever the background is, with Winter's colors put in, and Winter's
/// own look set rather than left to misty's defaults: chrome labels in the
/// proportional font, rounded boxes with an even shadow, and pages inset
/// from their pane's sides as far as an editor's text is.
fn misty_theme(theme: &Theme) -> MistyTheme {
    let mut misty = match is_light(theme) {
        true => MistyTheme::light(),
        false => MistyTheme::dark(),
    };
    misty.label_font = LabelFont::Proportional;
    misty.shape = Shape {
        shadow: Some(POPUP_SHADOW),
        ..Shape::MODERN
    };
    misty.spacing = Spacing {
        page_inset: PANE_H_PAD as u16,
    };
    // A picker's hints line in Winter's words.
    misty.strings.picker_actions = Cow::Borrowed(PICKER_ACTIONS);
    misty.strings.picker_preview = Cow::Borrowed(PICKER_PREVIEW);
    misty.strings.picker_save = Cow::Borrowed(PICKER_SAVE);
    misty.background = rgb(theme.background);
    misty.background_alpha = theme.background_alpha;
    misty.cursor = rgb(theme.cursor_bg);
    misty.foreground = rgb(theme.foreground);
    let palette = &mut misty.palette;
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
    misty.data = rgb(theme.ansi[ANSI_GREEN]);
    misty.hover = mix(theme.foreground, theme.menu_hover_bg, HOVER_LIFT_SHARE);
    let match_base = match is_light(theme) {
        true => theme.active_indicator,
        false => full_strength(theme.active_indicator),
    };
    misty.match_text = rgb(match_base.legible_on(theme.menu_bg, MATCH_MIN_CONTRAST));
    misty.muted = mix(theme.foreground, theme.background, MUTED_SHARE);
    misty.raised = rgb(theme.menu_bg);
    misty.scrollbar = rgb(theme.scrollbar);
    misty.scrollbar_hover = rgb(theme.scrollbar_hover());
    misty.search_current = rgb(theme.search_current_bg);
    misty.search_match = rgb(theme.search_match_bg);
    // A list's selected row in the menus' band color, apart from selected
    // text, as Winter's own lists drew it.
    misty.selected_row = rgb(theme.menu_hover_bg);
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
pub(crate) fn mix(front: ThemeRgb, back: ThemeRgb, share: f32) -> Rgb {
    let blend = |a: u8, b: u8| (f32::from(a) * share + f32::from(b) * (1.0 - share)).round() as u8;
    Rgb::new(
        blend(front.r, back.r),
        blend(front.g, back.g),
        blend(front.b, back.b),
    )
}

/// Scale `color` until its strongest channel reaches full intensity, keeping
/// its hue and saturation intact so dark-theme match text stays vivid.
fn full_strength(color: ThemeRgb) -> ThemeRgb {
    let top = color.r.max(color.g).max(color.b);
    if top == 0 {
        return color;
    }
    let scale = |channel: u8| (u32::from(channel) * 0xff / u32::from(top)) as u8;
    ThemeRgb::new(scale(color.r), scale(color.g), scale(color.b))
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
        assert_eq!(misty.background, rgb(dark.background));
        assert_eq!(misty.foreground, rgb(dark.foreground));
        assert_eq!(misty.selected_row, rgb(dark.menu_hover_bg));
        assert_ne!(
            misty.hover, misty.selected_row,
            "hover visible over selected row"
        );
        assert_ne!(misty.selected_row, misty.selection, "rows apart from text");
        let light = misty_theme(&Theme::light());
        assert!(is_light(&Theme::light()));
        assert!(!is_light(&dark));
        assert_eq!(light.surface, rgb(Theme::light().tabbar_bg));
    }

    #[test]
    fn test_matched_text_stays_a_vivid_legible_accent_on_dark_and_light_popups() {
        let dark = Theme::dark();
        let dark_matched = misty_theme(&dark).match_text;
        assert_eq!(dark_matched.b, 0xff, "vivid full-blue channel on dark");
        assert!(
            dark_matched.b > dark_matched.g && dark_matched.g > dark_matched.r,
            "keeps the theme's deeper blue hue: {dark_matched:?}"
        );
        assert!(
            dark_matched.r < 0x40,
            "stays rich rather than washed out toward white: {dark_matched:?}"
        );

        let light = Theme::light();
        let light_matched = misty_theme(&light).match_text;
        assert!(
            light_matched.b > light_matched.g && light_matched.g > light_matched.r,
            "keeps the deeper blue hue on light: {light_matched:?}"
        );
        assert!(
            light_matched.b <= light.active_indicator.b,
            "never lightened on a light popup: {light_matched:?}"
        );
    }

    #[test]
    fn test_setting_winters_theme_rebuilds_the_kept_misty_theme() {
        let mut themes = Themes::new(Theme::dark());
        assert_ne!(Theme::dark().menu_bg, Theme::light().menu_bg);
        themes.set(Theme::light());
        assert_eq!(themes.misty().raised, rgb(Theme::light().menu_bg));
        assert_eq!(*themes.misty(), misty_theme(themes.winter()));
    }
}
