//! Painting the status view: the header, each section, and the entries under
//! it, plus the mapping from a row back to what it stands for, and the
//! whole-diff output a key can put in the view's place.

use std::collections::HashSet;
use std::path::Path;

use crate::bytes_text;
use crate::editors::reltime;
use crate::model::paint::{expand_tabs, Icon, IconKind, Row, Span, Style};

use super::commit::{CommitContent, CommitFile};
use super::diff::{FileDiff, Hunk};
use crate::editors::git_parse::{Commit, FileStatus, RefKind, Section, Stash, Status, FIELD_SEP};
#[cfg(test)]
use super::progress::Operation;
use super::progress::{Progress, Step, StepState};
use crate::editors::word_diff::{decorate, Segment};

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

/// What a commit subject cut short to fit ends with.
const ELLIPSIS: char = '…';

/// The graph column drawn beside a commit. A literal marker rather than git's
/// `--graph` art: the log is read in reverse-chronological order, where every
/// commit sits on one lane, and variable-width art would misalign the author
/// line beneath it.
const GRAPH_MARK: &str = "* ";

/// Marks a commit that exists locally but not on the branch's upstream.
const MARK_UNPUSHED: &str = "↑ ";

/// Marks a commit the upstream has that the local branch does not.
const MARK_UNPULLED: &str = "↓ ";

/// Minimum gap between commit subject and the committer and time.
const MIN_GAP: usize = 2;

/// The widest a committer's name is shown in a commit row. A longer one is
/// cut, so one long name cannot take the subject's room on every row.
const MAX_AUTHOR_WIDTH: usize = 20;

/// Heading of the log view's commit list.
const LOG_TITLE: &str = "Commit logs";

/// Heading over the files a commit touched, which folds them all away at once.
const CHANGES_TITLE: &str = "Changes";

/// Heading of the stash section.
const STASH_TITLE: &str = "Stashes";

/// The headings of the list of branches and tags.
const REFS_LOCAL_TITLE: &str = "Branches";
const REFS_REMOTE_TITLE: &str = "Remote branches";
const REFS_TAG_TITLE: &str = "Tags";

/// Heading of the section listing what the upstream has and this branch does
/// not. The upstream's name follows it.
const UNPULLED_TITLE: &str = "Unpulled from";

/// Label of the line naming whatever git is part-way through.
const LABEL_STATE: &str = "State:";

/// What the log view says when more commits can be loaded.
pub(super) const LOG_MORE: &str = "Press '+' to display more commits";

/// The one column a diff line's marker takes, which a wrapped continuation
/// starts past.
const MARKER_WIDTH: usize = 1;

/// The four-space indent `git show` gives a commit's message lines, which the
/// summary takes back off: every row of the view starts at the pane's left
/// edge, so the columns go to the text rather than to nesting.
const MESSAGE_INDENT: &str = "    ";

/// Width a change's name is padded to, before the icon and the path, so every
/// path of a status or a commit starts at the same column whatever happened to
/// its file. Twelve, the width the same listing uses in magic-vscode.
const STATUS_WIDTH: usize = 12;

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
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
    /// A branch or a tag in the list of them.
    Ref(RefRow),
    /// The heading of the recent-commits section.
    RecentHeading,
}

/// A branch or a tag, as the list of them has it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RefRow {
    /// Whether it is a branch, which one, or a tag.
    pub kind: RefKind,
    /// Its short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
}

