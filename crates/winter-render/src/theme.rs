//! Terminal color theme: all visual colors used by the GPU renderer. Built-in
//! presets (WezTerm-inspired); user overrides are applied by the app layer from
//! the KDL config.

// ========================================================================
// Constants
// ========================================================================

/// How far the title bar band is pulled from the background toward black, on
/// a dark background and on a light one: enough to read as a band of its own,
/// little enough that it belongs with the terminal under it. The presets use
/// the same proportions, so every theme's band sits the same way.
const BAND_SHADE_DARK: f32 = 0.2;
const BAND_SHADE_LIGHT: f32 = 0.08;

/// How far a menu is lifted from the background toward white on a dark
/// background, so it reads as a card above the terminal. On a light one the
/// menu is white outright.
const MENU_LIFT_DARK: f32 = 0.06;

/// How much of the cursor color tints a hovered menu entry.
const MENU_HOVER_TINT: f32 = 0.2;

/// How much of the text color tints the cursor's row.
const CURSOR_LINE_TINT: f32 = 0.06;

/// The luminance below which a background counts as dark.
const DARK_LUMINANCE: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// A 24-bit RGB color stored as three `u8` values, plus an opacity.
///
/// A color a [`Theme`] holds is always opaque: translucent colors written in
/// the config are blended over the surface beneath them with [`Rgb::over`] when
/// the overrides are applied, so no renderer has to know about alpha.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    /// Opacity, 0 (invisible) to 255 (solid).
    pub a: u8,
    /// Blue channel, 0-255.
    pub b: u8,
    /// Green channel, 0-255.
    pub g: u8,
    /// Red channel, 0-255.
    pub r: u8,
}

/// The complete set of colors used to render the terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The 16 ANSI colors: indices 0-7 normal, 8-15 bright.
    pub ansi: [Rgb; 16],
    /// Terminal background, and the window's clear color.
    pub background: Rgb,
    /// How opaque the terminal background is, 0 (see-through) to 255 (solid).
    /// Below 255 the window is translucent: the background and the cells that
    /// leave it showing let the desktop through, while the chrome stays solid.
    pub background_alpha: u8,
    /// Color used to tint the tab bar briefly on a bell event and for the
    /// bell-dot indicator. Falls back to `cursor_bg` for the dot when `None`.
    pub bell: Option<Rgb>,
    /// Fill of the cursor.
    pub cursor_bg: Rgb,
    /// Color of the character under a block cursor.
    pub cursor_fg: Rgb,
    /// Background of the row the Normal-mode cursor is on (Vim's `cursorline`).
    /// Only painted where a cell has no background color of its own.
    pub cursor_line_bg: Rgb,
    /// Color of the line drawn between adjacent panes.
    pub divider: Rgb,
    /// Background of an `f`/`t` jump-label box (the easymotion-style overlay).
    pub find_label_bg: Rgb,
    /// Text color of an `f`/`t` jump label.
    pub find_label_fg: Rgb,
    /// Default text color.
    pub foreground: Rgb,
    /// Overrides for individual 256-color palette entries, as `(index, color)`.
    /// Sparse: an index absent here uses the computed xterm palette value.
    pub indexed: Vec<(u8, Rgb)>,
    /// Elevated surface of the dropdown menu panel, lighter than the terminal so
    /// the menu reads as a card floating above it.
    pub menu_bg: Rgb,
    /// Highlight behind a hovered dropdown menu item.
    pub menu_hover_bg: Rgb,
    /// Color of the scrollbar thumb (the active pane's; inactive panes dim it).
    pub scrollbar: Rgb,
    /// Highlight behind the match `n`/`N` is currently sitting on, distinct from
    /// the other matches (Vim's `CurSearch` vs `Search`). Blended over the cell's
    /// own background so the text's foreground stays readable: see
    /// `renderer::SEARCH_CURRENT_ALPHA`.
    pub search_current_bg: Rgb,
    /// Highlight behind the search matches other than the current one.
    pub search_match_bg: Rgb,
    /// Fill behind selected text.
    pub selection_bg: Rgb,
    /// Text color inside a selection.
    pub selection_fg: Rgb,
    /// Color of the 1px separator line between the status bar and terminal content.
    pub status_bar_border: Rgb,
    /// Text color of the status bar.
    pub status_bar_fg: Rgb,
    /// Fill behind the active tab in the tabbar.
    pub tab_active_bg: Rgb,
    /// Text color of the active tab's title.
    pub tab_active_fg: Rgb,
    /// Background of the tabbar / menubar bands.
    pub tabbar_bg: Rgb,
    /// The colors of the git views.
    pub git: GitColors,
    /// The colors source is read in.
    pub syntax: SyntaxColors,
    /// Color of the 1px outline drawn around the window's outer edge when it
    /// has no OS-drawn frame (the "Modern", undecorated title bar style).
    pub window_border: Rgb,
}

