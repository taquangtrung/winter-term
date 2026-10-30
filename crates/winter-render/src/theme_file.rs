//! A theme in KDL: the `colors` block a settings or theme file holds, the
//! overrides it comes to, how those are laid over a theme and written back,
//! and the built-in themes, which are theme files themselves.
//!
//! Every scalar color (one value, as opposed to the `ansi`, `git`, `syntax`
//! and `indexed` blocks) is named once, in the table `scalar_colors!` is
//! given: its KDL name, and the theme field it sets. The schema field, the
//! override slot, the theme it is applied to, and both writers (a settings
//! file's overrides, a theme file's full palette) all come from that one
//! entry, so a color added there is read, applied and saved everywhere.
//!
//! The background stands apart: it is not laid over anything, its alpha is
//! the window's own translucency, and the window asks for it on its own.

use std::collections::HashMap;

use crate::syntax::SyntaxColors;
use crate::theme::{GitColors, Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// How many colors the `ansi` block (and the `brights` block) names.
const ANSI_SLOTS: usize = 8;

/// The 8 standard ANSI color names, in SGR order (index 0 = black, ...,
/// index 7 = white). `brights` reuses the same names for its own 8 slots.
const ANSI_COLOR_NAMES: [&str; ANSI_SLOTS] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// The background's node name in the `colors` block.
const BACKGROUND: &str = "background";

/// The built-in dark theme: magic-vscode's One Color theme, from its editor
/// and terminal colors.
pub(crate) const DARK_THEME_KDL: &str = include_str!("../builtin/themes/dark.kdl");

/// The built-in light theme: the dark one's palette on a light surface, its
/// source colors darkened until they read there.
pub(crate) const LIGHT_THEME_KDL: &str = include_str!("../builtin/themes/light.kdl");

// ========================================================================
// Data Structures
// ========================================================================

/// One scalar color: its KDL name, how to read it from a theme, and how to
/// set it in one.
#[derive(Clone, Copy, Debug)]
pub struct ScalarColor {
    /// Its node name in the `colors` block.
    pub name: &'static str,
    /// Its value in a theme.
    pub read: fn(&Theme) -> Rgb,
    /// Set it in a theme.
    pub write: fn(&mut Theme, Rgb),
}

/// Per-color overrides parsed from the KDL `colors` block. Applied on top of the
/// active preset ([`Theme::dark`]/[`Theme::light`]); unset colors keep the preset.
#[derive(Clone, Debug, Default)]
pub struct ColorOverrides {
    /// The 8 standard ANSI colors, indexed in [`ANSI_COLOR_NAMES`] order. A
    /// `None` slot leaves that color at the active preset's value.
    pub ansi: [Option<Rgb>; ANSI_SLOTS],
    /// Terminal background. Its alpha is how see-through the window is.
    pub background: Option<Rgb>,
    /// The 8 bright ANSI colors, same slot order as [`Self::ansi`].
    pub brights: [Option<Rgb>; ANSI_SLOTS],
    /// Colors of the git views; any left unset keeps the preset's.
    pub git: GitColors,
    /// Overrides for individual ANSI palette slots, by index.
    pub indexed: Vec<(u8, Rgb)>,
    /// The scalar colors, in [`SCALAR_COLORS`] order.
    pub scalars: [Option<Rgb>; SCALAR_COUNT],
    /// Colors source is read in; any left unset keeps the preset's.
    pub syntax: SyntaxColors,
    /// The names in the `syntax` block no scope of the theme's table goes
    /// by, which color nothing and are worth saying so.
    pub unknown_syntax: Vec<String>,
}

/// A theme file: an optional `base` preset (`dark`/`light`) plus a `colors`
/// block layered over it, in the same color schema as the settings file.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlThemeFile {
    base: Option<String>,
    colors: Option<KdlColors>,
}

// ========================================================================
// The scalar colors
// ========================================================================

