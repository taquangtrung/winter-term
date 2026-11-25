//! What the text editor tells the user, kept typed until the window takes
//! it.
//!
//! The editor stores a [`TextNotice`] and turns it into a misty [`Notice`]
//! only in `take_notice`, so its wording and severity are decided in one
//! `match` and tests compare variants rather than sentences. What Cherry, a
//! language server, or the system said travels as a variant's data.

use std::path::PathBuf;

use cherry::{NoticeLevel, NoticeRequest};
use winter_model::editor::Notice;

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TextNotice {
    /// Formatting found the file already formatted.
    AlreadyFormatted,
    /// The file changed on disk under unsaved edits, by its name.
    ChangedOnDisk(String),
    /// A message Cherry gave, worded by Cherry, at its level.
    Cherry(NoticeRequest),
    /// A Cherry command failed, as Cherry said.
    CommandFailed(String),
    /// The cursor landed on a conflict.
    ConflictAt(ConflictPlace),
    /// How many conflicts are left after one was resolved.
    ConflictsLeft(usize),
    /// The file was deleted on disk, by its name.
    DeletedOnDisk(String),
    /// A server does not answer what was asked of it.
    DoesNotAnswer(Unanswered),
    /// The server took too long to format, so the file was saved as it was.
    FormatTookTooLong,
    /// Formatting changed the file.
    Formatted,
    /// The conflict carries no ancestor to keep.
    NoAncestor,
    /// No merge conflicts in the file.
    NoConflicts,
    /// The server offered no fixes at the cursor.
    NoFixes,
    /// The server found no references.
    NoReferences,
    /// The server knows no signature at the cursor.
    NoSignature,
    /// Going to something found nothing, by what was looked for.
    NoneFound(&'static str),
    /// A rebase plan's action was set on a line holding no commit.
    NotACommit,
    /// Formatting failed with the server's error, so the file was saved as
    /// it was.
    NotFormatted(String),
    /// The cursor is not in a merge conflict.
    NotInConflict,
    /// No running language server serves the file.
    NotServed,
    /// The server has nothing to say about what is under the cursor.
    NothingToSay,
    /// An inline source had nothing to suggest, by its title.
    NothingToSuggest(String),
    /// The jump list has nowhere further back (`true`) or forward.
    Nowhere(bool),
    /// A rebase plan starts by folding into nothing, with the action it
    /// starts with.
    PlanFolds(String),
    /// A file could not be read into the text.
    ReadFailed(FileFailure),
    /// A rename touched this many files.
    Renamed(usize),
    /// A language server failed a request, with its error.
    ServerFailed(String),
    /// The text changed while the server worked out its edits.
    StaleEdits,
    /// A server is still starting, by its name.
    StillStarting(String),
    /// Color swatches were turned on (`true`) or off.
    Swatches(bool),
    /// A close would drop edits not written.
    Unsaved,
    /// The file could not be written, with the system's error.
    WriteFailed(String),
}

/// Where the cursor landed among a file's conflicts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConflictPlace {
    /// Which, from 0.
    pub(crate) index: usize,
    /// How many there are.
    pub(crate) total: usize,
}

/// A request a server has no answer for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Unanswered {
    /// The server's name.
    pub(crate) server: String,
    /// What it was asked, as its capability names it.
    pub(crate) what: String,
}

/// A file that could not be read or written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileFailure {
    /// The system's error.
    pub(crate) error: String,
    /// The file.
    pub(crate) path: PathBuf,
}

// ========================================================================
// TextNotice
// ========================================================================

