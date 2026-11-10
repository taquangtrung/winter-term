//! Painting a listing: the header line, one line per entry, and the detail
//! columns behind the details toggle.

use std::time::{Duration, SystemTime};

use winter_model::paint::{self, ColumnAlign, ColumnName, GutterMark, Icon, IconKind, Span, Style};
use winter_model::units::format_size;

use super::entry::{Entry, EntryKind, Meta};
use super::tree::Row;
use super::vcs::GitStatus;

// ========================================================================
// Constants
// ========================================================================

/// Glyph on a directory whose children are listed beneath it.
const GLYPH_EXPANDED: &str = "⌄ ";

/// Glyph on a directory that can be expanded.
const GLYPH_COLLAPSED: &str = "› ";

/// Placeholder keeping a file's name aligned with the directories around it.
const GLYPH_NONE: &str = "  ";

/// Columns one level of nesting indents by.
const INDENT: usize = 2;

/// The icon of an entry marked for an operation, in the gutter's mark lane.
const MARKED: IconKind = IconKind::Gutter { mark: "marked" };

/// Column the detail columns begin at.
const DETAIL_COL: usize = 40;

/// Shown where a value does not apply, such as an unwalked directory's size.
const NO_VALUE: &str = "-";

/// Shown for a directory waiting its turn to be walked; the one being walked
/// wears the spinner instead.
const WALKING: &str = "…";

/// Seconds in the units an age is reported in, largest first.
const AGE_UNITS: [(&str, u64); 4] = [("d", 86400), ("h", 3600), ("m", 60), ("s", 1)];

/// Header label for the size column.
const HEADER_SIZE: &str = "Size";

/// Header label for the age column.
const HEADER_AGE: &str = "Age";

/// Header label for the permissions column.
const HEADER_PERMISSIONS: &str = "Permissions";

/// Width of the permissions column: the nine `rwx` characters.
const DETAIL_PERMISSIONS_WIDTH: usize = 9;

/// Width of the size column.
const DETAIL_SIZE_WIDTH: usize = 8;

/// Width of the age column.
const DETAIL_AGE_WIDTH: usize = 5;

/// Gap between detail columns.
const DETAIL_GAP: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// Which detail columns the listing shows, which its column names follow.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeaderFlags {
    /// Whether the detail columns are shown.
    pub show_details: bool,
    /// Whether directory sizes are being walked and shown.
    pub show_sizes: bool,
}

// ========================================================================
// Functions
// ========================================================================

/// The names of the detail columns, each over its column, when details or
/// sizes are on; none when neither is.
pub fn column_names(flags: HeaderFlags) -> Vec<ColumnName> {
    let HeaderFlags {
        show_details,
        show_sizes,
    } = flags;
    let name = |align, start: usize, width: usize, text: &str| ColumnName {
        align,
        cols: start..start + width,
        text: text.to_string(),
    };
    let mut names = Vec::new();
    if show_details || show_sizes {
        names.push(name(
            ColumnAlign::Right,
            DETAIL_COL,
            DETAIL_SIZE_WIDTH,
            HEADER_SIZE,
        ));
    }
    if show_details {
        let age = DETAIL_COL + DETAIL_SIZE_WIDTH + DETAIL_GAP.len();
        names.push(name(ColumnAlign::Right, age, DETAIL_AGE_WIDTH, HEADER_AGE));
        let permissions = age + DETAIL_AGE_WIDTH + DETAIL_GAP.len();
        names.push(name(
            ColumnAlign::Left,
            permissions,
            DETAIL_PERMISSIONS_WIDTH,
            HEADER_PERMISSIONS,
        ));
    }
    names
}

/// One row's name as the edit mode is drawing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditedName {
    /// The name as edited so far, without the `/` or `@` the listing decorates
    /// kinds with: the text being edited is exactly the name.
    pub text: String,
    /// Whether the name still differs from the entry's own, which draws it as
    /// marked for the rename to come.
    pub changed: bool,
}

/// How one row is drawn: what is on, and what has been measured for it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowStyle {
    /// What git says about the entry, where it is changed, holds a change,
    /// or is ignored.
    pub git: Option<GitStatus>,
    /// Whether the entry is marked for an operation.
    pub marked: bool,
    /// Whether the detail columns are shown.
    pub show_details: bool,
    /// Whether directory sizes are being walked and shown.
    pub show_sizes: bool,
    /// The walked total for a directory, once it is known.
    pub size: Option<u64>,
    /// The spinner frame a directory wears while its walk is under way.
    pub spinner: Option<char>,
}