/// Declare the scalar colors: for each, the schema field (its KDL name is the
/// field's in kebab case, which `name` must spell) and the theme's reader and
/// writer. Generates [`KdlColors`], [`SCALAR_COLORS`] and [`SCALAR_COUNT`].
macro_rules! scalar_colors {
    ($($field:ident: $name:literal, read: $read:expr, write: $write:expr;)*) => {
        /// The `colors` block as written in a settings or theme file.
        #[derive(serde::Deserialize, Default)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        pub struct KdlColors {
            $($field: Option<String>,)*
            background: Option<String>,
            /// `ansi { red "#c22727" }`.
            ansi: Option<HashMap<String, String>>,
            /// `brights { red "#d43f30" }`.
            brights: Option<HashMap<String, String>>,
            /// `git { modified "#8fb1f5" }`.
            git: Option<HashMap<String, String>>,
            /// `indexed { "136" "#af8700" }`.
            indexed: Option<HashMap<String, String>>,
            /// `syntax { entity.name.function "#bf73df" }`, by TextMate scope.
            syntax: Option<HashMap<String, String>>,
        }

        impl KdlColors {
            /// The scalar colors as written, in [`SCALAR_COLORS`] order.
            fn scalars(&self) -> [Option<&str>; SCALAR_COUNT] {
                [$(self.$field.as_deref(),)*]
            }
        }

        /// How many scalar colors there are.
        pub const SCALAR_COUNT: usize = [$($name),*].len();

        /// Every scalar color, in the order a file lists them.
        pub const SCALAR_COLORS: [ScalarColor; SCALAR_COUNT] = [$(ScalarColor {
            name: $name,
            read: $read,
            write: $write,
        },)*];
    };
}

scalar_colors! {
    active_indicator: "active-indicator",
        read: |t| t.active_indicator, write: |t, c| t.active_indicator = c;
    active_pane_border: "active-pane-border",
        read: |t| t.active_pane_border, write: |t, c| t.active_pane_border = c;
    foreground: "foreground",
        read: |t| t.foreground, write: |t, c| t.foreground = c;
    cursor_bg: "cursor-bg",
        read: |t| t.cursor_bg, write: |t, c| t.cursor_bg = c;
    cursor_fg: "cursor-fg",
        read: |t| t.cursor_fg, write: |t, c| t.cursor_fg = c;
    find_label_bg: "find-label-bg",
        read: |t| t.find_label_bg, write: |t, c| t.find_label_bg = c;
    find_label_fg: "find-label-fg",
        read: |t| t.find_label_fg, write: |t, c| t.find_label_fg = c;
    scrollbar: "scrollbar",
        read: |t| t.scrollbar, write: |t, c| t.scrollbar = c;
    search_current_bg: "search-current-bg",
        read: |t| t.search_current_bg, write: |t, c| t.search_current_bg = c;
    search_match_bg: "search-match-bg",
        read: |t| t.search_match_bg, write: |t, c| t.search_match_bg = c;
    selection_bg: "selection-bg",
        read: |t| t.selection_bg, write: |t, c| t.selection_bg = c;
    selection_fg: "selection-fg",
        read: |t| t.selection_fg, write: |t, c| t.selection_fg = c;
    split: "split",
        read: |t| t.divider, write: |t, c| t.divider = c;
    status_bar_border: "status-bar-border",
        read: |t| t.status_bar_border, write: |t, c| t.status_bar_border = c;
    status_bar_fg: "status-bar-fg",
        read: |t| t.status_bar_fg, write: |t, c| t.status_bar_fg = c;
    // A theme without one draws the active line's number like the others.
    line_number_active: "line-number-active",
        read: |t| t.line_number_active_fg(), write: |t, c| t.line_number_active = Some(c);
    // A theme without a strip color of its own shades the background.
    tab_strip: "tab-strip",
        read: |t| t.tab_strip_bg(), write: |t, c| t.tab_strip = Some(c);
    // A theme without a bell color of its own flashes the cursor's.
    visual_bell: "visual-bell",
        read: |t| t.bell.unwrap_or(t.cursor_bg), write: |t, c| t.bell = Some(c);
    window_border: "window-border",
        read: |t| t.window_border, write: |t, c| t.window_border = c;
}

// ========================================================================
// ColorOverrides
// ========================================================================