impl TextNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::AlreadyFormatted => Notice::info("already formatted").unlogged(),
            Self::ChangedOnDisk(name) => Notice::warning(format!(
                "{name} changed on disk; your unsaved edits are kept, and saving asks before \
                 writing over it"
            )),
            Self::Cherry(notice) => cherry_notice(notice),
            Self::CommandFailed(error) => Notice::error(error),
            Self::ConflictAt(ConflictPlace { index, total }) => {
                Notice::info(format!("conflict {} of {total}", index + 1)).unlogged()
            }
            Self::ConflictsLeft(left) => Notice::info(match left {
                0 => "every conflict resolved".to_string(),
                1 => "1 conflict left".to_string(),
                n => format!("{n} conflicts left"),
            })
            .unlogged(),
            Self::DeletedOnDisk(name) => Notice::warning(format!(
                "{name} was deleted on disk; its text is kept, and saving writes it back"
            )),
            Self::DoesNotAnswer(Unanswered { server, what }) => {
                Notice::info(format!("{server} does not answer {what}"))
            }
            Self::FormatTookTooLong => {
                Notice::info("the server took too long to format: saved as it is").unlogged()
            }
            Self::Formatted => Notice::info("formatted").unlogged(),
            Self::NoAncestor => {
                Notice::error("this conflict carries no ancestor (merge with conflictStyle diff3)")
            }
            Self::NoConflicts => Notice::info("no merge conflicts here"),
            Self::NoFixes => Notice::info("no fixes here").unlogged(),
            Self::NoneFound(what) => Notice::info(format!("no {what} found")),
            Self::NoReferences => Notice::info("no references found"),
            Self::NoSignature => Notice::info("no signature here").unlogged(),
            Self::NotACommit => Notice::error("not a commit line"),
            Self::NotFormatted(error) => {
                Notice::info(format!("not formatted ({error}): saved as it is"))
            }
            Self::NotInConflict => Notice::error("the cursor is not in a merge conflict"),
            Self::NotServed => Notice::info("no running language server serves this file"),
            Self::NothingToSay => Notice::info("nothing to say about this").unlogged(),
            Self::NothingToSuggest(source) => {
                Notice::info(format!("{source}: nothing to suggest here")).unlogged()
            }
            Self::Nowhere(true) => Notice::info("nowhere to go back to").unlogged(),
            Self::Nowhere(false) => Notice::info("nowhere to go forward to").unlogged(),
            Self::PlanFolds(action) => Notice::error(format!(
                "the first commit cannot {action}: there is nothing before it to fold into"
            )),
            Self::ReadFailed(FileFailure { error, path }) => {
                Notice::error(format!("{}: {error}", path.display()))
            }
            Self::Renamed(files) => Notice::info(match files {
                0 => "nothing to rename".to_string(),
                1 => "renamed in 1 file".to_string(),
                n => format!("renamed in {n} files, not yet saved"),
            }),
            Self::ServerFailed(error) => Notice::error(error),
            Self::StaleEdits => Notice::info("the file changed while the server worked: ask again"),
            Self::StillStarting(server) => Notice::info(format!("{server} is still starting")),
            Self::Swatches(on) => {
                let state = if on { "on" } else { "off" };
                Notice::info(format!("Color swatches {state}")).unlogged()
            }
            Self::Unsaved => Notice::error("No write since last change (add ! to override)"),
            Self::WriteFailed(error) => Notice::error(error),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A message Cherry gave, at its level: shared by every editor in the crate
/// that hosts a Cherry view.
pub(crate) fn cherry_notice(NoticeRequest { level, message }: NoticeRequest) -> Notice {
    match level {
        NoticeLevel::Error => Notice::error(message),
        NoticeLevel::Warning => Notice::warning(message),
        _ => Notice::info(message),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One of each variant Winter words itself, so a wording rule is checked
    /// against them all.
    fn every_notice() -> Vec<TextNotice> {
        let failure = FileFailure {
            error: "denied".to_string(),
            path: PathBuf::from("a.rs"),
        };
        vec![
            TextNotice::AlreadyFormatted,
            TextNotice::ChangedOnDisk("a.rs".to_string()),
            TextNotice::ConflictAt(ConflictPlace { index: 0, total: 2 }),
            TextNotice::ConflictsLeft(0),
            TextNotice::ConflictsLeft(1),
            TextNotice::ConflictsLeft(2),
            TextNotice::DeletedOnDisk("a.rs".to_string()),
            TextNotice::DoesNotAnswer(Unanswered {
                server: "rust-analyzer".to_string(),
                what: "rename".to_string(),
            }),
            TextNotice::FormatTookTooLong,
            TextNotice::Formatted,
            TextNotice::NoAncestor,
            TextNotice::NoConflicts,
            TextNotice::NoFixes,
            TextNotice::NoneFound("definition"),
            TextNotice::NoReferences,
            TextNotice::NoSignature,
            TextNotice::NotACommit,
            TextNotice::NotFormatted("timeout".to_string()),
            TextNotice::NotInConflict,
            TextNotice::NotServed,
            TextNotice::NothingToSay,
            TextNotice::NothingToSuggest("Copilot".to_string()),
            TextNotice::Nowhere(true),
            TextNotice::Nowhere(false),
            TextNotice::PlanFolds("squash".to_string()),
            TextNotice::ReadFailed(failure),
            TextNotice::Renamed(0),
            TextNotice::Renamed(3),
            TextNotice::StaleEdits,
            TextNotice::StillStarting("rust-analyzer".to_string()),
            TextNotice::Swatches(true),
            TextNotice::Unsaved,
        ]
    }

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        for notice in every_notice() {
            let text = notice.clone().into_notice().text;
            assert!(!text.is_empty(), "{notice:?} says nothing");
            assert!(
                !text.contains("..."),
                "{notice:?} writes `...`, not `…`: {text}"
            );
            // Vim's own message keeps its closing parenthesis, never a period.
            assert!(!text.ends_with('.'), "{notice:?} ends in a period: {text}");
        }
    }

    #[test]
    fn test_conflicts_are_counted_from_one() {
        let place = ConflictPlace { index: 0, total: 2 };
        assert_eq!(
            TextNotice::ConflictAt(place).into_notice().text,
            "conflict 1 of 2"
        );
    }
}
