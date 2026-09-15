//! The bundled SVG icon set: which icon names a path, and its SVG document.
//!
//! `build.rs` deflates every file under `assets/icons/` into one payload and
//! generates the sorted name-to-slice index beside it, both included here. An
//! icon is inflated only when something asks to draw it, so showing a listing
//! unpacks the dozen icons on screen rather than the whole set.
//!
//! Rasterizing is the renderer's job: this module hands out SVG bytes and
//! leaves the pixels to `winter_render`.

mod map;

use std::io::Read;

use flate2::read::DeflateDecoder;

use crate::model::page::PageIconKind;

include!(concat!(env!("OUT_DIR"), "/icons_index.rs"));

// ========================================================================
// Constants
// ========================================================================

/// The deflated icon payload, indexed by [`ICON_INDEX`].
static ICON_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icons.blob"));

/// Set prefix for the file-type icons.
const FILE_SET: &str = "file";

/// Set prefix for the Git status icons.
const GIT_SET: &str = "git";

// ========================================================================
// Free functions
// ========================================================================

/// The SVG document for a packed icon, inflated on demand.
///
/// `name` is `<set>/<stem>`, as [`file_icon`] and friends return.
pub(crate) fn svg(name: &str) -> Option<Vec<u8>> {
    let position = ICON_INDEX
        .binary_search_by(|(packed, _, _, _)| (*packed).cmp(name))
        .ok()?;
    let (_, offset, packed_len, raw_len) = ICON_INDEX[position];
    let start = offset as usize;
    let packed = ICON_BLOB.get(start..start + packed_len as usize)?;
    let mut out = Vec::with_capacity(raw_len as usize);
    DeflateDecoder::new(packed).read_to_end(&mut out).ok()?;
    Some(out)
}

/// The icon naming a file, chosen by exact name first and extension second, so
/// that `Makefile` and `Cargo.toml` beat what their extensions would say.
pub(crate) fn file_icon(file_name: &str) -> String {
    let lowercase = file_name.to_ascii_lowercase();
    if let Some(stem) = lookup(map::FILE_NAMES, &lowercase) {
        return qualified(FILE_SET, stem);
    }
    match extension_stem(map::FILE_EXTENSIONS, &lowercase) {
        Some(stem) => qualified(FILE_SET, stem),
        None => qualified(FILE_SET, map::DEFAULT_FILE),
    }
}

/// The icon naming a directory, which differs by whether it is expanded.
pub(crate) fn dir_icon(dir_name: &str, expanded: bool) -> String {
    let lowercase = dir_name.to_ascii_lowercase();
    if let Some(stem) = lookup(map::FOLDER_NAMES, &lowercase) {
        // The expanded artwork is the same stem with `_opened` appended, and
        // not every folder has one: fall back to the collapsed icon rather
        // than to the generic folder, which would lose the folder's identity.
        if expanded {
            let opened = format!("{stem}_opened");
            if contains(&qualified(FILE_SET, &opened)) {
                return qualified(FILE_SET, &opened);
            }
        }
        return qualified(FILE_SET, stem);
    }
    let fallback = if expanded {
        map::DEFAULT_FOLDER_OPEN
    } else {
        map::DEFAULT_FOLDER
    };
    qualified(FILE_SET, fallback)
}

/// The icon for a Git working-tree status.
pub(crate) fn git_icon(status: &str) -> String {
    qualified(GIT_SET, status)
}

/// The packed name for a page's declared icon, or `None` when nothing is
/// bundled for it.
pub(crate) fn name_for(kind: &PageIconKind) -> Option<String> {
    let name = match kind {
        PageIconKind::Dir { name, expanded } => dir_icon(name, *expanded),
        PageIconKind::File { name } => file_icon(name),
        PageIconKind::Git { status } => git_icon(status),
    };
    contains(&name).then_some(name)
}

/// Whether a packed icon exists under this name.
fn contains(name: &str) -> bool {
    ICON_INDEX
        .binary_search_by(|(packed, _, _, _)| (*packed).cmp(name))
        .is_ok()
}

/// The stem for the longest extension suffix of `lowercase` the table names.
///
/// Every suffix is tried, longest first, so a theme carrying a compound
/// extension (`tar.gz`, `d.ts`) wins over its tail (`gz`, `ts`). Starting at
/// the first dot rather than the last also gives a dotfile the extension its
/// name implies: `.bashrc` resolves through `bashrc`, where the standard
/// library would call the whole thing a stem with no extension at all.
fn extension_stem<'t>(table: &'t [(&'t str, &'t str)], lowercase: &str) -> Option<&'t str> {
    let mut rest = lowercase;
    while let Some((_, extension)) = rest.split_once('.') {
        if let Ok(position) = table.binary_search_by(|(key, _)| (*key).cmp(extension)) {
            return Some(table[position].1);
        }
        rest = extension;
    }
    None
}

