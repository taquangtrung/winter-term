//! Painting the status view: the header, each section, and the entries under
//! it, plus the mapping from a row back to what it stands for — and the
//! whole-diff output a key can put in the view's place.

use std::collections::HashSet;
use std::path::Path;

use crate::model::page::{PageIcon, PageIconKind, PageRow, PageSpan, PageStyle};

use super::commit::{CommitContent, CommitFile};
use super::diff::{FileDiff, Hunk};
use super::parse::{Commit, FileStatus, RefKind, Section, Stash, Status};
#[cfg(test)]
use super::progress::Operation;
use super::progress::Progress;
use super::reltime;
use super::words::{decorate, Segment};

// ========================================================================
// Constants
// ========================================================================

/// Heading of the recent-commits section.
const RECENT_TITLE: &str = "Recent commits";

/// Marks a row whose contents are hidden.
const GLYPH_COLLAPSED: &str = "▶ ";

/// Marks a row whose contents are shown.
const GLYPH_EXPANDED: &str = "▼ ";

/// Column the values in the status header's label block start at, so `Repo:`,
/// `Head:`, `Merge:` and `Tag:` all line their values up.
const LABEL_WIDTH: usize = 10;

/// Labels of the status header block, in the order they are drawn.
const LABEL_REPO: &str = "Repo:";
const LABEL_HEAD: &str = "Head:";
const LABEL_MERGE: &str = "Merge:";
const LABEL_TAG: &str = "Tag:";

/// What the header says when `HEAD` names no commit yet, which is a fresh
/// `git init` before its first commit. Borrowed from magit, which has said this
/// for long enough that it reads as the state's name.
const EMPTY_HEAD: &str = "In the beginning there was darkness";

/// What the header calls a `HEAD` that is not on a branch.
const DETACHED: &str = "detached";

/// What the merge line says when the branch tracks nothing.
const NO_UPSTREAM: &str = "Unpushed";

/// The graph column drawn beside a commit. A literal marker rather than git's
/// `--graph` art: the log is read in reverse-chronological order, where every
/// commit sits on one lane, and variable-width art would misalign the author
/// line beneath it.
const GRAPH_MARK: &str = "* ";

/// Marks a commit that exists locally but not on the branch's upstream.
const MARK_UNPUSHED: &str = "↑ ";

/// Heading of the log view's commit list.
const LOG_TITLE: &str = "Commit logs";

/// Heading over the files a commit touched, which folds them all away at once.
const CHANGES_TITLE: &str = "Changes";

/// Heading of the stash section.
const STASH_TITLE: &str = "Stashes";

/// Heading of the section listing what the upstream has and this branch does
/// not. The upstream's name follows it.
const UNPULLED_TITLE: &str = "Unpulled from";

/// Label of the line naming whatever git is part-way through.
const LABEL_STATE: &str = "State:";

/// What the log view says when more commits can be loaded.
const LOG_MORE: &str = "Press '+' to display more commits";

/// The one column a diff line's marker takes, which a wrapped continuation
/// starts past.
const MARKER_WIDTH: usize = 1;

/// The four-space indent `git show` gives a commit's message lines, which the
/// summary takes back off: every row of the view starts at the pane's left
/// edge, so the columns go to the text rather than to nesting.
const MESSAGE_INDENT: &str = "    ";

/// Column the change code is drawn in, before the path.
const CODE_WIDTH: usize = 2;

/// Width a commit's change name is padded to, before the icon and the path,
/// so every path of a commit starts at the same column whatever happened to
/// its file. Twelve, the width the same listing uses in magic-vscode.
const STATUS_WIDTH: usize = 12;

/// Glyph drawn for a file row when the icon style is a font rather than
/// artwork. One generic file: the Git view is about what changed, and a
/// per-language glyph set belongs to the listing tools.
const FILE_GLYPH: char = '\u{f016}';

/// The line that names the file a whole-diff output describes.
const FILE_MARK: &str = "diff --git ";

/// The line that opens a hunk of a whole-diff output.
const HUNK_MARK: &str = "@@";

/// Lines of file-header detail a patch keeps but the eye can skip.
const DETAIL_MARKS: [&str; 3] = ["index ", "old mode ", "new mode "];

// ========================================================================
// Data Structures
// ========================================================================

/// What one row of the view stands for, so a key acting "at point" knows what
/// it is acting on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item {
    /// The heading over every file a commit touched.
    CommitChanges,
    /// A stash, by its index in the list.
    Stash(usize),
    /// The heading of the stash section.
    StashHeading,
    /// The heading of the unpulled-commits section.
    UnpulledHeading,
    /// A file of the commit view, by its index in the commit's file list.
    CommitFile(usize),
    /// A hunk of the commit view, by its file's index and its own index within
    /// that file.
    CommitHunk(usize, usize),
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
    /// The column the row's wrapped continuation starts at, reserving the
    /// diff marker's room; zero for the rows that are not diff lines.
    pub wrap_indent: usize,
}

// ========================================================================
// Functions
// ========================================================================

/// Build every row of the view: a header block, then each non-empty section,
/// then the stashes, what the upstream is holding, and the recent commits.
///
/// Everything drawn is passed in, so this stays a pure function of what the
/// page has read.
pub fn build(
    content: StatusContent,
    collapsed: &dyn Fn(Block) -> bool,
    diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
) -> Vec<ViewRow> {
    let StatusContent {
        status,
        commits,
        unpulled,
        stashes,
        progress,
        root,
        message,
        now,
    } = content;
    let mut rows = header_block(status, commits, root, progress, message);
    rows.push(blank_row());
    for section in Section::all() {
        let files: Vec<&FileStatus> = status
            .files
            .iter()
            .filter(|file| file.section == section)
            .collect();
        if files.is_empty() {
            continue;
        }
        let shut = collapsed(Block::Files(section));
        rows.push(heading_row(
            section.title(),
            files.len(),
            shut,
            Item::Heading(section),
        ));
        if !shut {
            for file in files {
                let key = FileRow {
                    path: file.path.clone(),
                    section: file.section,
                };
                let diff = diffs(&key);
                rows.push(file_row(file, diff.is_some()));
                // An expanded file shows its own diff, hunk by hunk, which is
                // what makes staging part of a file possible.
                if let Some(diff) = diff {
                    rows.extend(hunk_rows(&key, &diff));
                }
            }
        }
        rows.push(blank_row());
    }
    if !stashes.is_empty() {
        let shut = collapsed(Block::Stashes);
        rows.push(heading_row(
            STASH_TITLE,
            stashes.len(),
            shut,
            Item::StashHeading,
        ));
        if !shut {
            rows.extend(
                stashes
                    .iter()
                    .enumerate()
                    .map(|(index, stash)| stash_row(stash, index)),
            );
        }
        rows.push(blank_row());
    }
    if !unpulled.is_empty() {
        let shut = collapsed(Block::Unpulled);
        let title = match &status.upstream {
            Some(upstream) => format!("{UNPULLED_TITLE} {upstream}"),
            None => UNPULLED_TITLE.to_string(),
        };
        rows.push(heading_row(
            &title,
            unpulled.len(),
            shut,
            Item::UnpulledHeading,
        ));
        if !shut {
            for commit in unpulled {
                // Nothing here is unpushed: these are the commits the other
                // side has, which is the opposite direction.
                rows.extend(commit_rows_decorated(commit, now, false));
            }
        }
        rows.push(blank_row());
    }
    if !commits.is_empty() {
        let shut = collapsed(Block::Recent);
        rows.push(heading_row(
            RECENT_TITLE,
            commits.len(),
            shut,
            Item::RecentHeading,
        ));
        if !shut {
            for (index, commit) in commits.iter().enumerate() {
                // The commits ahead of the upstream are the newest ones, so the
                // first `ahead` entries of a reverse-chronological log are
                // exactly what has not been pushed.
                let unpushed = index < status.ahead;
                rows.extend(commit_rows_decorated(commit, now, unpushed));
            }
        }
    }
    rows
}

