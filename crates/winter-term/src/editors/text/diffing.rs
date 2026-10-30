//! Starting a diff session from the buffer being edited, its text as it
//! stands (edits not yet saved included) one side of it: against another
//! file or a revision git has; against two other files at once; merged with another file into this one; or the
//! merge conflicts it holds, resolved in a merge of their two sides. (The
//! buffer against its file on disk, the clipboard, or another buffer is
//! `compare`'s.)

use std::path::{Path, PathBuf};

use crate::editors::text_file;
use crate::model::editor::{
    CommandOutput, CommandRequest, DiffMerge, DiffRequest, DiffText, EditorOutcome, JobRequest,
    Notice, PromptMode, PromptRequest, MERGE_CONFLICTS_3WAY,
    DIFF_FILE, MERGE_FILE, DIFF_REVISION, DIFF_FILES,
};

use super::conflict::{self, Resolution};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question.
pub(super) const ASK_DIFF_FILE: &str = "diff-file";
pub(super) const ASK_DIFF_MERGE: &str = "diff-merge";
pub(super) const ASK_DIFF_FILES: &str = "diff-three";
pub(super) const ASK_DIFF_REVISION: &str = "diff-revision";

/// Which job a revision's text is read by.
pub(super) const TAG_DIFF_REVISION: &str = "diff-revision";

/// The revision asked for first.
const DEFAULT_REVISION: &str = "HEAD";

// ========================================================================
// TextEditor: diff sessions
// ========================================================================

