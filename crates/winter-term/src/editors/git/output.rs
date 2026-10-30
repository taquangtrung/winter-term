//! Output that fills the view in place of the status: a log, a blame, a
//! diff, or one commit, and how it is painted.

use std::collections::HashSet;

use crate::editors::list::ListView;
use crate::model::paint::{row_height, row_text, Frame, Icon, Span, Style};

use super::commit::CommitContent;
use crate::editors::git_parse::Commit;
use super::rows::{self, CommitFolds, ViewRow};
use super::{GitEditor, HEADER_ROWS};

// ========================================================================
// Data Structures
// ========================================================================

/// Output filling the view in place of the status: what it is, its lines,
/// and, for a view that paints its own, the rows it paints.
#[derive(Clone, Debug)]
pub struct Output {
    /// The repository-relative path a blame is of, since a blame is one row
    /// per line of one file and so every row names a place in it. `None` for
    /// every other kind of output.
    pub(super) blamed: Option<String>,
    /// Whether the heading over `commit`'s files is shut, which hides every
    /// one of them.
    pub(super) changes_shut: bool,
    /// The commit being shown, kept so folding a file or a hunk can redraw at a
    /// different depth without re-running `git show`. `None` for every other
    /// kind of output.
    pub(super) commit: Option<CommitContent>,
    /// Files of `commit` showing no diff.
    pub(super) folded_files: HashSet<usize>,
    /// `(file, hunk)` pairs of `commit` showing only their header.
    pub(super) folded_hunks: HashSet<(usize, usize)>,
    /// The text a search reads over the view: the lines as they came back,
    /// or one entry per painted row for the views that paint their own.
    pub(super) lines: Vec<String>,
    /// Commits of `title == "Log"`, kept so window resize can re-render columns.
    pub(super) log_commits: Option<Vec<Commit>>,
    /// The cursor over the lines, and the window they are read through.
    pub(super) list: ListView<usize>,
    /// Whether more can be asked for, which only the log offers.
    pub(super) more: bool,
    /// The painted rows, one per line, for output that paints itself; empty for
    /// everything else, which paints straight from its lines.
    ///
    /// A [`ViewRow`] rather than bare spans because a painted view may also
    /// carry an icon per row and an `Item` saying what the row stands for,
    /// which is what lets the commit view fold at the cursor.
    pub(super) rows: Vec<ViewRow>,
    /// What produced it, for the title.
    pub(super) title: String,
}

// ========================================================================
// Output
// ========================================================================

impl Output {
    /// Output titled `title`, showing `lines`, painted as `rows` where it
    /// paints itself, with the cursor on its first line.
    fn new(title: &str, lines: Vec<String>, rows: Vec<ViewRow>) -> Self {
        let mut list = ListView::new();
        list.reset_keys((0..lines.len()).collect());
        Self {
            blamed: None,
            changes_shut: false,
            commit: None,
            folded_files: HashSet::new(),
            folded_hunks: HashSet::new(),
            lines,
            list,
            log_commits: None,
            more: false,
            rows,
            title: title.to_string(),
        }
    }
}

// ========================================================================
// GitEditor: output
// ========================================================================

impl GitEditor {
    /// Show `text` in place of the status view, as it came back.
    pub(super) fn show_output(&mut self, title: &str, text: &str, more: bool) {
        let lines = text.lines().map(str::to_string).collect();
        self.replace_output(Output {
            more,
            ..Output::new(title, lines, Vec::new())
        });
    }

    /// Show painted rows in place of the status view, with the text a search
    /// reads over them: one entry per row, since a painted view may show more
    /// rows than the text it came from had, or fewer.
    pub(super) fn show_painted(&mut self, title: &str, rows: Vec<ViewRow>) {
        let lines = rows.iter().map(|row| row_text(&row.spans)).collect();
        self.replace_output(Output::new(title, lines, rows));
    }

    /// Open every file and hunk of the commit showing, for a view whose diff
    /// is the whole of what it is for.
    pub(super) fn open_whole_commit(&mut self) {
        if let Some(output) = self.output.as_mut() {
            output.folded_files.clear();
            output.folded_hunks.clear();
            repaint_commit(output);
        }
    }