/// The status header: which repository this is, what `HEAD` is, what it tracks
/// and how far it has diverged, and any tag pointing at it.
///
/// One labelled line each, values aligned at [`LABEL_WIDTH`], which is what
/// makes the block scannable down its left edge instead of read as a sentence.
fn header_block(
    status: &Status,
    commits: &[Commit],
    root: Option<&Path>,
    progress: Option<&Progress>,
    message: Option<&str>,
) -> Vec<ViewRow> {
    let mut rows = Vec::new();
    if let Some(root) = root {
        rows.push(label_row(
            LABEL_REPO,
            vec![PageSpan::new(
                PageStyle::HeadingPlain,
                abbreviate_home(root),
            )],
        ));
    }
    // The subject of the commit HEAD points at, which the log tail already
    // carries, so naming it costs no extra git call.
    let head_subject = commits.first().map(|commit| commit.subject.clone());
    match (&status.branch, &head_subject) {
        // A repository with no commit yet has nothing for the rest of the block
        // to describe.
        (None, None) => rows.push(label_row(
            LABEL_HEAD,
            vec![PageSpan::new(PageStyle::Dim, EMPTY_HEAD)],
        )),
        (branch, subject) => {
            let name = branch.clone().unwrap_or_else(|| DETACHED.to_string());
            let mut spans = vec![PageSpan::new(PageStyle::RefHead, name)];
            if let Some(subject) = subject {
                spans.push(PageSpan::plain(format!(" {subject}")));
            }
            rows.push(label_row(LABEL_HEAD, spans));
        }
    }
    // The merge line names the upstream and how far the branch has drifted from
    // it, which is the pair a reader needs before pushing or pulling.
    let mut merge = match &status.upstream {
        Some(upstream) => vec![PageSpan::new(PageStyle::RefRemote, upstream.clone())],
        None => vec![PageSpan::new(PageStyle::Dim, NO_UPSTREAM)],
    };
    if status.ahead > 0 || status.behind > 0 {
        merge.push(PageSpan::new(
            PageStyle::Unpushed,
            format!(" ↑{} ↓{}", status.ahead, status.behind),
        ));
    }
    rows.push(label_row(LABEL_MERGE, merge));
    // What git is part-way through, and the keys that finish it. Nothing in a
    // porcelain status says a rebase is open, so without this line the only
    // sign of one is a section of conflicts and no reason for them.
    if let Some(progress) = progress {
        rows.push(label_row(LABEL_STATE, progress_spans(progress)));
        rows.push(label_row(
            "",
            vec![PageSpan::new(PageStyle::Dim, progress.operation.keys())],
        ));
    }
    // A tag on HEAD reads off the same decoration the commit rows use.
    let head_tag = commits
        .first()
        .and_then(|commit| commit.refs.iter().find(|entry| entry.kind == RefKind::Tag));
    if let Some(tag) = head_tag {
        rows.push(label_row(
            LABEL_TAG,
            vec![PageSpan::new(PageStyle::RefTag, tag.name.clone())],
        ));
    }
    if let Some(message) = message {
        rows.push(ViewRow {
            icon: None,
            item: Item::None,
            spans: vec![PageSpan::new(PageStyle::Accent, message)],
            wrap_indent: 0,
        });
    }
    rows
}

/// One labelled header line: the label padded to [`LABEL_WIDTH`], then `value`.
fn label_row(label: &str, value: Vec<PageSpan>) -> ViewRow {
    let mut spans = vec![PageSpan::new(
        PageStyle::HeadingPlain,
        format!("{label:<LABEL_WIDTH$}"),
    )];
    spans.extend(value);
    ViewRow {
        icon: None,
        item: Item::None,
        spans,
        wrap_indent: LABEL_WIDTH,
    }
}

/// What git is doing, said in the order a reader asks it: which operation,
/// what it is working on, and how far along it is.
fn progress_spans(progress: &Progress) -> PageRow {
    let mut spans = vec![PageSpan::new(
        PageStyle::ChangeConflict,
        progress.operation.title(),
    )];
    if !progress.detail.is_empty() {
        spans.push(PageSpan::plain(format!(" {}", progress.detail)));
    }
    if let Some((at, of)) = progress.step {
        spans.push(PageSpan::new(PageStyle::Dim, format!("  ({at}/{of})")));
    }
    spans
}

/// One stash's row: what git calls it, then the message it was pushed with.
fn stash_row(stash: &Stash, index: usize) -> ViewRow {
    ViewRow {
        icon: None,
        item: Item::Stash(index),
        spans: vec![
            PageSpan::new(PageStyle::Accent, format!("{} ", stash.name)),
            PageSpan::plain(stash.subject.clone()),
        ],
        wrap_indent: stash.name.chars().count() + 1,
    }
}

/// `path` with the home directory written `~`, the way a path is read aloud.
/// Left whole when it is not under home, or when there is no home to compare
/// against.
fn abbreviate_home(path: &Path) -> String {
    let display = path.to_string_lossy().to_string();
    let Some(home) = std::env::var_os("HOME") else {
        return display;
    };
    let home = Path::new(&home);
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => display,
    }
}

/// The triangle a foldable row opens with: pointing right while what hangs off
/// it is hidden, down while it shows. Every row a fold key acts on carries one
/// and no other row does, so the mark says both that the row folds and which
/// way it currently stands, the way magic-vscode's tree marks its own.
fn fold_glyph(folded: bool) -> &'static str {
    if folded {
        GLYPH_COLLAPSED
    } else {
        GLYPH_EXPANDED
    }
}

/// The column a row's own text starts at, past its fold mark.
fn fold_width() -> usize {
    GLYPH_EXPANDED.chars().count()
}

fn heading_row(title: &str, count: usize, collapsed: bool, item: Item) -> ViewRow {
    let glyph = fold_glyph(collapsed);
    let style = match item {
        Item::Heading(section) => heading_style(section),
        // The recent commits are not a kind of change, so they take the hue
        // every heading without one of its own wears.
        _ => PageStyle::HeadingPlain,
    };
    ViewRow {
        icon: None,
        item,
        spans: vec![
            PageSpan::new(style, format!("{glyph}{title}")),
            PageSpan::new(PageStyle::Dim, format!(" ({count})")),
        ],
        wrap_indent: 0,
    }
}

/// The hue a section's heading wears: what sits under it, said in color, the
/// way magic-vscode colors its own headings. Staged and unstaged are the pair
/// read against each other most often, so they are furthest apart.
fn heading_style(section: Section) -> PageStyle {
    match section {
        Section::Staged => PageStyle::HeadingStaged,
        Section::Unstaged => PageStyle::HeadingUnstaged,
        Section::Unmerged => PageStyle::HeadingConflict,
        Section::Untracked => PageStyle::HeadingUntracked,
    }
}

/// A file's row: a band the file's changes sit under, bright while they show
/// beneath it and receded once folded shut. Its fold mark opens the row, then
/// the status letter — which says what changed, where the icon says what kind
/// of file it is — and the icon's columns follow it, blank, with one more
/// keeping the name off the artwork.
fn file_row(file: &FileStatus, expanded: bool) -> ViewRow {
    let name = match &file.renamed_from {
        Some(from) => format!("{from} → {}", file.path),
        None => file.path.clone(),
    };
    let reserved = " ".repeat(PageIcon::WIDTH + 1);
    let band = if expanded {
        PageStyle::Section
    } else {
        PageStyle::SectionFolded
    };
    let mark = fold_glyph(!expanded);
    ViewRow {
        icon: Some(PageIcon {
            col: fold_width() + CODE_WIDTH,
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
                change_style(file.code),
                format!("{mark}{:<CODE_WIDTH$}", file.code),
            ),
            PageSpan::new(band, format!("{reserved}{name}")),
        ],
        wrap_indent: 0,
    }
}

/// The last row of the block `index` heads: the run of rows after it that
/// appeared when it was opened, which is what has to be brought into view once
/// it is. A row that heads nothing is its own block.
pub fn block_end(rows: &[ViewRow], index: usize) -> usize {
    let Some(head) = rows.get(index) else {
        return index;
    };
    let mut end = index;
    for (at, row) in rows.iter().enumerate().skip(index + 1) {
        if !hangs_off(&head.item, &row.item) {
            break;
        }
        end = at;
    }
    end
}

/// Whether `row` is one of the rows `head` shows when it is open: a section's
/// files and their hunks, a file's own hunks, a commit's files and hunks, and
/// the commits under the recent heading.
fn hangs_off(head: &Item, row: &Item) -> bool {
    match (head, row) {
        (Item::Heading(section), Item::File(file)) => file.section == *section,
        (Item::Heading(section), Item::Hunk(hunk)) => hunk.section == *section,
        (Item::File(file), Item::Hunk(hunk)) => {
            hunk.path == file.path && hunk.section == file.section
        }
        (Item::RecentHeading, Item::Commit(_)) => true,
        (Item::CommitChanges, Item::CommitFile(_) | Item::CommitHunk(_, _)) => true,
        (Item::CommitFile(file), Item::CommitFile(other)) => file == other,
        (Item::CommitFile(file), Item::CommitHunk(other, _)) => file == other,
        (Item::CommitHunk(file, hunk), Item::CommitHunk(other, index)) => {
            (file, hunk) == (other, index)
        }
        _ => false,
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
        rows.extend(
            hunk_lines(hunk, None)
                .into_iter()
                .map(|(spans, wrap_indent)| ViewRow {
                    icon: None,
                    item: item.clone(),
                    spans,
                    wrap_indent,
                }),
        );
    }
    rows
}

/// One hunk's painted rows: its header band with its counts, then its body
/// lines word-decorated, each reserving its marker column when it wraps —
/// the hunk body every view that shows a diff paints.
///
/// A hunk sits flush left whichever view shows it. What a line belongs to is
/// read off its band above, not off an indent, and the columns an indent
/// would take are columns of diff that would otherwise wrap.
///
/// `fold` is the mark the header opens with, and whether it opens with one at
/// all: a commit folds a hunk on its own, so its headers carry a triangle
/// saying which way they stand, while the status view folds a hunk by folding
/// the file it belongs to, so a triangle there would mark a fold no key makes.
fn hunk_lines(hunk: &Hunk, fold: Option<bool>) -> Vec<(PageRow, usize)> {
    let (added, removed) = hunk.counts();
    let mark = fold.map(fold_glyph).unwrap_or_default();
    let mut rows = vec![(
        vec![PageSpan::new(
            PageStyle::Hunk,
            format!("{mark}{}  +{added} -{removed}", hunk.header),
        )],
        0,
    )];
    rows.extend(
        hunk.lines
            .iter()
            .zip(decorate(&hunk.lines))
            .map(|(line, segments)| {
                // A wrapped diff line continues past its marker, so the
                // spilled text lines up under the text, not the marker.
                (line_spans(line, &segments), MARKER_WIDTH)
            }),
    );
    rows
}

