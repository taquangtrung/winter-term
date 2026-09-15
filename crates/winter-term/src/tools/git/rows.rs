//! Painting the status view: the header, each section, and the entries under
//! it, plus the mapping from a row back to what it stands for.

use crate::model::page::{PageIcon, PageIconKind, PageRow, PageSpan, PageStyle};

use super::diff::FileDiff;
use super::parse::{Commit, FileStatus, Section, Status};

// ========================================================================
// Constants
// ========================================================================

/// Heading of the recent-commits section.
const RECENT_TITLE: &str = "Recent commits";

/// Marks a section whose entries are hidden.
const GLYPH_COLLAPSED: &str = "› ";

/// Marks a section whose entries are shown.
const GLYPH_EXPANDED: &str = "⌄ ";

/// Indent every entry sits at, under its heading.
const ENTRY_INDENT: &str = "  ";

/// Indent a diff line sits at, under its file.
const HUNK_INDENT: &str = "    ";

/// Column the change code is drawn in, before the path.
const CODE_WIDTH: usize = 2;

/// Glyph drawn for a file row when the icon style is a font rather than
/// artwork. One generic file: the Git view is about what changed, and a
/// per-language glyph set belongs to the listing tools.
const FILE_GLYPH: char = '\u{f016}';

// ========================================================================
// Data Structures
// ========================================================================

/// What one row of the view stands for, so a key acting "at point" knows what
/// it is acting on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item {
    /// A commit in the recent list.
    Commit(String),
    /// A path in one of the change sections.
    File(FileRow),
    /// A section heading.
    Heading(Section),
    /// A hunk of a file's diff, or one of its lines: both act on the hunk, so
    /// a key works anywhere inside it rather than only on its header.
    Hunk(HunkRow),
    /// The header, or a blank line: nothing to act on.
    None,
    /// The heading of the recent-commits section.
    RecentHeading,
}

/// One hunk of an expanded file's diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HunkRow {
    /// Which hunk of that file's diff, counting from zero.
    pub index: usize,
    /// The path the hunk belongs to.
    pub path: String,
    /// The section the file was listed under, which decides which way a patch
    /// is applied.
    pub section: Section,
}

/// A path listed in a section.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileRow {
    /// The path, relative to the repository root.
    pub path: String,
    /// The section it was listed under, which decides what staging means.
    pub section: Section,
}

/// One painted row: its spans, what it stands for, and the icon the host
/// should draw over the columns the spans left clear for it.
#[derive(Clone, Debug)]
pub struct ViewRow {
    /// What acting on this row acts on.
    pub item: Item,
    /// The icon to draw beside the row, for the rows that have one. Its `row`
    /// is filled in by whoever assembles the page, which is the only place the
    /// row's final position is known.
    pub icon: Option<PageIcon>,
    /// The row's painted spans.
    pub spans: PageRow,
}

// ========================================================================
// Functions
// ========================================================================

/// Build every row of the view: a header, then each non-empty section, then the
/// recent commits.
pub fn build(
    status: &Status,
    commits: &[Commit],
    collapsed: &dyn Fn(Option<Section>) -> bool,
    message: Option<&str>,
    diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
) -> Vec<ViewRow> {
    let mut rows = vec![header_row(status, message), blank_row()];
    for section in Section::all() {
        let files: Vec<&FileStatus> = status
            .files
            .iter()
            .filter(|file| file.section == section)
            .collect();
        if files.is_empty() {
            continue;
        }
        let shut = collapsed(Some(section));
        rows.push(heading_row(
            section.title(),
            files.len(),
            shut,
            Item::Heading(section),
        ));
        if !shut {
            for file in files {
                let row = file_row(file);
                let key = match &row.item {
                    Item::File(key) => Some(key.clone()),
                    _ => None,
                };
                rows.push(row);
                // An expanded file shows its own diff, hunk by hunk, which is
                // what makes staging part of a file possible.
                if let Some(diff) = key.as_ref().and_then(diffs) {
                    rows.extend(hunk_rows(key.as_ref().expect("a file row"), &diff));
                }
            }
        }
        rows.push(blank_row());
    }
    if !commits.is_empty() {
        let shut = collapsed(None);
        rows.push(heading_row(
            RECENT_TITLE,
            commits.len(),
            shut,
            Item::RecentHeading,
        ));
        if !shut {
            rows.extend(commits.iter().map(commit_row));
        }
    }
    rows
}

