//! Painting the status view: the header, each section, and the entries under
//! it, plus the mapping from a row back to what it stands for — and the
//! whole-diff output a key can put in the view's place.

use std::collections::HashSet;
use std::path::Path;

use crate::model::page::{PageIcon, PageIconKind, PageRow, PageSpan, PageStyle};

use super::commit::{CommitContent, CommitFile};
use super::diff::{FileDiff, Hunk};
use super::parse::{Commit, FileStatus, RefKind, Section, Status};
use super::reltime;
use super::words::{decorate, Segment};

// ========================================================================
// Constants
// ========================================================================

/// Heading of the recent-commits section.
const RECENT_TITLE: &str = "Recent commits";

/// Marks a section whose entries are hidden.
const GLYPH_COLLAPSED: &str = "▶ ";

/// Marks a section whose entries are shown.
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

/// What the log view says when more commits can be loaded.
const LOG_MORE: &str = "Press '+' to display more commits";

/// Indent every entry sits at, under its heading.
const ENTRY_INDENT: &str = "  ";

/// The one column a diff line's marker takes, which a wrapped continuation
/// starts past.
const MARKER_WIDTH: usize = 1;

/// The four-space indent `git show` gives a commit's message lines.
const MESSAGE_INDENT: &str = "    ";

/// Indent a diff line sits at, under its file, in the status view — where a
/// hunk hangs off a file that hangs off a section, and the indent is what
/// shows that nesting.
const HUNK_INDENT: &str = "    ";

/// Indent a diff line sits at in a commit view: none. A commit shows one
/// commit's files with no section tree above them, so there is no nesting for
/// an indent to convey, and giving the columns back to the code means fewer
/// long lines wrap.
const COMMIT_HUNK_INDENT: &str = "";

/// Column the change code is drawn in, before the path.
const CODE_WIDTH: usize = 2;

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
/// then the recent commits.
///
/// `root` is the repository's path, drawn on the `Repo:` line; `now` is the
/// current Unix time, which the commit rows measure their age against. Both are
/// passed in rather than read here so this stays a pure function of what it is
/// given.
pub fn build(
    status: &Status,
    commits: &[Commit],
    root: Option<&Path>,
    now: i64,
    collapsed: &dyn Fn(Option<Section>) -> bool,
    message: Option<&str>,
    diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
) -> Vec<ViewRow> {
    let mut rows = header_block(status, commits, root, message);
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
        let shut = collapsed(Some(section));
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
    if !commits.is_empty() {
        let shut = collapsed(None);
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
    message: Option<&str>,
) -> Vec<ViewRow> {
    let mut rows = Vec::new();
    if let Some(root) = root {
        rows.push(label_row(
            LABEL_REPO,
            vec![PageSpan::plain(abbreviate_home(root))],
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
            PageStyle::Accent,
            format!(" ↑{} ↓{}", status.ahead, status.behind),
        ));
    }
    rows.push(label_row(LABEL_MERGE, merge));
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
        PageStyle::Header,
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
        wrap_indent: 0,
    }
}