/// One hunk of an expanded file's diff.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
    /// is filled in by whoever assembles the editor, which is the only place the
    /// row's final position is known.
    pub icon: Option<Icon>,
    /// The row's painted spans.
    pub spans: Row,
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
/// editor has read.
pub fn build(
    content: StatusContent,
    collapsed: &dyn Fn(Block) -> bool,
    diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
) -> Vec<ViewRow> {
    let StatusContent {
        status,
        commits,
        more_commits,
        unpulled,
        stashes,
        progress,
        root,
        now,
        cols,
    } = content;
    let mut rows = header_block(status, commits, unpulled, root, progress);
    rows.push(blank_row());
    // What an operation part-way through works through, where git lists it,
    // first: it is what the reader has to finish before anything else.
    if let Some(progress) = progress.filter(|progress| !progress.steps.is_empty()) {
        let title = match progress.detail.is_empty() {
            true => progress.operation.title().to_string(),
            false => format!("{} {}", progress.operation.title(), progress.detail),
        };
        rows.push(heading_row(&title, progress.steps.len(), false, Item::None));
        rows.extend(progress.steps.iter().map(step_row));
        rows.push(blank_row());
    }
    // The unpulled and recent commits share columns, so their committers line
    // up from one list to the next.
    let columns = MetaColumns::of(unpulled.iter().chain(commits.iter()), now);
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
                rows.extend(commit_rows_decorated(
                    commit,
                    now,
                    Drift::Unpulled,
                    columns,
                    cols,
                ));
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
                let drift = match index < status.ahead {
                    true => Drift::Unpushed,
                    false => Drift::Shared,
                };
                rows.extend(commit_rows_decorated(commit, now, drift, columns, cols));
            }
            if more_commits {
                rows.push(blank_row());
                rows.push(ViewRow {
                    icon: None,
                    item: Item::None,
                    spans: vec![Span::new(Style::Dim, LOG_MORE)],
                    wrap_indent: 0,
                });
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
    unpulled: &[Commit],
    root: Option<&Path>,
    progress: Option<&Progress>,
) -> Vec<ViewRow> {
    let mut rows = Vec::new();
    if let Some(root) = root {
        rows.push(label_row(
            LABEL_REPO,
            vec![Span::new(Style::HeadingPlain, abbreviate_home(root))],
        ));
    }
    // The commit HEAD points at, which the log tail already carries, so naming
    // it costs no extra git call.
    let head_commit = commits.first();
    match (&status.branch, head_commit) {
        // A repository with no commit yet has nothing for the rest of the block
        // to describe.
        (None, None) => rows.push(label_row(
            LABEL_HEAD,
            vec![Span::new(Style::Dim, EMPTY_HEAD)],
        )),
        (branch, commit) => {
            let name = branch.clone().unwrap_or_else(|| DETACHED.to_string());
            let mut spans = vec![Span::new(Style::RefHead, name)];
            if let Some(commit) = commit {
                spans.push(Span::plain(" "));
                spans.push(Span::new(Style::Accent, commit.hash.clone()));
                spans.push(Span::plain(format!(" {}", commit.subject)));
            }
            rows.push(label_row(LABEL_HEAD, spans));
        }
    }
    // The merge line names the upstream and how far the branch has drifted from
    // it, which is the pair a reader needs before pushing or pulling.
    let mut merge = match &status.upstream {
        Some(upstream) => vec![Span::new(Style::RefRemote, upstream.clone())],
        None => vec![Span::new(Style::Dim, NO_UPSTREAM)],
    };
    // Only a side that has drifted is counted: `↑ 2` alone when nothing waits
    // to be pulled, rather than a `↓0` to read past.
    let drift: String = [("↑", status.ahead), ("↓", status.behind)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(arrow, count)| format!(" {arrow} {count}"))
        .collect();
    if !drift.is_empty() {
        merge.push(Span::new(Style::Unpushed, drift));
    }
    if let Some(upstream) = &status.upstream {
        if let Some(commit) = upstream_commit(upstream, status, commits, unpulled) {
            merge.push(Span::plain(" "));
            merge.push(Span::new(Style::Accent, commit.hash.clone()));
            merge.push(Span::plain(format!(" {}", commit.subject)));
        }
    }
    rows.push(label_row(LABEL_MERGE, merge));
    // What git is part-way through, and the keys that finish it. Nothing in a
    // porcelain status says a rebase is open, so without this line the only
    // sign of one is a section of conflicts and no reason for them.
    if let Some(progress) = progress {
        rows.push(label_row(LABEL_STATE, progress_spans(progress)));
        rows.push(label_row(
            "",
            vec![Span::new(Style::Dim, progress.operation.keys())],
        ));
    }
    // A tag on HEAD reads off the same decoration the commit rows use.
    let head_tag = commits
        .first()
        .and_then(|commit| commit.refs.iter().find(|entry| entry.kind == RefKind::Tag));
    if let Some(tag) = head_tag {
        rows.push(label_row(
            LABEL_TAG,
            vec![Span::new(Style::RefTag, tag.name.clone())],
        ));
    }
    rows
}

