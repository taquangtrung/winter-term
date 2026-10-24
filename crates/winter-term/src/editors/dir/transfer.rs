//! The operations a listing runs on its entries: creating, renaming,
//! copying, moving, deleting, and changing mode, and the questions each asks
//! first.
//!
//! What an operation acts on is taken when its question is asked, and the
//! answer acts on exactly that: the listing may be read again while the
//! question is open, and a yes about one entry must never land on another.

use std::path::{Path, PathBuf};

use crate::model::editor::{
    EditorOutcome, FileOps, JobRequest, Notice, OpenTarget, PickQuestion, PromptMode, PromptRequest,
};

use super::{
    name_of, ops, DirEditor, ASK_COPY, ASK_DELETE, ASK_MKDIR, ASK_MODE, ASK_MOVE, ASK_NEW_FILE,
    ASK_RENAME, ASK_TRASH,
};

// ========================================================================
// DirEditor: operations
// ========================================================================

impl DirEditor {
    /// Ask for a yes-or-no answer, for what cannot be undone.
    pub(super) fn confirm(&self, tag: &'static str, label: String) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag,
        })
    }

    /// Ask to rename the entry under the cursor, starting from its own name.
    pub(super) fn ask_rename(&mut self) -> EditorOutcome {
        let Some(row) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let (path, name) = (row.entry.path.clone(), row.entry.name.clone());
        self.pending_targets = Some(vec![path]);
        self.ask(ASK_RENAME, "Rename to: ".to_string(), name)
    }

    /// Ask for the mode to give the targets.
    pub(super) fn ask_mode(&mut self) -> EditorOutcome {
        self.pending_targets = Some(self.targets());
        self.ask(ASK_MODE, "Mode: ".to_string(), String::new())
    }

    /// Ask to send the targets to the trash (`x`), or with `for_good` to
    /// delete them past recovering (`X`), naming how many there are so a
    /// marked set is never removed on a glance at one filename.
    pub(super) fn ask_delete(&mut self, for_good: bool) -> EditorOutcome {
        let (tag, verb) = match for_good {
            true => (ASK_DELETE, "Delete for good"),
            false => (ASK_TRASH, "Trash"),
        };
        let targets = self.targets();
        let label = match targets.len() {
            0 => return EditorOutcome::Consumed,
            1 => format!("{verb} {}? (y/n) ", name_of(&targets[0])),
            count => format!("{verb} {count} entries? (y/n) "),
        };
        self.pending_targets = Some(targets);
        self.confirm(tag, label)
    }

    /// Ask where the targets go, for a copy or a move.
    pub(super) fn ask_destination(&mut self, tag: &'static str, verb: &str) -> EditorOutcome {
        let targets = self.targets();
        let Some(first) = targets.first().cloned() else {
            return EditorOutcome::Consumed;
        };
        let count = targets.len();
        self.pending_targets = Some(targets);
        // One entry is transferred under a new name, which is nothing to
        // list; several go into a directory, which is. Either way a listing
        // open beside this one is where it most likely goes, so it is first.
        if count == 1 {
            let initial = self
                .neighbors
                .first()
                .map(|dir| format!("{}/", dir.display()))
                .unwrap_or_default();
            return self.ask(tag, format!("{verb} {} to: ", name_of(&first)), initial);
        }
        let label = format!("{verb} {count} entries into");
        let mut choices: Vec<String> = self
            .neighbors
            .iter()
            .map(|dir| dir.display().to_string())
            .collect();
        choices.extend(self.directory_names());
        match PickQuestion::new(tag, &label).over(choices) {
            Some(request) => EditorOutcome::Pick(request),
            None => self.ask(tag, format!("{label}: "), String::new()),
        }
    }

    /// The directories listed here, by name. Names rather than paths because
    /// that is all a destination may be: a transfer resolves its answer
    /// against this directory and refuses anything that reaches outside it
    /// (see [`ops::resolve_name`]).
    fn directory_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.entry.is_dir())
            .map(|row| name_of(&row.entry.path))
            .collect()
    }

    /// Carry out the answer to a question, and report what happened.
    pub(super) fn apply_answer(&mut self, tag: &'static str, answer: &str) -> EditorOutcome {
        let targets = self.pending_targets.take().unwrap_or_default();
        // The operations a large tree makes slow run as a job, off the
        // window's thread; the report comes back through `on_job`.
        let work = match tag {
            ASK_COPY => Some(self.transfer_pairs(&targets, answer).map(FileOps::Copy)),
            ASK_MOVE => Some(self.transfer_pairs(&targets, answer).map(FileOps::Move)),
            ASK_DELETE => Some(Ok(FileOps::Delete(targets.clone()))),
            ASK_TRASH => Some(Ok(FileOps::Trash(targets.clone()))),
            _ => None,
        };
        match work {
            Some(Ok(work)) if file_ops_is_empty(&work) => return EditorOutcome::Consumed,
            Some(Ok(work)) => return EditorOutcome::Job(JobRequest::FileOps(work)),
            Some(Err(report)) => {
                self.notice = Some(Notice::error(format!("failed: {report}")));
                return EditorOutcome::Consumed;
            }
            None => {}
        }
        let result = match tag {
            ASK_MKDIR => self.create(answer, true),
            ASK_MODE => change_mode(&targets, answer),
            ASK_NEW_FILE => self.create(answer, false),
            ASK_RENAME => rename(&targets, answer),
            _ => Ok(String::new()),
        };
        let created = result.is_ok();
        self.notice = Some(match result {
            Ok(report) => Notice::info(report),
            Err(report) => Notice::error(format!("failed: {report}")),
        });
        self.marks.clear();
        // A file is made to be written in, so it opens with the cursor in it
        // rather than as one more empty row to find again. The listing lands
        // on it too, for when the editor closes over the top of it.
        let made = (tag == ASK_NEW_FILE && created)
            .then(|| ops::resolve_name(&self.root, answer))
            .flatten();
        self.select_after_read = made.clone();
        self.reload();
        match made {
            Some(path) => EditorOutcome::OpenPath(OpenTarget::file(path)),
            None => EditorOutcome::Consumed,
        }
    }

    fn create(&mut self, name: &str, directory: bool) -> Result<String, String> {
        let Some(path) = ops::resolve_name(&self.root, name) else {
            return Err(format!("not a name: {name}"));
        };
        let result = if directory {
            ops::create_dir(&path)
        } else {
            ops::create_file(&path)
        };
        result
            .map(|()| format!("created {}", name_of(&path)))
            .map_err(|e| e.to_string())
    }

    /// Where each of `targets` goes when copied or moved to `destination`: a
    /// name in this directory for a single target and a directory for
    /// several, or a whole path, written out in full, to a directory
    /// elsewhere (the listing beside this one, say). Resolved here, cheaply;
    /// the copying or moving itself is a job.
    fn transfer_pairs(
        &self,
        targets: &[PathBuf],
        destination: &str,
    ) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let single = targets.len() == 1;
        let elsewhere = Path::new(destination.trim());
        targets
            .iter()
            .map(|from| {
                let to = if elsewhere.is_absolute() {
                    match elsewhere.is_dir() {
                        true => Some(elsewhere.join(name_of(from))),
                        // A whole path to something not there yet names the
                        // one target's new home outright, in a directory
                        // that is.
                        false => (single && elsewhere.parent().is_some_and(Path::is_dir))
                            .then(|| elsewhere.to_path_buf()),
                    }
                } else if single {
                    ops::resolve_name(&self.root, destination)
                } else {
                    ops::resolve_name(&self.root, destination).map(|dir| dir.join(name_of(from)))
                };
                to.map(|to| (from.clone(), to))
                    .ok_or_else(|| format!("not a name: {destination}"))
            })
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Rename the one entry of `targets` to `name`, in the directory it is in:
/// an entry listed under an expanded directory stays in that directory, it
/// does not move up to the listing's root.
fn rename(targets: &[PathBuf], name: &str) -> Result<String, String> {
    let [from] = targets else {
        return Err("nothing selected".to_string());
    };
    let Some(to) = from.parent().and_then(|dir| ops::resolve_name(dir, name)) else {
        return Err(format!("not a name: {name}"));
    };
    ops::move_entry(from, &to)
        .map(|()| format!("renamed to {}", name_of(&to)))
        .map_err(|e| e.to_string())
}

/// Give each of `targets` the octal mode `text` names.
fn change_mode(targets: &[PathBuf], text: &str) -> Result<String, String> {
    let Some(mode) = ops::parse_mode(text) else {
        return Err(format!("not an octal mode: {text}"));
    };
    let mut done = 0;
    for path in targets {
        ops::set_mode(path, mode).map_err(|e| format!("{e} ({done} done)"))?;
        done += 1;
    }
    Ok(format!("set mode on {done}"))
}

/// Whether `work` has nothing to do: the question it answers had no targets
/// left by the time it was answered.
fn file_ops_is_empty(work: &FileOps) -> bool {
    match work {
        FileOps::Copy(pairs) | FileOps::Move(pairs) => pairs.is_empty(),
        FileOps::Delete(paths) | FileOps::Trash(paths) => paths.is_empty(),
    }
}
