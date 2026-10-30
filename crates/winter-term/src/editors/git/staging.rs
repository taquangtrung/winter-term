//! Staging, unstaging, and discarding: what is under the cursor, a selection of
//! rows, or the lines of a hunk.

use std::ops::RangeInclusive;

use crate::editors::git_exec::{self, Pathspec, Rev};
use crate::editors::git_parse::{RefKind, Section};
use crate::model::editor::{EditorOutcome, Notice, PromptRequest};

use super::rows::{FileRow, HunkRow, Item};
use super::{Discard, GitEditor, RowsAct, ASK_DISCARD};

// ========================================================================
// GitEditor: staging and discarding
// ========================================================================

impl GitEditor {
    /// The hunk row `row` is in, and which line of that hunk's body it is;
    /// `None` for the hunk's own header line, or a row that is no hunk's.
    pub(super) fn hunk_line_at(&self, row: usize) -> Option<(HunkRow, usize)> {
        let Some(Item::Hunk(hunk)) = self.rows.get(row).map(|view| view.item.clone()) else {
            return None;
        };
        let first = (0..=row)
            .rev()
            .take_while(|at| self.rows[*at].item == Item::Hunk(hunk.clone()))
            .last()?;
        let body = (row - first).checked_sub(1)?;
        Some((hunk, body))
    }

    /// Stage, unstage, or discard (once confirmed) the lines `lines` of
    /// `hunk`'s body, leaving its other changes where they are.
    pub(super) fn act_on_hunk_lines(
        &mut self,
        hunk: HunkRow,
        lines: RangeInclusive<usize>,
        act: RowsAct,
    ) -> EditorOutcome {
        let (wanted, verb) = match act {
            RowsAct::Discard => (Section::Unstaged, "discard"),
            RowsAct::Stage => (Section::Unstaged, "stage"),
            RowsAct::Unstage => (Section::Staged, "unstage"),
        };
        if hunk.section != wanted {
            self.notice = Some(Notice::error(format!("nothing to {verb} in this section")));
            return EditorOutcome::Consumed;
        }
        let file = FileRow {
            path: hunk.path.clone(),
            section: hunk.section,
        };
        let unstage = act == RowsAct::Unstage;
        let patch = self
            .repo.diffs
            .get(&file)
            .and_then(|diff| diff.patch_for_lines(hunk.index, lines, unstage));
        let (Some(root), Some(patch)) = (self.repo.root.clone(), patch) else {
            self.notice = Some(Notice::error("no change on those lines"));
            return EditorOutcome::Consumed;
        };
        // Staging's patch, taken back out of the working tree, is what
        // discarding those lines is.
        match act {
            RowsAct::Discard => {
                self.menus.pending_discard = Some(Discard::Lines(patch));
                self.ask_discard_pending()
            }
            RowsAct::Stage => EditorOutcome::Job(git_exec::apply_patch(&root, patch, git_exec::ApplyTarget::Index)),
            RowsAct::Unstage => {
                EditorOutcome::Job(git_exec::apply_patch(&root, patch, git_exec::ApplyTarget::IndexReverse))
            }
        }
    }

    /// `s`, `u`, or `x` over the rows a selection covers, `rows` on screen:
    /// the lines it covers where they are all in one hunk, else every file
    /// it covers.
    pub(super) fn act_on_selected_rows(&mut self, rows: RangeInclusive<usize>, act: RowsAct) -> EditorOutcome {
        let last_row = self.rows.len().saturating_sub(1);
        let Some(first) = self.row_at_screen(*rows.start()) else {
            return EditorOutcome::Consumed;
        };
        let last = self.row_at_screen(*rows.end()).unwrap_or(last_row);
        if let (Some((hunk, from)), Some((other, to))) = (self.hunk_line_at(first), self.hunk_line_at(last)) {
            if hunk == other {
                return self.act_on_hunk_lines(hunk, from.min(to)..=from.max(to), act);
            }
        }
        let mut files: Vec<FileRow> = Vec::new();
        for row in &self.rows[first..=last.max(first)] {
            if let Item::File(file) = &row.item {
                if !files.contains(file) {
                    files.push(file.clone());
                }
            }
        }
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let paths_where = |keep: &dyn Fn(Section) -> bool| -> Vec<Pathspec> {
            files
                .iter()
                .filter(|file| keep(file.section))
                .map(|file| Pathspec::new(file.path.clone()))
                .collect()
        };
        let (paths, verb) = match act {
            RowsAct::Stage => (paths_where(&|section| section != Section::Staged), "stage"),
            RowsAct::Unstage => (paths_where(&|section| section == Section::Staged), "unstage"),
            RowsAct::Discard => {
                let tracked = paths_where(&|section| section != Section::Untracked);
                let untracked = paths_where(&|section| section == Section::Untracked);
                let discard = match (tracked.is_empty(), untracked.is_empty()) {
                    (false, true) => Discard::Tracked(tracked),
                    (true, false) => Discard::Untracked(untracked),
                    (true, true) => return self.say_nothing_selected("discard"),
                    (false, false) => {
                        self.notice = Some(Notice::error("select tracked and untracked files apart to discard them"));
                        return EditorOutcome::Consumed;
                    }
                };
                self.menus.pending_discard = Some(discard);
                return self.ask_discard_pending();
            }
        };
        if paths.is_empty() {
            return self.say_nothing_selected(verb);
        }
        EditorOutcome::Job(match act {
            RowsAct::Unstage => git_exec::unstage(&root, &paths),
            RowsAct::Discard | RowsAct::Stage => git_exec::stage(&root, &paths),
        })
    }

