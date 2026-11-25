//! What a listing tells the user, kept typed until the window takes it.
//!
//! The listing stores a [`DirNotice`] and turns it into a misty [`Notice`]
//! only in `take_notice`, so its wording and severity are decided in one
//! `match` and tests compare variants rather than sentences.

use winter_field::FieldNotice;
use winter_model::editor::Notice;
use winter_model::notice::SearchMiss;

// ========================================================================
// Constants
// ========================================================================

/// What a failed archiver is reported as, its output under it.
const ARCHIVER_FAILED: &str = "the archiver failed";

/// What a failed git action on an entry is reported as, git's output under it.
const GIT_FAILED: &str = "git failed";

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DirNotice {
    /// The last archiver finished.
    ArchiveDone,
    /// An archive was asked for one file in a format that holds several.
    ArchiveHoldsOne(String),
    /// An archiver failed, with what it said.
    ArchiverFailed(String),
    /// Archives were asked of entries on another machine.
    ArchivesLocalOnly,
    /// A question was cancelled.
    Cancelled,
    /// A comparison was asked of entries on another machine.
    CompareLocalOnly,
    /// A comparison was asked of the wrong number of entries.
    CompareWhat,
    /// Edited names were given up, this many of them.
    Discarded(usize),
    /// An operation finished, with the report it made.
    Done(String),
    /// Making entries executable stopped part-way.
    ExecutableFailed(PartFailure),
    /// An operation failed, with the report it made.
    Failed(String),
    /// What the name field said.
    Field(FieldNotice),
    /// A git action on an entry finished.
    GitDone,
    /// A git action on an entry failed, with what git said.
    GitFailed(String),
    /// The listing was cut short at this many entries.
    ListedFirst(usize),
    /// One entry made executable (`true`) or not.
    MadeExecutable(bool),
    /// Editing names ended with none changed.
    NoChanges,
    /// The actions menu was asked for where none applies.
    NoSuchAction,
    /// An edited name is no name.
    NotAName(String),
    /// A name asked for is no kind of archive this makes.
    NotArchiveKind(String),
    /// The actions menu was opened where none applies.
    NothingToActOn,
    /// The names were edited this many times over.
    Renamed(usize),
    /// A search over the entries that moved nowhere.
    Search(SearchMiss),
    /// A batch of renames was turned down, so editing goes on.
    StillEditing,
    /// Executable toggled on this many entries.
    ToggledExecutable(usize),
    /// Files to compare could not be read, for the reason given.
    Unreadable(String),
}

/// An operation over several entries that stopped part-way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PartFailure {
    /// How many entries were done before it stopped.
    pub(crate) done: usize,
    /// Why it stopped.
    pub(crate) error: String,
}

// ========================================================================
// DirNotice
// ========================================================================

impl DirNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::ArchiveDone => Notice::info("done"),
            Self::ArchiveHoldsOne(name) => Notice::refusal(format!(
                "archives: {name} holds one file; name a .zip or a .tar.gz"
            )),
            Self::ArchiverFailed(detail) => Notice::error(ARCHIVER_FAILED).with_detail(detail),
            Self::ArchivesLocalOnly => Notice::refusal("archives: only entries on this machine"),
            Self::Cancelled => Notice::info("cancelled"),
            Self::CompareLocalOnly => Notice::refusal("compare: only entries on this machine"),
            Self::CompareWhat => {
                Notice::refusal("mark two entries to compare them, or three files")
            }
            Self::Discarded(count) => Notice::info(format!("discarded {count}")),
            Self::Done(report) => Notice::info(report),
            Self::ExecutableFailed(PartFailure { done, error }) => {
                Notice::error(format!("{error} ({done} done)"))
            }
            Self::Failed(report) => Notice::error(format!("failed: {report}")),
            Self::Field(notice) => notice.into_notice(),
            Self::GitDone => Notice::info("git: done"),
            Self::GitFailed(detail) => Notice::error(GIT_FAILED).with_detail(detail),
            Self::ListedFirst(count) => Notice::info(format!("listed the first {count} entries")),
            Self::MadeExecutable(true) => Notice::info("made +x (executable)"),
            Self::MadeExecutable(false) => Notice::info("made -x (not executable)"),
            Self::NoChanges => Notice::info("no changes"),
            Self::NoSuchAction => {
                Notice::info("no such action here: press one listed, or Esc").unlogged()
            }
            Self::NotAName(name) => Notice::error(format!("failed: not a name: {name}")),
            Self::NotArchiveKind(name) => {
                Notice::refusal(format!("archives: {name} is no kind of archive this makes"))
            }
            Self::NothingToActOn => Notice::info("nothing here to act on").unlogged(),
            Self::Renamed(count) => Notice::info(format!("renamed {count}")),
            Self::Search(miss) => miss.into_notice(),
            Self::StillEditing => Notice::info("not renamed; still editing"),
            Self::ToggledExecutable(count) => {
                Notice::info(format!("toggled executable on {count} entries"))
            }
            Self::Unreadable(why) => Notice::error(why),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::notify::Severity;

    use super::*;

    /// One of each variant Winter words itself, so a wording rule is checked
    /// against them all.
    fn every_notice() -> Vec<DirNotice> {
        vec![
            DirNotice::ArchiveDone,
            DirNotice::ArchiveHoldsOne("a.gz".to_string()),
            DirNotice::ArchiverFailed("tar: error".to_string()),
            DirNotice::ArchivesLocalOnly,
            DirNotice::Cancelled,
            DirNotice::CompareLocalOnly,
            DirNotice::CompareWhat,
            DirNotice::Discarded(2),
            DirNotice::ExecutableFailed(PartFailure {
                done: 1,
                error: "permission denied".to_string(),
            }),
            DirNotice::Failed("exists".to_string()),
            DirNotice::GitDone,
            DirNotice::GitFailed("fatal".to_string()),
            DirNotice::ListedFirst(10),
            DirNotice::MadeExecutable(true),
            DirNotice::MadeExecutable(false),
            DirNotice::NoChanges,
            DirNotice::NoSuchAction,
            DirNotice::NotAName("a/b".to_string()),
            DirNotice::NotArchiveKind("a.txt".to_string()),
            DirNotice::NothingToActOn,
            DirNotice::Renamed(3),
            DirNotice::Search(SearchMiss::NoSearch),
            DirNotice::StillEditing,
            DirNotice::ToggledExecutable(4),
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
            assert!(!text.ends_with('.'), "{notice:?} ends in a period: {text}");
        }
    }

    #[test]
    fn test_a_failed_tool_is_named_with_what_it_said_under_it() {
        let notice = DirNotice::ArchiverFailed("tar: error".to_string()).into_notice();
        assert_eq!(notice.severity, Severity::Error);
        assert_eq!(notice.text, ARCHIVER_FAILED);
        assert_eq!(notice.detail.as_deref(), Some("tar: error"));
    }

    #[test]
    fn test_a_part_done_failure_says_how_far_it_got() {
        let failure = PartFailure {
            done: 2,
            error: "permission denied".to_string(),
        };
        let text = DirNotice::ExecutableFailed(failure).into_notice().text;
        assert_eq!(text, "permission denied (2 done)");
    }
}
