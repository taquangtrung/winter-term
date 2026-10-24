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

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

use flate2::read::DeflateDecoder;

use crate::model::paint::IconKind;

include!(concat!(env!("OUT_DIR"), "/icons_index.rs"));

// ========================================================================
// Constants
// ========================================================================

/// The deflated icon payload, indexed by [`ICON_INDEX`].
static ICON_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icons.blob"));

/// Icons already inflated for a list, by packed name, each held once however
/// many rows draw it. Bounded by the size of the bundled set.
static SHARED_SVGS: OnceLock<Mutex<SvgCache>> = OnceLock::new();

/// Set prefix for the file-type icons.
const FILE_SET: &str = "file";

/// Set prefix for the Git status icons.
const GIT_SET: &str = "git";

/// The set gutter marks are drawn from.
const GUTTER_SET: &str = "gutter";

/// Editor kind to the packed icon naming it. A kind not listed gets
/// [`FALLBACK_EDITOR_ICON`].
const EDITOR_ICONS: [(&str, &str); 9] = [
    ("dir", "file/default_folder"),
    ("git", "file/file_git"),
    ("grep", "file/file_search"),
    ("image", "file/file_image"),
    ("keys", "file/file_key"),
    ("notifications", "file/file_bell"),
    ("pdf", "file/file_pdf"),
    ("proc", "file/file_binary"),
    ("sys", "file/file_host"),
];

/// The icon for a terminal tab: the app's own.
const TERMINAL_ICON: &str = "app/winter-terminal";

/// The icon for an editor with no artwork of its own.
const FALLBACK_EDITOR_ICON: &str = "file/default_file";

// ========================================================================
// Data Structures
// ========================================================================

/// Packed name to its inflated SVG, or `None` for a name that failed to
/// inflate, so it is not retried every frame.
type SvgCache = HashMap<String, Option<Arc<[u8]>>>;

// ========================================================================
// Free functions
// ========================================================================

/// The SVG document for a packed icon, inflated on demand.
///
/// `name` is `<set>/<stem>`, as [`file_icon`] and friends return.
fn svg(name: &str) -> Option<Vec<u8>> {
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

/// The SVG a list row draws for `kind`, inflated the first time any row asks
/// and shared after that.
///
/// A list redraws every visible row each frame and many rows share an icon,
/// so inflating per row, as [`svg`] would, repeats the same work hundreds of
/// times over.
pub(crate) fn shared_svg(kind: &IconKind) -> Option<Arc<[u8]>> {
    named_svg(&name_for(kind)?)
}

/// The SVG for the packed icon `name`, as [`name_for`] gives it, from the
/// same shared cache as [`shared_svg`]: one copy of each icon's document
/// however many surfaces draw it.
pub(crate) fn named_svg(name: &str) -> Option<Arc<[u8]>> {
    let mut cache = SHARED_SVGS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hit) = cache.get(name) {
        return hit.clone();
    }
    let inflated = svg(name).map(Arc::from);
    cache.insert(name.to_string(), inflated.clone());
    inflated
}

/// The icon naming a file, chosen by exact name first and extension second, so
/// that `Makefile` and `Cargo.toml` beat what their extensions would say.
fn file_icon(file_name: &str) -> String {
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
fn dir_icon(dir_name: &str, expanded: bool) -> String {
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
fn git_icon(status: &str) -> String {
    qualified(GIT_SET, status)
}

/// The icon naming a kind of editor (`dir`, `git`, ...), or the default file
/// icon when the kind has none of its own.
fn editor_icon(kind: &str) -> String {
    EDITOR_ICONS
        .iter()
        .find(|(name, _)| *name == kind)
        .map_or(FALLBACK_EDITOR_ICON, |(_, icon)| icon)
        .to_string()
}

/// The icon naming a terminal tab.
fn terminal_icon() -> String {
    TERMINAL_ICON.to_string()
}

/// The packed name for an editor's declared icon, or `None` when nothing is
/// bundled for it.
pub(crate) fn name_for(kind: &IconKind) -> Option<String> {
    let name = match kind {
        IconKind::Dir { name, expanded } => dir_icon(name, *expanded),
        IconKind::File { name } => file_icon(name),
        IconKind::Git { status } => git_icon(status),
        IconKind::Gutter { mark } => qualified(GUTTER_SET, mark),
        IconKind::Terminal => terminal_icon(),
        IconKind::Editor { kind } => editor_icon(kind),
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

    #[test]
    fn test_editor_icons_are_all_packed() {
        for (kind, icon) in EDITOR_ICONS {
            assert_eq!(editor_icon(kind), icon);
            assert!(contains(icon), "{icon} is not packed");
        }
        assert!(contains(TERMINAL_ICON));
        assert!(contains(FALLBACK_EDITOR_ICON));
        assert_eq!(editor_icon("no-such-tool"), FALLBACK_EDITOR_ICON);
    }

    #[test]
    fn test_the_gutter_mark_is_packed() {
        // A name the set does not hold draws nothing at all in the gutter.
        let mark = IconKind::Gutter { mark: "marked" };
        assert_eq!(name_for(&mark).as_deref(), Some("gutter/marked"));
    }
}