    /// Say a selection covers nothing a key could `verb`.
    pub(super) fn say_nothing_selected(&mut self, verb: &str) -> EditorOutcome {
        self.notice = Some(Notice::error(format!("nothing selected to {verb}: select files, or lines of one hunk")));
        EditorOutcome::Consumed
    }

    /// Stage, unstage, or discard the hunk under the cursor by applying its own
    /// patch, which leaves the rest of the file untouched.
    pub(super) fn apply_hunk(&self, target: git_exec::ApplyTarget) -> Option<EditorOutcome> {
        let Some(Item::Hunk(hunk)) = self.selected().map(|row| row.item.clone()) else {
            return None;
        };
        // A hunk goes into the index from the unstaged side and back out from
        // the staged one; from the other side, or from a conflict's combined
        // diff, the patch is against content it does not describe.
        let from = match target {
            git_exec::ApplyTarget::IndexReverse => Section::Staged,
            git_exec::ApplyTarget::Index | git_exec::ApplyTarget::WorktreeReverse => Section::Unstaged,
        };
        if hunk.section != from {
            return Some(EditorOutcome::Consumed);
        }
        let root = self.repo.root.as_ref()?;
        let file = FileRow {
            path: hunk.path.clone(),
            section: hunk.section,
        };
        let patch = self.repo.diffs.get(&file)?.patch_for(hunk.index)?;
        Some(EditorOutcome::Job(git_exec::apply_patch(root, patch, target)))
    }

