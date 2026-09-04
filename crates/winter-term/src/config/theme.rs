//! Discovering, reading, and writing user theme files.

use super::{color_named_block_kdl, color_overrides_from_kdl, config_dir, kdl_string};

use super::schema::KdlThemeFile;
use std::path::PathBuf;
use winter_render::{Theme, ThemeRgb};

// ========================================================================
// Themes
// ========================================================================

/// Names (filename without `.kdl`) of the theme files in `themes/`, sorted. An
/// absent or unreadable directory yields an empty list.
pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "kdl" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    names.sort();
    names
}
/// Load and resolve `themes/<name>.kdl` into a full [`Theme`] (its `base` preset
/// with its `colors` layered on top), or `None` if the file is missing or
/// unparseable.
pub fn load_named_theme(name: &str) -> Option<Theme> {
    let text = std::fs::read_to_string(named_theme_path(name)).ok()?;
    parse_theme_file(&text)
}
/// The file path a named theme's colors are read from and saved to:
/// `themes/<name>.kdl`.
pub fn named_theme_path(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.kdl"))
}
/// Resolve a theme file's text into a [`Theme`]: start from the named `base`
/// preset (defaulting to dark) and apply its color overrides.
pub(crate) fn parse_theme_file(text: &str) -> Option<Theme> {
    let kdl: KdlThemeFile = kdl::de::from_str(text).ok()?;
    let mut theme = match kdl.base.as_deref() {
        Some("light") => Theme::light(),
        _ => Theme::dark(),
    };
    if let Some(colors) = kdl.colors {
        color_overrides_from_kdl(colors).apply(&mut theme);
    }
    Some(theme)
}
/// The user theme directory: `<config_dir>/themes`.
pub(crate) fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}
/// Whether `name` is safe to use as a `themes/<name>.kdl` file stem: non-empty
/// and restricted to characters that can't escape `themes_dir()` (no `/`, `.`,
/// or other path-traversal characters).
pub fn is_valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
/// Write a new `themes/<name>.kdl` seeded with `theme`'s full resolved colors,
/// so the file is immediately usable as a starting point for hand-editing.
/// Errors if `name` is invalid or a theme with that name already exists (never
/// overwrites a file the user may have edited).
pub fn save_named_theme(name: &str, theme: &Theme) -> std::io::Result<()> {
    if !is_valid_theme_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "theme name must be non-empty and contain only letters, numbers, - or _",
        ));
    }
    std::fs::create_dir_all(themes_dir())?;
    let path = named_theme_path(name);
    if path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("a theme named \"{name}\" already exists"),
        ));
    }
    std::fs::write(path, theme_kdl(theme))
}
/// Whether `bg` reads as a light surface color (perceptual luminance above
/// the midpoint), used to pick the closer built-in preset as a theme's `base`.
pub(crate) fn is_light_background(bg: ThemeRgb) -> bool {
    let luma = 0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32;
    luma > 127.0
}
/// Serialize `theme`'s full resolved colors as theme-file KDL text: a `base`
/// matching the theme's overall brightness, plus a fully populated `colors`
/// block. `base` isn't purely informational: chrome colors outside the
/// `colors` schema (menu/tab surfaces) come from the base preset, not from
/// `colors`, so picking the closer preset keeps those parts consistent too.
pub(crate) fn theme_kdl(theme: &Theme) -> String {
    let base = if is_light_background(theme.background) {
        "light"
    } else {
        "dark"
    };
    let mut out = format!("base {}\n\ncolors {{\n", kdl_string(base));
    let scalars: [(&str, ThemeRgb); 11] = [
        ("background", theme.background),
        ("foreground", theme.foreground),
        ("cursor-bg", theme.cursor_bg),
        ("cursor-fg", theme.cursor_fg),
        ("scrollbar", theme.scrollbar),
        ("selection-bg", theme.selection_bg),
        ("selection-fg", theme.selection_fg),
        ("split", theme.divider),
        ("status-bar-border", theme.status_bar_border),
        ("visual-bell", theme.bell.unwrap_or(theme.cursor_bg)),
        ("window-border", theme.window_border),
    ];
    for (name, rgb) in scalars {
        out.push_str(&format!("    {name} {}\n", kdl_string(&rgb.to_hex())));
    }
    let ansi: [Option<ThemeRgb>; 8] = std::array::from_fn(|i| Some(theme.ansi[i]));
    let brights: [Option<ThemeRgb>; 8] = std::array::from_fn(|i| Some(theme.ansi[8 + i]));
    if let Some(line) = color_named_block_kdl("ansi", &ansi) {
        out.push_str(&line);
    }
    if let Some(line) = color_named_block_kdl("brights", &brights) {
        out.push_str(&line);
    }
    if !theme.indexed.is_empty() {
        out.push_str("    indexed {\n");
        for (index, rgb) in &theme.indexed {
            out.push_str(&format!(
                "        {} {}\n",
                kdl_string(&index.to_string()),
                kdl_string(&rgb.to_hex())
            ));
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}
/// The 8 standard ANSI color names, in SGR order (index 0 = black, ...,
/// index 7 = white). `brights` reuses the same names for its own 8 slots.
pub(crate) const ANSI_COLOR_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(theme.background, ThemeRgb::parse_hex("#282a36").unwrap());
        assert_eq!(theme.foreground, ThemeRgb::parse_hex("#f8f8f2").unwrap());
        // An unspecified color keeps the light base.
        assert_eq!(theme.cursor_bg, Theme::light().cursor_bg);
    }
    #[test]
    fn test_parse_theme_file_defaults_base_to_dark() {
        let theme = parse_theme_file("colors {\n    background \"#000000\"\n}").unwrap();
        assert_eq!(theme.cursor_bg, Theme::dark().cursor_bg);
    }
    #[test]
    fn test_is_valid_theme_name_accepts_and_rejects() {
        assert!(is_valid_theme_name("dracula"));
        assert!(is_valid_theme_name("Solarized-Dark_2"));
        assert!(!is_valid_theme_name(""));
        assert!(!is_valid_theme_name("../evil"));
        assert!(!is_valid_theme_name("has/slash"));
        assert!(!is_valid_theme_name("has.dot"));
        assert!(!is_valid_theme_name("has space"));
    }
    #[test]
    fn test_is_light_background() {
        assert!(!is_light_background(Theme::dark().background));
        assert!(is_light_background(Theme::light().background));
    }
    #[test]
    fn test_theme_kdl_round_trips_through_parse_theme_file() {
        for theme in [Theme::dark(), Theme::light()] {
            let restored =
                parse_theme_file(&theme_kdl(&theme)).expect("generated theme kdl parses");
            assert_eq!(restored, theme);
        }
    }
    #[test]
    fn test_save_named_theme_rejects_invalid_name() {
        // Validation happens before any filesystem access, so this is safe to
        // run without touching the real theme directory.
        let err = save_named_theme("../evil", &Theme::dark()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
