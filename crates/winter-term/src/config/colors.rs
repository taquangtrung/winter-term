//! The `colors` block: what a settings or theme file says about colors, the
//! overrides it comes to, and how those are laid over a theme and written back.
//!
//! Every scalar color (one value, as opposed to the `ansi`, `git`, `syntax`
//! and `indexed` blocks) is named once, in the table [`scalar_colors!`] is
//! given: its KDL name, and the theme field it sets. The schema field, the
//! override slot, the theme it is applied to, and both writers (a settings
//! file's overrides, a theme file's full palette) all come from that one
//! entry, so a color added there is read, applied and saved everywhere.
//!
//! The background stands apart: it is not laid over anything, its alpha is
//! the window's own translucency, and the window asks for it on its own.

use std::collections::HashMap;

use winter_render::{GitColors, SyntaxColors, Theme, ThemeRgb};

use super::schema::kdl_string;
use super::theme::ANSI_COLOR_NAMES;

// ========================================================================
// Constants
// ========================================================================

/// How many colors the `ansi` block (and the `brights` block) names.
const ANSI_SLOTS: usize = 8;

/// The background's node name in the `colors` block.
const BACKGROUND: &str = "background";

// ========================================================================
// Data Structures
// ========================================================================

/// One scalar color: its KDL name, how to read it from a theme, and how to
/// set it in one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScalarColor {
    /// Its node name in the `colors` block.
    pub(crate) name: &'static str,
    /// Its value in a theme.
    pub(crate) read: fn(&Theme) -> ThemeRgb,
    /// Set it in a theme.
    pub(crate) write: fn(&mut Theme, ThemeRgb),
}

/// Per-color overrides parsed from the KDL `colors` block. Applied on top of the
/// active preset ([`Theme::dark`]/[`Theme::light`]); unset colors keep the preset.
#[derive(Clone, Debug, Default)]
pub struct ColorOverrides {
    /// The 8 standard ANSI colors, indexed in [`ANSI_COLOR_NAMES`] order. A
    /// `None` slot leaves that color at the active preset's value.
    pub ansi: [Option<ThemeRgb>; ANSI_SLOTS],
    /// Terminal background. Its alpha is how see-through the window is.
    pub background: Option<ThemeRgb>,
    /// The 8 bright ANSI colors, same slot order as [`Self::ansi`].
    pub brights: [Option<ThemeRgb>; ANSI_SLOTS],
    /// Colors of the git views; any left unset keeps the preset's.
    pub git: GitColors,
    /// Overrides for individual ANSI palette slots, by index.
    pub indexed: Vec<(u8, ThemeRgb)>,
    /// The scalar colors, in [`SCALAR_COLORS`] order.
    pub scalars: [Option<ThemeRgb>; SCALAR_COUNT],
    /// Colors source is read in; any left unset keeps the preset's.
    pub syntax: SyntaxColors,
    /// The names in the `syntax` block no scope of the theme's table goes
    /// by, which color nothing and are worth saying so.
    pub unknown_syntax: Vec<String>,
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
        pub(super) struct KdlColors {
            $(pub(super) $field: Option<String>,)*
            pub(super) background: Option<String>,
            /// `ansi { red "#c22727" }`.
            pub(super) ansi: Option<HashMap<String, String>>,
            /// `brights { red "#d43f30" }`.
            pub(super) brights: Option<HashMap<String, String>>,
            /// `git { modified "#8fb1f5" }`.
            pub(super) git: Option<HashMap<String, String>>,
            /// `indexed { "136" "#af8700" }`.
            pub(super) indexed: Option<HashMap<String, String>>,
            /// `syntax { entity.name.function "#bf73df" }`, by TextMate scope.
            pub(super) syntax: Option<HashMap<String, String>>,
        }

        impl KdlColors {
            /// The scalar colors as written, in [`SCALAR_COLORS`] order.
            fn scalars(&self) -> [Option<&str>; SCALAR_COUNT] {
                [$(self.$field.as_deref(),)*]
            }
        }

        /// How many scalar colors there are.
        pub(crate) const SCALAR_COUNT: usize = [$($name),*].len();