/// One entry: its fold glyph, indented name, and the detail columns when they
/// are on, with the icon the host should draw over the columns kept for it;
/// and its gutter's one mark: its mark for an operation where it is marked,
/// else its git state, blank where it is clean.
///
/// The icon's columns are left blank here: the host draws the artwork over
/// them.
pub fn entry_row(
    row: &Row,
    edit: Option<&EditedName>,
    style: RowStyle,
    now: SystemTime,
) -> (paint::Row, Vec<Icon>, Vec<Option<GutterMark>>) {
    let (name, name_style) = match edit {
        Some(edited) => (
            edited.text.clone(),
            if edited.changed {
                Style::Marked
            } else {
                name_style(row.entry.kind)
            },
        ),
        None => (kind_decorated_name(&row.entry), name_style(row.entry.kind)),
    };
    let (prefix, icon_col) = name_prefix(row);
    let icon = Icon {
        col: icon_col,
        kind: icon_kind(row),
        row: 0,
    };
    let mut spans = vec![Span::new(name_style, format!("{prefix}{name}"))];
    let label_len = prefix.chars().count() + name.chars().count();
    let trailing = match style.show_details {
        true => Some(details(row, now, style)),
        false => file_or_dir_size(row, style),
    };
    if let Some(trailing) = trailing {
        let gap = DETAIL_COL.saturating_sub(label_len).max(1);
        spans.push(Span::new(
            Style::Dim,
            format!("{}{}", " ".repeat(gap), trailing),
        ));
    }
    // A mark for an operation over the git state: the name says the row is
    // marked too, and the operation is what the user is about to do.
    let git = style.git.map(|status| {
        GutterMark::Icon(IconKind::Git {
            status: status.icon_stem().to_string(),
        })
    });
    let marks = vec![style.marked.then_some(GutterMark::Icon(MARKED)).or(git)];
    (spans, vec![icon], marks)
}

/// What leads `row`'s name: its indent, fold glyph, and the icon's columns
/// with a blank keeping the name off the artwork; and the column the icon
/// starts at.
fn name_prefix(row: &Row) -> (String, usize) {
    let indent = " ".repeat(row.depth * INDENT);
    // `.` and `..` never open in place, so they have nothing to fold.
    let glyph = match (row.entry.is_dir() && !row.is_nav(), row.expanded) {
        (true, true) => GLYPH_EXPANDED,
        (true, false) => GLYPH_COLLAPSED,
        (false, _) => GLYPH_NONE,
    };
    let icon_col = indent.chars().count() + glyph.chars().count();
    (
        format!("{indent}{glyph}{}", " ".repeat(Icon::WIDTH + 1)),
        icon_col,
    )
}

/// The column `row`'s name starts at as [`entry_row`] paints it: where a
/// caret in the name being edited is counted from.
pub fn name_col(row: &Row) -> usize {
    name_prefix(row).0.chars().count()
}

/// The name with the suffix its kind is drawn with: the `/` a directory wears
/// and the `@` a link does are decoration, not part of the name.
/// What a row's icon depicts: a folder, open or shut as the row is, or a file
/// by its name.
pub fn icon_kind(row: &Row) -> IconKind {
    match row.entry.is_dir() {
        true => IconKind::Dir {
            expanded: row.expanded,
            name: row.entry.name.clone(),
        },
        false => IconKind::File {
            name: row.entry.name.clone(),
        },
    }
}

fn kind_decorated_name(entry: &Entry) -> String {
    match entry.kind {
        EntryKind::Dir => format!("{}/", entry.name),
        EntryKind::File => entry.name.clone(),
        EntryKind::Symlink => format!("{}@", entry.name),
    }
}

/// The size of an entry when directory/file sizes are shown on their own.
fn file_or_dir_size(row: &Row, style: RowStyle) -> Option<String> {
    if !style.show_sizes {
        return None;
    }
    let size = match (row.entry.is_dir(), style.size) {
        (true, Some(bytes)) => format_size(bytes),
        (true, None) => style
            .spinner
            .map_or_else(|| WALKING.to_string(), String::from),
        (false, _) => format_size(row.entry.meta.len),
    };
    Some(format!("{size:>DETAIL_SIZE_WIDTH$}"))
}

/// Unix mode bits as the nine `rwx` characters, ignoring the file-type bits the
/// entry's own kind already carries.
pub fn format_mode(mode: u32) -> String {
    const FLAGS: [(u32, char); 9] = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    FLAGS
        .iter()
        .map(|&(bit, ch)| if mode & bit != 0 { ch } else { '-' })
        .collect()
}