// ========================================================================
// GitColors
// ========================================================================

/// Declares a set of optional theme colors and the config names of its colors
/// from one list, so a color is added in one place. A color left `None` is
/// derived from the theme's own palette by whatever draws it, so a theme that
/// sets none still gets hues that belong to it. A color may be translucent;
/// the drawing code lays it over the background.
macro_rules! color_set {
    ($(#[$sdoc:meta])* $set:ident { $($(#[$doc:meta])* $field:ident = $name:literal,)* }) => {
        $(#[$sdoc])*
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
        pub struct $set {
            $($(#[$doc])* pub $field: Option<Rgb>,)*
        }

        impl $set {
            /// The config name of every color, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$($name),*];

            /// The color called `name` in the config.
            pub fn get(&self, name: &str) -> Option<Rgb> {
                match name {
                    $($name => self.$field,)*
                    _ => None,
                }
            }

            /// Set the color called `name`; a name nothing has is ignored.
            pub fn set(&mut self, name: &str, color: Option<Rgb>) {
                match name {
                    $($name => self.$field = color,)*
                    _ => {}
                }
            }
        }
    };
}

color_set! {
    /// The colors of the git views.
    GitColors {
    /// A file that arrived.
    added = "added",
    /// A file that left.
    deleted = "deleted",
    /// The words a diff's added line gained.
    inserted_text = "inserted-text",
    /// A diff's added line.
    inserted_line = "inserted-line",
    /// A file edited where it stands.
    modified = "modified",
    /// A file that moved.
    renamed = "renamed",
    /// A file a merge left contested.
    unmerged = "unmerged",
    /// The path of a file with a conflict, on its row.
    file_conflict = "file-conflict",
    /// The path of a staged file, on its row.
    file_staged = "file-staged",
    /// The path of an unstaged file, on its row.
    file_unstaged = "file-unstaged",
    /// The path of an untracked file, on its row.
    file_untracked = "file-untracked",
    /// The band behind a file's row, folded or not: the change's name sits on
    /// it beside the fold mark, so a band that changed with the fold would
    /// split the row into two tones.
    file_band = "file-band",
    /// The band behind a hunk's header.
    hunk_band = "hunk-band",
    /// The staged section's heading.
    header_staged = "header-staged",
    /// The unstaged section's heading.
    header_unstaged = "header-unstaged",
    /// The untracked section's heading.
    header_untracked = "header-untracked",
    /// Every other heading.
    header = "header",
    /// The checked-out branch.
    ref_head = "ref-head",
    /// A local branch.
    ref_local = "ref-local",
    /// A branch on a remote.
    ref_remote = "ref-remote",
    /// A tag.
    ref_tag = "ref-tag",
    /// The words a diff's removed line lost.
    removed_text = "removed-text",
    /// A diff's removed line.
    removed_line = "removed-line",
    /// A commit the upstream has not seen, as magic-vscode paints it.
    unpushed = "unpushed",
    }
}

color_set! {
    /// The colors source is read in, for the five kinds of token the editor
    /// tells apart.
    SyntaxColors {
    /// Prose about the code.
    comment = "comment",
    /// A word of the language.
    keyword = "keyword",
    /// A literal number.
    number = "number",
    /// A literal string.
    string = "string",
    /// The name of a type.
    type_name = "type",
    }
}

impl GitColors {
    /// The palette of magic-vscode's One Color theme, which the dark preset
    /// shares so the two editors' git views read alike.
    pub fn magic() -> Self {
        let hex = |s: &str| Rgb::parse_hex(s);
        Self {
            added: hex("#91af22"),
            deleted: hex("#da5f59"),
            file_band: hex("#14708acc"),
            file_conflict: hex("#f14c4c"),
            file_staged: hex("#1bb1b1"),
            file_unstaged: hex("#da8548"),
            file_untracked: hex("#63b417"),
            header: hex("#51afef"),
            header_staged: hex("#1bb1b1"),
            header_unstaged: hex("#da8548"),
            header_untracked: hex("#63b417"),
            hunk_band: hex("#1f3e4e"),
            inserted_line: hex("#23372dfb"),
            inserted_text: hex("#215714df"),
            modified: hex("#8fb1f5"),
            ref_head: hex("#cf81f5"),
            ref_local: hex("#cf81f5"),
            ref_remote: hex("#6ab123"),
            ref_tag: hex("#2d9574"),
            removed_line: hex("#512b3299"),
            removed_text: hex("#8f3f3fdf"),
            renamed: hex("#d1b32f"),
            unmerged: hex("#a75abe"),
            unpushed: hex("#7bdd0b"),
        }
    }
}

impl SyntaxColors {
    /// magic-vscode's One Color token colors, for the dark preset.
    pub fn magic() -> Self {
        let hex = |s: &str| Rgb::parse_hex(s);
        Self {
            comment: hex("#718b9f"),
            keyword: hex("#51afef"),
            number: hex("#da8548"),
            string: hex("#2d9574"),
            type_name: hex("#cf517a"),
        }
    }
}

// ========================================================================
// Rgb
// ========================================================================

impl Default for Rgb {
    fn default() -> Self {
        Self::new(0, 0, 0)
    }
}

impl Rgb {
    /// A solid color from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { a: 255, b, g, r }
    }

    /// The same color at opacity `a`.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Whether the color hides nothing beneath it.
    pub const fn is_opaque(self) -> bool {
        self.a == 255
    }

    /// The solid color this one makes when laid over `under`, whose own opacity
    /// is not looked at: what a translucent color on a surface comes to.
    pub fn over(self, under: Rgb) -> Rgb {
        let t = self.a as f32 / 255.0;
        under.with_alpha(255).mix(self.with_alpha(255), t)
    }

    /// The channels as `0.0..=1.0` floats, the form wgpu wants. Despite the
    /// name this is a plain rescale, not an sRGB-to-linear conversion; the
    /// surface is configured to do that transfer itself.
    pub fn as_linear(self) -> (f32, f32, f32) {
        (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        )
    }

    /// The color as an opaque `glyphon::Color` for text rendering.
    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(self.r, self.g, self.b, 255)
    }

    /// Parse `#rrggbb` or `#rrggbbaa`, returning `None` for any other shape.
    /// The leading `#` is required and three-digit shorthand is not accepted.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if !s.is_ascii() || !matches!(s.len(), 6 | 8) {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = match s.len() {
            8 => u8::from_str_radix(&s[6..8], 16).ok()?,
            _ => 255,
        };
        Some(Self { a, b, g, r })
    }

    /// `self` moved `t` of the way toward `other`, channel by channel.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }

    /// Perceived brightness, from 0.0 for black to 1.0 for white.
    pub fn luminance(self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }

    /// Format as `#rrggbb`, or `#rrggbbaa` when translucent: the inverse of
    /// [`Self::parse_hex`].
    pub fn to_hex(self) -> String {
        match self.is_opaque() {
            true => format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b),
            false => format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a),
        }
    }
}