impl ColorOverrides {
    /// The overrides a `colors` block comes to. A value that is not a hex
    /// color, a name nothing has, or an index that is no palette slot is
    /// left unset.
    pub fn from_kdl(kdl: KdlColors) -> Self {
        let scalars = kdl
            .scalars()
            .map(|value| value.and_then(Rgb::parse_hex));
        let mut git = GitColors::default();
        for (name, value) in kdl.git.unwrap_or_default() {
            git.set(&name, Rgb::parse_hex(&value));
        }
        let mut syntax = SyntaxColors::default();
        let mut unknown_syntax = Vec::new();
        for (name, value) in kdl.syntax.unwrap_or_default() {
            match SyntaxColors::knows(&name) {
                true => syntax.set(&name, Rgb::parse_hex(&value)),
                false => unknown_syntax.push(name),
            }
        }
        unknown_syntax.sort();
        Self {
            ansi: named_slots(kdl.ansi.as_ref()),
            background: kdl.background.as_deref().and_then(Rgb::parse_hex),
            brights: named_slots(kdl.brights.as_ref()),
            git,
            indexed: kdl
                .indexed
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(index, value)| {
                    Some((index.parse::<u8>().ok()?, Rgb::parse_hex(&value)?))
                })
                .collect(),
            scalars,
            syntax,
            unknown_syntax,
        }
    }

    /// The override set for the scalar color named `name`, if any.
    pub fn scalar(&self, name: &str) -> Option<Rgb> {
        let index = SCALAR_COLORS.iter().position(|color| color.name == name)?;
        self.scalars[index]
    }

    /// Set, or with `None` clear, the scalar color named `name`.
    pub fn set_scalar(&mut self, name: &str, value: Option<Rgb>) {
        let index = SCALAR_COLORS
            .iter()
            .position(|color| color.name == name)
            .expect("a scalar color");
        self.scalars[index] = value;
    }

    /// Overwrite the matching fields of `theme` with any colors set here.
    ///
    /// Every color may be translucent (`#rrggbbaa`). The terminal background's
    /// alpha makes the window see-through; every other color is laid over the
    /// background in effect here, once, so the theme holds them solid.
    pub fn apply(&self, theme: &mut Theme) {
        let surface = (theme.background, theme.foreground);
        if let Some(c) = self.background {
            // Its alpha is how see-through the window is, not a blend: there
            // is nothing of the preset's to lay it over once the desktop shows.
            theme.background = c.with_alpha(u8::MAX);
            theme.background_alpha = c.a;
        }
        let under = theme.background;
        let solid = |c: Rgb| c.over(under);
        for (color, value) in SCALAR_COLORS.iter().zip(&self.scalars) {
            if let Some(c) = value {
                (color.write)(theme, solid(*c));
            }
        }
        // Kept translucent: the views lay these over whatever they are drawn
        // on, which is the background today but need not stay so.
        for name in GitColors::NAMES {
            if let Some(c) = self.git.get(name) {
                theme.git.set(name, Some(c));
            }
        }
        for name in SyntaxColors::names() {
            if let Some(c) = self.syntax.get(name) {
                theme.syntax.set(name, Some(c));
            }
        }
        for (i, c) in self.ansi.iter().enumerate() {
            if let Some(c) = c {
                theme.ansi[i] = solid(*c);
            }
        }
        for (i, c) in self.brights.iter().enumerate() {
            if let Some(c) = c {
                theme.ansi[ANSI_SLOTS + i] = solid(*c);
            }
        }
        for (idx, c) in &self.indexed {
            let c = solid(*c);
            if let Some(slot) = theme.indexed.iter_mut().find(|(i, _)| i == idx) {
                slot.1 = c;
            } else {
                theme.indexed.push((*idx, c));
            }
        }
        // The window chrome has no colors of its own to set, so it follows
        // the terminal's: a theme with its own background gets a title bar,
        // tabs, and menus that belong with it. One that keeps the preset's
        // keeps the preset's own chrome too.
        if (theme.background, theme.foreground) != surface {
            theme.fit_chrome_to_background();
        }
    }

    /// The `colors` block that writes these overrides back, or `None` when
    /// none is set (an untouched config keeps the preset and writes no block).
    pub fn to_kdl(&self) -> Option<String> {
        let git: Vec<(&str, Rgb)> = GitColors::NAMES
            .iter()
            .filter_map(|&name| Some((name, self.git.get(name)?)))
            .collect();
        let syntax: Vec<(&str, Rgb)> = SyntaxColors::names()
            .filter_map(|name| Some((name, self.syntax.get(name)?)))
            .collect();
        let any = self.background.is_some()
            || self.scalars.iter().any(Option::is_some)
            || self.ansi.iter().any(Option::is_some)
            || self.brights.iter().any(Option::is_some)
            || !git.is_empty()
            || !syntax.is_empty()
            || !self.indexed.is_empty();
        if !any {
            return None;
        }
        let mut out = String::from("colors {\n");
        out.push_str(&scalars_kdl(self.background, &self.scalars));
        for (block, entries) in [("git", git), ("syntax", syntax)] {
            if entries.is_empty() {
                continue;
            }
            out.push_str(&format!("    {block} {{\n"));
            for (name, rgb) in entries {
                // A scope selector naming a scope inside another is two words.
                let name = match name.contains(' ') {
                    true => kdl_string(name),
                    false => name.to_string(),
                };
                out.push_str(&format!("        {name} {}\n", kdl_string(&rgb.to_hex())));
            }
            out.push_str("    }\n");
        }
        out.push_str(&palette_kdl(&self.ansi, &self.brights, &self.indexed));
        out.push_str("}\n");
        Some(out)
    }
}

