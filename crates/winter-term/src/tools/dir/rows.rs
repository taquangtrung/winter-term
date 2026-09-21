//! Painting a listing: the header line, one line per entry, and the detail
//! columns behind the details toggle.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::model::page::{PageIcon, PageIconKind, PageRow, PageSpan, PageStyle};
use crate::model::path::format_full_path;
use crate::model::units::format_size;

use super::entry::{Entry, EntryKind, Meta};
use super::icons::icon_for;
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

/// Drawn inside an edited name where the next keystroke lands, the same glyph
/// the host's prompt line draws for its own caret.
const CARET: &str = "│";

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

// ========================================================================
// Data Structures
// ========================================================================

/// What the header reports about the listing's state, beyond where it is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeaderFlags {
    /// Which mode the name editor is in, when it is open at all.
    pub editing: Option<&'static str>,
    /// Whether dotfiles are listed.
    pub show_hidden: bool,
    /// Whether the detail columns are shown.
    pub show_details: bool,
    /// Whether directory sizes are being walked and shown.
    pub show_sizes: bool,
}

// ========================================================================
// Functions
// ========================================================================

/// Decorate a path with syntax spans:
/// - Path separators (`/` and `\`) styled as subtle delimiters (`PageStyle::SyntaxComment`).
/// - All directory components styled with the bold directory color (`PageStyle::HeaderAccent`).
pub fn path_spans(path: &str) -> Vec<PageSpan> {
    if path.is_empty() {
        return Vec::new();
    }

    // A bare root (e.g. "/" or "\" or "C:\" or "C:/") is itself the active directory.
    let is_root = path == "/"
        || path == "\\"
        || (path.len() <= 3 && path.ends_with(':'))
        || (path.len() == 3
            && path.as_bytes()[1] == b':'
            && (path.as_bytes()[2] == b'\\' || path.as_bytes()[2] == b'/'));
    if is_root {
        return vec![PageSpan::new(PageStyle::HeaderAccent, path)];
    }

    enum Token {
        Separator(String),
        Component(String),
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut in_sep = false;

    for ch in path.chars() {
        let is_sep = ch == '/' || ch == '\\';
        if is_sep == in_sep {
            current.push(ch);
        } else {
            if !current.is_empty() {
                if in_sep {
                    tokens.push(Token::Separator(current));
                } else {
                    tokens.push(Token::Component(current));
                }
                current = String::new();
            }
            in_sep = is_sep;
            current.push(ch);
        }
    }
    if !current.is_empty() {
        if in_sep {
            tokens.push(Token::Separator(current));
        } else {
            tokens.push(Token::Component(current));
        }
    }

    tokens
        .into_iter()
        .map(|token| match token {
            Token::Separator(sep) => PageSpan::new(PageStyle::SyntaxComment, sep),
            Token::Component(comp) => PageSpan::new(PageStyle::HeaderAccent, comp),
        })
        .collect()
}

/// The header: where the listing is, which toggles are on, how much is marked,
/// and what the last operation reported.
pub fn header_row(
    root: &str,
    flags: HeaderFlags,
    marked: usize,
    message: Option<&str>,
) -> PageRow {
    let HeaderFlags {
        editing,
        show_hidden,
        show_details,
        show_sizes,
    } = flags;

    let formatted_root = format_full_path(Path::new(root));
    let mut spans = path_spans(&formatted_root);

    // Status tags: active editor mode, toggles, marked count.
    if let Some(mode) = editing {
        spans.push(PageSpan::new(PageStyle::Dim, "  editing:"));
        spans.push(PageSpan::new(PageStyle::HeaderAccent, mode));
    }
    if show_hidden {
        spans.push(PageSpan::new(PageStyle::Dim, "  "));
        spans.push(PageSpan::new(PageStyle::Header, "dotfiles"));
    }
    if show_details {
        spans.push(PageSpan::new(PageStyle::Dim, "  "));
        spans.push(PageSpan::new(PageStyle::Header, "details"));
    }
    if show_sizes {
        spans.push(PageSpan::new(PageStyle::Dim, "  "));
        spans.push(PageSpan::new(PageStyle::Header, "sizes"));
    }
    if marked > 0 {
        spans.push(PageSpan::new(PageStyle::Dim, "  "));
        spans.push(PageSpan::new(PageStyle::SyntaxNumber, marked.to_string()));
        spans.push(PageSpan::new(PageStyle::Dim, " marked"));
    }
    if let Some(message) = message {
        spans.push(PageSpan::new(PageStyle::Dim, "  "));
        spans.push(PageSpan::new(PageStyle::HeaderAccent, message));
    }
    spans
}

/// One row's name as the edit mode is drawing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditedName {
    /// The name as edited so far, without the `/` or `@` the listing decorates
    /// kinds with: the text being edited is exactly the name.
    pub text: String,
    /// Where the caret sits, in characters within `text`, or `None` on a row
    /// the listing's cursor is not on.
    pub caret: Option<usize>,
    /// Whether the name still differs from the entry's own, which draws it as
    /// marked for the rename to come.
    pub changed: bool,
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
pub fn entry_row(
    row: &Row,
    edit: Option<&EditedName>,
    style: RowStyle,
    now: SystemTime,
) -> (PageRow, PageIcon) {
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
    let (name, name_style, caret) = match edit {
        Some(edited) => (
            edited.text.clone(),
            if edited.changed {
                PageStyle::Marked
            } else {
                name_style(row.entry.kind)
            },
            edited.caret,
        ),
        None => (
            kind_decorated_name(&row.entry),
            name_style(row.entry.kind),
            None,
        ),
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
    let prefix = format!("{mark}{indent}{glyph}{reserved}");
    let mut spans = Vec::new();
    match caret {
        None => spans.push(PageSpan::new(name_style, format!("{prefix}{name}"))),
        Some(at) => {
            // The caret is its own span so it stands out against the name, and
            // the name is split by character so a multibyte one is not cut.
            let before: String = name.chars().take(at).collect();
            let after: String = name.chars().skip(at).collect();
            spans.push(PageSpan::new(name_style, format!("{prefix}{before}")));
            spans.push(PageSpan::new(PageStyle::Accent, CARET.to_string()));
            if !after.is_empty() {
                spans.push(PageSpan::new(name_style, after));
            }
        }
    }
    let label_len = prefix.chars().count() + name.chars().count() + usize::from(caret.is_some());
    let trailing = match (style.show_details, style.size) {
        (true, size) => Some(details(row, now, size)),
        (false, Some(size)) => Some(format!("{:>8}", format_size(size))),
        (false, None) => walking_note(row, style),
    };
    if let Some(trailing) = trailing {
        let gap = DETAIL_COL.saturating_sub(label_len).max(1);
        spans.push(PageSpan::new(
            PageStyle::Dim,
            format!("{}{}", " ".repeat(gap), trailing),
        ));
    }
    (spans, icon)
}

/// The name with the suffix its kind is drawn with: the `/` a directory wears
/// and the `@` a link does are decoration, not part of the name.
fn kind_decorated_name(entry: &Entry) -> String {
    match entry.kind {
        EntryKind::Dir => format!("{}/", entry.name),
        EntryKind::File => entry.name.clone(),
        EntryKind::Symlink => format!("{}@", entry.name),
    }
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
        entry_row(row, None, style, now).0
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

    #[test]
    fn test_an_edited_name_draws_a_caret_and_drops_the_kind_suffix() {
        // The `/` a directory usually wears is decoration, not part of the
        // text being edited: leaving it on would put the caret a column off
        // the name and let a backspace eat a slash that was never really
        // there. The caret lands between the name's halves, never inside a
        // character, and the name is split by character to keep it so.
        let dir = row("src", EntryKind::Dir, 0, false);
        let edited = EditedName {
            text: "source".to_string(),
            caret: Some(3),
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
        assert!(painted.contains("sou│rce"), "got {painted:?}");
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
            caret: None,
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
            spans.iter().any(|span| span.style == PageStyle::Marked),
            "got {spans:?}"
        );

        let same = EditedName {
            text: "notes.txt".to_string(),
            caret: None,
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
            spans.iter().all(|span| span.style != PageStyle::Marked),
            "got {spans:?}"
        );
    }

    #[test]
    fn test_the_header_says_when_the_names_are_editable() {
        let plain = header_row("/tmp", HeaderFlags::default(), 0, None);
        assert!(!text(plain).contains("editing"));
        let editing = header_row(
            "/tmp",
            HeaderFlags {
                editing: Some("insert"),
                ..HeaderFlags::default()
            },
            0,
            None,
        );
        assert!(text(editing).contains("editing"));
    }

    #[test]
    fn test_the_header_path_is_formatted_for_os() {
        #[cfg(windows)]
        {
            let home = crate::model::path::home_dir().unwrap_or_else(|| PathBuf::from(r"C:\Users\User"));
            let sub = home.join("projects").join("winter");
            let row = header_row(&sub.to_string_lossy(), HeaderFlags::default(), 0, None);
            let row_str = text(row);
            let expected_prefix = crate::model::path::format_full_path(&sub);
            assert!(row_str.starts_with(&expected_prefix), "expected {expected_prefix}, got {row_str:?}");

            let forward_slash_path = "C:/Windows/System32";
            let row_win = header_row(forward_slash_path, HeaderFlags::default(), 0, None);
            let row_win_str = text(row_win);
            assert!(row_win_str.starts_with(r"C:\Windows\System32"), "got {row_win_str:?}");
        }
        #[cfg(not(windows))]
        {
            let home = crate::model::path::home_dir().unwrap_or_else(|| PathBuf::from("/home/user"));
            let sub = home.join("projects").join("winter");
            let row = header_row(&sub.to_string_lossy(), HeaderFlags::default(), 0, None);
            let row_str = text(row);
            let expected_prefix = crate::model::path::format_full_path(&sub);
            assert!(row_str.starts_with(&expected_prefix), "expected {expected_prefix}, got {row_str:?}");

            let posix_path = "/var/log";
            let row_posix = header_row(posix_path, HeaderFlags::default(), 0, None);
            let row_posix_str = text(row_posix);
            assert!(row_posix_str.starts_with("/var/log"), "got {row_posix_str:?}");
        }
    }

    #[test]
    fn test_path_spans_syntax_decorating() {
        // Windows-style path
        let win_spans = path_spans(r"C:\Users\Trung\winter-term");
        assert_eq!(
            win_spans,
            vec![
                PageSpan::new(PageStyle::HeaderAccent, "C:"),
                PageSpan::new(PageStyle::SyntaxComment, "\\"),
                PageSpan::new(PageStyle::HeaderAccent, "Users"),
                PageSpan::new(PageStyle::SyntaxComment, "\\"),
                PageSpan::new(PageStyle::HeaderAccent, "Trung"),
                PageSpan::new(PageStyle::SyntaxComment, "\\"),
                PageSpan::new(PageStyle::HeaderAccent, "winter-term"),
            ]
        );

        // POSIX-style path
        let posix_spans = path_spans("/home/user/winter-term");
        assert_eq!(
            posix_spans,
            vec![
                PageSpan::new(PageStyle::SyntaxComment, "/"),
                PageSpan::new(PageStyle::HeaderAccent, "home"),
                PageSpan::new(PageStyle::SyntaxComment, "/"),
                PageSpan::new(PageStyle::HeaderAccent, "user"),
                PageSpan::new(PageStyle::SyntaxComment, "/"),
                PageSpan::new(PageStyle::HeaderAccent, "winter-term"),
            ]
        );

        // Root paths are active directory headers
        assert_eq!(
            path_spans("/"),
            vec![PageSpan::new(PageStyle::HeaderAccent, "/")]
        );
        assert_eq!(
            path_spans(r"C:\"),
            vec![PageSpan::new(PageStyle::HeaderAccent, r"C:\")]
        );
        assert_eq!(
            path_spans("C:/"),
            vec![PageSpan::new(PageStyle::HeaderAccent, "C:/")]
        );
        assert_eq!(
            path_spans("single"),
            vec![PageSpan::new(PageStyle::HeaderAccent, "single")]
        );
        assert_eq!(path_spans(""), Vec::<PageSpan>::new());
    }

    #[test]
    fn test_header_row_syntax_decorating_spans() {
        let flags = HeaderFlags {
            editing: Some("insert"),
            show_hidden: true,
            show_details: true,
            show_sizes: true,
        };
        let row = header_row("/tmp", flags, 5, Some("file created"));

        // Verify sort information is removed
        assert!(!row.iter().any(|span| span.text.contains("sort")));

        // Verify key and value styling
        assert!(row.iter().any(|span| span.style == PageStyle::Dim && span.text == "  editing:"));
        assert!(row.iter().any(|span| span.style == PageStyle::HeaderAccent && span.text == "insert"));
        assert!(row.iter().any(|span| span.style == PageStyle::Header && span.text == "dotfiles"));
        assert!(row.iter().any(|span| span.style == PageStyle::Header && span.text == "details"));
        assert!(row.iter().any(|span| span.style == PageStyle::Header && span.text == "sizes"));
        assert!(row.iter().any(|span| span.style == PageStyle::SyntaxNumber && span.text == "5"));
        assert!(row.iter().any(|span| span.style == PageStyle::Dim && span.text == " marked"));
        assert!(row.iter().any(|span| span.style == PageStyle::HeaderAccent && span.text == "file created"));
    }
}
