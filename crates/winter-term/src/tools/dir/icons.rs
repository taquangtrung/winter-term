//! The glyph a listing draws beside an entry's name.
//!
//! Category glyphs from the Font Awesome range every Nerd Font carries, rather
//! than per-language logos from the patch-specific ranges, so a listing looks
//! the same across fonts instead of showing holes in some of them.

use super::entry::{Entry, EntryKind};

// ========================================================================
// Constants
// ========================================================================

/// A directory.
const FOLDER: char = '\u{f07b}';

/// Anything with no more specific glyph.
const FILE: char = '\u{f016}';

/// A symbolic link, whatever it points at.
const LINK: char = '\u{f0c1}';

/// A file the owner may execute.
const EXECUTABLE: char = '\u{f135}';

/// Source code.
const CODE: char = '\u{f121}';

/// A shell script.
const SHELL: char = '\u{f120}';

/// Configuration and data.
const CONFIG: char = '\u{f013}';

/// Prose: markdown, plain text, a manual page.
const DOCUMENT: char = '\u{f02d}';

/// A raster or vector image.
const IMAGE: char = '\u{f03e}';

/// A compressed archive.
const ARCHIVE: char = '\u{f1c6}';

/// A lock file, checked in and rarely read by hand.
const LOCK: char = '\u{f023}';

/// Extension to glyph, lowercase and without the dot. One line per addition.
const BY_EXTENSION: [(&str, char); 30] = [
    ("bmp", IMAGE),
    ("bz2", ARCHIVE),
    ("c", CODE),
    ("cpp", CODE),
    ("css", CODE),
    ("fish", SHELL),
    ("gif", IMAGE),
    ("go", CODE),
    ("gz", ARCHIVE),
    ("h", CODE),
    ("html", CODE),
    ("jpeg", IMAGE),
    ("jpg", IMAGE),
    ("js", CODE),
    ("json", CONFIG),
    ("kdl", CONFIG),
    ("lock", LOCK),
    ("md", DOCUMENT),
    ("png", IMAGE),
    ("py", CODE),
    ("rs", CODE),
    ("sh", SHELL),
    ("svg", IMAGE),
    ("toml", CONFIG),
    ("ts", CODE),
    ("txt", DOCUMENT),
    ("webp", IMAGE),
    ("xz", ARCHIVE),
    ("yaml", CONFIG),
    ("zip", ARCHIVE),
];

/// Mode bit for owner-execute.
const OWNER_EXECUTE: u32 = 0o100;

// ========================================================================
// Functions
// ========================================================================

/// The glyph for `entry`. Kind wins over extension, so a directory named
/// `assets.png` is still a folder, and an executable with no known extension
/// reads as one rather than as a generic file.
pub fn icon_for(entry: &Entry) -> char {
    match entry.kind {
        EntryKind::Dir => FOLDER,
        EntryKind::Symlink => LINK,
        EntryKind::File => icon_for_file(entry),
    }
}

fn icon_for_file(entry: &Entry) -> char {
    if let Some(icon) = extension_icon(&entry.name) {
        return icon;
    }
    if entry
        .meta
        .mode
        .is_some_and(|mode| mode & OWNER_EXECUTE != 0)
    {
        return EXECUTABLE;
    }
    FILE
}

/// The glyph for `name`'s extension. A dotfile with no second dot has no
/// extension: `.gitignore` is a name, not a `gitignore` file.
fn extension_icon(name: &str) -> Option<char> {
    let extension = name
        .trim_start_matches('.')
        .rsplit_once('.')?
        .1
        .to_lowercase();
    BY_EXTENSION
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, icon)| *icon)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::dir::entry::Meta;
    use std::path::PathBuf;

    fn entry(name: &str, kind: EntryKind, mode: Option<u32>) -> Entry {
        Entry::new(
            kind,
            Meta {
                len: 0,
                mode,
                modified: None,
            },
            name.to_string(),
            PathBuf::from(name),
        )
    }

    #[test]
    fn test_kind_wins_over_a_misleading_extension() {
        let dir = entry("assets.png", EntryKind::Dir, None);
        assert_eq!(icon_for(&dir), FOLDER);
        let link = entry("current.rs", EntryKind::Symlink, None);
        assert_eq!(icon_for(&link), LINK);
    }

    #[test]
    fn test_extension_matching_ignores_case() {
        let upper = entry("README.MD", EntryKind::File, None);
        assert_eq!(icon_for(&upper), DOCUMENT);
    }

    #[test]
    fn test_an_executable_without_a_known_extension_reads_as_one() {
        let script = entry("winter", EntryKind::File, Some(0o755));
        assert_eq!(icon_for(&script), EXECUTABLE);
        let plain = entry("notes", EntryKind::File, Some(0o644));
        assert_eq!(icon_for(&plain), FILE);
    }

    #[test]
    fn test_a_known_extension_beats_the_execute_bit() {
        // A checked-in script marked executable is still a shell script, and a
        // source file that happens to be +x is still source.
        let shell = entry("install.sh", EntryKind::File, Some(0o755));
        assert_eq!(icon_for(&shell), SHELL);
    }

    #[test]
    fn test_a_dotfile_is_a_name_not_an_extension() {
        // `.gitignore` has no extension: read as one it would match nothing and
        // stripping the dot would make `gitignore` the extension of nothing.
        let dotfile = entry(".gitignore", EntryKind::File, None);
        assert_eq!(icon_for(&dotfile), FILE);
        let dotted = entry(".config.toml", EntryKind::File, None);
        assert_eq!(icon_for(&dotted), CONFIG);
    }
}
