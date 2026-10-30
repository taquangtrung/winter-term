//! What a key does to the thing under the cursor: open it, apply or reset to
//! it, yank it, blame it, compare it, or ignore it.

use std::path::Path;

use crate::bytes_text;
use crate::editors::background::Background;
use crate::editors::git_exec::{self, Pathspec, ResetMode, Rev};
use crate::editors::text_file;
use crate::model::editor::{
    CommandOutput, DiffRequest, DiffText, EditorOutcome, Notice, SpawnRequest,
};
use crate::model::paint::row_text;

use super::popup::Popup;
use super::requests::Asked;
use super::rows::{FileRow, HunkRow, Item};
use super::{
    append_line, ignore_pattern, GitEditor, HeadCompare, ASK_REBASE,
    ASK_RESET_MIXED, DEFAULT_REMOTE, IGNORE_THREAD, IGNORE_WAIT, WORKTREE_READ_THREAD,
};

// ========================================================================
// GitEditor: acting on what is at point
// ========================================================================

impl GitEditor {
    /// Open a menu, which owns the next keystroke.
    pub(super) fn open_popup(&mut self, popup: Popup) -> EditorOutcome {
        self.menus.popup = Some(popup);
        EditorOutcome::Consumed
    }

    /// The keymap's `a` and `-`: apply or reverse the hunk under the cursor,
    /// doing nothing when the cursor is not on one.
    pub(super) fn apply_at_point(&self, target: git_exec::ApplyTarget) -> EditorOutcome {
        self.apply_revision_at_point(target)
            .or_else(|| self.apply_hunk(target))
            .unwrap_or(EditorOutcome::Consumed)
    }

    /// `a` or `-` over a stash or a commit, as magit has them: a stash
    /// applied with what it had staged, a commit's changes brought in or
    /// taken back out, left uncommitted. `None` off those rows.
    pub(super) fn apply_revision_at_point(&self, target: git_exec::ApplyTarget) -> Option<EditorOutcome> {
        let root = self.repo.root.as_ref()?;
        let reverse = target == git_exec::ApplyTarget::IndexReverse;
        let request = match self.selected()?.item.clone() {
            Item::Stash(index) if !reverse => {
                git_exec::stash_apply(root, &Rev::parse(&self.repo.stashes.get(index)?.name).ok()?)
            }
            Item::Commit(hash) => {
                let rev = Rev::parse(&hash).ok()?;
                match reverse {
                    true => git_exec::revert_changes(root, &rev),
                    false => git_exec::cherry_pick_changes(root, &rev),
                }
            }
            _ => return None,
        };
        Some(EditorOutcome::Job(request))
    }