impl TextEditor {
    /// Run diff command `name`, or `None` for a name that is none.
    pub(super) fn diff_command(&mut self, name: &str) -> Option<EditorOutcome> {
        let dir = self.doc().path.parent().map_or_else(String::new, |dir| format!("{}/", dir.display()));
        let ask = |label: &str, initial: String, tag: &'static str| {
            EditorOutcome::Prompt(PromptRequest {
                initial,
                label: label.to_string(),
                mode: PromptMode::Text,
                tag,
            })
        };
        Some(match name {
            DIFF_FILE => ask("Compare with file: ", dir, ASK_DIFF_FILE),
            MERGE_FILE => ask("Merge into this file from: ", dir, ASK_DIFF_MERGE),
            DIFF_FILES => ask("Compare with two files (B C): ", dir, ASK_DIFF_FILES),
            DIFF_REVISION => ask("Compare with revision: ", DEFAULT_REVISION.to_string(), ASK_DIFF_REVISION),
            MERGE_CONFLICTS_3WAY => self.merge_conflicts(),
            _ => return None,
        })
    }

    /// The answer to one of this module's questions, or `None` for another
    /// module's.
    pub(super) fn on_diff_prompt(&mut self, tag: &str, answer: Option<String>) -> Option<EditorOutcome> {
        let known = [ASK_DIFF_FILE, ASK_DIFF_MERGE, ASK_DIFF_FILES, ASK_DIFF_REVISION];
        if !known.contains(&tag) {
            return None;
        }
        let Some(answer) = answer.map(|answer| answer.trim().to_string()).filter(|answer| !answer.is_empty()) else {
            return Some(EditorOutcome::Consumed);
        };
        Some(match tag {
            ASK_DIFF_FILE => match self.file_side(&answer) {
                Some(other) => compare(vec![self.side_of(self.at), other]),
                None => EditorOutcome::Consumed,
            },
            ASK_DIFF_MERGE => match self.file_side(&answer) {
                Some(other) => EditorOutcome::Diff(DiffRequest {
                    ancestor: None,
                    merge: Some(DiffMerge {
                        output: Some(self.doc().path.clone()),
                    }),
                    texts: vec![self.side_of(self.at), other],
                }),
                None => EditorOutcome::Consumed,
            },
            ASK_DIFF_FILES => {
                let names: Vec<&str> = answer.split_whitespace().collect();
                let [b, c] = names[..] else {
                    self.modal.report(Notice::error("Diff: name two files, B and C"));
                    return Some(EditorOutcome::Consumed);
                };
                match (self.file_side(b), self.file_side(c)) {
                    (Some(b), Some(c)) => compare(vec![self.side_of(self.at), b, c]),
                    _ => EditorOutcome::Consumed,
                }
            }
            ASK_DIFF_REVISION => self.read_revision(&answer),
            _ => EditorOutcome::Consumed,
        })
    }

    /// The file `name` names, relative to the buffer's directory, as a side
    /// written back to it; `None`, said, where it cannot be read.
    fn file_side(&mut self, name: &str) -> Option<DiffText> {
        let named = PathBuf::from(shellexpand_home(name));
        let path = match named.is_absolute() {
            true => named,
            false => self.doc().path.parent().unwrap_or(Path::new(".")).join(named),
        };
        match text_file::load(&path) {
            Ok(loaded) => Some(DiffText {
                label: crate::model::path::abbreviate_home(&path),
                lines: loaded.lines,
                path: Some(path),
                shape: Some(loaded.shape),
            }),
            Err(e) => {
                self.modal.report(Notice::error(format!("Diff: {}: {e}", path.display())));
                None
            }
        }
    }

    /// Ask git for the buffer's file as `revision` has it.
    fn read_revision(&mut self, revision: &str) -> EditorOutcome {
        let path = self.doc().path.clone();
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return EditorOutcome::Consumed;
        };
        self.diff_revision = Some(revision.to_string());
        EditorOutcome::Job(JobRequest::Command(CommandRequest {
            id: 0,
            args: vec!["show".to_string(), format!("{revision}:./{}", name.to_string_lossy())],
            cwd: dir.to_path_buf(),
            env: Vec::new(),
            program: "git".to_string(),
            stdin: None,
            tag: TAG_DIFF_REVISION,
        }))
    }

    /// Git answered with the revision's text: compare it with the buffer.
    pub(super) fn revision_read(&mut self, output: &CommandOutput) -> EditorOutcome {
        let revision = self.diff_revision.take().unwrap_or_else(|| DEFAULT_REVISION.to_string());
        if !output.succeeded() {
            self.modal.report(Notice::error(format!("Diff: {}", output.failure())));
            return EditorOutcome::Consumed;
        }
        let name = self.doc().path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        compare(vec![
            DiffText {
                label: format!("{revision}:{name}"),
                lines: output.stdout.lines().map(str::to_string).collect(),
                path: None,
                shape: None,
            },
            self.side_of(self.at),
        ])
    }

    /// The buffer's merge conflicts, resolved in a merge: A is the file with
    /// every conflict taking our side, B taking theirs, the ancestor its
    /// version where every conflict carries one, and the merge is written
    /// back over the file.
    fn merge_conflicts(&mut self) -> EditorOutcome {
        let lines = self.buffer().lines().to_vec();
        let conflicts = conflict::find(&lines);
        if conflicts.is_empty() {
            self.modal.report(Notice::info("Diff: no merge conflicts in this file"));
            return EditorOutcome::Consumed;
        }
        let path = self.doc().path.clone();
        let name = crate::model::path::abbreviate_home(&path);
        // Each version is the file's own, laid out as it is: the merge is
        // written back over the file in the first one's shape.
        let shape = Some(self.doc().shape());
        let side = |resolution: Resolution, label: &str| {
            version(&lines, &conflicts, resolution).map(|lines| DiffText {
                label: format!("{name} ({label})"),
                lines,
                path: None,
                shape,
            })
        };
        let (Some(ours), Some(theirs)) = (side(Resolution::Ours, "ours"), side(Resolution::Theirs, "theirs")) else {
            return EditorOutcome::Consumed;
        };
        EditorOutcome::Diff(DiffRequest {
            ancestor: side(Resolution::Base, "ancestor"),
            merge: Some(DiffMerge { output: Some(path) }),
            texts: vec![ours, theirs],
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A comparison of `texts`.
fn compare(texts: Vec<DiffText>) -> EditorOutcome {
    EditorOutcome::Diff(DiffRequest {
        ancestor: None,
        merge: None,
        texts,
    })
}

/// `lines` with every conflict in `conflicts` resolved as `resolution`
/// keeps it, or `None` where one cannot be (an ancestor a conflict lacks).
fn version(lines: &[String], conflicts: &[conflict::Conflict], resolution: Resolution) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut at = 0;
    for found in conflicts {
        out.extend_from_slice(&lines[at..found.start]);
        out.extend(found.kept(lines, resolution)?);
        at = found.end + 1;
    }
    out.extend_from_slice(&lines[at.min(lines.len())..]);
    Some(out)
}

/// `name` with a leading `~` read as the home directory.
fn shellexpand_home(name: &str) -> String {
    match (name.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => name.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn test_each_side_of_a_conflicted_file_is_the_file_with_that_side_kept() {
        let text = lines("a\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\nz");
        let conflicts = conflict::find(&text);
        assert_eq!(version(&text, &conflicts, Resolution::Ours), Some(lines("a\nours\nz")));
        assert_eq!(version(&text, &conflicts, Resolution::Theirs), Some(lines("a\ntheirs\nz")));
        assert_eq!(version(&text, &conflicts, Resolution::Base), Some(lines("a\nold\nz")));
        let plain = lines("<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> t");
        assert_eq!(version(&plain, &conflict::find(&plain), Resolution::Base), None, "no ancestor to show");
    }
}