/// Binary search one of the generated tables.
fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    let position = table
        .binary_search_by(|(packed, _)| (*packed).cmp(key))
        .ok()?;
    Some(table[position].1)
}

/// Join a set prefix and a stem into the name [`svg`] indexes by.
fn qualified(set: &str, stem: &str) -> String {
    format!("{set}/{stem}")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_file_name_beats_the_extension() {
        // `Cargo.toml` and `Makefile` are the reason the name table is
        // consulted first: matching on the extension would draw the generic
        // TOML icon for the one and the default for the other.
        assert_eq!(file_icon("Cargo.toml"), "file/file_cargo");
        assert_ne!(file_icon("Cargo.toml"), file_icon("config.toml"));
    }

    #[test]
    fn test_compound_extension_beats_its_tail() {
        // Against a synthetic table, because the shipped theme happens to carry
        // no compound extensions: this pins the walk itself, which a swap to a
        // theme that does carry them (Material, say) would otherwise rely on
        // untested. Splitting on the last dot alone resolves `tar.gz` as `gz`.
        let table: &[(&str, &str)] = &[("gz", "plain"), ("tar.gz", "tarball")];
        assert_eq!(extension_stem(table, "archive.tar.gz"), Some("tarball"));
        assert_eq!(extension_stem(table, "archive.gz"), Some("plain"));
        assert_eq!(extension_stem(table, "archive.zip"), None);
    }

    #[test]
    fn test_a_dotfile_resolves_through_its_tail() {
        // `Path::extension` calls `.bashrc` a stem with no extension, so
        // anything built on it draws the default icon for every dotfile that
        // is not spelled out in the name table.
        let table: &[(&str, &str)] = &[("bashrc", "shell")];
        assert_eq!(extension_stem(table, ".bashrc"), Some("shell"));
    }

    #[test]
    fn test_unknown_file_falls_back_to_the_default_icon() {
        assert_eq!(file_icon("mystery.qqqqq"), "file/default_file");
        assert_eq!(file_icon("no-extension-at-all"), "file/default_file");
    }

    #[test]
    fn test_every_name_the_tables_reach_is_actually_packed() {
        // The tables are generated from a theme JSON and the artwork is packed
        // from a directory; nothing but this ties the two together. A mapping
        // naming an icon that was never copied in would draw nothing at all,
        // silently, for every file of that type.
        for (key, stem) in map::FILE_EXTENSIONS
            .iter()
            .chain(map::FILE_NAMES)
            .chain(map::FOLDER_NAMES)
        {
            assert!(
                contains(&qualified(FILE_SET, stem)),
                "{key:?} maps to {stem:?}, which is not packed"
            );
        }
        for stem in [
            map::DEFAULT_FILE,
            map::DEFAULT_FOLDER,
            map::DEFAULT_FOLDER_OPEN,
        ] {
            assert!(
                contains(&qualified(FILE_SET, stem)),
                "{stem:?} is not packed"
            );
        }
    }

    #[test]
    fn test_packed_icons_inflate_to_the_recorded_length() {
        // Guards the offset arithmetic: a wrong offset or length still inflates
        // for most entries, but comes back the wrong size.
        for (name, _, _, raw_len) in ICON_INDEX.iter().take(64) {
            let svg = svg(name).unwrap_or_else(|| panic!("{name} failed to inflate"));
            assert_eq!(svg.len(), *raw_len as usize, "{name}");
            assert!(
                svg.starts_with(b"<") || svg.starts_with(b"\xef\xbb\xbf"),
                "{name}"
            );
        }
    }

    #[test]
    fn test_the_last_packed_icon_inflates() {
        // The final entry is the one a length computed from the *next* offset
        // would get wrong, so it is worth naming separately from the sample above.
        let (name, _, _, raw_len) = ICON_INDEX.last().expect("the set is not empty");
        assert_eq!(svg(name).expect("inflates").len(), *raw_len as usize);
    }

    #[test]
    fn test_git_statuses_all_have_artwork() {
        for status in [
            "git-added",
            "git-clean",
            "git-conflict",
            "git-deleted",
            "git-ignored",
            "git-modified",
            "git-renamed",
            "git-untracked",
        ] {
            assert!(contains(&git_icon(status)), "{status} is not packed");
        }
    }
}