// ========================================================================
// Writing
// ========================================================================

/// The scalar color nodes for every value set: the `background`, then the
/// `scalars` in table order.
fn scalars_kdl(
    background: Option<Rgb>,
    scalars: &[Option<Rgb>; SCALAR_COUNT],
) -> String {
    let background = background.map(|rgb| (BACKGROUND, rgb));
    let rest = SCALAR_COLORS
        .iter()
        .zip(scalars)
        .filter_map(|(color, value)| Some((color.name, (*value)?)));
    background
        .into_iter()
        .chain(rest)
        .map(|(name, rgb)| format!("    {name} {}\n", kdl_string(&rgb.to_hex())))
        .collect()
}

/// Every scalar color of `theme`, in table order, as a theme file writes them.
fn theme_scalars(theme: &Theme) -> [Option<Rgb>; SCALAR_COUNT] {
    SCALAR_COLORS.map(|color| Some((color.read)(theme)))
}

/// The `ansi`, `brights` and `indexed` blocks for whatever of them is set.
fn palette_kdl(
    ansi: &[Option<Rgb>; ANSI_SLOTS],
    brights: &[Option<Rgb>; ANSI_SLOTS],
    indexed: &[(u8, Rgb)],
) -> String {
    let mut out = String::new();
    for (name, colors) in [("ansi", ansi), ("brights", brights)] {
        if colors.iter().all(Option::is_none) {
            continue;
        }
        out.push_str(&format!("    {name} {{\n"));
        for (color_name, value) in ANSI_COLOR_NAMES.iter().zip(colors) {
            if let Some(rgb) = value {
                out.push_str(&format!(
                    "        {color_name} {}\n",
                    kdl_string(&rgb.to_hex())
                ));
            }
        }
        out.push_str("    }\n");
    }
    if !indexed.is_empty() {
        out.push_str("    indexed {\n");
        for (index, rgb) in indexed {
            out.push_str(&format!(
                "        {} {}\n",
                kdl_string(&index.to_string()),
                kdl_string(&rgb.to_hex())
            ));
        }
        out.push_str("    }\n");
    }
    out
}

/// Look each ANSI color name up in `map`, in [`ANSI_COLOR_NAMES`] order, so a
/// name absent from `map` leaves that slot unset rather than shifting the rest
/// into the wrong position.
fn named_slots(map: Option<&HashMap<String, String>>) -> [Option<Rgb>; ANSI_SLOTS] {
    let mut out = [None; ANSI_SLOTS];
    if let Some(map) = map {
        for (slot, name) in out.iter_mut().zip(ANSI_COLOR_NAMES) {
            *slot = map.get(name).and_then(|value| Rgb::parse_hex(value));
        }
    }
    out
}

// ========================================================================
// Theme files
// ========================================================================

/// Resolve a theme file's text into a [`Theme`]: start from the named `base`
/// preset (defaulting to dark) and apply its color overrides.
pub fn parse_theme_file(text: &str) -> Result<Theme, String> {
    let kdl: KdlThemeFile = kdl::de::from_str(text).map_err(|e| e.to_string())?;
    let mut theme = match kdl.base.as_deref() {
        Some("light") => Theme::light(),
        _ => Theme::dark(),
    };
    if let Some(colors) = kdl.colors {
        ColorOverrides::from_kdl(colors).apply(&mut theme);
    }
    Ok(theme)
}

/// A built-in theme from its file: every color the file names, laid over a
/// theme that sets none, with the window chrome derived from them. It has no
/// `base`, being one.
pub(crate) fn builtin_theme(text: &str) -> Theme {
    let kdl: KdlThemeFile = kdl::de::from_str(text).expect("a built-in theme parses");
    let mut theme = Theme::unset();
    if let Some(colors) = kdl.colors {
        ColorOverrides::from_kdl(colors).apply(&mut theme);
    }
    theme.fit_chrome_to_background();
    theme
}

/// Serialize `theme`'s full resolved colors as theme-file KDL text: a `base`
/// matching the theme's overall brightness, plus a fully populated `colors`
/// block. `base` isn't purely informational: the git and syntax colors are
/// not written, and come from the base preset, so picking the closer preset
/// keeps those parts consistent too.
pub fn theme_kdl(theme: &Theme) -> String {
    let base = if is_light_background(theme.background) {
        "light"
    } else {
        "dark"
    };
    let mut out = format!("base {}\n\ncolors {{\n", kdl_string(base));
    out.push_str(&scalars_kdl(Some(theme.background), &theme_scalars(theme)));
    let ansi: [Option<Rgb>; ANSI_SLOTS] = std::array::from_fn(|i| Some(theme.ansi[i]));
    let brights: [Option<Rgb>; ANSI_SLOTS] =
        std::array::from_fn(|i| Some(theme.ansi[ANSI_SLOTS + i]));
    out.push_str(&palette_kdl(&ansi, &brights, &theme.indexed));
    out.push_str("}\n");
    out
}

