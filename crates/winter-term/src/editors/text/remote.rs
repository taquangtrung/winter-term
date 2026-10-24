//! Reading and writing a buffer's file wherever its path says it is: the
//! local disk, or root, a container, or an SSH host (see [`crate::remote`]).
//!
//! Where there is a `sudo` (see [`remote::sudo_available`]), a local file
//! the user may not read opens as root instead (`sudo:///...`), and one they
//! may not write is offered to root when saved. Where `sudo` wants a
//! password, the editor asks for it in a secret prompt and hands it to
//! `sudo -v` once, without keeping it: sudo's own timestamp then spares the
//! next read or write the question, as it does in a shell.

use std::io;
use std::path::{Path, PathBuf};

use crate::model::editor::{EditorOutcome, Notice, PromptMode, PromptReply, PromptRequest};
use crate::remote::{self, Remote, RemoteError};

use super::file::{self, FileStamp, LoadedFile};
use super::{name_of, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question asked on the way to a file elsewhere.
pub(super) const ASK_AS_ROOT: &str = "as-root";
pub(super) const ASK_PASSWORD_OPEN: &str = "password-open";
pub(super) const ASK_PASSWORD_SAVE: &str = "password-save";

/// What the password prompts ask.
const PASSWORD_LABEL: &str = "Password for sudo: ";

// ========================================================================
// Functions
// ========================================================================

/// Read the file at `path`, wherever it is.
pub(super) fn load(path: &Path) -> Result<LoadedFile, RemoteError> {
    let Some(at) = remote::remote_of(path) else {
        return file::load(path).map_err(failed);
    };
    let stamp = stamp_of(&at);
    file::check_size(path, stamp.len).map_err(failed)?;
    let raw = remote::read(&at)?;
    file::decode(path, raw, stamp).map_err(failed)
}

/// Read `path` for a new buffer, giving back the path the buffer is for and
/// what it holds. A local file only root can read becomes root's; where
/// sudo wants a password first, there is nothing yet (`None`).
pub(super) fn read_for_open(path: PathBuf) -> io::Result<(PathBuf, Option<LoadedFile>)> {
    if remote::remote_of(&path).is_some() {
        return match load(&path) {
            Ok(loaded) => Ok((path, Some(loaded))),
            Err(RemoteError::NeedsPassword) => Ok((path, None)),
            Err(RemoteError::Failed(reason)) => Err(io::Error::other(reason)),
        };
    }
    let denied = match file::load(&path) {
        Ok(loaded) => return Ok((path, Some(loaded))),
        Err(e) if is_denied(&e) => e,
        Err(e) => return Err(e),
    };
    let as_root = Remote::sudo(&path).to_path();
    match load(&as_root) {
        Ok(loaded) => Ok((as_root, Some(loaded))),
        Err(RemoteError::NeedsPassword) => Ok((as_root, None)),
        // Why sudo could not help says less than why it was asked to.
        Err(RemoteError::Failed(_)) => Err(denied),
    }
}

/// Whether something else has written to `path` since `stamp` was taken.
pub(super) fn is_stale(path: &Path, stamp: &FileStamp) -> bool {
    match remote::remote_of(path) {
        // A remote file whose stat cannot be read is not known to have moved.
        Some(at) => remote::stat(&at).is_some_and(|now| stamp_from(now) != *stamp),
        None => file::is_stale(path, stamp),
    }
}

/// What the remote file at `at` reads as now, or nothing known.
fn stamp_of(at: &Remote) -> FileStamp {
    remote::stat(at).map(stamp_from).unwrap_or_default()
}

/// A remote stat as a stamp to compare.
fn stamp_from(stat: remote::Stat) -> FileStamp {
    FileStamp {
        len: stat.len,
        modified: stat.modified,
    }
}

/// Whether `e` refused the user something root could be asked for.
fn is_denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied && remote::sudo_available()
}

/// A local failure in the remote's terms.
fn failed(e: io::Error) -> RemoteError {
    RemoteError::Failed(e.to_string())
}