/// The painted rows of a whole-diff output: the file and hunk lines styled as
/// headings, the header detail receded, and every changed line word-decorated
/// the way an expanded file's hunks are, without the indent those sit under.
/// A `git show` carries commit detail above its first hunk, which is left as
/// ordinary text: nothing is a diff's body until a hunk opens.
/// The painted rows of a whole-diff output — the file and hunk lines styled as
/// headings, the header detail receded, and every changed line word-decorated
/// the way an expanded file's hunks are — with, per row, the column its
/// wrapped continuation starts at: a diff line reserves its marker's room. A
/// `git show` carries commit detail above its first hunk, which is left as
/// ordinary text: nothing is a diff's body until a hunk opens.
pub fn diff_rows(lines: &[String]) -> (Vec<PageRow>, Vec<usize>) {
    let mut rows: Vec<PageRow> = Vec::with_capacity(lines.len());
    let mut wrap_indents: Vec<usize> = Vec::with_capacity(lines.len());
    let mut in_hunk = false;
    let mut block = 0..0;
    for (index, line) in lines.iter().enumerate() {
        if in_hunk && is_changed(line) {
            if block.is_empty() {
                block = index..index;
            }
            block.end = index + 1;
            continue;
        }
        let flushed = block.clone();
        rows.extend(
            lines[flushed.clone()]
                .iter()
                .zip(decorate(&lines[flushed.clone()]))
                .map(|(line, segments)| line_spans(line, &segments)),
        );
        wrap_indents.extend(std::iter::repeat_n(MARKER_WIDTH, flushed.len()));
        block = 0..0;
        rows.push(vec![line_span(lines, index, in_hunk)]);
        // A hunk's context line carries the same one-column marker the
        // changed lines do, and its wraps reserve it the same way.
        wrap_indents.push(if in_hunk && line.starts_with(' ') {
            MARKER_WIDTH
        } else {
            0
        });
        in_hunk |= line.starts_with(HUNK_MARK);
    }
    let flushed = block.len();
    rows.extend(
        lines[block.clone()]
            .iter()
            .zip(decorate(&lines[block]))
            .map(|(line, segments)| line_spans(line, &segments)),
    );
    wrap_indents.extend(std::iter::repeat_n(MARKER_WIDTH, flushed));
    (rows, wrap_indents)
}

/// [`diff_rows`] as view rows, for the output view to paint. A whole-diff view
/// has nothing to act on at point, so every row stands for [`Item::None`].
pub fn diff_view_rows(lines: &[String]) -> Vec<ViewRow> {
    let (spans, wrap_indents) = diff_rows(lines);
    spans
        .into_iter()
        .zip(wrap_indents)
        .map(|(spans, wrap_indent)| ViewRow {
            icon: None,
            item: Item::None,
            spans,
            wrap_indent,
        })
        .collect()
}

/// A block of the status view that folds as a unit.
///
/// The file sections fold by which section they are; the rest are one of a
/// kind. What folds is what a heading heads, so every variant here answers to
/// one [`Item`] the cursor can sit on.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Block {
    /// One of the change sections, by which one.
    Files(Section),
    /// The recent commits.
    Recent,
    /// The stashes.
    Stashes,
    /// The commits the upstream has and this branch does not.
    Unpulled,
}

/// The block a row's heading folds, or `None` where the row heads no block.
pub fn fold_block(item: &Item) -> Option<Block> {
    match item {
        Item::Heading(section) => Some(Block::Files(*section)),
        Item::RecentHeading | Item::Commit(_) => Some(Block::Recent),
        Item::StashHeading | Item::Stash(_) => Some(Block::Stashes),
        Item::UnpulledHeading => Some(Block::Unpulled),
        _ => None,
    }
}

/// Everything the status view draws, gathered from git by the page.
///
/// A struct rather than a parameter list: the view is the sum of half a dozen
/// separate reads, and each one that arrives should not shuffle the order of
/// the others at every call site.
#[derive(Clone, Copy)]
pub struct StatusContent<'a> {
    /// The working tree, as `git status` reports it.
    pub status: &'a Status,
    /// The tail of the log, for the recent-commits section.
    pub commits: &'a [Commit],
    /// What the upstream has and this branch does not, newest first.
    pub unpulled: &'a [Commit],
    /// The stashes, newest first.
    pub stashes: &'a [Stash],
    /// What git is part-way through, when it is part-way through anything.
    pub progress: Option<&'a Progress>,
    /// The repository's path, drawn on the `Repo:` line.
    pub root: Option<&'a Path>,
    /// What the last command reported, drawn under the header.
    pub message: Option<&'a str>,
    /// The current Unix time, which the commit rows measure ages against.
    pub now: i64,
}

/// How far a commit is opened, one level at a time: the heading over its
/// files, then each file, then each hunk.
#[derive(Clone, Copy, Debug)]
pub struct CommitFolds<'a> {
    /// Whether the heading is shut, which hides every file under it.
    pub shut: bool,
    /// Files showing no diff, by their index in the commit.
    pub files: &'a HashSet<usize>,
    /// `(file, hunk)` pairs showing only their header.
    pub hunks: &'a HashSet<(usize, usize)>,
}

/// The rows of a commit's content: the summary `git show` opens with —
/// identity, author, date, message — then a heading saying how many files it
/// touched, then each file's changes under a band of its own, carrying the
/// file's icon, with the same hunk bodies the status view paints for the
/// working tree. A commit then reads the way the tree does, not the way a
/// patch does: bands and word-decorated lines, no `diff --git` or `index`
/// noise.
///
/// `folds` says how far the reader has opened it, so the same content draws at
/// whatever depth they left it at.
pub fn commit_view_rows(content: &CommitContent, folds: CommitFolds) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (index, line) in content.summary.iter().enumerate() {
        rows.push(ViewRow {
            icon: None,
            item: Item::None,
            spans: summary_row(line, index == 0),
            wrap_indent: 0,
        });
    }
    // The heading the files hang off, counted the way the status view counts
    // its sections. A commit with no file at all — a merge, or an empty one —
    // has nothing to head.
    if content.files.is_empty() {
        return rows;
    }
    rows.push(heading_row(
        CHANGES_TITLE,
        content.files.len(),
        folds.shut,
        Item::CommitChanges,
    ));
    if folds.shut {
        return rows;
    }
    for (file_index, file) in content.files.iter().enumerate() {
        let shut = folds.files.contains(&file_index);
        rows.push(commit_file_row(file, file_index, shut));
        if shut {
            continue;
        }
        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            let item = Item::CommitHunk(file_index, hunk_index);
            let hunk_shut = folds.hunks.contains(&(file_index, hunk_index));
            let lines = hunk_lines(hunk, Some(hunk_shut));
            // A folded hunk keeps its header, which carries the counts, and
            // drops the body: enough to say what is there without showing it.
            let take = if hunk_shut { 1 } else { lines.len() };
            rows.extend(
                lines
                    .into_iter()
                    .take(take)
                    .map(|(spans, wrap_indent)| ViewRow {
                        icon: None,
                        item: item.clone(),
                        spans,
                        wrap_indent,
                    }),
            );
        }
        // A file with no hunks — binary, a mode change — still says what
        // happened to it: git's own note under the band.
        if let Some(note) = &file.note {
            rows.push(ViewRow {
                icon: None,
                item: Item::CommitFile(file_index),
                spans: vec![PageSpan::new(PageStyle::Dim, note.clone())],
                wrap_indent: 0,
            });
        }
    }
    rows
}

/// One line of the summary `git show` opens a commit with, dedented: the
/// identity line as a heading, the message as ordinary text, and the rest —
/// author, dates, the change totals — receded.
///
/// git indents a commit's message by [`MESSAGE_INDENT`] and its shortstat by
/// one column; both come off, so the summary starts where every other row
/// does. Only that prefix comes off a message line, so indentation the author
/// wrote into the message survives.
fn summary_row(line: &str, identity: bool) -> PageRow {
    let message = line.starts_with(MESSAGE_INDENT);
    let text = match message {
        true => &line[MESSAGE_INDENT.len()..],
        false => line.trim_start(),
    };
    if identity {
        vec![PageSpan::new(PageStyle::Header, text)]
    } else if message || text.is_empty() {
        vec![PageSpan::plain(text)]
    } else {
        vec![PageSpan::new(PageStyle::Dim, text)]
    }
}

