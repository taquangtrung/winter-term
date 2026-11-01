//! Comparing and merging texts, and the files they come from.

use std::path::PathBuf;

use super::jobs::JobRequest;

// ========================================================================
// Data Structures
// ========================================================================

/// Two files or directories to show the differences between, in a diff
/// beside the work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompareRequest {
    /// The files written only to be compared, removed once the view goes,
    /// each with the file it stands for, where it stands for one: a buffer's
    /// text written out is the file the buffer edits, so a hunk opens that.
    pub copies: Vec<(PathBuf, Option<PathBuf>)>,
    /// The side the changes go to.
    pub new: PathBuf,
    /// The side the changes come from.
    pub old: PathBuf,
    /// What the comparison is of, for the tab.
    pub title: String,
}

/// A git view to read in a tab of its own, as Magit reads a commit in a
/// buffer of its own: a commit, a stash, a log, a diff, or a blame, asked
/// for from the status or from another such view.
#[derive(Clone, Debug, PartialEq)]
pub struct GitViewRequest {
    /// Where the repository keeps its own files, where git said.
    pub git_dir: Option<PathBuf>,
    /// The read whose answer is the view.
    pub request: JobRequest,
    /// The repository's working tree.
    pub root: PathBuf,
}

/// Texts to compare or merge in the diff tool, as Emacs' ediff does:
/// two or three to compare, or two to merge (against their ancestor where
/// one is given) into a third.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffRequest {
    /// The ancestor two merged texts share.
    pub ancestor: Option<DiffText>,
    /// Where a merge is written, for a merge; `None` to compare.
    pub merge: Option<DiffMerge>,
    /// The texts, A and B, and C for a three-way comparison.
    pub texts: Vec<DiffText>,
}

/// What a file's bytes said about their own layout, kept so that saving an
/// unrelated one-line change does not rewrite every line of the file. Both of
/// these are invisible on screen and loud in a diff.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileShape {
    /// Whether the file began with a byte order mark.
    pub bom: bool,
    /// Whether lines ended `\r\n` rather than `\n`.
    pub crlf: bool,
    /// Whether the last line ended with a newline of its own.
    pub trailing_newline: bool,
}

/// One text of a diff session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffText {
    /// What it is called: a file's path, a revision, a buffer.
    pub label: String,
    /// What it reads, which may be a buffer's unsaved text.
    pub lines: Vec<String>,
    /// The file it is written back to, or `None` for one that is not (a
    /// revision, the clipboard).
    pub path: Option<PathBuf>,
    /// How that file's lines are laid out, which a write keeps: carried
    /// from where the text was read, so the diff does not read the file
    /// again to learn it. `None` where no file stands behind the text.
    pub shape: Option<FileShape>,
}

/// Where a diff session's merge is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffMerge {
    /// The file, or `None` for a merge only read.
    pub output: Option<PathBuf>,
}
