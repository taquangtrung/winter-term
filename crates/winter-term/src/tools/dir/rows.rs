//! Painting a listing: the header line, one line per entry, and the detail
//! columns behind the details toggle.

use std::time::{Duration, SystemTime};

use crate::model::page::{PageIcon, PageIconKind, PageRow, PageSpan, PageStyle};

use super::entry::{EntryKind, Meta};
use super::icons::icon_for;
use super::listing::SortKey;
use super::tree::Row;

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

/// Drawn at the start of a marked row.
const MARK: &str = "*";

/// Keeps an unmarked row's name aligned with the marked ones.
const UNMARKED: &str = " ";

/// Column the detail columns begin at.
const DETAIL_COL: usize = 40;

/// Shown where a value does not apply, such as an unwalked directory's size.
const NO_VALUE: &str = "-";

/// Shown for a directory whose walk has not finished.
const WALKING: &str = "...";

/// Seconds in the units an age is reported in, largest first.
const AGE_UNITS: [(&str, u64); 4] = [("d", 86400), ("h", 3600), ("m", 60), ("s", 1)];

/// Size units, each 1024 times the one before it.
const SIZE_UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];

/// The divisor between two size units.
const SIZE_STEP: u64 = 1024;

// ========================================================================
// Functions
// ========================================================================

/// The header: where the listing is, which toggles are on, how much is marked,
/// and what the last operation reported.
pub fn header_row(
    root: &str,
    sort: SortKey,
    show_hidden: bool,
    show_details: bool,
    marked: usize,
    message: Option<&str>,
    show_sizes: bool,
) -> PageRow {
    let mut flags = format!("sort:{}", sort.label());
    if show_hidden {
        flags.push_str("  dotfiles");
    }
    if show_details {
        flags.push_str("  details");
    }
    if show_sizes {
        flags.push_str("  sizes");
    }
    if marked > 0 {
        flags.push_str(&format!("  {marked} marked"));
    }
    let mut spans = vec![
        PageSpan::new(PageStyle::Header, format!("{root}  ")),
        PageSpan::new(PageStyle::Dim, flags),
    ];
    if let Some(message) = message {
        spans.push(PageSpan::new(PageStyle::Accent, format!("  {message}")));
    }
    spans
}

/// How one row is drawn: what is on, and what has been measured for it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowStyle {
    /// Whether the entry is marked for an operation.
    pub marked: bool,
    /// Whether the detail columns are shown.
    pub show_details: bool,
    /// Whether directory sizes are being walked and shown.
    pub show_sizes: bool,
    /// The walked total for a directory, once it is known.
    pub size: Option<u64>,
}

/// One entry: its mark, fold glyph, indented name, and the detail columns when
/// they are on, with the icon the host should draw over the reserved columns.
///
/// The icon's columns are left blank here whatever the icon setting is, so the
/// name and the detail columns land in the same place however the host ends up
/// drawing it, and changing the setting never reflows the listing.
pub fn entry_row(row: &Row, style: RowStyle, now: SystemTime) -> (PageRow, PageIcon) {
    let indent = " ".repeat(row.depth * INDENT);
    let glyph = if row.entry.is_dir() {
        if row.expanded {
            GLYPH_EXPANDED
        } else {
            GLYPH_COLLAPSED
        }
    } else {
        GLYPH_NONE
    };
    let name = match row.entry.kind {
        EntryKind::Dir => format!("{}/", row.entry.name),
        EntryKind::File => row.entry.name.clone(),
        EntryKind::Symlink => format!("{}@", row.entry.name),
    };
    let mark = if style.marked { MARK } else { UNMARKED };
    let icon_col = mark.chars().count() + indent.chars().count() + glyph.chars().count();
    let icon = PageIcon {
        col: icon_col,
        glyph: icon_for(&row.entry),
        kind: if row.entry.is_dir() {
            PageIconKind::Dir {
                expanded: row.expanded,
                name: row.entry.name.clone(),
            }
        } else {
            PageIconKind::File {
                name: row.entry.name.clone(),
            }
        },
        row: 0,
    };
    // The icon's own columns, plus one blank keeping the name off the artwork.
    let reserved = " ".repeat(PageIcon::WIDTH + 1);
    let label = format!("{mark}{indent}{glyph}{reserved}{name}");
    let name_style = if style.marked {
        PageStyle::Marked
    } else {
        name_style(row.entry.kind)
    };
    let mut spans = vec![PageSpan::new(name_style, label.clone())];
    let trailing = match (style.show_details, style.size) {
        (true, size) => Some(details(row, now, size)),
        (false, Some(size)) => Some(format!("{:>8}", format_size(size))),
        (false, None) => walking_note(row, style),
    };
    if let Some(trailing) = trailing {
        let gap = DETAIL_COL.saturating_sub(label.chars().count()).max(1);
        spans.push(PageSpan::new(
            PageStyle::Dim,
            format!("{}{}", " ".repeat(gap), trailing),
        ));
    }
    (spans, icon)
}