    /// Stage what the cursor is on. Untracked files are staged by the same key,
    /// since "add this" is what the user means either way.
    pub(super) fn stage(&self, all: bool) -> EditorOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(git_exec::ApplyTarget::Index) {
                return outcome;
            }
        }
        let Some(root) = &self.repo.root else {
            return EditorOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(git_exec::stage(root, &Pathspec::all(&paths)))
    }

    /// Stage everything, the way Magic's stage-all does: every untracked file
    /// when the cursor is in that section, every tracked change anywhere
    /// else. Splitting the two halves keeps an unasked-for file from being
    /// swept into the index unread.
    pub(super) fn stage_all(&self) -> EditorOutcome {
        let Some(root) = &self.repo.root else {
            return EditorOutcome::Consumed;
        };
        if self.section_at_cursor() == Some(Section::Untracked) {
            return self.stage_untracked();
        }
        EditorOutcome::Job(git_exec::stage_all_tracked(root))
    }

    /// Stage every untracked file the view lists, wherever the cursor is,
    /// by name: only what is on screen goes in, never a file that appeared
    /// since the last read.
    pub(super) fn stage_untracked(&self) -> EditorOutcome {
        let Some(root) = &self.repo.root else {
            return EditorOutcome::Consumed;
        };
        let paths = self.repo.paths_in(Section::Untracked);
        if paths.is_empty() {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(git_exec::stage(root, &Pathspec::all(&paths)))
    }

    pub(super) fn unstage(&self, all: bool) -> EditorOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(git_exec::ApplyTarget::IndexReverse) {
                return outcome;
            }
        }
        let Some(root) = &self.repo.root else {
            return EditorOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(git_exec::unstage(root, &Pathspec::all(&paths)))
    }

    /// Ask before throwing away work, naming what will go: nothing recovers a
    /// discarded working-tree change. What will go is decided now, while the
    /// question names it, and kept for the answer: the tree may be read again
    /// while the question is open, and a re-read that moved the rows must not
    /// turn a yes about one file into a discard of another.
    pub(super) fn ask_discard(&mut self) -> EditorOutcome {
        // A staged hunk is the index's change, not the working tree's, so
        // reversing it out of the tree reverts the wrong thing; and `restore`
        // on a staged file throws away its unstaged changes, which are not
        // what is on screen.
        if self.section_at_cursor() == Some(Section::Staged) {
            self.notice = Some(Notice::error("nothing to discard in this section: unstage it first"));
            return EditorOutcome::Consumed;
        }
        let Some(discard) = self.discard_at_point() else {
            return EditorOutcome::Consumed;
        };
        self.menus.pending_discard = Some(discard);
        self.ask_discard_pending()
    }

    /// Ask whether to go ahead with the discard waiting on an answer.
    pub(super) fn ask_discard_pending(&mut self) -> EditorOutcome {
        let Some(discard) = &self.menus.pending_discard else {
            return EditorOutcome::Consumed;
        };
        let label = match discard {
            // A remote branch goes from the server, for everyone, not only
            // from this clone's list: the question says so.
            Discard::Ref(entry) => match (entry.kind, self.repo.split_remote(&entry.name)) {
                (RefKind::Remote, Some((remote, branch))) => {
                    format!("Delete branch {branch} on the server {remote}, for everyone?")
                }
                (RefKind::Head | RefKind::Local | RefKind::Remote | RefKind::Tag, _) => {
                    format!("Delete {}?", entry.name)
                }
            },
            Discard::Stash(name) => format!("Drop {}?", name.as_str()),
            Discard::Stashes => "Drop every stash?".to_string(),
            Discard::Hunk(_) => format!(
                "Discard this hunk of {}?",
                self.targets().first().map_or("the file", String::as_str)
            ),
            Discard::Lines(_) => "Discard the selected lines?".to_string(),
            Discard::Tracked(paths) | Discard::Untracked(paths) => match &paths[..] {
                [path] => format!("Discard changes to {}?", path.as_str()),
                _ => format!("Discard changes to {} files?", paths.len()),
            },
        };
        EditorOutcome::Prompt(PromptRequest::confirm(ASK_DISCARD, &label))
    }

    /// What discarding at the cursor would throw away: the hunk under it, or
    /// the files it is on, deleted where git does not track them.
    pub(super) fn discard_at_point(&self) -> Option<Discard> {
        match self.selected().map(|row| row.item.clone()) {
            Some(Item::Stash(index)) => {
                let name = Rev::parse(&self.repo.stashes.get(index)?.name).ok()?;
                return Some(Discard::Stash(name));
            }
            Some(Item::StashHeading) if !self.repo.stashes.is_empty() => return Some(Discard::Stashes),
            _ => {}
        }
        if let Some(Item::Hunk(hunk)) = self.selected().map(|row| row.item.clone()) {
            let file = FileRow {
                path: hunk.path.clone(),
                section: hunk.section,
            };
            return self
                .repo.diffs
                .get(&file)?
                .patch_for(hunk.index)
                .map(Discard::Hunk);
        }
        let paths = Pathspec::all(&self.targets());
        if paths.is_empty() {
            return None;
        }
        // Untracked files are not in the index, so `restore` cannot reach
        // them: deleting is what discarding means for those.
        Some(match self.section_at_cursor() == Some(Section::Untracked) {
            true => Discard::Untracked(paths),
            false => Discard::Tracked(paths),
        })
    }

    /// Carry out the discard the reader said yes to.
    pub(super) fn discard(&mut self) -> EditorOutcome {
        let (Some(root), Some(discard)) = (self.repo.root.clone(), self.menus.pending_discard.take()) else {
            return EditorOutcome::Consumed;
        };
        EditorOutcome::Job(match discard {
            Discard::Hunk(patch) | Discard::Lines(patch) => {
                git_exec::apply_patch(&root, patch, git_exec::ApplyTarget::WorktreeReverse)
            }
            Discard::Ref(entry) => match self.delete_ref(&root, &entry) {
                Some(job) => job,
                None => return EditorOutcome::Consumed,
            },
            Discard::Stash(name) => git_exec::stash_drop(&root, &name),
            Discard::Stashes => git_exec::stash(&root, "clear"),
            Discard::Tracked(paths) => git_exec::discard(&root, &paths),
            Discard::Untracked(paths) => git_exec::remove_untracked(&root, &paths),
        })
    }
}