    /// Show one commit, kept as content rather than as lines so its files and
    /// hunks can be folded and unfolded in place.
    ///
    /// It opens shut: every file down to its band and every hunk down to its
    /// header, so a commit reads first as the list of what it touched and the
    /// patch is asked for a piece at a time. The status view opens the same
    /// way, and a commit touching thirty files is unreadable as a wall of
    /// diff. `Tab` opens what the cursor is on, `S-TAB` all of it.
    pub(super) fn show_commit(&mut self, title: &str, content: CommitContent) {
        let folded_files = (0..content.files.len()).collect();
        let folded_hunks = content
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, entry)| (0..entry.hunks.len()).map(move |hunk| (file, hunk)))
            .collect();
        let mut output = Output {
            commit: Some(content),
            folded_files,
            folded_hunks,
            ..Output::new(title, Vec::new(), Vec::new())
        };
        // The same commit read again keeps what was opened of it.
        if let Some(old) = self
            .output
            .as_ref()
            .filter(|old| self.keep_place && old.title == title)
        {
            output.changes_shut = old.changes_shut;
            output.folded_files = old.folded_files.clone();
            output.folded_hunks = old.folded_hunks.clone();
        }
        repaint_commit(&mut output);
        self.replace_output(output);
    }

    /// Put `output` up in place of whatever was. Where it is the output that
    /// was showing, read again after a command changed the repository, the
    /// cursor stays on the row it was on, as near as the new rows allow.
    fn replace_output(&mut self, mut output: Output) {
        if std::mem::take(&mut self.keep_place) {
            if let Some(old) = &self.output {
                output.list.set_cursor(old.list.cursor());
                *output.list.view_mut() = old.list.view().clone();
            }
        }
        self.output = Some(output);
    }

    /// The frame the output paints over the whole view, or `None` while the
    /// status view is up.
    pub(super) fn paint_output(&mut self, rows: usize, cols: usize, wrap: bool) -> Option<Frame> {
        let Output {
            lines,
            list,
            rows: painted_rows,
            title,
            ..
        } = self.output.as_mut()?;
        let visible = rows.saturating_sub(HEADER_ROWS).max(1);
        let height = |index: usize| {
            let indent = painted_rows.get(index).map_or(0, |row| row.wrap_indent);
            row_height(&lines[index], cols, wrap, indent)
        };
        let placed = list.place(visible, height);
        let window = placed.window;
        let header = vec![Span::new(Style::Header, title.clone())];
        let mut painted = vec![header];
        let mut wrap_indents = vec![0];
        let mut icons: Vec<Icon> = Vec::new();
        for (index, line) in lines
            .iter()
            .enumerate()
            .skip(window.start)
            .take(window.count)
        {
            match painted_rows.get(index) {
                Some(row) => {
                    if let Some(icon) = &row.icon {
                        icons.push(Icon {
                            row: painted.len(),
                            ..icon.clone()
                        });
                    }
                    painted.push(row.spans.clone());
                    wrap_indents.push(row.wrap_indent);
                }
                // Output that never painted itself shows as plain text.
                None => {
                    painted.push(vec![Span::plain(line.clone())]);
                    wrap_indents.push(0);
                }
            }
        }
        let content = Frame::new(painted)
            .with_icons(icons)
            .with_wrap_indents(wrap_indents);
        Some(placed.apply(content, HEADER_ROWS))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Redraw a commit output's rows at its current fold depth, keeping the search
/// text in step with them.
///
/// Called after every fold change rather than the rows being edited in place:
/// folding a file removes a variable number of rows, and rebuilding from the
/// content is the only way the row list, the search text, and the fold sets
/// cannot drift apart.
pub(super) fn repaint_commit(output: &mut Output) {
    let Some(content) = &output.commit else {
        return;
    };
    output.rows = rows::commit_view_rows(
        content,
        CommitFolds {
            shut: output.changes_shut,
            files: &output.folded_files,
            hunks: &output.folded_hunks,
        },
    );
    output.lines = output.rows.iter().map(|row| row_text(&row.spans)).collect();
    output.list.reset_keys((0..output.rows.len()).collect());
}