/// What a directory shows while its size is still being walked. Nothing at all
/// when sizes were never asked for.
fn walking_note(row: &Row, style: RowStyle) -> Option<String> {
    (style.show_sizes && row.entry.is_dir()).then(|| format!("{WALKING:>8}"))
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

/// A byte count in the largest unit that leaves it under four digits.
pub fn format_size(len: u64) -> String {
    let mut size = len as f64;
    let mut unit = 0;
    while size >= SIZE_STEP as f64 && unit + 1 < SIZE_UNITS.len() {
        size /= SIZE_STEP as f64;
        unit += 1;
    }
    if unit == 0 {
        return format!("{len}{}", SIZE_UNITS[0]);
    }
    format!("{size:.1}{}", SIZE_UNITS[unit])
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

fn details(row: &Row, now: SystemTime, walked: Option<u64>) -> String {
    let size = match (row.entry.is_dir(), walked) {
        (true, Some(bytes)) => format_size(bytes),
        (true, None) => NO_VALUE.to_string(),
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

fn name_style(kind: EntryKind) -> PageStyle {
    match kind {
        EntryKind::Dir => PageStyle::Accent,
        EntryKind::File => PageStyle::Normal,
        EntryKind::Symlink => PageStyle::Dim,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    /// The spans of a row, dropping the icon these tests do not assert on.
    fn entry_row_only(row: &Row, style: RowStyle, now: SystemTime) -> PageRow {
        entry_row(row, style, now).0
    }

    use super::*;
    use crate::tools::dir::entry::Entry;
    use std::path::PathBuf;

    fn row(name: &str, kind: EntryKind, depth: usize, expanded: bool) -> Row {
        Row::new(
            depth,
            Entry::new(kind, Meta::default(), name.to_string(), PathBuf::from(name)),
            expanded,
        )
    }

    fn text(spans: PageRow) -> String {
        spans.into_iter().map(|span| span.text).collect()
    }

    #[test]
    fn test_mode_bits_read_as_rwx_triples() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o640), "rw-r-----");
        assert_eq!(format_mode(0o000), "---------");
    }

    #[test]
    fn test_size_steps_up_a_unit_at_a_time() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(1023), "1023B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1024 * 1024 * 3), "3.0M");
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
        assert!(dir.starts_with(" ⌄ "), "got {dir:?}");
        assert!(dir.ends_with(" src/"), "got {dir:?}");
        let file = text(entry_row_only(
            &row("main.rs", EntryKind::File, 1, false),
            RowStyle::default(),
            now,
        ));
        assert!(
            file.starts_with("     "),
            "the mark column, one indent, then the glyph column"
        );
        assert!(file.ends_with(" main.rs"), "got {file:?}");
    }

    #[test]
    fn test_a_marked_row_leads_with_its_mark() {
        let painted = text(entry_row_only(
            &row("notes.txt", EntryKind::File, 0, false),
            RowStyle {
                marked: true,
                ..RowStyle::default()
            },
            SystemTime::UNIX_EPOCH,
        ));
        assert!(painted.starts_with('*'), "got {painted:?}");
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
}