/// A file's row: a band the file's changes sit under, bright while they show
/// beneath it and receded once folded shut. The status letter stays on the
/// band — it says what changed, where the icon says what kind of file it is —
/// and the icon's columns follow it, blank, with one more keeping the name
/// off the artwork.
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
        spans: vec![PageSpan::new(
            band,
            format!("{ENTRY_INDENT}{:<CODE_WIDTH$}{reserved}{name}", file.code),
        )],
        wrap_indent: 0,
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
            hunk_lines(hunk, HUNK_INDENT)
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
/// `indent` is the column the hunk sits at, which differs by view: the status
/// view nests a hunk under its file inside a section tree, so it passes
/// [`HUNK_INDENT`]; a commit view has no such tree and paints its diff flush
/// left, so it passes `""`.
fn hunk_lines(hunk: &Hunk, indent: &str) -> Vec<(PageRow, usize)> {
    let (added, removed) = hunk.counts();
    let mut rows = vec![(
        vec![PageSpan::new(
            PageStyle::Hunk,
            format!("{indent}{}  +{added} -{removed}", hunk.header),
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
                (
                    line_spans(line, &segments, indent),
                    indent.chars().count() + MARKER_WIDTH,
                )
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
                .map(|(line, segments)| line_spans(line, &segments, "")),
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
            .map(|(line, segments)| line_spans(line, &segments, "")),
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

/// The rows of a commit's content: the summary `git show` opens with —
/// identity, author, date, message, change totals — then each file's changes
/// under a band of its own, carrying the file's icon, with the same hunk bodies
/// the status view paints for the working tree. A commit then reads the way the
/// tree does, not the way a patch does: bands and word-decorated lines, no
/// `diff --git` or `index` noise.
///
/// `folded_files` holds the indices of files showing no diff, and `folded_hunks`
/// the `(file, hunk)` pairs showing only their header, so the same content can be
/// drawn at whatever depth the reader has opened it to.
pub fn commit_view_rows(
    content: &CommitContent,
    folded_files: &HashSet<usize>,
    folded_hunks: &HashSet<(usize, usize)>,
) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (index, line) in content.summary.iter().enumerate() {
        rows.push(ViewRow {
            icon: None,
            item: Item::None,
            spans: summary_row(line, index == 0),
            wrap_indent: 0,
        });
    }
    for (file_index, file) in content.files.iter().enumerate() {
        let shut = folded_files.contains(&file_index);
        rows.push(commit_file_row(file, file_index, shut));
        if shut {
            continue;
        }
        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            let item = Item::CommitHunk(file_index, hunk_index);
            let hunk_shut = folded_hunks.contains(&(file_index, hunk_index));
            let lines = hunk_lines(hunk, COMMIT_HUNK_INDENT);
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

/// One line of the summary `git show` opens a commit with: the identity line
/// as a heading, the message as ordinary text, and the rest — author, dates,
/// the change totals — receded.
fn summary_row(line: &str, identity: bool) -> PageRow {
    if identity {
        vec![PageSpan::new(PageStyle::Header, line)]
    } else if line.starts_with(MESSAGE_INDENT) || line.trim().is_empty() {
        vec![PageSpan::plain(line)]
    } else {
        vec![PageSpan::new(PageStyle::Dim, line)]
    }
}

/// A file's band in a commit: its change code, its icon, its path, and what its
/// hunks add and remove — the row the status view gives a file, standing in for
/// the `diff --git` and `---`/`+++` lines it replaces.
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
    let (added, removed) = file.counts();
    ViewRow {
        icon: Some(PageIcon {
            col: ENTRY_INDENT.chars().count() + CODE_WIDTH,
            glyph: FILE_GLYPH,
            kind: PageIconKind::File {
                name: leaf_name(&file.path).to_string(),
            },
            row: 0,
        }),
        item: Item::CommitFile(index),
        spans: vec![
            PageSpan::new(
                band,
                format!(
                    "{ENTRY_INDENT}{:<CODE_WIDTH$}{reserved}{}",
                    file.code, file.path
                ),
            ),
            PageSpan::new(PageStyle::Dim, format!("  +{added} -{removed}")),
        ],
        wrap_indent: 0,
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
    // Inside a hunk, a space-prefixed line is context, which reads as neither
    // arriving nor leaving.
    if in_hunk && line.starts_with(' ') {
        return PageSpan::new(PageStyle::Dim, line.clone());
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
fn line_spans(line: &str, segments: &[Segment], indent: &str) -> PageRow {
    let (base, edit) = line_styles(line);
    let marker = line.chars().next().unwrap_or(' ');
    let mut spans = PageRow::new();
    push_span(&mut spans, base, &format!("{indent}{marker}"));
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
        Some(_) | None => (PageStyle::Dim, PageStyle::Dim),
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
        PageSpan::new(PageStyle::Accent, format!("{ENTRY_INDENT}{} ", commit.hash)),
        PageSpan::new(PageStyle::Dim, GRAPH_MARK),
    ];
    if unpushed {
        spans.push(PageSpan::new(PageStyle::Accent, MARK_UNPUSHED));
    }
    for entry in &commit.refs {
        spans.push(PageSpan::new(
            ref_style(entry.kind),
            format!("{} ", entry.name),
        ));
    }
    spans.push(PageSpan::plain(commit.subject.clone()));
    // The author line starts under the graph column: past the entry indent, the
    // hash, and the space after it.
    let indent = ENTRY_INDENT.chars().count() + commit.hash.chars().count() + 1;
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
            vec![PageSpan::plain(abbreviate_home(root))],
        ));
        rows.push(blank_row());
    }
    rows.push(ViewRow {
        icon: None,
        item: Item::None,
        spans: vec![PageSpan::new(PageStyle::Header, LOG_TITLE)],
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

    fn open(_section: Option<Section>) -> bool {
        false
    }

    fn shut(_section: Option<Section>) -> bool {
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
        collapsed: &dyn Fn(Option<Section>) -> bool,
        diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
    ) -> Vec<ViewRow> {
        build(status, commits, None, NOW, collapsed, None, diffs)
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
            "  abc1234 * do the thing",
            "the identity line carries the hash, the graph mark, and the subject"
        );
        assert_eq!(
            text(commit_rows[1]),
            "          Someone   1 hour",
            "the authorship line sits under the graph column"
        );
        assert_eq!(
            commit_rows[1].wrap_indent, 10,
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
        assert_eq!(text(row), "  abc1234 * main origin/main v1.0 do the thing");
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
        assert_eq!(lines[1], "  abc1234 * do the thing");
        assert_eq!(lines[2], "          Someone   1 hour");
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
        // its body lines reserve the indent and marker their text sits after.
        assert_eq!(rows[0].wrap_indent, 0);
        assert_eq!(rows[1].wrap_indent, 5);
        assert_eq!(rows[2].wrap_indent, 5);
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

    /// The commit view with everything open, which is how it first draws.
    fn commit_view(lines: &[String]) -> Vec<ViewRow> {
        commit_view_rows(
            &CommitContent::parse(lines),
            &HashSet::new(),
            &HashSet::new(),
        )
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
            " 2 files changed, 10 insertions(+), 2 deletions(-)",
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
        assert_eq!(rows[4].spans, vec![PageSpan::plain("    do the thing")]);
        assert_eq!(
            rows[6].spans,
            vec![PageSpan::new(
                PageStyle::Dim,
                " 2 files changed, 10 insertions(+), 2 deletions(-)"
            )],
            "the totals recede with the rest of the summary"
        );
        assert!(rows.iter().all(|row| row.wrap_indent == 0));
        assert!(
            rows.iter().all(|row| row.item == Item::None),
            "nothing in the summary is actionable"
        );
    }

    #[test]
    fn test_each_file_gets_a_band_with_its_code_and_hunk_count() {
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
            PageSpan::new(PageStyle::Section, "  M    kept.rs"),
            "a modified file carries its code and path"
        );
        assert_eq!(
            kept.spans[1],
            PageSpan::new(PageStyle::Dim, "  +1 -1"),
            "the band totals what its hunks change"
        );
        assert_eq!(
            kept.icon,
            Some(PageIcon {
                col: ENTRY_INDENT.chars().count() + CODE_WIDTH,
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
            PageSpan::new(PageStyle::Section, "  A    added.txt"),
            "a new file says so with the status view's code"
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
        let open = commit_view_rows(&content, &HashSet::new(), &HashSet::new());
        let shut = commit_view_rows(&content, &HashSet::from([0]), &HashSet::new());

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
            band.spans[0].style,
            PageStyle::SectionFolded,
            "a shut file's band recedes, the way the working tree's does"
        );
        assert!(
            text(band).contains("f.rs"),
            "the band still names the file so it can be unfolded"
        );
        assert_eq!(
            open.iter()
                .find(|row| row.item == Item::CommitFile(0))
                .map(|row| row.spans[0].style),
            Some(PageStyle::Section),
            "an open file's band is the bright one"
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
        let rows = commit_view_rows(&content, &HashSet::new(), &HashSet::from([(0, 0)]));
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
    fn test_a_commits_diff_sits_flush_left_while_the_trees_stays_indented() {
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
            "@@ -1,2 +1,2 @@  +1 -1",
            "a commit's hunk header starts at the left margin"
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

        // The working-tree view keeps the nesting its section tree needs.
        let diff = parse_diff("--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n");
        let file = FileRow {
            path: "f.rs".to_string(),
            section: Section::Staged,
        };
        let tree = hunk_rows(&file, &diff);
        assert!(
            text(&tree[0]).starts_with(HUNK_INDENT),
            "the status view still indents a hunk under its file"
        );
        assert_eq!(
            tree[1].wrap_indent,
            HUNK_INDENT.chars().count() + MARKER_WIDTH
        );
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
        assert_eq!(band.spans[0].style, PageStyle::Section);
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
                PageSpan::new(PageStyle::Removed, "    -    "),
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
                PageSpan::new(PageStyle::Added, "    +    "),
                PageSpan::new(PageStyle::AddedEdit, "new"),
                PageSpan::new(PageStyle::Added, "();"),
            ]
        );
        assert_eq!(text(added), "    +    new();");
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
                PageSpan::new(PageStyle::Added, "    +"),
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
        // Context reads as neither arriving nor leaving, and the changed
        // lines carry only their edit in the harder tint.
        assert_eq!(rows[5], vec![PageSpan::new(PageStyle::Dim, " fn main() {")]);
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