/// The header: the branch, what it tracks, and how far apart they are.
fn header_row(status: &Status, message: Option<&str>) -> ViewRow {
    let branch = status
        .branch
        .clone()
        .unwrap_or_else(|| "detached".to_string());
    let mut spans = vec![PageSpan::new(PageStyle::Header, branch)];
    if let Some(upstream) = &status.upstream {
        spans.push(PageSpan::new(PageStyle::Dim, format!("  → {upstream}")));
    }
    if status.ahead > 0 || status.behind > 0 {
        spans.push(PageSpan::new(
            PageStyle::Accent,
            format!("  ↑{} ↓{}", status.ahead, status.behind),
        ));
    }
    if let Some(message) = message {
        spans.push(PageSpan::new(PageStyle::Accent, format!("  {message}")));
    }
    ViewRow {
        icon: None,
        item: Item::None,
        spans,
    }
}

fn heading_row(title: &str, count: usize, collapsed: bool, item: Item) -> ViewRow {
    let glyph = if collapsed {
        GLYPH_COLLAPSED
    } else {
        GLYPH_EXPANDED
    };
    ViewRow {
        icon: None,
        item,
        spans: vec![
            PageSpan::new(PageStyle::Header, format!("{glyph}{title}")),
            PageSpan::new(PageStyle::Dim, format!(" ({count})")),
        ],
    }
}

fn file_row(file: &FileStatus) -> ViewRow {
    let name = match &file.renamed_from {
        Some(from) => format!("{from} → {}", file.path),
        None => file.path.clone(),
    };
    // The status letter stays: it says what changed, where the icon says what
    // kind of file it is. The icon's columns follow it, blank, with one more
    // keeping the name off the artwork.
    let reserved = " ".repeat(PageIcon::WIDTH + 1);
    ViewRow {
        icon: Some(PageIcon {
            col: ENTRY_INDENT.chars().count() + CODE_WIDTH,
            glyph: FILE_GLYPH,
            kind: PageIconKind::File {
                name: leaf_name(&file.path).to_string(),
            },
            row: 0,
        }),
        item: Item::File(FileRow {
            path: file.path.clone(),
            section: file.section,
        }),
        spans: vec![
            PageSpan::new(
                code_style(file.section),
                format!("{ENTRY_INDENT}{:<CODE_WIDTH$}", file.code),
            ),
            PageSpan::plain(format!("{reserved}{name}")),
        ],
    }
}

/// The last path segment, which is what the icon set matches names against: a
/// table keyed on `Cargo.toml` never fires for `crates/winter-term/Cargo.toml`.
fn leaf_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Every row of one file's diff: each hunk's header, then its lines, all
/// standing for the same hunk.
fn hunk_rows(file: &FileRow, diff: &FileDiff) -> Vec<ViewRow> {
    let mut rows = Vec::new();
    for (index, hunk) in diff.hunks.iter().enumerate() {
        let item = Item::Hunk(HunkRow {
            index,
            path: file.path.clone(),
            section: file.section,
        });
        let (added, removed) = hunk.counts();
        rows.push(ViewRow {
            icon: None,
            item: item.clone(),
            spans: vec![
                PageSpan::new(PageStyle::Header, format!("{HUNK_INDENT}{}", hunk.header)),
                PageSpan::new(PageStyle::Dim, format!("  +{added} -{removed}")),
            ],
        });
        rows.extend(hunk.lines.iter().map(|line| ViewRow {
            icon: None,
            item: item.clone(),
            spans: vec![PageSpan::new(
                line_style(line),
                format!("{HUNK_INDENT}{line}"),
            )],
        }));
    }
    rows
}

/// Added lines read as arriving, removed as leaving, context as neither.
fn line_style(line: &str) -> PageStyle {
    match line.chars().next() {
        Some('+') => PageStyle::Accent,
        Some('-') => PageStyle::Marked,
        Some(_) | None => PageStyle::Dim,
    }
}