// ========================================================================
// Theme
// ========================================================================

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The built-in dark preset: magic-vscode's One Color theme, from its
    /// editor and terminal colors, with the window chrome derived from the
    /// background as for any theme.
    pub fn dark() -> Self {
        let mut theme = Self::dark_base();
        theme.fit_chrome_to_background();
        theme
    }

    fn dark_base() -> Self {
        Self {
            background: Rgb::parse_hex("#262a32").unwrap(),
            background_alpha: 255,
            foreground: Rgb::parse_hex("#bbc2cf").unwrap(),
            cursor_bg: Rgb::parse_hex("#51afef").unwrap(),
            cursor_fg: Rgb::parse_hex("#000000").unwrap(),
            cursor_line_bg: Rgb::parse_hex("#343b3d").unwrap(),
            scrollbar: Rgb::parse_hex("#135581").unwrap(),
            search_current_bg: Rgb::parse_hex("#275bad").unwrap(),
            search_match_bg: Rgb::parse_hex("#e6b422").unwrap(),
            selection_bg: Rgb::parse_hex("#2257a0").unwrap(),
            selection_fg: Rgb::parse_hex("#000000").unwrap(),
            divider: Rgb::parse_hex("#51554f").unwrap(),
            find_label_bg: Rgb::parse_hex("#a8d5ff").unwrap(),
            find_label_fg: Rgb::parse_hex("#0b4fa8").unwrap(),
            status_bar_border: Rgb::parse_hex("#3a3f45").unwrap(),
            bell: Some(Rgb::parse_hex("#e08020").unwrap()),
            menu_bg: Rgb::parse_hex("#353a3c").unwrap(),
            menu_hover_bg: Rgb::parse_hex("#3e4548").unwrap(),
            status_bar_fg: Rgb::parse_hex("#15181a").unwrap(),
            tab_active_bg: Rgb::parse_hex("#2a2f31").unwrap(),
            tab_active_fg: Rgb::parse_hex("#d8d8d8").unwrap(),
            tabbar_bg: Rgb::parse_hex("#222627").unwrap(),
            window_border: Rgb::parse_hex("#414248").unwrap(),
            git: GitColors::magic(),
            syntax: SyntaxColors::magic(),
            ansi: [
                Rgb::parse_hex("#1b2229").unwrap(), // black
                Rgb::parse_hex("#c22727").unwrap(), // red
                Rgb::parse_hex("#71b312").unwrap(), // green
                Rgb::parse_hex("#faa213").unwrap(), // yellow
                Rgb::parse_hex("#4fa2fa").unwrap(), // blue
                Rgb::parse_hex("#bb67b2").unwrap(), // magenta
                Rgb::parse_hex("#21afbf").unwrap(), // cyan
                Rgb::parse_hex("#bbc2cf").unwrap(), // white
                Rgb::parse_hex("#7a7a7a").unwrap(), // bright black
                Rgb::parse_hex("#d43f30").unwrap(), // bright red
                Rgb::parse_hex("#71b312").unwrap(), // bright green
                Rgb::parse_hex("#ebb909").unwrap(), // bright yellow
                Rgb::parse_hex("#5da2eb").unwrap(), // bright blue
                Rgb::parse_hex("#c97df5").unwrap(), // bright magenta
                Rgb::parse_hex("#04cfe1").unwrap(), // bright cyan
                Rgb::parse_hex("#e1ebfa").unwrap(), // bright white
            ],
            indexed: vec![(136, Rgb::parse_hex("#af8700").unwrap())],
        }
    }

    /// The built-in light preset. Reuses the dark palette but swaps the surface
    /// colors (background, foreground, cursor, divider, bell).
    pub fn light() -> Self {
        Self {
            background: Rgb::parse_hex("#f2f2f2").unwrap(),
            foreground: Rgb::parse_hex("#1e1e1e").unwrap(),
            cursor_bg: Rgb::parse_hex("#262626").unwrap(),
            cursor_fg: Rgb::parse_hex("#ffffff").unwrap(),
            cursor_line_bg: Rgb::parse_hex("#e4e4e4").unwrap(),
            selection_fg: Rgb::parse_hex("#000000").unwrap(),
            divider: Rgb::parse_hex("#cccccc").unwrap(),
            bell: Some(Rgb::parse_hex("#c85010").unwrap()),
            menu_bg: Rgb::parse_hex("#ffffff").unwrap(),
            menu_hover_bg: Rgb::parse_hex("#e6eefb").unwrap(),
            tab_active_bg: Rgb::parse_hex("#f2f2f2").unwrap(),
            tab_active_fg: Rgb::parse_hex("#1e1e1e").unwrap(),
            tabbar_bg: Rgb::parse_hex("#dfdfdf").unwrap(),
            // The git palette is tuned to the dark preset; against a light
            // background the views derive their colors from the theme instead.
            git: GitColors::default(),
            syntax: SyntaxColors::default(),
            ..Self::dark()
        }
    }

    /// Re-derive the window chrome from the background and the text color in
    /// effect: the title bar band, the active tab, the menu card, and the
    /// cursor's row. A theme or a color override that changes the background
    /// would otherwise sit under the preset's chrome, a dark title bar over a
    /// light terminal or the reverse.
    pub fn fit_chrome_to_background(&mut self) {
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        let dark = self.background.luminance() < DARK_LUMINANCE;
        let (band, menu) = match dark {
            true => (BAND_SHADE_DARK, self.background.mix(white, MENU_LIFT_DARK)),
            false => (BAND_SHADE_LIGHT, white),
        };
        self.tabbar_bg = self.background.mix(black, band);
        self.tab_active_bg = self.background;
        self.tab_active_fg = self.foreground;
        self.menu_bg = menu;
        self.menu_hover_bg = menu.mix(self.cursor_bg, MENU_HOVER_TINT);
        self.cursor_line_bg = self.background.mix(self.foreground, CURSOR_LINE_TINT);
    }

    /// The ANSI color at `index` as an RGB triple, or black when out of range.
    pub fn ansi_color(&self, index: u8) -> (u8, u8, u8) {
        if (index as usize) < self.ansi.len() {
            let c = self.ansi[index as usize];
            (c.r, c.g, c.b)
        } else {
            (0, 0, 0)
        }
    }

    /// The theme's override for 256-color palette `index`, if it sets one.
    pub fn indexed_color(&self, index: u8) -> Option<(u8, u8, u8)> {
        for &(idx, c) in &self.indexed {
            if idx == index {
                return Some((c.r, c.g, c.b));
            }
        }
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_chrome_follows_a_background_the_theme_changed() {
        // A light background over the dark preset: the band must come out
        // light and just darker than the terminal, not the preset's near-black.
        let mut theme = Theme::dark();
        theme.background = Rgb::parse_hex("#fdf6e3").unwrap();
        theme.foreground = Rgb::parse_hex("#586e75").unwrap();
        theme.fit_chrome_to_background();
        let band = theme.tabbar_bg.luminance();
        assert!(band > 0.7, "a light band: {}", theme.tabbar_bg.to_hex());
        assert!(band < theme.background.luminance(), "darker than the terminal");
        assert_eq!(theme.tab_active_bg, theme.background);

        // And the presets' own bands are the ones the rule derives.
        for preset in [Theme::dark(), Theme::light()] {
            let mut fitted = preset.clone();
            fitted.fit_chrome_to_background();
            assert_eq!(fitted.tabbar_bg, preset.tabbar_bg, "{}", preset.tabbar_bg.to_hex());
        }
    }

    #[test]
    fn test_default_theme_is_valid() {
        let theme = Theme::default();
        assert_ne!(theme.background, theme.foreground);
        assert_eq!(theme.ansi.len(), 16);
        let (r, g, b) = theme.ansi_color(0);
        assert_eq!((r, g, b), (0x1b, 0x22, 0x29), "magic-one black");
    }

    #[test]
    fn test_light_differs_from_dark() {
        assert_ne!(Theme::light().background, Theme::dark().background);
        assert_eq!(Theme::light().ansi, Theme::dark().ansi);
    }

    #[test]
    fn test_parse_hex_rgb() {
        assert_eq!(Rgb::parse_hex("#ff0000"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(Rgb::parse_hex("#00ff00"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(Rgb::parse_hex("#0000ff"), Some(Rgb::new(0, 0, 255)));
        assert_eq!(Rgb::parse_hex("ff0000"), None);
        assert_eq!(Rgb::parse_hex("#fff"), None);
        assert_eq!(Rgb::parse_hex("#gggggg"), None);
    }

    #[test]
    fn test_parse_hex_alpha_and_laying_it_over_a_surface() {
        let teal = Rgb::parse_hex("#14708acc").unwrap();
        assert_eq!((teal.r, teal.g, teal.b, teal.a), (0x14, 0x70, 0x8a, 0xcc));
        assert_eq!(teal.to_hex(), "#14708acc");
        assert_eq!(Rgb::parse_hex("#14708aff").unwrap().to_hex(), "#14708a");
        assert_eq!(Rgb::parse_hex("#14708a0"), None);
        assert_eq!(Rgb::parse_hex("#é4708a"), None, "multi-byte text must not panic");

        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert_eq!(white.with_alpha(0).over(black), black);
        assert_eq!(white.over(black), white);
        assert_eq!(white.with_alpha(128).over(black), Rgb::new(128, 128, 128));
    }

    #[test]
    fn test_git_color_names_reach_their_fields_and_the_presets_differ() {
        let mut colors = GitColors::default();
        for (n, name) in GitColors::NAMES.iter().enumerate() {
            colors.set(name, Some(Rgb::new(n as u8, 0, 0)));
        }
        for (n, name) in GitColors::NAMES.iter().enumerate() {
            assert_eq!(colors.get(name), Some(Rgb::new(n as u8, 0, 0)), "{name}");
        }
        assert_eq!(colors.get("nonsense"), None);
        assert!(Theme::dark().git.file_band.is_some());
        assert_eq!(Theme::light().git, GitColors::default());
        assert_eq!(Theme::light().syntax, SyntaxColors::default());
        assert_eq!(Theme::dark().syntax.get("type"), Rgb::parse_hex("#cf517a"));
    }

    #[test]
    fn test_to_hex_roundtrips_parse_hex() {
        for hex in ["#ff0000", "#00ff00", "#0000ff", "#2a2f31", "#000000"] {
            assert_eq!(Rgb::parse_hex(hex).unwrap().to_hex(), hex);
        }
    }

    #[test]
    fn test_rgb_as_linear() {
        let c = Rgb::new(255, 128, 0);
        let (r, g, b) = c.as_linear();
        assert!((r - 1.0).abs() < f32::EPSILON);
        assert!((g - 128.0 / 255.0).abs() < 1e-6);
        assert!((b - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_indexed_color_not_found() {
        let theme = Theme::default();
        assert_eq!(theme.indexed_color(0), None);
        assert!(theme.indexed_color(136).is_some());
    }
}