/// A file's band in a commit: its fold mark, what the commit did to it, its
/// icon, its path, and what its hunks add and remove — the row the status view
/// gives a file, standing in for the `diff --git` and `---`/`+++` lines it
/// replaces.
///
/// Bright while its diff shows beneath it and receded once folded shut, the same
/// way the working tree's file rows read, so one habit covers both views.
fn commit_file_row(file: &CommitFile, index: usize, folded: bool) -> ViewRow {
    let reserved = " ".repeat(PageIcon::WIDTH + 1);
    let band = if folded {
        PageStyle::SectionFolded
    } else {
        PageStyle::Section
    };
    let mark = fold_glyph(folded);
    ViewRow {
        icon: Some(PageIcon {
            col: fold_width() + STATUS_WIDTH,
            glyph: FILE_GLYPH,
            kind: PageIconKind::File {
                name: leaf_name(&file.path).to_string(),
            },
            row: 0,
        }),
        item: Item::CommitFile(index),
        spans: vec![
            PageSpan::new(
                change_style(file.code),
                format!("{mark}{:<STATUS_WIDTH$}", status_label(file.code)),
            ),
            PageSpan::new(band, format!("{reserved}{}", file.path)),
            PageSpan::new(PageStyle::Dim, format!(" ({})", file.hunks.len())),
        ],
        wrap_indent: 0,
    }
}

/// What a commit did to a file, written out. `git show` says it in a letter,
/// which has to be looked up to be read; the band says the word the letter
/// stands for instead, the way magic-vscode's change list does. A letter the
/// parser does not produce reads as a plain change, which is the weakest
/// thing any of them means.
/// The hue a change's name and fold mark wear, by what the change was: the
/// colors magic-vscode gives its own change list, which read as a legend once
/// learned — green arriving, red leaving, blue edited in place, yellow moved,
/// magenta contested. A copy and an untracked file are both new where they
/// stand, and a type change is a modification, so they share those. Anything
/// else is painted as a modification, the weakest thing a code can mean.
fn change_style(code: char) -> PageStyle {
    match code {
        '?' | 'A' | 'C' => PageStyle::ChangeAdded,
        'D' => PageStyle::ChangeDeleted,
        'R' => PageStyle::ChangeRenamed,
        'U' => PageStyle::ChangeConflict,
        _ => PageStyle::ChangeModified,
    }
}

fn status_label(code: char) -> &'static str {
    match code {
        'A' => "new file",
        'D' => "deleted",
        'M' => "modified",
        'R' => "renamed",
        _ => "changed",
    }
}

/// Whether a line carries a hunk's changed text: the removed, added, and
/// no-newline-marker lines a word diff pairs up. Context lines are left out,
/// since a pairing never reaches across one.
fn is_changed(line: &str) -> bool {
    line.starts_with('-') || line.starts_with('+') || line.starts_with('\\')
}

/// The single span a line outside a changed block paints as: a file's band
/// for the file line, a hunk's band for the hunk line, receded for header
/// detail and hunk context, and ordinary text for anything else, which is a
/// `git show`'s commit detail.
fn line_span(lines: &[String], index: usize, in_hunk: bool) -> PageSpan {
    let line = &lines[index];
    if line.starts_with(FILE_MARK) {
        return PageSpan::new(PageStyle::Section, line.clone());
    }
    if line.starts_with(HUNK_MARK) {
        return PageSpan::new(PageStyle::Hunk, line.clone());
    }
    // Inside a hunk, a space-prefixed line is context: ordinary text, since
    // it is the code the changes around it are read against. What carries the
    // eye is the tint on the lines that did change, not a fade on the ones
    // that did not.
    if in_hunk && line.starts_with(' ') {
        return PageSpan::new(PageStyle::Normal, line.clone());
    }
    // The file-pair lines only count as header when they truly pair: a
    // removed line of body text can itself begin `-- `.
    let paired = (line.starts_with("--- ")
        && lines
            .get(index + 1)
            .is_some_and(|next| next.starts_with("+++ ")))
        || (line.starts_with("+++ ") && index > 0 && lines[index - 1].starts_with("--- "));
    let detail = paired || DETAIL_MARKS.iter().any(|mark| line.starts_with(mark));
    let style = if detail {
        PageStyle::Dim
    } else {
        PageStyle::Normal
    };
    PageSpan::new(style, line.clone())
}

/// One diff line as spans: the whole line wears its side's tint, and the
/// stretches the edit touched wear it harder, so the eye lands on what
/// actually changed. A line the word diff says nothing about paints as one
/// span, all of it in its side's tint.
fn line_spans(line: &str, segments: &[Segment]) -> PageRow {
    let (base, edit) = line_styles(line);
    let marker = line.chars().next().unwrap_or(' ');
    let mut spans = PageRow::new();
    push_span(&mut spans, base, &marker.to_string());
    for segment in segments {
        let style = if segment.changed { edit } else { base };
        push_span(&mut spans, style, &segment.text);
    }
    spans
}

/// Add a span, joining it with the previous one when they share a style, so a
/// line paints as few runs as it can.
fn push_span(spans: &mut PageRow, style: PageStyle, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            last.text.push_str(text);
            return;
        }
    }
    spans.push(PageSpan::new(style, text));
}

/// The two styles one diff line wears: the tint all of it carries as its
/// side, and the one its edited words carry harder. Added lines read as
/// arriving, removed as leaving, and context as neither.
fn line_styles(line: &str) -> (PageStyle, PageStyle) {
    match line.chars().next() {
        Some('+') => (PageStyle::Added, PageStyle::AddedEdit),
        Some('-') => (PageStyle::Removed, PageStyle::RemovedEdit),
        Some(_) | None => (PageStyle::Normal, PageStyle::Normal),
    }
}

/// One commit's rows: its identity line, then its authorship line beneath.
///
/// Two lines rather than one because a single line has to choose between the
/// subject and the metadata, and both are wanted — so the subject gets the full
/// width and the author and age sit under it, dimmed, aligned past the hash so
/// the hashes and subjects each read as their own column.
fn commit_rows_decorated(commit: &Commit, now: i64, unpushed: bool) -> Vec<ViewRow> {
    let item = Item::Commit(commit.hash.clone());
    let mut spans = vec![
        PageSpan::new(PageStyle::Accent, format!("{} ", commit.hash)),
        PageSpan::new(PageStyle::Dim, GRAPH_MARK),
    ];
    if unpushed {
        spans.push(PageSpan::new(PageStyle::Unpushed, MARK_UNPUSHED));
    }
    for entry in &commit.refs {
        spans.push(PageSpan::new(
            ref_style(entry.kind),
            format!("{} ", entry.name),
        ));
    }
    // An unpushed commit carries its color across the whole subject, not just
    // the marker: the row says at a glance which commits the upstream has yet
    // to see, the way magic-vscode's log does.
    let subject = match unpushed {
        true => PageSpan::new(PageStyle::Unpushed, commit.subject.clone()),
        false => PageSpan::plain(commit.subject.clone()),
    };
    spans.push(subject);
    // The author line starts under the graph column: past the hash and the
    // space after it.
    let indent = commit.hash.chars().count() + 1;
    let authorship = ViewRow {
        icon: None,
        item: item.clone(),
        spans: vec![PageSpan::new(
            PageStyle::Dim,
            format!(
                "{}{}   {}",
                " ".repeat(indent),
                commit.author,
                reltime::ago(commit.time, now)
            ),
        )],
        wrap_indent: indent,
    };
    vec![
        ViewRow {
            icon: None,
            item,
            spans,
            wrap_indent: indent,
        },
        authorship,
    ]
}

/// How loudly a ref beside a commit is drawn, by what kind of ref it is.
fn ref_style(kind: RefKind) -> PageStyle {
    match kind {
        RefKind::Head => PageStyle::RefHead,
        RefKind::Local => PageStyle::RefLocal,
        RefKind::Remote => PageStyle::RefRemote,
        RefKind::Tag => PageStyle::RefTag,
    }
}

/// The rows of the log view: which repository this is, then every commit the way
/// the status view's tail draws them, then the hint that more can be loaded.
pub fn log_rows(commits: &[Commit], root: Option<&Path>, now: i64, more: bool) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    if let Some(root) = root {
        rows.push(label_row(
            LABEL_REPO,
            vec![PageSpan::new(
                PageStyle::HeadingPlain,
                abbreviate_home(root),
            )],
        ));
        rows.push(blank_row());
    }
    rows.push(ViewRow {
        icon: None,
        item: Item::None,
        spans: vec![PageSpan::new(PageStyle::HeadingPlain, LOG_TITLE)],
        wrap_indent: 0,
    });
    for commit in commits {
        // Nothing in a log view is "unpushed": it lists whatever revisions were
        // asked for, which need not be the checked-out branch at all.
        rows.extend(commit_rows_decorated(commit, now, false));
    }
    if more {
        rows.push(blank_row());
        rows.push(ViewRow {
            icon: None,
            item: Item::None,
            spans: vec![PageSpan::new(PageStyle::Dim, LOG_MORE)],
            wrap_indent: 0,
        });
    }
    rows
}

