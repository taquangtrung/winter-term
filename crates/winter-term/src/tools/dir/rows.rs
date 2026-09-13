//! Painting a listing: the header line, one line per entry, and the detail
//! columns behind the details toggle.

use std::time::{Duration, SystemTime};

use crate::model::page::{PageRow, PageSpan, PageStyle};

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

/// Column the detail columns begin at.
const DETAIL_COL: usize = 40;

/// Shown where a value does not apply, such as a directory's size.
const NO_VALUE: &str = "-";

/// Seconds in the units an age is reported in, largest first.
const AGE_UNITS: [(&str, u64); 4] = [("d", 86400), ("h", 3600), ("m", 60), ("s", 1)];

/// Size units, each 1024 times the one before it.
const SIZE_UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];

/// The divisor between two size units.
const SIZE_STEP: u64 = 1024;

// ========================================================================
// Functions
// ========================================================================

/// The header: where the listing is, and which toggles are on.
pub fn header_row(root: &str, sort: SortKey, show_hidden: bool, show_details: bool) -> PageRow {
    let mut flags = format!("sort:{}", sort.label());
    if show_hidden {
        flags.push_str("  dotfiles");
    }
    if show_details {
        flags.push_str("  details");
    }
    vec![
        PageSpan::new(PageStyle::Header, format!("{root}  ")),
        PageSpan::new(PageStyle::Dim, flags),
    ]
}

/// One entry: its fold glyph, indented name, and the detail columns when on.
pub fn entry_row(row: &Row, show_details: bool, now: SystemTime) -> PageRow {
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
    let label = format!("{indent}{glyph}{} {name}", icon_for(&row.entry));
    let mut spans = vec![PageSpan::new(name_style(row.entry.kind), label.clone())];
    if show_details {
        let gap = DETAIL_COL.saturating_sub(label.chars().count()).max(1);
        spans.push(PageSpan::new(
            PageStyle::Dim,
            format!("{}{}", " ".repeat(gap), details(row, now)),
        ));
    }
    spans
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

fn details(row: &Row, now: SystemTime) -> String {
    let size = if row.entry.is_dir() {
        NO_VALUE.to_string()
    } else {
        format_size(row.entry.meta.len)
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
        let dir = text(entry_row(&row("src", EntryKind::Dir, 0, true), false, now));
        assert!(dir.starts_with("⌄ "), "got {dir:?}");
        assert!(dir.ends_with(" src/"), "got {dir:?}");
        let file = text(entry_row(
            &row("main.rs", EntryKind::File, 1, false),
            false,
            now,
        ));
        assert!(file.starts_with("    "), "one indent plus the glyph column");
        assert!(file.ends_with(" main.rs"), "got {file:?}");
    }

    #[test]
    fn test_a_long_name_keeps_one_space_before_its_details() {
        // The detail columns are placed by padding to a fixed column; a name
        // past that column must still not run into its own size field.
        let name = "a".repeat(DETAIL_COL + 10);
        let painted = text(entry_row(
            &row(&name, EntryKind::File, 0, false),
            true,
            SystemTime::UNIX_EPOCH,
        ));
        assert!(painted.contains(&format!("{name} ")), "got {painted:?}");
    }
}