/// Whether `bg` reads as a light surface color (perceptual luminance above
/// the midpoint), used to pick the closer built-in preset as a theme's `base`.
fn is_light_background(bg: Rgb) -> bool {
    let luma = 0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32;
    luma > 127.0
}

/// Quote `s` as a KDL string argument, escaping backslashes and double quotes.
pub fn kdl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::syntax::SCOPE_RULES;

    use super::*;

    #[test]
    fn test_every_builtin_theme_names_each_color_it_must() {
        // A color a built-in file leaves out is black, not an error, and a
        // misspelt scope colors nothing: both have to be caught here.
        for (name, text) in [("dark", DARK_THEME_KDL), ("light", LIGHT_THEME_KDL)] {
            let kdl: KdlThemeFile = kdl::de::from_str(text).expect("a built-in theme parses");
            assert!(kdl.base.is_none(), "{name} is a base, so has none");
            let colors = ColorOverrides::from_kdl(kdl.colors.expect("a colors block"));
            assert!(colors.background.is_some(), "{name}: background");
            for (color, value) in SCALAR_COLORS.iter().zip(&colors.scalars) {
                assert!(value.is_some(), "{name}: {}", color.name);
            }
            assert!(colors.ansi.iter().chain(&colors.brights).all(Option::is_some), "{name}");
            assert!(colors.unknown_syntax.is_empty(), "{name}: {:?}", colors.unknown_syntax);
            for (index, rule) in SCOPE_RULES.iter().enumerate() {
                if rule.colors() {
                    assert!(colors.syntax.at(index).is_some(), "{name}: {}", rule.scope);
                }
            }
        }
    }

    #[test]
    fn test_the_background_is_applied_before_the_colors_laid_over_it() {
        // A color listed before the background in a file still blends over
        // the new background, not the preset's.
        let mut overrides = ColorOverrides {
            background: Rgb::parse_hex("#ffffff"),
            ..ColorOverrides::default()
        };
        overrides.set_scalar("active-indicator", Rgb::parse_hex("#00000080"));
        let mut theme = Theme::dark();
        overrides.apply(&mut theme);
        let grey = theme.active_indicator;
        assert!(
            grey.r > 100 && grey.r < 150,
            "half black over white: {grey:?}"
        );
    }

    #[test]
    fn test_parse_theme_file_applies_base_and_colors() {
        let theme = parse_theme_file(
            r##"
base "light"
colors {
    background "#282a36"
    foreground "#f8f8f2"
}
"##,
        )
        .expect("theme file parses");
        // Base preset is light, then the two colors override it.
        assert_eq!(theme.background, Rgb::parse_hex("#282a36").unwrap());
        assert_eq!(theme.foreground, Rgb::parse_hex("#f8f8f2").unwrap());
        // An unspecified color keeps the light base.
        assert_eq!(theme.cursor_bg, Theme::light().cursor_bg);
    }

    #[test]
    fn test_parse_theme_file_defaults_base_to_dark() {
        let theme = parse_theme_file("colors {\n    background \"#000000\"\n}").unwrap();
        assert_eq!(theme.cursor_bg, Theme::dark().cursor_bg);
        assert!(parse_theme_file("colors {{{").is_err());
    }

    #[test]
    fn test_is_light_background() {
        assert!(!is_light_background(Theme::dark().background));
        assert!(is_light_background(Theme::light().background));
    }

    #[test]
    fn test_a_theme_with_its_own_background_gets_a_title_bar_that_matches_it() {
        let theme = parse_theme_file("colors {\n    background \"#fdf6e3\"\n}").unwrap();
        assert!(
            theme.tabbar_bg.luminance() > 0.7,
            "a light band over a light terminal, not the dark preset's: {}",
            theme.tabbar_bg.to_hex()
        );
    }

    #[test]
    fn test_theme_kdl_round_trips_through_parse_theme_file() {
        for theme in [Theme::dark(), Theme::light()] {
            let restored =
                parse_theme_file(&theme_kdl(&theme)).expect("generated theme kdl parses");
            assert_eq!(restored, theme);
        }
    }
}