/// A question whose answer is the sudo password.
fn password_prompt(tag: &'static str) -> EditorOutcome {
    EditorOutcome::Prompt(PromptRequest {
        initial: String::new(),
        label: PASSWORD_LABEL.to_string(),
        mode: PromptMode::Secret,
        tag,
    })
}

// ========================================================================
// TextEditor: elsewhere
// ========================================================================

impl TextEditor {
    /// Write the buffer to its file, wherever that is. A local file the user
    /// may not write is offered to root instead.
    pub(super) fn write_file(&mut self) -> EditorOutcome {
        // A stand-in for a file not yet read would replace the file.
        if self.doc().unread {
            return self.read_in();
        }
        let doc = self.doc();
        let saved = match remote::remote_of(&doc.path) {
            None => match file::save(&doc.path, doc.buffer.lines(), doc.shape) {
                Ok(stamp) => Ok(stamp),
                Err(e) if is_denied(&e) => return self.ask_as_root(),
                Err(e) => Err(failed(e)),
            },
            Some(at) => {
                let bytes = file::joined(doc.buffer.lines(), doc.shape);
                remote::write(&at, bytes.as_bytes()).map(|()| stamp_of(&at))
            }
        };
        match saved {
            Ok(stamp) => self.mark_written(stamp),
            Err(RemoteError::NeedsPassword) => return password_prompt(ASK_PASSWORD_SAVE),
            Err(RemoteError::Failed(reason)) => self
                .modal
                .report(Notice::error(format!("could not write: {reason}"))),
        }
        EditorOutcome::Consumed
    }

    /// Read the buffer's file in for the first time, asking for the password
    /// where sudo wants one.
    pub(super) fn read_in(&mut self) -> EditorOutcome {
        match load(&self.doc().path) {
            Ok(loaded) => {
                self.doc_mut().unread = false;
                self.replace_text(loaded);
                self.doc_mut().learn_grammar();
                if let Some(line) = self.doc_mut().land_on.take() {
                    self.buffer_mut().move_to_line(line.saturating_sub(1));
                }
            }
            Err(RemoteError::NeedsPassword) => return password_prompt(ASK_PASSWORD_OPEN),
            Err(RemoteError::Failed(reason)) => self.modal.report(Notice::error(format!(
                "could not read: {reason}; any key tries again"
            ))),
        }
        EditorOutcome::Consumed
    }

    /// Ask whether a file the user cannot write should be written as root.
    fn ask_as_root(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!(
                "{} is not writable. Save as root? (y/n) ",
                name_of(&self.doc().path)
            ),
            mode: PromptMode::Confirm,
            tag: ASK_AS_ROOT,
        })
    }

    /// Carry on with what an answer to one of the questions above allows.
    pub(super) fn answer_elsewhere(&mut self, reply: PromptReply) -> EditorOutcome {
        match (reply.tag, reply.answer) {
            (ASK_AS_ROOT, Some(_)) => {
                let as_root = Remote::sudo(&self.doc().path);
                // The stamp is root's view of the file from here on, so the
                // next save compares like with like.
                self.doc_mut().stamp = stamp_of(&as_root);
                self.doc_mut().path = as_root.to_path();
                self.write_file()
            }
            (ASK_PASSWORD_OPEN, Some(password)) => match self.authenticate(&password) {
                true => self.read_in(),
                false => EditorOutcome::Consumed,
            },
            (ASK_PASSWORD_OPEN, None) => {
                self.modal.report(Notice::info(
                    "not read: any key asks for the password again",
                ));
                EditorOutcome::Consumed
            }
            (ASK_PASSWORD_SAVE, Some(password)) => match self.authenticate(&password) {
                true => self.write_file(),
                false => EditorOutcome::Consumed,
            },
            _ => EditorOutcome::Consumed,
        }
    }

    /// Give sudo the password, saying why where it is not taken.
    fn authenticate(&mut self, password: &str) -> bool {
        match remote::sudo_authenticate(password) {
            Ok(()) => true,
            Err(RemoteError::Failed(reason)) => {
                self.modal.report(Notice::error(reason));
                false
            }
            Err(RemoteError::NeedsPassword) => {
                self.modal
                    .report(Notice::error("sudo did not take the password"));
                false
            }
        }
    }
}