    /// Reset to the commit under the cursor, which is the only revision the
    /// view can name without asking.
    pub(super) fn reset_at_point(&mut self, mode: ResetMode) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
            Some(Ok(rev)) => EditorOutcome::Job(git_exec::reset(&root, mode, &rev)),
            Some(Err(_)) | None => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
        }
    }

    /// Rebase interactively from the commit the cursor is on, keeping it and
    /// every commit after it in the plan, as magit's `r i` does. Off a commit
    /// there is nothing to start from, so the plan is of what the branch has
    /// over one picked from a list instead.
    ///
    /// Picking the checked-out branch itself, the only way in before, gives
    /// git an empty plan: rewriting the last few commits needs a start below
    /// them, which only a commit can say.
    pub(super) fn rebase_interactively(&mut self, root: &Path) -> EditorOutcome {
        match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
            Some(Ok(commit)) => EditorOutcome::Job(git_exec::rebase_from(root, &commit)),
            Some(Err(_)) | None => self.pick(ASK_REBASE, "Rebase onto", git_exec::Candidates::Branches),
        }
    }

    /// Copy what the cursor is on: a commit hash, or a path.
    /// Copy the row under the cursor as it reads.
    pub(super) fn yank_line(&self) -> EditorOutcome {
        match self.selected().map(|row| row_text(&row.spans)) {
            Some(text) if !text.trim().is_empty() => EditorOutcome::Yank(text.trim().to_string()),
            Some(_) | None => EditorOutcome::Consumed,
        }
    }

    pub(super) fn yank_at_point(&mut self) -> EditorOutcome {
        let value = self.revision_at_cursor().or_else(|| self.path_at_cursor());
        match value {
            Some(value) => EditorOutcome::Yank(value),
            None => EditorOutcome::Consumed,
        }
    }

    /// Blame the file the cursor is on, or on one of its hunks: a heading or a
    /// commit has no single path to run against.
    pub(super) fn blame_at_point(&mut self) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        match self.selected().map(|row| row.item.clone()) {
            Some(Item::File(FileRow { path, .. })) | Some(Item::Hunk(HunkRow { path, .. })) => {
                let job = git_exec::blame(&root, &Pathspec::new(path.clone()));
                EditorOutcome::Job(self.in_flight.ask(job, Asked::Blame(path)))
            }
            _ => {
                self.notice = Some(Notice::error("no file to blame here"));
                EditorOutcome::Consumed
            }
        }
    }

    /// The file at point as `HEAD` has it against the working tree, in the
    /// diff tool: `HEAD`'s copy is read first.
    pub(super) fn diff_tool_at_point(&mut self) -> EditorOutcome {
        let (Some(root), Some(path)) = (self.repo.root.clone(), self.path_at_cursor()) else {
            self.notice = Some(Notice::error("no file to compare here"));
            return EditorOutcome::Consumed;
        };
        let job = git_exec::show_head(&root, &path);
        EditorOutcome::Job(self.in_flight.ask(job, Asked::Head(path)))
    }

    /// `HEAD`'s copy of the file the diff tool was asked for: compare it
    /// with the working tree's.
    pub(super) fn head_read(&mut self, output: &CommandOutput, path: String) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let head: Vec<String> = match output.succeeded() {
            true => output.stdout.lines().map(str::to_string).collect(),
            // A file the last commit does not have is compared with nothing.
            false => Vec::new(),
        };
        // The working tree's copy is read off the window's thread: a large
        // file or a slow mount would otherwise hold the window.
        let file = root.join(bytes_text::to_path(&path));
        let from = file.clone();
        let read = Background::start(WORKTREE_READ_THREAD, move |_| text_file::load(&from).ok());
        self.head_compare = Some(HeadCompare { file, head, path, read });
        EditorOutcome::Consumed
    }

    /// The diff tool's comparison of `HEAD`'s copy with the working tree's,
    /// once the working tree's has been read: `None` while it is still
    /// being read, or when nothing is. A file the working tree has no text
    /// of (gone, or not text) is compared as empty and is no file to write
    /// back to, so a save from the diff cannot wipe it.
    pub(super) fn take_head_compare(&mut self) -> Option<EditorOutcome> {
        let done = self.head_compare.take_if(|compare| compare.read.is_done())?;
        let loaded = done.read.poll().flatten();
        let (lines, shape) = match loaded {
            Some(loaded) => (loaded.lines, Some(loaded.shape)),
            None => (Vec::new(), None),
        };
        Some(EditorOutcome::Diff(DiffRequest {
            ancestor: None,
            merge: None,
            texts: vec![
                DiffText {
                    label: format!("HEAD:{}", done.path),
                    lines: done.head,
                    path: None,
                    shape: None,
                },
                DiffText {
                    label: done.path,
                    lines,
                    path: shape.is_some().then_some(done.file),
                    shape,
                },
            ],
        }))
    }

    /// Ask for the remote's URL, which the reply turns into a browser editor.
    pub(super) fn open_in_remote(&self) -> EditorOutcome {
        match &self.repo.root {
            Some(root) => EditorOutcome::Job(git_exec::remote_url(root, DEFAULT_REMOTE)),
            None => EditorOutcome::Consumed,
        }
    }

    /// Run git in a pane of its own, for a command that wants a terminal.
    pub(super) fn spawn_git(&self, root: &std::path::Path, args: &[&str]) -> EditorOutcome {
        EditorOutcome::Spawn(SpawnRequest {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: root.to_path_buf(),
            program: "git".to_string(),
        })
    }

    /// The path the cursor is on, whatever kind of row it is.
    pub(super) fn path_at_cursor(&self) -> Option<String> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => Some(file.path.clone()),
            Some(Item::Hunk(hunk)) => Some(hunk.path.clone()),
            _ => None,
        }
    }

    /// The revision the cursor is on, for a command that takes one: in the
    /// output while one fills the view (a log's commits), else the status.
    pub(super) fn revision_at_cursor(&self) -> Option<String> {
        let row = match &self.output {
            Some(output) => output.rows.get(output.list.cursor()),
            None => self.selected(),
        };
        match row.map(|row| &row.item) {
            Some(Item::Commit(hash)) => Some(hash.clone()),
            _ => None,
        }
    }

    /// Append the path under the cursor to `.gitignore`, by extension when
    /// asked. Writing the file directly is simpler than any git command for it.
    pub(super) fn ignore_path(&mut self, by_extension: bool) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let Some(path) = self.path_at_cursor() else {
            return EditorOutcome::Consumed;
        };
        let pattern = ignore_pattern(&path, by_extension);
        let file = root.join(".gitignore");
        // Written off the window's thread: the file may sit on a slow mount
        // like any other in the tree.
        let work = Background::start(IGNORE_THREAD, move |_| {
            append_line(&file, &pattern)
                .map(|()| pattern)
                .map_err(|e| e.to_string())
        });
        match work.wait(IGNORE_WAIT) {
            Some(written) => self.take_ignored(written),
            None => {
                self.ignoring = Some(work);
                EditorOutcome::Consumed
            }
        }
    }

    /// Report what appending to `.gitignore` came to, and read the tree
    /// again so the ignored file leaves the view.
    pub(super) fn take_ignored(&mut self, written: Result<String, String>) -> EditorOutcome {
        match written {
            Ok(pattern) => {
                self.notice = Some(Notice::info(format!("ignored {pattern}")));
                self.refresh()
            }
            Err(e) => {
                self.notice = Some(Notice::error(format!("failed: {e}")));
                EditorOutcome::Consumed
            }
        }
    }
}