/// How long ago `modified` was, in the largest unit that fits. A timestamp in
/// the future reads as `0s` rather than wrapping into a huge age.
pub fn format_age(modified: SystemTime, now: SystemTime) -> String {
    let elapsed = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    for (label, seconds) in AGE_UNITS {
        if elapsed >= seconds {
            return format!("{}{label}", elapsed / seconds);
        }
    }
    format!("0{}", AGE_UNITS[AGE_UNITS.len() - 1].0)
}

fn details(row: &Row, now: SystemTime, style: RowStyle) -> String {
    let size = match (row.entry.is_dir(), style.size) {
        (true, Some(bytes)) => format_size(bytes),
        (true, None) => style
            .spinner
            .map_or_else(|| NO_VALUE.to_string(), String::from),
        (false, _) => format_size(row.entry.meta.len),
    };
    format!(
        "{:>8}  {:>5}  {}",
        size,
        age_or_placeholder(&row.entry.meta, now),
        mode_or_placeholder(&row.entry.meta),
    )
}

fn age_or_placeholder(meta: &Meta, now: SystemTime) -> String {
    meta.modified
        .map(|modified| format_age(modified, now))
        .unwrap_or_else(|| NO_VALUE.to_string())
}

fn mode_or_placeholder(meta: &Meta) -> String {
    meta.mode
        .map(format_mode)
        .unwrap_or_else(|| NO_VALUE.to_string())
}