fn commit_row(commit: &Commit) -> ViewRow {
    ViewRow {
        icon: None,
        item: Item::Commit(commit.hash.clone()),
        spans: vec![
            PageSpan::new(PageStyle::Accent, format!("{ENTRY_INDENT}{} ", commit.hash)),
            PageSpan::plain(commit.subject.clone()),
        ],
    }
}

fn blank_row() -> ViewRow {
    ViewRow {
        icon: None,
        item: Item::None,
        spans: Vec::new(),
    }
}

/// Staged changes read as settled, unstaged and untracked as pending, and a
/// conflict as something demanding attention.
fn code_style(section: Section) -> PageStyle {
    match section {
        Section::Staged => PageStyle::Accent,
        Section::Unmerged => PageStyle::Marked,
        Section::Unstaged | Section::Untracked => PageStyle::Dim,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn file(section: Section, path: &str, code: char) -> FileStatus {
        FileStatus {
            section,
            path: path.to_string(),
            renamed_from: None,
            code,
        }
    }

    fn status_with(files: Vec<FileStatus>) -> Status {
        Status {
            ahead: 0,
            behind: 0,
            branch: Some("main".to_string()),
            files,
            upstream: None,
        }
    }

    fn text(row: &ViewRow) -> String {
        row.spans.iter().map(|span| span.text.clone()).collect()
    }

    fn open(_section: Option<Section>) -> bool {
        false
    }

    fn shut(_section: Option<Section>) -> bool {
        true
    }

    fn no_diffs(_file: &FileRow) -> Option<FileDiff> {
        None
    }

    #[test]
    fn test_an_empty_section_gets_no_heading() {
        // A clean tree should not read as four empty headings.
        let rows = build(&status_with(Vec::new()), &[], &open, None, &no_diffs);
        assert!(!rows.iter().any(|row| matches!(row.item, Item::Heading(_))));
    }

    #[test]
    fn test_a_collapsed_section_keeps_its_heading_and_count() {
        let status = status_with(vec![
            file(Section::Staged, "a.rs", 'M'),
            file(Section::Staged, "b.rs", 'A'),
        ]);
        let rows = build(&status, &[], &shut, None, &no_diffs);
        let heading = rows
            .iter()
            .find(|row| matches!(row.item, Item::Heading(Section::Staged)))
            .expect("the heading");
        assert!(text(heading).contains("(2)"), "got {:?}", text(heading));
        assert!(!rows.iter().any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_rename_shows_where_it_came_from() {
        let mut renamed = file(Section::Staged, "new.rs", 'R');
        renamed.renamed_from = Some("old.rs".to_string());
        let rows = build(&status_with(vec![renamed]), &[], &open, None, &no_diffs);
        let row = rows
            .iter()
            .find(|row| matches!(row.item, Item::File(_)))
            .expect("the file row");
        assert!(text(row).contains("old.rs → new.rs"), "got {:?}", text(row));
    }

    #[test]
    fn test_a_file_row_remembers_which_section_listed_it() {
        // Staging and unstaging are the same key on different sections, so the
        // row has to carry which one it came from.
        let status = status_with(vec![
            file(Section::Staged, "both.rs", 'M'),
            file(Section::Unstaged, "both.rs", 'M'),
        ]);
        let rows = build(&status, &[], &open, None, &no_diffs);
        let sections: Vec<Section> = rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::File(file) => Some(file.section),
                _ => None,
            })
            .collect();
        assert_eq!(sections, [Section::Unstaged, Section::Staged]);
    }

    #[test]
    fn test_the_header_shows_tracking_and_divergence() {
        let status = Status {
            ahead: 2,
            behind: 1,
            branch: Some("main".to_string()),
            files: Vec::new(),
            upstream: Some("origin/main".to_string()),
        };
        let rows = build(&status, &[], &open, None, &no_diffs);
        let header = text(&rows[0]);
        assert!(header.contains("origin/main"), "got {header:?}");
        assert!(header.contains("↑2 ↓1"), "got {header:?}");
    }

    #[test]
    fn test_commits_carry_their_hash_for_acting_on() {
        let commits = vec![Commit {
            hash: "abc1234".to_string(),
            subject: "do the thing".to_string(),
        }];
        let rows = build(&status_with(Vec::new()), &commits, &open, None, &no_diffs);
        assert!(rows
            .iter()
            .any(|row| row.item == Item::Commit("abc1234".to_string())));
    }
}