fn blank_row() -> ViewRow {
    ViewRow {
        icon: None,
        item: Item::None,
        spans: Vec::new(),
        wrap_indent: 0,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tools::git::diff::parse_diff;
    use crate::tools::git::parse::Ref;

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

    fn open(_block: Block) -> bool {
        false
    }

    fn shut(_block: Block) -> bool {
        true
    }

    fn no_diffs(_file: &FileRow) -> Option<FileDiff> {
        None
    }

    /// A fixed "now" so relative ages in the rendered rows are deterministic.
    const NOW: i64 = 1_800_000_000;

    /// [`build`] with the arguments the view supplies at runtime pinned: no
    /// repository path (so the `Repo:` line is left off and row 0 is the `Head:`
    /// line) and a fixed clock.
    fn built(
        status: &Status,
        commits: &[Commit],
        collapsed: &dyn Fn(Block) -> bool,
        diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
    ) -> Vec<ViewRow> {
        build(content(status, commits), collapsed, diffs)
    }

    /// The view's content with everything the page reads separately left
    /// empty, and a fixed clock: what a test that is not about the stashes or
    /// the upstream draws.
    fn content<'a>(status: &'a Status, commits: &'a [Commit]) -> StatusContent<'a> {
        StatusContent {
            status,
            commits,
            unpulled: &[],
            stashes: &[],
            progress: None,
            root: None,
            message: None,
            now: NOW,
        }
    }

    /// A commit with only the fields a test cares about set.
    fn commit(hash: &str, subject: &str) -> Commit {
        Commit {
            hash: hash.to_string(),
            subject: subject.to_string(),
            author: "Someone".to_string(),
            time: NOW - 3_600,
            refs: Vec::new(),
        }
    }

    #[test]
    fn test_an_empty_section_gets_no_heading() {
        // A clean tree should not read as four empty headings.
        let rows = built(&status_with(Vec::new()), &[], &open, &no_diffs);
        assert!(!rows.iter().any(|row| matches!(row.item, Item::Heading(_))));
    }

    #[test]
    fn test_a_collapsed_section_keeps_its_heading_and_count() {
        let status = status_with(vec![
            file(Section::Staged, "a.rs", 'M'),
            file(Section::Staged, "b.rs", 'A'),
        ]);
        let rows = built(&status, &[], &shut, &no_diffs);
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
        let rows = built(&status_with(vec![renamed]), &[], &open, &no_diffs);
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
        let rows = built(&status, &[], &open, &no_diffs);
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
        let rows = built(&status, &[], &open, &no_diffs);
        let block: Vec<String> = rows.iter().map(text).collect();
        let head = block.iter().find(|line| line.starts_with("Head:")).unwrap();
        let merge = block
            .iter()
            .find(|line| line.starts_with("Merge:"))
            .unwrap();
        assert_eq!(head, "Head:     main", "the head line names the branch");
        assert_eq!(
            merge, "Merge:    origin/main ↑2 ↓1",
            "the merge line names the upstream and the divergence"
        );
    }

    #[test]
    fn test_the_header_labels_align_their_values() {
        // The block is read down its left edge, so every value starts in the
        // same column regardless of how long its label is.
        let status = status_with(Vec::new());
        let rows = built(&status, &[], &open, &no_diffs);
        for row in rows.iter().take(2) {
            let label = &row.spans[0].text;
            assert_eq!(
                label.chars().count(),
                LABEL_WIDTH,
                "label {label:?} is not padded to the value column"
            );
        }
    }

    #[test]
    fn test_a_branch_with_no_upstream_says_so() {
        let rows = built(&status_with(Vec::new()), &[], &open, &no_diffs);
        let merge = rows
            .iter()
            .map(text)
            .find(|line| line.starts_with("Merge:"))
            .expect("the merge line");
        assert_eq!(merge, format!("Merge:    {NO_UPSTREAM}"));
    }

    #[test]
    fn test_a_repository_with_no_commit_yet_says_so() {
        let status = Status {
            ahead: 0,
            behind: 0,
            branch: None,
            files: Vec::new(),
            upstream: None,
        };
        let rows = built(&status, &[], &open, &no_diffs);
        assert_eq!(text(&rows[0]), format!("Head:     {EMPTY_HEAD}"));
    }

    #[test]
    fn test_a_tag_on_head_shows_in_the_header() {
        let mut tagged = commit("abc1234", "ship it");
        tagged.refs = vec![
            Ref {
                kind: RefKind::Head,
                name: "main".to_string(),
            },
            Ref {
                kind: RefKind::Tag,
                name: "v1.4.0".to_string(),
            },
        ];
        let rows = built(&status_with(Vec::new()), &[tagged], &open, &no_diffs);
        let tag = rows
            .iter()
            .map(text)
            .find(|line| line.starts_with("Tag:"))
            .expect("the tag line");
        assert_eq!(tag, "Tag:      v1.4.0");
    }

    #[test]
    fn test_the_head_line_carries_the_head_commits_subject() {
        let rows = built(
            &status_with(Vec::new()),
            &[commit("abc1234", "fix the parser")],
            &open,
            &no_diffs,
        );
        assert_eq!(text(&rows[0]), "Head:     main fix the parser");
    }

    #[test]
    fn test_commits_carry_their_hash_for_acting_on() {
        let commits = vec![commit("abc1234", "do the thing")];
        let rows = built(&status_with(Vec::new()), &commits, &open, &no_diffs);
        assert!(rows
            .iter()
            .any(|row| row.item == Item::Commit("abc1234".to_string())));
    }

    #[test]
    fn test_a_commit_reads_as_an_identity_line_over_an_authorship_line() {
        let commits = vec![commit("abc1234", "do the thing")];
        let rows = built(&status_with(Vec::new()), &commits, &open, &no_diffs);
        let commit_rows: Vec<&ViewRow> = rows
            .iter()
            .filter(|row| row.item == Item::Commit("abc1234".to_string()))
            .collect();
        assert_eq!(commit_rows.len(), 2, "a commit takes two rows");
        assert_eq!(
            text(commit_rows[0]),
            "abc1234 * do the thing",
            "the identity line carries the hash, the graph mark, and the subject"
        );
        assert_eq!(
            text(commit_rows[1]),
            "        Someone   1 hour",
            "the authorship line sits under the graph column"
        );
        assert_eq!(
            commit_rows[1].wrap_indent, 8,
            "both rows wrap to the column their text starts at"
        );
    }

    #[test]
    fn test_refs_are_drawn_beside_a_commit_by_what_kind_they_are() {
        let mut decorated = commit("abc1234", "do the thing");
        decorated.refs = vec![
            Ref {
                kind: RefKind::Head,
                name: "main".to_string(),
            },
            Ref {
                kind: RefKind::Remote,
                name: "origin/main".to_string(),
            },
            Ref {
                kind: RefKind::Tag,
                name: "v1.0".to_string(),
            },
        ];
        let rows = built(&status_with(Vec::new()), &[decorated], &open, &no_diffs);
        let row = rows
            .iter()
            .find(|row| row.item == Item::Commit("abc1234".to_string()))
            .expect("the commit row");
        assert_eq!(text(row), "abc1234 * main origin/main v1.0 do the thing");
        let styles: Vec<PageStyle> = row.spans.iter().map(|span| span.style).collect();
        assert!(styles.contains(&PageStyle::RefHead), "got {styles:?}");
        assert!(styles.contains(&PageStyle::RefRemote), "got {styles:?}");
        assert!(styles.contains(&PageStyle::RefTag), "got {styles:?}");
    }

    #[test]
    fn test_the_commits_ahead_of_the_upstream_are_marked_unpushed() {
        // A reverse-chronological log puts the unpushed commits first, so the
        // count of commits ahead is also how many rows carry the mark.
        let status = Status {
            ahead: 1,
            behind: 0,
            branch: Some("main".to_string()),
            files: Vec::new(),
            upstream: Some("origin/main".to_string()),
        };
        let commits = vec![commit("newer12", "not pushed"), commit("older34", "pushed")];
        let rows = built(&status, &commits, &open, &no_diffs);
        let line = |hash: &str| -> String {
            rows.iter()
                .filter(|row| row.item == Item::Commit(hash.to_string()))
                .map(text)
                .next()
                .expect("the commit row")
        };
        assert!(
            line("newer12").contains(MARK_UNPUSHED),
            "got {:?}",
            line("newer12")
        );
        assert!(
            !line("older34").contains(MARK_UNPUSHED),
            "got {:?}",
            line("older34")
        );
    }

    #[test]
    fn test_the_log_view_heads_its_commits_and_offers_more() {
        let commits = vec![commit("abc1234", "do the thing")];
        let rows = log_rows(&commits, None, NOW, true);
        let lines: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(lines[0], LOG_TITLE, "the list is headed");
        assert_eq!(lines[1], "abc1234 * do the thing");
        assert_eq!(lines[2], "        Someone   1 hour");
        assert_eq!(
            lines.last().map(String::as_str),
            Some(LOG_MORE),
            "a log that can grow says so"
        );
    }

    #[test]
    fn test_hunk_lines_reserve_their_marker_column_when_wrapped() {
        let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n");
        let file = FileRow {
            path: "f".to_string(),
            section: Section::Staged,
        };
        let rows = hunk_rows(&file, &diff);
        // The hunk's header band wraps from the pane's edge like any row;
        // its body lines reserve the marker column their text sits after.
        assert_eq!(rows[0].wrap_indent, 0);
        assert_eq!(rows[1].wrap_indent, MARKER_WIDTH);
        assert_eq!(rows[2].wrap_indent, MARKER_WIDTH);
    }

    #[test]
    fn test_a_diff_outputs_lines_reserve_their_marker_column() {
        let lines: Vec<String> = "diff --git a/f b/f\nindex 123..456 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n"
            .lines()
            .map(str::to_string)
            .collect();
        let (_, indents) = diff_rows(&lines);
        // Nothing before the hunk wraps past a marker; the hunk's context and
        // changed lines all carry the one-column marker their wraps keep.
        assert_eq!(&indents[..5], &[0, 0, 0, 0, 0]);
        assert_eq!(&indents[5..], &[1, 1, 1]);
    }

    fn shown(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    /// The commit view with everything open. The page itself first draws one
    /// shut, and opens it from there.
    fn commit_view(lines: &[String]) -> Vec<ViewRow> {
        commit_view_rows(
            &CommitContent::parse(lines),
            opened(&HashSet::new(), &HashSet::new()),
        )
    }

    /// The folds of a commit whose heading is open, with `files` and `hunks`
    /// shut under it.
    fn opened<'a>(
        files: &'a HashSet<usize>,
        hunks: &'a HashSet<(usize, usize)>,
    ) -> CommitFolds<'a> {
        CommitFolds {
            shut: false,
            files,
            hunks,
        }
    }

    #[test]
    fn test_a_commits_summary_reads_with_its_message_loudest() {
        let lines = shown(&[
            "commit abc1234567890 (HEAD -> main)",
            "Author: Someone <a@b.c>",
            "Date:   2026-09-15 12:00:00 +0000",
            "",
            "    do the thing",
            "",
        ]);
        let rows = commit_view(&lines);
        assert_eq!(
            rows[0].spans,
            vec![PageSpan::new(
                PageStyle::Header,
                "commit abc1234567890 (HEAD -> main)"
            )],
            "the identity line heads the view"
        );
        assert_eq!(
            rows[1].spans,
            vec![PageSpan::new(PageStyle::Dim, "Author: Someone <a@b.c>")],
            "the headers recede"
        );
        assert_eq!(
            rows[4].spans,
            vec![PageSpan::plain("do the thing")],
            "the message loses the four columns git indented it by"
        );
        assert!(rows.iter().all(|row| row.wrap_indent == 0));
        assert!(
            rows.iter().all(|row| row.item == Item::None),
            "nothing in the summary is actionable"
        );
    }

    #[test]
    fn test_a_commits_files_hang_off_one_counted_heading() {
        // The commit reads as the status view does: a heading saying how many
        // files it touched, each band saying how many hunks it holds. Both
        // count in the units the keys act in, where git's own line totals
        // count something no key here opens.
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/a.rs b/a.rs",
            "--- a/a.rs",
            "+++ b/a.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            "@@ -9 +9 @@",
            "-second",
            "+third",
            "diff --git a/b.rs b/b.rs",
            "--- a/b.rs",
            "+++ b/b.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
        ]);
        let rows = commit_view(&lines);
        let line = |needle: &str| -> String {
            rows.iter()
                .map(text)
                .find(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle}"))
        };
        assert_eq!(line(CHANGES_TITLE), "▼ Changes (2)", "two files under it");
        assert!(line("a.rs").ends_with(" (2)"), "got {:?}", line("a.rs"));
        assert!(line("b.rs").ends_with(" (1)"), "got {:?}", line("b.rs"));
        assert!(
            !rows.iter().any(|row| text(row).contains("changed,")),
            "and nothing said in lines changed"
        );

        // A commit with no file at all heads nothing.
        let bare = commit_view(&shown(&["commit abc", "", "    message"]));
        assert!(bare.iter().all(|row| !text(row).contains(CHANGES_TITLE)));
    }

    #[test]
    fn test_the_stashes_and_the_upstreams_commits_get_sections_of_their_own() {
        let status = Status {
            ahead: 0,
            behind: 2,
            branch: Some("main".to_string()),
            files: Vec::new(),
            upstream: Some("origin/main".to_string()),
        };
        let stashes = vec![
            Stash {
                name: "stash@{0}".to_string(),
                subject: "WIP on main: half a thing".to_string(),
            },
            Stash {
                name: "stash@{1}".to_string(),
                subject: "spike".to_string(),
            },
        ];
        let unpulled = vec![commit("aaa1111", "theirs")];
        let rows = build(
            StatusContent {
                stashes: &stashes,
                unpulled: &unpulled,
                ..content(&status, &[])
            },
            &open,
            &no_diffs,
        );
        let lines: Vec<String> = rows.iter().map(text).collect();

        assert!(
            lines.iter().any(|line| line == "▼ Stashes (2)"),
            "got {lines:?}"
        );
        assert!(lines
            .iter()
            .any(|line| line.contains("stash@{0} WIP on main")));
        assert!(
            lines
                .iter()
                .any(|line| line == "▼ Unpulled from origin/main (1)"),
            "the section names where the commits came from, got {lines:?}"
        );
        assert!(lines.iter().any(|line| line.contains("theirs")));
    }

    #[test]
    fn test_every_heading_folds_the_block_it_heads() {
        let status = Status {
            ahead: 0,
            behind: 1,
            branch: Some("main".to_string()),
            files: vec![file(Section::Staged, "s.rs", 'M')],
            upstream: Some("origin/main".to_string()),
        };
        let stashes = vec![Stash {
            name: "stash@{0}".to_string(),
            subject: "spike".to_string(),
        }];
        let unpulled = vec![commit("aaa1111", "theirs")];
        let rows = build(
            StatusContent {
                stashes: &stashes,
                unpulled: &unpulled,
                ..content(&status, &[commit("bbb2222", "mine")])
            },
            &shut,
            &no_diffs,
        );
        let lines: Vec<String> = rows.iter().map(text).collect();

        // Every block's own heading survives its fold, and nothing under any
        // of them does.
        for heading in ["Staged changes", "Stashes", "Unpulled from", RECENT_TITLE] {
            assert!(
                lines.iter().any(|line| line.contains(heading)),
                "{heading} keeps its heading, got {lines:?}"
            );
        }
        // By hash for the commits: their subjects also read on the `Head:`
        // line, which is not part of any block.
        for hidden in ["s.rs", "stash@{0}", "aaa1111", "bbb2222"] {
            assert!(
                !lines.iter().any(|line| line.contains(hidden)),
                "{hidden} is folded away, got {lines:?}"
            );
        }
        for item in [
            Item::Heading(Section::Staged),
            Item::StashHeading,
            Item::UnpulledHeading,
            Item::RecentHeading,
        ] {
            assert!(fold_block(&item).is_some(), "{item:?} folds a block");
        }
    }

    #[test]
    fn test_an_unfinished_operation_says_what_it_is_and_how_to_finish_it() {
        // Nothing in a porcelain status says a rebase is open: without this
        // line the only sign of one is a section of conflicts and no reason.
        let progress = Progress {
            operation: Operation::Rebase,
            detail: "feature onto abc1234".to_string(),
            step: Some((3, 7)),
        };
        let status = status_with(Vec::new());
        let rows = build(
            StatusContent {
                progress: Some(&progress),
                ..content(&status, &[])
            },
            &open,
            &no_diffs,
        );
        let lines: Vec<String> = rows.iter().map(text).collect();

        assert!(
            lines.iter().any(|line| line.starts_with("State:")
                && line.contains("Rebasing feature onto abc1234")
                && line.contains("(3/7)")),
            "got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("r c continue")),
            "and the keys that finish it, got {lines:?}"
        );
    }

    #[test]
    fn test_a_section_heading_wears_the_hue_of_what_it_holds() {
        // The headings are what the eye lands on first, so each says which
        // section it is by color as well as by name, and the count beside it
        // stays receded.
        let files = vec![
            file(Section::Staged, "s.rs", 'M'),
            file(Section::Unstaged, "u.rs", 'M'),
            file(Section::Untracked, "n.rs", '?'),
            file(Section::Unmerged, "c.rs", 'U'),
        ];
        let rows = built(&status_with(files), &[], &open, &no_diffs);
        let heading = |section: Section| -> &ViewRow {
            rows.iter()
                .find(|row| row.item == Item::Heading(section))
                .unwrap_or_else(|| panic!("no heading for {section:?}"))
        };
        assert_eq!(
            heading(Section::Staged).spans[0].style,
            PageStyle::HeadingStaged
        );
        assert_eq!(
            heading(Section::Unstaged).spans[0].style,
            PageStyle::HeadingUnstaged
        );
        assert_eq!(
            heading(Section::Untracked).spans[0].style,
            PageStyle::HeadingUntracked
        );
        assert_eq!(
            heading(Section::Unmerged).spans[0].style,
            PageStyle::HeadingConflict
        );
        assert_eq!(
            heading(Section::Staged).spans[1].style,
            PageStyle::Dim,
            "the count stays receded"
        );

        // A heading over rows that are not a kind of change takes the hue
        // every other heading of a git view wears.
        let rows = built(
            &status_with(Vec::new()),
            &[commit("abc1234", "do the thing")],
            &open,
            &no_diffs,
        );
        let recent = rows
            .iter()
            .find(|row| row.item == Item::RecentHeading)
            .expect("the recent heading");
        assert_eq!(recent.spans[0].style, PageStyle::HeadingPlain);
    }

    #[test]
    fn test_an_unpushed_commit_carries_its_color_across_the_row() {
        let status = Status {
            ahead: 1,
            behind: 0,
            branch: Some("main".to_string()),
            files: Vec::new(),
            upstream: Some("origin/main".to_string()),
        };
        let commits = vec![commit("newer12", "not pushed"), commit("older34", "pushed")];
        let rows = built(&status, &commits, &open, &no_diffs);
        let styles = |hash: &str| -> Vec<PageStyle> {
            rows.iter()
                .find(|row| row.item == Item::Commit(hash.to_string()))
                .expect("the commit row")
                .spans
                .iter()
                .map(|span| span.style)
                .collect()
        };
        let ahead = styles("newer12");
        assert!(
            ahead.iter().filter(|s| **s == PageStyle::Unpushed).count() >= 2,
            "the marker and the subject both, got {ahead:?}"
        );
        assert!(
            styles("older34")
                .iter()
                .all(|style| *style != PageStyle::Unpushed),
            "a commit the upstream already has reads plainly"
        );
    }

    #[test]
    fn test_a_change_is_told_by_the_hue_its_name_wears() {
        // The name and the mark before it carry the change's own color, so a
        // commit's file list reads as a legend rather than as words to be
        // parsed one at a time. The path keeps the band's own color: it says
        // which file, not what happened to it.
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/kept.rs b/kept.rs",
            "--- a/kept.rs",
            "+++ b/kept.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            "diff --git a/gone.rs b/gone.rs",
            "deleted file mode 100644",
            "--- a/gone.rs",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-old",
            "diff --git a/new.rs b/new.rs",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/new.rs",
            "@@ -0,0 +1 @@",
            "+fresh",
        ]);
        let rows = commit_view(&lines);
        let hue = |needle: &str| -> PageStyle {
            rows.iter()
                .find(|row| text(row).contains(needle))
                .unwrap_or_else(|| panic!("no band for {needle}"))
                .spans[0]
                .style
        };
        assert_eq!(hue("kept.rs"), PageStyle::ChangeModified);
        assert_eq!(hue("gone.rs"), PageStyle::ChangeDeleted);
        assert_eq!(hue("new.rs"), PageStyle::ChangeAdded);

        // The working tree's own codes reach the same colors, including the
        // two the commit view never produces.
        assert_eq!(change_style('?'), PageStyle::ChangeAdded);
        assert_eq!(change_style('U'), PageStyle::ChangeConflict);
        assert_eq!(change_style('R'), PageStyle::ChangeRenamed);
    }

    #[test]
    fn test_a_folds_mark_points_the_way_the_row_stands() {
        // The mark is the only thing on a band that says whether what hangs
        // off it is showing, since a shut band and an open one otherwise read
        // the same, and it has to follow the fold rather than the row's kind.
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/f.rs b/f.rs",
            "--- a/f.rs",
            "+++ b/f.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
        ]);
        let content = CommitContent::parse(&lines);
        let band = |rows: &[ViewRow], needle: &str| -> String {
            text(
                rows.iter()
                    .find(|row| text(row).contains(needle))
                    .unwrap_or_else(|| panic!("no row for {needle}")),
            )
        };

        let open = commit_view_rows(&content, opened(&HashSet::new(), &HashSet::new()));
        assert!(band(&open, "f.rs").starts_with(GLYPH_EXPANDED));
        assert!(band(&open, "@@").starts_with(GLYPH_EXPANDED));

        let shut_hunk = HashSet::from([(0, 0)]);
        let hunk_shut = commit_view_rows(&content, opened(&HashSet::new(), &shut_hunk));
        assert!(
            band(&hunk_shut, "f.rs").starts_with(GLYPH_EXPANDED),
            "the file is still open"
        );
        assert!(
            band(&hunk_shut, "@@").starts_with(GLYPH_COLLAPSED),
            "its hunk is not"
        );

        let shut_file = HashSet::from([0]);
        let file_shut = commit_view_rows(&content, opened(&shut_file, &HashSet::new()));
        assert!(band(&file_shut, "f.rs").starts_with(GLYPH_COLLAPSED));
    }

    #[test]
    fn test_a_status_file_row_marks_whether_its_diff_is_showing() {
        let status = status_with(vec![file(Section::Staged, "f.rs", 'M')]);
        let shut = built(&status, &[], &open, &no_diffs);
        let row = shut
            .iter()
            .find(|row| matches!(row.item, Item::File(_)))
            .expect("the file row");
        assert!(
            text(row).starts_with(GLYPH_COLLAPSED),
            "a file whose diff has never been read reads as shut, got {:?}",
            text(row)
        );

        let diff = parse_diff("--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n");
        let with_diff = |_file: &FileRow| Some(diff.clone());
        let open_rows = built(&status, &[], &open, &with_diff);
        let row = open_rows
            .iter()
            .find(|row| matches!(row.item, Item::File(_)))
            .expect("the file row");
        assert!(
            text(row).starts_with(GLYPH_EXPANDED),
            "one showing its diff reads as open, got {:?}",
            text(row)
        );
    }

    #[test]
    fn test_a_bands_path_starts_at_one_column_whatever_happened_to_its_file() {
        // The change is written out, and the words are not all one length, so
        // the column a path starts at has to come from the padding rather than
        // from the word before it: a list of paths that steps in and out with
        // what happened to each one cannot be read down.
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/gone.rs b/gone.rs",
            "deleted file mode 100644",
            "--- a/gone.rs",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-old",
            "diff --git a/kept.rs b/kept.rs",
            "--- a/kept.rs",
            "+++ b/kept.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
        ]);
        let rows = commit_view(&lines);
        let band = |needle: &str| -> String {
            let opens_with = format!("{GLYPH_EXPANDED}{needle}");
            text(
                rows.iter()
                    .find(|row| text(row).starts_with(&opens_with))
                    .unwrap_or_else(|| panic!("no band saying {needle}")),
            )
        };
        let deleted = band("deleted");
        let modified = band("modified");
        assert_eq!(
            deleted.find("gone.rs"),
            modified.find("kept.rs"),
            "both paths start at the same column, got {deleted:?} and {modified:?}"
        );
    }

    #[test]
    fn test_each_file_gets_a_band_with_its_change_and_hunk_count() {
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/kept.rs b/kept.rs",
            "--- a/kept.rs",
            "+++ b/kept.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            "diff --git a/gone.txt b/added.txt",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/added.txt",
            "@@ -0,0 +1 @@",
            "+fresh",
        ]);
        let rows = commit_view(&lines);
        let kept = rows
            .iter()
            .find(|row| text(row).contains("kept.rs"))
            .expect("the kept file's band");
        assert_eq!(
            kept.spans[0],
            PageSpan::new(PageStyle::ChangeModified, "▼ modified    "),
            "the fold mark and the change's name lead the band, in the hue \
             that change wears"
        );
        assert_eq!(
            kept.spans[1],
            PageSpan::new(PageStyle::Section, "   kept.rs"),
            "the path follows on the band itself"
        );
        assert_eq!(
            kept.spans[2],
            PageSpan::new(PageStyle::Dim, " (1)"),
            "the band counts the hunks it holds"
        );
        assert_eq!(
            kept.icon,
            Some(PageIcon {
                col: fold_width() + STATUS_WIDTH,
                glyph: FILE_GLYPH,
                kind: PageIconKind::File {
                    name: "kept.rs".to_string()
                },
                row: 0,
            }),
            "a file in a commit carries its icon, the way the working tree's does"
        );
        assert_eq!(
            kept.item,
            Item::CommitFile(0),
            "the band knows which file it is, so a fold key can act on it"
        );
        let added = rows
            .iter()
            .find(|row| text(row).contains("added.txt"))
            .expect("the added file's band");
        assert_eq!(
            added.spans[0],
            PageSpan::new(PageStyle::ChangeAdded, "▼ new file    "),
            "a new file says so in the words git writes it as"
        );
        assert_eq!(
            added.icon.as_ref().map(|icon| &icon.kind),
            Some(&PageIconKind::File {
                name: "added.txt".to_string()
            }),
            "the icon is named for the file, so the icon set can match it"
        );
        // The hunk bodies carry the marker column their wraps reserve.
        let body = rows
            .iter()
            .find(|row| text(row).contains("fresh"))
            .expect("the added line");
        assert!(body
            .spans
            .iter()
            .any(|span| span.style == PageStyle::AddedEdit));
    }

    #[test]
    fn test_an_icon_leaves_its_columns_clear_in_the_band_text() {
        // The icon is drawn over the row, so the text must not put glyphs where
        // the artwork lands or the two overlap.
        let rows = commit_view(&shown(&[
            "diff --git a/src/deep/path/file.rs b/src/deep/path/file.rs",
            "--- a/src/deep/path/file.rs",
            "+++ b/src/deep/path/file.rs",
            "@@ -1 +1 @@",
            "-a",
            "+b",
        ]));
        let band = rows
            .iter()
            .find(|row| row.item == Item::CommitFile(0))
            .expect("the band");
        let icon = band.icon.as_ref().expect("its icon");
        let text = text(band);
        let reserved: String = text.chars().skip(icon.col).take(PageIcon::WIDTH).collect();
        assert_eq!(
            reserved, "  ",
            "the icon's columns are blank in the text, got {text:?}"
        );
        assert_eq!(
            icon.kind,
            PageIconKind::File {
                name: "file.rs".to_string()
            },
            "the icon is named for the leaf, not the whole path"
        );
    }

    #[test]
    fn test_folding_a_file_hides_its_hunks_but_keeps_its_band() {
        let lines = shown(&[
            "diff --git a/f.rs b/f.rs",
            "--- a/f.rs",
            "+++ b/f.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
        ]);
        let content = CommitContent::parse(&lines);
        let open = commit_view_rows(&content, opened(&HashSet::new(), &HashSet::new()));
        let shut_file = HashSet::from([0]);
        let shut = commit_view_rows(&content, opened(&shut_file, &HashSet::new()));

        assert!(open.iter().any(|row| text(row).contains("+new")));
        assert!(
            !shut.iter().any(|row| text(row).contains("+new")),
            "a folded file shows none of its diff"
        );
        let band = shut
            .iter()
            .find(|row| row.item == Item::CommitFile(0))
            .expect("the band survives the fold");
        assert_eq!(
            band.spans[1].style,
            PageStyle::SectionFolded,
            "a shut file's path recedes, the way the working tree's does"
        );
        assert!(
            text(band).contains("f.rs"),
            "the band still names the file so it can be unfolded"
        );
        assert_eq!(
            open.iter()
                .find(|row| row.item == Item::CommitFile(0))
                .map(|row| row.spans[1].style),
            Some(PageStyle::Section),
            "an open file's path is the bright one"
        );
    }

    #[test]
    fn test_folding_a_hunk_keeps_its_header_and_drops_its_body() {
        let lines = shown(&[
            "diff --git a/f.rs b/f.rs",
            "--- a/f.rs",
            "+++ b/f.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            "@@ -9 +9 @@",
            "-second",
            "+third",
        ]);
        let content = CommitContent::parse(&lines);
        // Shut the first hunk only, so the second proves folding is per-hunk.
        let shut_hunk = HashSet::from([(0, 0)]);
        let rows = commit_view_rows(&content, opened(&HashSet::new(), &shut_hunk));
        let lines: Vec<String> = rows.iter().map(text).collect();

        assert!(
            lines.iter().any(|line| line.contains("@@ -1 +1 @@")),
            "a folded hunk keeps its header, which carries the counts"
        );
        assert!(
            !lines.iter().any(|line| line.contains("-old")),
            "but shows none of its body, got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("+third")),
            "the hunk left open still shows its body"
        );
    }

    #[test]
    fn test_every_view_paints_its_diff_flush_left() {
        let lines = shown(&[
            "commit abc",
            "",
            "    message",
            "diff --git a/f.rs b/f.rs",
            "--- a/f.rs",
            "+++ b/f.rs",
            "@@ -1,2 +1,2 @@",
            " context",
            "-old",
            "+new",
        ]);
        let rows = commit_view(&lines);

        // The hunk header opens at column 0, not under an indent.
        let (header_at, header) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| text(row).contains("@@"))
            .expect("the hunk header");
        assert_eq!(
            text(header),
            "▼ @@ -1,2 +1,2 @@  +1 -1",
            "a commit's hunk header starts at the left margin, behind the mark \
             saying it folds"
        );

        // Body lines start with their bare marker, no leading blanks.
        for (offset, expected) in [(1, " context"), (2, "-old"), (3, "+new")] {
            let row = &rows[header_at + offset];
            assert_eq!(
                text(row),
                expected,
                "a commit's diff body line carries only its marker, no indent"
            );
            assert_eq!(
                row.wrap_indent, MARKER_WIDTH,
                "its wrapped continuation still clears just the marker"
            );
        }

        // The working-tree view paints the same hunk the same way: what a line
        // belongs to is read off the band above it, not off an indent.
        let diff = parse_diff("--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n");
        let file = FileRow {
            path: "f.rs".to_string(),
            section: Section::Staged,
        };
        let tree = hunk_rows(&file, &diff);
        assert_eq!(
            text(&tree[0]),
            "@@ -1,2 +1,2 @@  +1 -1",
            "the status view's hunk header starts at the left margin too, and \
             carries no mark: folding there acts on the file, not the hunk"
        );
        assert_eq!(
            text(&tree[1]),
            " context",
            "its body carries only its marker"
        );
        assert_eq!(tree[1].wrap_indent, MARKER_WIDTH);
    }

    #[test]
    fn test_a_binary_file_shows_gits_note_under_its_band() {
        let lines = shown(&[
            "commit abc",
            "diff --git a/logo.png b/logo.png",
            "index 111..222 100644",
            "Binary files a/logo.png and b/logo.png differ",
        ]);
        let rows = commit_view(&lines);
        let band = rows
            .iter()
            .find(|row| text(row).contains("logo.png"))
            .expect("the band");
        assert_eq!(band.spans[1].style, PageStyle::Section);
        assert_eq!(
            rows.last().map(|row| row.spans.clone()),
            Some(vec![PageSpan::new(
                PageStyle::Dim,
                "Binary files a/logo.png and b/logo.png differ"
            )]),
            "the note stands under the band rather than nothing at all"
        );
    }

    #[test]
    fn test_a_replaced_line_paints_only_its_edited_words_loudly() {
        // The whole line wears its side's tint, and only the edited words
        // wear it harder.
        let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-    old();\n+    new();\n");
        let file = FileRow {
            path: "f".to_string(),
            section: Section::Staged,
        };
        let rows = hunk_rows(&file, &diff);
        let removed = rows
            .iter()
            .find(|row| text(row).contains("old();"))
            .expect("the removed line");
        assert_eq!(
            removed.spans,
            vec![
                PageSpan::new(PageStyle::Removed, "-    "),
                PageSpan::new(PageStyle::RemovedEdit, "old"),
                PageSpan::new(PageStyle::Removed, "();"),
            ]
        );
        let added = rows
            .iter()
            .find(|row| text(row).contains("new();"))
            .expect("the added line");
        assert_eq!(
            added.spans,
            vec![
                PageSpan::new(PageStyle::Added, "+    "),
                PageSpan::new(PageStyle::AddedEdit, "new"),
                PageSpan::new(PageStyle::Added, "();"),
            ]
        );
        assert_eq!(text(added), "+    new();");
    }

    #[test]
    fn test_a_line_without_a_counterpart_paints_wholly_as_the_edit() {
        // A pure insertion has no removed text to word-diff against, so every
        // word of it is the edit and wears the harder tint; only its marker
        // stays structural.
        let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n keep\n+    added();\n");
        let file = FileRow {
            path: "f".to_string(),
            section: Section::Staged,
        };
        let rows = hunk_rows(&file, &diff);
        let added = rows
            .iter()
            .find(|row| text(row).contains("added();"))
            .expect("the added line");
        assert_eq!(
            added.spans,
            vec![
                PageSpan::new(PageStyle::Added, "+"),
                PageSpan::new(PageStyle::AddedEdit, "    added();"),
            ]
        );
    }

    #[test]
    fn test_a_diff_output_styles_headers_and_decorates_changes() {
        let lines: Vec<String> = "diff --git a/f b/f\nindex 123..456 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n"
            .lines()
            .map(str::to_string)
            .collect();
        let (rows, _) = diff_rows(&lines);
        assert_eq!(
            rows[0],
            vec![PageSpan::new(PageStyle::Section, "diff --git a/f b/f")]
        );
        assert_eq!(
            rows[1],
            vec![PageSpan::new(PageStyle::Dim, "index 123..456 100644")]
        );
        assert_eq!(rows[2], vec![PageSpan::new(PageStyle::Dim, "--- a/f")]);
        assert_eq!(rows[3], vec![PageSpan::new(PageStyle::Dim, "+++ b/f")]);
        assert_eq!(
            rows[4],
            vec![PageSpan::new(PageStyle::Hunk, "@@ -1,3 +1,3 @@")]
        );
        // Context is ordinary text, where the header detail above it recedes,
        // and the changed lines carry only their edit in the harder tint.
        assert_eq!(
            rows[5],
            vec![PageSpan::new(PageStyle::Normal, " fn main() {")]
        );
        assert_eq!(
            rows[6],
            vec![
                PageSpan::new(PageStyle::Removed, "-    "),
                PageSpan::new(PageStyle::RemovedEdit, "old"),
                PageSpan::new(PageStyle::Removed, "();"),
            ]
        );
        assert_eq!(
            rows[7],
            vec![
                PageSpan::new(PageStyle::Added, "+    "),
                PageSpan::new(PageStyle::AddedEdit, "new"),
                PageSpan::new(PageStyle::Added, "();"),
            ]
        );
    }

    #[test]
    fn test_a_shows_commit_detail_stays_plain_until_its_first_hunk() {
        // `git show` answers with commit metadata above the diff, which stays
        // plain: nothing is a diff's body until a hunk opens.
        let lines: Vec<String> = "commit abc123\nAuthor: A <a@b>\n\n    fix the thing\n\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n"
            .lines()
            .map(str::to_string)
            .collect();
        let (rows, _) = diff_rows(&lines);
        assert_eq!(rows[0], vec![PageSpan::plain("commit abc123")]);
        assert_eq!(rows[3], vec![PageSpan::plain("    fix the thing")]);
        assert_eq!(
            rows[9],
            vec![
                PageSpan::new(PageStyle::Removed, "-"),
                PageSpan::new(PageStyle::RemovedEdit, "old"),
            ]
        );
    }
}
