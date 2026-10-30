//! Discovering, reading, and writing user theme files.

use std::path::PathBuf;

use winter_render::{parse_theme_file, theme_kdl, Theme};

use super::config_dir;

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
/// with its `colors` layered on top), or say why it could not be: a name that
/// is not a plain file name (it may not reach outside the themes directory),
/// a file that is missing, or one that does not parse.
pub fn load_named_theme(name: &str) -> Result<Theme, String> {
    let is_plain_name =
        !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..";
    if !is_plain_name {
        return Err(format!("theme \"{name}\" is not a theme name"));
    }
    let path = named_theme_path(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("theme \"{name}\": {}: {e}", path.display()))?;
    parse_theme_file(&text).map_err(|e| format!("theme \"{name}\" does not parse: {e}"))
}
/// The file path a named theme's colors are read from and saved to:
/// `themes/<name>.kdl`.
pub fn named_theme_path(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.kdl"))
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_save_named_theme_rejects_invalid_name() {
        // Validation happens before any filesystem access, so this is safe to
        // run without touching the real theme directory.
        let err = save_named_theme("../evil", &Theme::dark()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_a_theme_that_cannot_be_used_says_why() {
        // Read-only, but a name was a path: `../x` read outside the themes.
        assert!(load_named_theme("../secrets")
            .unwrap_err()
            .contains("not a theme name"));
    }
}