        /// Every scalar color, in the order a file lists them.
        pub(crate) const SCALAR_COLORS: [ScalarColor; SCALAR_COUNT] = [$(ScalarColor {
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
    scrollbar: "scrollbar",
        read: |t| t.scrollbar, write: |t, c| t.scrollbar = c;
    selection_bg: "selection-bg",
        read: |t| t.selection_bg, write: |t, c| t.selection_bg = c;
    selection_fg: "selection-fg",
        read: |t| t.selection_fg, write: |t, c| t.selection_fg = c;
    split: "split",
        read: |t| t.divider, write: |t, c| t.divider = c;
    status_bar_border: "status-bar-border",
        read: |t| t.status_bar_border, write: |t, c| t.status_bar_border = c;
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
    pub(super) fn from_kdl(kdl: KdlColors) -> Self {
        let scalars = kdl
            .scalars()
            .map(|value| value.and_then(ThemeRgb::parse_hex));
        let mut git = GitColors::default();
        for (name, value) in kdl.git.unwrap_or_default() {
            git.set(&name, ThemeRgb::parse_hex(&value));
        }
        let mut syntax = SyntaxColors::default();
        let mut unknown_syntax = Vec::new();
        for (name, value) in kdl.syntax.unwrap_or_default() {
            match SyntaxColors::knows(&name) {
                true => syntax.set(&name, ThemeRgb::parse_hex(&value)),
                false => unknown_syntax.push(name),
            }
        }
        unknown_syntax.sort();
        Self {
            ansi: named_slots(kdl.ansi.as_ref()),
            background: kdl.background.as_deref().and_then(ThemeRgb::parse_hex),
            brights: named_slots(kdl.brights.as_ref()),
            git,
            indexed: kdl
                .indexed
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(index, value)| {
                    Some((index.parse::<u8>().ok()?, ThemeRgb::parse_hex(&value)?))
                })
                .collect(),
            scalars,
            syntax,
            unknown_syntax,
        }
    }

    /// The override set for the scalar color named `name`, if any.
    #[cfg(test)]
    pub(crate) fn scalar(&self, name: &str) -> Option<ThemeRgb> {
        let index = SCALAR_COLORS.iter().position(|color| color.name == name)?;
        self.scalars[index]
    }

    /// Set, or with `None` clear, the scalar color named `name`.
    #[cfg(test)]
    pub(crate) fn set_scalar(&mut self, name: &str, value: Option<ThemeRgb>) {
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
        let solid = |c: ThemeRgb| c.over(under);
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
    pub(super) fn to_kdl(&self) -> Option<String> {
        let git: Vec<(&str, ThemeRgb)> = GitColors::NAMES
            .iter()
            .filter_map(|&name| Some((name, self.git.get(name)?)))
            .collect();
        let syntax: Vec<(&str, ThemeRgb)> = SyntaxColors::names()
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
pub(super) fn scalars_kdl(
    background: Option<ThemeRgb>,
    scalars: &[Option<ThemeRgb>; SCALAR_COUNT],
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
pub(super) fn theme_scalars(theme: &Theme) -> [Option<ThemeRgb>; SCALAR_COUNT] {
    SCALAR_COLORS.map(|color| Some((color.read)(theme)))
}

/// The `ansi`, `brights` and `indexed` blocks for whatever of them is set.
pub(super) fn palette_kdl(
    ansi: &[Option<ThemeRgb>; ANSI_SLOTS],
    brights: &[Option<ThemeRgb>; ANSI_SLOTS],
    indexed: &[(u8, ThemeRgb)],
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
fn named_slots(map: Option<&HashMap<String, String>>) -> [Option<ThemeRgb>; ANSI_SLOTS] {
    let mut out = [None; ANSI_SLOTS];
    if let Some(map) = map {
        for (slot, name) in out.iter_mut().zip(ANSI_COLOR_NAMES) {
            *slot = map.get(name).and_then(|value| ThemeRgb::parse_hex(value));
        }
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::config::Config;

    use super::*;

    #[test]
    fn test_every_scalar_color_is_read_applied_and_written_under_its_name() {
        // One table drives the schema, the overrides, the theme and both
        // writers: a color set in a file must come back out of each.
        for (index, color) in SCALAR_COLORS.iter().enumerate() {
            let text = format!("colors {{\n    {} \"#123456\"\n}}", color.name);
            let config = Config::parse(&text);
            let rgb = ThemeRgb::parse_hex("#123456");
            assert_eq!(config.colors.scalars[index], rgb, "{} is read", color.name);
            let mut theme = Theme::dark();
            config.colors.apply(&mut theme);
            assert_eq!(Some((color.read)(&theme)), rgb, "{} is applied", color.name);
            let written = config.colors.to_kdl().expect("a colors block");
            assert!(written.contains(color.name), "{} is written", color.name);
        }
    }

    #[test]
    fn test_the_background_is_applied_before_the_colors_laid_over_it() {
        // A color listed before the background in a file still blends over
        // the new background, not the preset's.
        let mut overrides = ColorOverrides {
            background: ThemeRgb::parse_hex("#ffffff"),
            ..ColorOverrides::default()
        };
        overrides.set_scalar("active-indicator", ThemeRgb::parse_hex("#00000080"));
        let mut theme = Theme::dark();
        overrides.apply(&mut theme);
        let grey = theme.active_indicator;
        assert!(
            grey.r > 100 && grey.r < 150,
            "half black over white: {grey:?}"
        );
    }
}