/// The commit at the tip of the upstream branch, where one of the commit lists
/// holds it.
fn upstream_commit<'a>(
    upstream: &str,
    status: &Status,
    commits: &'a [Commit],
    unpulled: &'a [Commit],
) -> Option<&'a Commit> {
    if let Some(commit) = unpulled
        .iter()
        .chain(commits.iter())
        .find(|c| c.refs.iter().any(|r| r.name == upstream))
    {
        return Some(commit);
    }
    if status.behind > 0 {
        if let Some(commit) = unpulled.first() {
            return Some(commit);
        }
    }
    if status.behind == 0 {
        if let Some(commit) = commits.get(status.ahead) {
            return Some(commit);
        }
    }
    None
}

/// One labelled header line: the label padded to [`LABEL_WIDTH`], then `value`.
fn label_row(label: &str, value: Vec<Span>) -> ViewRow {
    let mut spans = vec![Span::new(
        Style::HeadingPlain,
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
fn progress_spans(progress: &Progress) -> Row {
    let mut spans = vec![Span::new(Style::Operation, progress.operation.title())];
    if !progress.detail.is_empty() {
        spans.push(Span::plain(format!(" {}", progress.detail)));
    }
    if let Some((at, of)) = progress.step {
        spans.push(Span::new(Style::Dim, format!("  ({at}/{of})")));
    }
    spans
}

/// One step of an operation part-way through: how far it is with it, what
/// it does, the commit, and the commit's subject. A step on a commit is that
/// commit, for every key a commit answers.
fn step_row(step: &Step) -> ViewRow {
    let state = step.state.word();
    let lead = format!("{state:<4} {:<6} ", step.action);
    let style = match step.state {
        StepState::Stopped => Style::Operation,
        StepState::Done | StepState::Todo => Style::Dim,
    };
    let mut spans = vec![Span::new(style, lead.clone())];
    if !step.hash.is_empty() {
        spans.push(Span::new(Style::Accent, format!("{} ", step.hash)));
    }
    spans.push(Span::plain(step.subject.clone()));
    ViewRow {
        icon: None,
        item: match step.hash.is_empty() {
            true => Item::None,
            false => Item::Commit(step.hash.clone()),
        },
        spans,
        wrap_indent: lead.chars().count(),
    }
}

/// One stash's row: what git calls it, then the message it was pushed with.
fn stash_row(stash: &Stash, index: usize) -> ViewRow {
    ViewRow {
        icon: None,
        item: Item::Stash(index),
        spans: vec![
            Span::new(Style::Accent, format!("{} ", stash.name)),
            Span::plain(stash.subject.clone()),
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
        _ => Style::HeadingPlain,
    };
    ViewRow {
        icon: None,
        item,
        spans: vec![
            Span::new(style, format!("{glyph}{title}")),
            Span::new(Style::Dim, format!(" ({count})")),
        ],
        wrap_indent: 0,
    }
}

/// The hue a section's heading wears: what sits under it, said in color, the
/// way magic-vscode colors its own headings. Staged and unstaged are the pair
/// read against each other most often, so they are furthest apart.
fn heading_style(section: Section) -> Style {
    match section {
        Section::Staged => Style::HeadingStaged,
        Section::Unstaged => Style::HeadingUnstaged,
        Section::Unmerged => Style::HeadingConflict,
        Section::Untracked => Style::HeadingUntracked,
    }
}

/// A file's row: a band the file's changes sit under, bright while they show
/// beneath it and receded once folded shut. Its fold mark opens the row, then
/// the change's name, which says what changed, where the icon says what kind
/// of file it is, and the icon's columns follow it, blank, with one more
/// keeping the name off the artwork.
fn file_row(file: &FileStatus, expanded: bool) -> ViewRow {
    let name = match &file.renamed_from {
        Some(from) => format!("{} → {}", bytes_text::display(from), bytes_text::display(&file.path)),
        None => bytes_text::display(&file.path).into_owned(),
    };
    let reserved = " ".repeat(Icon::WIDTH + 1);
    // The path wears its section's hue whether the row is folded or not, as
    // magic-vscode's does, so the fold changes the mark and nothing else.
    let band = match file.section {
        Section::Staged => Style::FileStaged,
        Section::Unmerged => Style::FileConflict,
        Section::Unstaged => Style::FileUnstaged,
        Section::Untracked => Style::FileUntracked,
    };
    let mark = fold_glyph(!expanded);
    ViewRow {
        icon: Some(Icon {
            col: fold_width() + STATUS_WIDTH,
            kind: IconKind::File {
                name: leaf_name(&file.path).to_string(),
            },
            row: 0,
        }),
        item: Item::File(FileRow {
            path: file.path.clone(),
            section: file.section,
        }),
        spans: vec![
            Span::new(
                change_style(file.code),
                format!("{mark}{:<STATUS_WIDTH$}", status_label(file.code)),
            ),
            Span::new(band, format!("{reserved}{name}")),
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
/// lines word-decorated, each reserving its marker column when it wraps,
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
fn hunk_lines(hunk: &Hunk, fold: Option<bool>) -> Vec<(Row, usize)> {
    let (added, removed) = hunk.counts();
    let mark = fold.map(fold_glyph).unwrap_or_default();
    let mut rows = vec![(
        vec![Span::new(
            Style::Hunk,
            format!("{mark}{}  +{added} -{removed}", expand_tabs(&hunk.header)),
        )],
        0,
    )];
    // A CRLF file's lines carry their carriage return for the patch's sake;
    // on screen it is no character.
    let shown: Vec<String> = hunk
        .lines
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    rows.extend(
        shown
            .iter()
            .zip(decorate(&shown))
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
/// The painted rows of a whole-diff output, the file and hunk lines styled as
/// headings, the header detail receded, and every changed line word-decorated
/// the way an expanded file's hunks are, with, per row, the column its
/// wrapped continuation starts at: a diff line reserves its marker's room. A
/// `git show` carries commit detail above its first hunk, which is left as
/// ordinary text: nothing is a diff's body until a hunk opens.
pub fn diff_rows(lines: &[String]) -> (Vec<Row>, Vec<usize>) {
    let mut rows: Vec<Row> = Vec::with_capacity(lines.len());
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

/// Which side of a branch pair a listed commit is missing from, which is
/// what its arrow says.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Drift {
    /// On both sides, or in a list where the question does not arise.
    Shared,
    /// Here, and not yet on the upstream.
    Unpushed,
    /// On the upstream, and not yet here.
    Unpulled,
}

/// How wide the committer and age columns are across one list of commits,
/// so every row's committer and age start in their own column.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MetaColumns {
    /// The longest age.
    age: usize,
    /// The longest committer's name, short of [`MAX_AUTHOR_WIDTH`].
    author: usize,
}

impl MetaColumns {
    /// The columns that fit every one of `commits`, their ages told from `now`.
    fn of<'a>(commits: impl IntoIterator<Item = &'a Commit>, now: i64) -> Self {
        commits
            .into_iter()
            .fold(Self::default(), |columns, commit| Self {
                age: columns
                    .age
                    .max(reltime::short_ago(commit.time, now).chars().count()),
                author: columns
                    .author
                    .max(commit.author.chars().count().min(MAX_AUTHOR_WIDTH)),
            })
    }

    /// `author` and `ago` laid into the columns: both left-aligned in their
    /// respective columns.
    fn lay_out(&self, author: &str, ago: &str) -> String {
        let (author_width, age_width) = (self.author, self.age);
        match author_width {
            0 => format!("{ago:<age_width$}"),
            _ => {
                let author = truncate(author, author_width);
                format!("{author:<author_width$}   {ago:<age_width$}")
            }
        }
    }
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

/// Everything the status view draws, gathered from git by the editor.
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
    /// Whether the log goes on past `commits`, which the section says so.
    pub more_commits: bool,
    /// What the upstream has and this branch does not, newest first.
    pub unpulled: &'a [Commit],
    /// The stashes, newest first.
    pub stashes: &'a [Stash],
    /// What git is part-way through, when it is part-way through anything.
    pub progress: Option<&'a Progress>,
    /// The repository's path, drawn on the `Repo:` line.
    pub root: Option<&'a Path>,
    /// The current Unix time, which the commit rows measure ages against.
    pub now: i64,
    /// Available pane width in columns, for right-aligning metadata and truncating subjects.
    pub cols: Option<usize>,
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

/// The rows of a commit's content: the summary `git show` opens with,
/// identity, author, date, message, then a heading saying how many files it
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
    // its sections. A commit with no file at all, a merge, or an empty one,
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
        // A file with no hunks, binary, a mode change, still says what
        // happened to it: git's own note under the band.
        if let Some(note) = &file.note {
            rows.push(ViewRow {
                icon: None,
                item: Item::CommitFile(file_index),
                spans: vec![Span::new(Style::Dim, note.clone())],
                wrap_indent: 0,
            });
        }
    }
    rows
}

/// One line of the summary `git show` opens a commit with, dedented: the
/// identity line as a heading, the message as ordinary text, and the rest,
/// author, dates, the change totals, receded.
///
/// git indents a commit's message by [`MESSAGE_INDENT`] and its shortstat by
/// one column; both come off, so the summary starts where every other row
/// does. Only that prefix comes off a message line, so indentation the author
/// wrote into the message survives.
fn summary_row(line: &str, identity: bool) -> Row {
    let message = line.starts_with(MESSAGE_INDENT);
    let text = match message {
        true => &line[MESSAGE_INDENT.len()..],
        false => line.trim_start(),
    };
    if identity {
        vec![Span::new(Style::Header, text)]
    } else if message || text.is_empty() {
        vec![Span::plain(text)]
    } else {
        vec![Span::new(Style::Dim, text)]
    }
}

/// A file's band in a commit: its fold mark, what the commit did to it, its
/// icon, its path, and what its hunks add and remove, the row the status view
/// gives a file, standing in for the `diff --git` and `---`/`+++` lines it
/// replaces.
///
/// Bright while its diff shows beneath it and receded once folded shut, the same
/// way the working tree's file rows read, so one habit covers both views.
fn commit_file_row(file: &CommitFile, index: usize, folded: bool) -> ViewRow {
    let reserved = " ".repeat(Icon::WIDTH + 1);
    let band = if folded {
        Style::SectionFolded
    } else {
        Style::Section
    };
    let mark = fold_glyph(folded);
    ViewRow {
        icon: Some(Icon {
            col: fold_width() + STATUS_WIDTH,
            kind: IconKind::File {
                name: leaf_name(&file.path).to_string(),
            },
            row: 0,
        }),
        item: Item::CommitFile(index),
        spans: vec![
            Span::new(
                change_style(file.code),
                format!("{mark}{:<STATUS_WIDTH$}", status_label(file.code)),
            ),
            Span::new(band, format!("{reserved}{}", bytes_text::display(&file.path))),
            Span::new(Style::Dim, format!(" ({})", file.hunks.len())),
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
/// learned, green arriving, red leaving, blue edited in place, yellow moved,
/// magenta contested. A copy and an untracked file are both new where they
/// stand, and a type change is a modification, so they share those. Anything
/// else is painted as a modification, the weakest thing a code can mean.
fn change_style(code: char) -> Style {
    match code {
        '?' | 'A' | 'C' => Style::ChangeAdded,
        'D' => Style::ChangeDeleted,
        'R' => Style::ChangeRenamed,
        'U' => Style::ChangeConflict,
        _ => Style::ChangeModified,
    }
}

fn status_label(code: char) -> &'static str {
    match code {
        '?' => "untracked",
        'A' => "new file",
        'C' => "copied",
        'D' => "deleted",
        'M' => "modified",
        'R' => "renamed",
        'T' => "type changed",
        'U' => "unmerged",
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
fn line_span(lines: &[String], index: usize, in_hunk: bool) -> Span {
    let line = &lines[index];
    if line.starts_with(FILE_MARK) {
        return Span::new(Style::Section, expand_tabs(line));
    }
    if line.starts_with(HUNK_MARK) {
        return Span::new(Style::Hunk, expand_tabs(line));
    }
    // Inside a hunk, a space-prefixed line is context: ordinary text, since
    // it is the code the changes around it are read against. What carries the
    // eye is the tint on the lines that did change, not a fade on the ones
    // that did not.
    if in_hunk && line.starts_with(' ') {
        return Span::new(Style::Normal, expand_tabs(line));
    }
    // The file-pair lines only count as header when they truly pair: a
    // removed line of body text can itself begin `-- `.
    let paired = (line.starts_with("--- ")
        && lines
            .get(index + 1)
            .is_some_and(|next| next.starts_with("+++ ")))
        || (line.starts_with("+++ ") && index > 0 && lines[index - 1].starts_with("--- "));
    let detail = paired || DETAIL_MARKS.iter().any(|mark| line.starts_with(mark));
    let style = if detail { Style::Dim } else { Style::Normal };
    Span::new(style, expand_tabs(line))
}

/// One diff line as spans: the whole line wears its side's tint, and the
/// stretches the edit touched wear it harder, so the eye lands on what
/// actually changed. A line the word diff says nothing about paints as one
/// span, all of it in its side's tint.
fn line_spans(line: &str, segments: &[Segment]) -> Row {
    let (base, edit) = line_styles(line);
    let marker = line.chars().next().unwrap_or(' ');
    let mut spans = Row::new();
    push_span(&mut spans, base, &marker.to_string());
    for segment in segments {
        let style = if segment.changed { edit } else { base };
        push_span(&mut spans, style, &expand_tabs(&segment.text));
    }
    spans
}

/// Add a span, joining it with the previous one when they share a style, so a
/// line paints as few runs as it can.
fn push_span(spans: &mut Row, style: Style, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            last.text.push_str(text);
            return;
        }
    }
    spans.push(Span::new(style, text));
}

/// The two styles one diff line wears: the tint all of it carries as its
/// side, and the one its edited words carry harder. Added lines read as
/// arriving, removed as leaving, and context as neither.
fn line_styles(line: &str) -> (Style, Style) {
    match line.chars().next() {
        Some('+') => (Style::Added, Style::AddedEdit),
        Some('-') => (Style::Removed, Style::RemovedEdit),
        Some(_) | None => (Style::Normal, Style::Normal),
    }
}

/// `text`, cut short with an ellipsis to fit in `room` columns.
fn truncate(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_string();
    }
    if room == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(room.saturating_sub(1)).collect();
    cut.push(ELLIPSIS);
    cut
}

/// One commit's row: hash, graph mark, drift mark, refs, subject, then the
/// committer and age laid into `columns` at the right edge.
fn commit_rows_decorated(
    commit: &Commit,
    now: i64,
    drift: Drift,
    columns: MetaColumns,
    cols: Option<usize>,
) -> Vec<ViewRow> {
    let item = Item::Commit(commit.hash.clone());
    let mut spans = vec![
        Span::new(Style::Accent, format!("{} ", commit.hash)),
        Span::new(Style::Dim, GRAPH_MARK),
    ];
    // Only the arrow says which side the commit is missing from; the
    // subject stays plain, so drifted commits read like the rest.
    match drift {
        Drift::Unpushed => spans.push(Span::new(Style::Unpushed, MARK_UNPUSHED)),
        Drift::Unpulled => spans.push(Span::new(Style::Unpushed, MARK_UNPULLED)),
        Drift::Shared => {}
    }
    for entry in &commit.refs {
        spans.push(Span::new(ref_style(entry.kind), format!("{} ", entry.name)));
    }

    let right_text = columns.lay_out(&commit.author, &reltime::short_ago(commit.time, now));

    let prefix_len: usize = spans.iter().map(|s| s.text.chars().count()).sum();
    let right_len = right_text.chars().count();

    let (subject, padding) = match cols {
        Some(cols) if cols > 0 => {
            let room = cols.saturating_sub(prefix_len + right_len + MIN_GAP);
            let truncated = truncate(&commit.subject, room);
            let subject_len = truncated.chars().count();
            let pad = cols
                .saturating_sub(prefix_len + subject_len + right_len)
                .max(MIN_GAP);
            (truncated, pad)
        }
        _ => (commit.subject.clone(), 3),
    };

    if !subject.is_empty() {
        spans.push(Span::plain(subject));
    }
    spans.push(Span::new(
        Style::Dim,
        format!("{}{}", " ".repeat(padding), right_text),
    ));

    let indent = commit.hash.chars().count() + 1;
    vec![ViewRow {
        icon: None,
        item,
        spans,
        wrap_indent: indent,
    }]
}

/// The list of branches and tags `git for-each-ref` printed (see
/// [`crate::editors::git_exec::ref_list`]): the local branches, the remote ones, then the
/// tags, each under its heading, a row a ref with its commit and subject.
pub fn ref_rows(listed: &str) -> Vec<ViewRow> {
    let mut groups: [(&str, Vec<ViewRow>); 3] =
        [(REFS_LOCAL_TITLE, Vec::new()), (REFS_REMOTE_TITLE, Vec::new()), (REFS_TAG_TITLE, Vec::new())];
    for line in listed.lines() {
        let fields: Vec<&str> = line.split(FIELD_SEP).collect();
        let [head, full, hash, subject] = fields[..] else {
            continue;
        };
        let (kind, name, group) = if let Some(name) = full.strip_prefix("refs/heads/") {
            let kind = if head == "*" { RefKind::Head } else { RefKind::Local };
            (kind, name, 0)
        } else if let Some(name) = full.strip_prefix("refs/remotes/") {
            // `origin/HEAD` names whichever branch the remote defaults to,
            // which is listed in its own right.
            if name.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, name, 1)
        } else if let Some(name) = full.strip_prefix("refs/tags/") {
            (RefKind::Tag, name, 2)
        } else {
            continue;
        };
        groups[group].1.push(ViewRow {
            icon: None,
            item: Item::Ref(RefRow {
                kind,
                name: name.to_string(),
            }),
            spans: vec![
                Span::new(ref_style(kind), format!("{name} ")),
                Span::new(Style::Accent, format!("{hash} ")),
                Span::plain(subject.to_string()),
            ],
            wrap_indent: name.chars().count() + 1,
        });
    }
    let mut rows = Vec::new();
    for (title, group) in groups.into_iter().filter(|(_, group)| !group.is_empty()) {
        if !rows.is_empty() {
            rows.push(blank_row());
        }
        rows.push(heading_row(title, group.len(), false, Item::None));
        rows.extend(group);
    }
    rows
}

/// How loudly a ref beside a commit is drawn, by what kind of ref it is.
fn ref_style(kind: RefKind) -> Style {
    match kind {
        RefKind::Head => Style::RefHead,
        RefKind::Local => Style::RefLocal,
        RefKind::Remote => Style::RefRemote,
        RefKind::Tag => Style::RefTag,
    }
}

/// The rows of the log view: which repository this is, then every commit the way
/// the status view's tail draws them, then the hint that more can be loaded.
pub fn log_rows(
    commits: &[Commit],
    root: Option<&Path>,
    now: i64,
    more: bool,
    cols: Option<usize>,
) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    if let Some(root) = root {
        rows.push(label_row(
            LABEL_REPO,
            vec![Span::new(Style::HeadingPlain, abbreviate_home(root))],
        ));
        rows.push(blank_row());
    }
    rows.push(ViewRow {
        icon: None,
        item: Item::None,
        spans: vec![Span::new(Style::HeadingPlain, LOG_TITLE)],
        wrap_indent: 0,
    });
    let columns = MetaColumns::of(commits, now);
    for commit in commits {
        // Nothing in a log view is "unpushed": it lists whatever revisions were
        // asked for, which need not be the checked-out branch at all.
        rows.extend(commit_rows_decorated(
            commit,
            now,
            Drift::Shared,
            columns,
            cols,
        ));
    }
    if more {
        rows.push(blank_row());
        rows.push(ViewRow {
            icon: None,
            item: Item::None,
            spans: vec![Span::new(Style::Dim, LOG_MORE)],
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
#[path = "rows_tests.rs"]
mod tests;