fn name_style(kind: EntryKind) -> Style {
    match kind {
        EntryKind::Dir => Style::Accent,
        EntryKind::File => Style::Normal,
        EntryKind::Symlink => Style::Dim,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    /// The spans of a row, dropping the icon these tests do not assert on.
    fn entry_row_only(row: &Row, style: RowStyle, now: SystemTime) -> paint::Row {
        entry_row(row, None, style, now).0
    }

    use super::*;
    use crate::entry::Entry;
    use std::path::PathBuf;

    fn row(name: &str, kind: EntryKind, depth: usize, expanded: bool) -> Row {
        Row::new(
            depth,
            Entry::new(kind, Meta::default(), name.to_string(), PathBuf::from(name)),
            expanded,
        )
    }

    fn text(spans: paint::Row) -> String {
        spans.into_iter().map(|span| span.text).collect()
    }

    #[test]
    fn test_mode_bits_read_as_rwx_triples() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o640), "rw-r-----");
        assert_eq!(format_mode(0o000), "---------");
    }

    #[test]
    fn test_age_reports_the_largest_fitting_unit() {
        const BASE: u64 = 1_000_000;
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(BASE);
        let ago = |secs: u64| {
            format_age(
                SystemTime::UNIX_EPOCH + Duration::from_secs(BASE - secs),
                now,
            )
        };
        assert_eq!(ago(0), "0s");
        assert_eq!(ago(59), "59s");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(3600), "1h");
        assert_eq!(ago(86_400 * 3), "3d");
    }

    #[test]
    fn test_age_of_a_file_from_the_future_does_not_wrap() {
        // `duration_since` errors when the timestamp is ahead of the clock, and
        // an unwrapped negative would come back as a multi-billion-second age.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let ahead = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(format_age(ahead, now), "0s");
    }

    #[test]
    fn test_nesting_indents_and_marks_the_expanded_directory() {
        let now = SystemTime::UNIX_EPOCH;
        let dir = text(entry_row_only(
            &row("src", EntryKind::Dir, 0, true),
            RowStyle::default(),
            now,
        ));
        assert!(
            dir.starts_with("⌄ "),
            "the glyph first, the gutter apart: {dir:?}"
        );
        assert!(dir.ends_with(" src/"), "got {dir:?}");
        let file = text(entry_row_only(
            &row("main.rs", EntryKind::File, 1, false),
            RowStyle::default(),
            now,
        ));
        assert!(
            file.starts_with("    "),
            "one indent, then the glyph column"
        );
        assert!(file.ends_with(" main.rs"), "got {file:?}");
    }

    #[test]
    fn test_a_changed_entry_wears_its_git_icon_in_the_gutter_and_a_clean_one_none() {
        let file = row("lib.rs", EntryKind::File, 1, false);
        let (_, icons, marks) = entry_row(
            &file,
            None,
            RowStyle {
                git: Some(GitStatus::Modified),
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(
            marks[0],
            Some(GutterMark::Icon(IconKind::Git {
                status: "git-modified".to_string()
            })),
            "in the gutter's lane"
        );
        assert_eq!(icons.len(), 1, "the row's own icons are the entry's alone");

        let (_, _, clean) = entry_row(&file, None, RowStyle::default(), SystemTime::UNIX_EPOCH);
        assert_eq!(
            clean,
            [None],
            "a clean, unmarked entry leaves the lane blank"
        );
    }

    #[test]
    fn test_a_marked_row_wears_the_mark_in_the_lane_over_its_git_state() {
        let (_, _, marks) = entry_row(
            &row("notes.txt", EntryKind::File, 0, false),
            None,
            RowStyle {
                git: Some(GitStatus::Modified),
                marked: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(
            marks,
            [Some(GutterMark::Icon(MARKED))],
            "the mark, not the git state"
        );
    }

    #[test]
    fn test_a_directory_shows_its_walked_total_in_place_of_a_dash() {
        // A directory's own `len` is meaningless, so the detail column shows a
        // dash until a walk has something to put there.
        let dir = row("src", EntryKind::Dir, 0, false);
        let unwalked = text(entry_row_only(
            &dir,
            RowStyle {
                show_details: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(unwalked.contains('-'), "got {unwalked:?}");

        let walked = text(entry_row_only(
            &dir,
            RowStyle {
                show_details: true,
                size: Some(2048),
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(walked.contains("2.0K"), "got {walked:?}");
    }

    #[test]
    fn test_a_directory_being_walked_says_so_only_when_sizes_are_on() {
        let dir = row("src", EntryKind::Dir, 0, false);
        let quiet = text(entry_row_only(
            &dir,
            RowStyle::default(),
            SystemTime::UNIX_EPOCH,
        ));
        assert!(quiet.ends_with("src/"), "got {quiet:?}");

        let walking = text(entry_row_only(
            &dir,
            RowStyle {
                show_sizes: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(walking.ends_with(WALKING), "got {walking:?}");
    }

    #[test]
    fn test_a_file_shows_its_size_when_sizes_are_on() {
        let mut file = row("doc.txt", EntryKind::File, 0, false);
        file.entry.meta.len = 2048;
        let quiet = text(entry_row_only(
            &file,
            RowStyle::default(),
            SystemTime::UNIX_EPOCH,
        ));
        assert!(quiet.ends_with("doc.txt"), "got {quiet:?}");

        let with_size = text(entry_row_only(
            &file,
            RowStyle {
                show_sizes: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(with_size.ends_with("2.0K"), "got {with_size:?}");
    }

    #[test]
    fn test_a_long_name_keeps_one_space_before_its_details() {
        // The detail columns are placed by padding to a fixed column; a name
        // past that column must still not run into its own size field.
        let name = "a".repeat(DETAIL_COL + 10);
        let painted = text(entry_row_only(
            &row(&name, EntryKind::File, 0, false),
            RowStyle {
                show_details: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(painted.contains(&format!("{name} ")), "got {painted:?}");
    }

    #[test]
    fn test_an_edited_name_drops_the_kind_suffix_and_draws_no_caret_of_its_own() {
        // The `/` a directory usually wears is decoration, not part of the
        // text being edited: leaving it on would put the caret a column off
        // the name and let a backspace eat a slash that was never really
        // there. The caret is the window's (see `DirEditor::caret`), so the
        // name itself is drawn whole. Regression: a `│` drawn into the name
        // took a column of its own, beside the window's block cursor.
        let dir = row("src", EntryKind::Dir, 0, false);
        let edited = EditedName {
            text: "source".to_string(),
            changed: true,
        };
        let painted = text(
            entry_row(
                &dir,
                Some(&edited),
                RowStyle::default(),
                SystemTime::UNIX_EPOCH,
            )
            .0,
        );
        assert!(
            painted.contains("source") && !painted.contains('│'),
            "got {painted:?}"
        );
        let (prefix, _) = name_prefix(&dir);
        assert_eq!(
            painted.find("source"),
            Some(prefix.len()),
            "where name_col says"
        );
        assert_eq!(name_col(&dir), prefix.chars().count());
        assert!(!painted.contains('/'), "got {painted:?}");
    }

    #[test]
    fn test_an_edited_name_is_marked_until_it_matches_the_entry_again() {
        // A pending rename is something selected for an operation, which is
        // what the marked style is for; matching again puts the kind's own
        // style back.
        let file = row("notes.txt", EntryKind::File, 0, false);
        let changed = EditedName {
            text: "draft.md".to_string(),
            changed: true,
        };
        let spans = entry_row(
            &file,
            Some(&changed),
            RowStyle::default(),
            SystemTime::UNIX_EPOCH,
        )
        .0;
        assert!(
            spans.iter().any(|span| span.style == Style::Marked),
            "got {spans:?}"
        );

        let same = EditedName {
            text: "notes.txt".to_string(),
            changed: false,
        };
        let spans = entry_row(
            &file,
            Some(&same),
            RowStyle::default(),
            SystemTime::UNIX_EPOCH,
        )
        .0;
        assert!(
            spans.iter().all(|span| span.style != Style::Marked),
            "got {spans:?}"
        );
    }
}
