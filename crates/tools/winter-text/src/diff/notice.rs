//! What the diff and merge editor tells the user, kept typed until the
//! window takes it, and what it asks before an action.
//!
//! The editor stores a [`DiffNotice`] and turns it into a misty [`Notice`]
//! only in `take_notice`, and asks a [`DiffConfirmation`] as a misty
//! [`Confirmation`], so the wording and severity are decided in one `match`
//! each and tests compare variants rather than sentences.

use std::fmt;
use std::path::PathBuf;

use cherry::NoticeRequest;
use winter_model::editor::{Confirmation, Notice};

use super::letter_of;
use crate::notice::{FileFailure, cherry_notice};

// ========================================================================
// Data Structures
// ========================================================================

/// A question the editor asks before an action, kept until it is answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiffConfirmation {
    /// Quitting with edits not written, on these sides (`A`, `B`).
    Quit(Vec<&'static str>),
}

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiffNotice {
    /// Stepping found no region past the last (`true`) or before the first.
    AtEnd(bool),
    /// Case was left out of the comparison (`true`) or counted again.
    Case(bool),
    /// A message Cherry gave while a text was edited in place.
    Cherry(NoticeRequest),
    /// Stepping goes through only the conflicts (`true`) or every region.
    ConflictsOnly(bool),
    /// A region was copied from one text to another.
    Copied(Copied),
    /// How many regions differ.
    Differ(usize),
    /// A text is being edited in place, by its side.
    Editing(usize),
    /// A merge edits only its result.
    MergeEditsResult,
    /// A merge keeps its sides where they are.
    MergeKeepsSides,
    /// A command wanted a region and none is selected.
    NoRegionSelected,
    /// The region number typed is no region.
    NoSuchRegion,
    /// A text with no file was asked to be written, by its side.
    NotAFile(usize),
    /// Changed words are shown (`true`) or not.
    Refine(bool),
    /// Where the session stands.
    Status(DiffStatus),
    /// A copy of the texts could not be made, with the system's error.
    TempFailed(String),
    /// Whitespace was left out of the comparison (`true`) or counted again.
    Whitespace(bool),
    /// A text could not be written.
    WriteFailed(FileFailure),
    /// A text was written to its file.
    Wrote(PathBuf),
}

/// A region copied from one text to another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Copied {
    /// The side copied from.
    pub(crate) from: usize,
    /// The region, from 0.
    pub(crate) region: usize,
    /// The side copied over.
    pub(crate) to: usize,
}

/// Where a comparison stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DiffStatus {
    /// The current region, from 0, if one is selected.
    pub(crate) at: Option<usize>,
    /// Whether case is left out.
    pub(crate) case_left_out: bool,
    /// How many regions differ.
    pub(crate) count: usize,
    /// For a merge, how many conflicts are left.
    pub(crate) conflicts_left: Option<usize>,
    /// Whether whitespace is left out.
    pub(crate) whitespace_left_out: bool,
}

// ========================================================================
// DiffConfirmation
// ========================================================================

impl DiffConfirmation {
    /// The question the window asks: the one place its wording and whether
    /// it warns are decided.
    pub(crate) fn to_confirmation(&self) -> Confirmation {
        match self {
            Self::Quit(sides) => Confirmation::destructive(
                "Discard Unsaved Changes?",
                format!(
                    "Not written: side {}. Quitting loses those edits.",
                    sides.join(", ")
                ),
                "Discard",
            ),
        }
    }
}

// ========================================================================
// DiffNotice
// ========================================================================

impl DiffNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::AtEnd(forward) => {
                let edge = if forward { "last" } else { "first" };
                Notice::info(format!("at the {edge} region")).unlogged()
            }
            Self::Case(left_out) => {
                let state = if left_out { "left out" } else { "counted" };
                Notice::info(format!("case {state}")).unlogged()
            }
            Self::Cherry(notice) => cherry_notice(notice),
            Self::ConflictsOnly(only) => {
                let state = if only {
                    "only conflicts"
                } else {
                    "every region"
                };
                Notice::info(format!("stepping through {state}")).unlogged()
            }
            Self::Copied(Copied { from, region, to }) => Notice::info(format!(
                "region {} copied {} to {}",
                region + 1,
                letter_of(from),
                letter_of(to)
            ))
            .unlogged(),
            Self::Differ(count) => Notice::info(match count {
                0 => "no differences".to_string(),
                1 => "1 region differs".to_string(),
                n => format!("{n} regions differ"),
            })
            .unlogged(),
            Self::Editing(side) => Notice::info(format!(
                "editing {}: C-c C-c or C-S-c goes back to the regions",
                letter_of(side)
            ))
            .unlogged(),
            Self::MergeEditsResult => Notice::info("a merge edits its result, C").unlogged(),
            Self::MergeKeepsSides => {
                Notice::info("a merge keeps its sides where they are").unlogged()
            }
            Self::NoRegionSelected => {
                Notice::info("no region selected: n goes to the first").unlogged()
            }
            Self::NoSuchRegion => Notice::info("no such region").unlogged(),
            Self::NotAFile(side) => {
                Notice::info(format!("{} is not a file to write to", letter_of(side)))
            }
            Self::Refine(on) => {
                let state = if on { "on" } else { "off" };
                Notice::info(format!("changed words shown {state}")).unlogged()
            }
            Self::Status(status) => Notice::info(status.to_string()),
            Self::TempFailed(error) => Notice::error(error),
            Self::Whitespace(left_out) => {
                let state = if left_out { "left out" } else { "counted" };
                Notice::info(format!("whitespace {state}")).unlogged()
            }
            Self::WriteFailed(FileFailure { error, path }) => {
                Notice::error(format!("{}: {error}", path.display()))
            }
            Self::Wrote(path) => Notice::info(format!("wrote {}", path.display())),
        }
    }
}

// ========================================================================
// DiffStatus
// ========================================================================

impl fmt::Display for DiffStatus {
    /// `region 2 of 5, 1 conflicts left, whitespace left out`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(at) => write!(f, "region {} of {}", at + 1, self.count)?,
            None => write!(f, "region none of {}", self.count)?,
        }
        if let Some(open) = self.conflicts_left {
            write!(f, ", {open} conflicts left")?;
        }
        if self.whitespace_left_out {
            write!(f, ", whitespace left out")?;
        }
        if self.case_left_out {
            write!(f, ", case left out")?;
        }
        Ok(())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{A, B, C};
    use super::*;

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        let status = DiffStatus {
            at: Some(0),
            case_left_out: true,
            count: 2,
            conflicts_left: Some(1),
            whitespace_left_out: true,
        };
        let every = [
            DiffNotice::AtEnd(true),
            DiffNotice::Case(true),
            DiffNotice::ConflictsOnly(true),
            DiffNotice::Copied(Copied {
                from: A,
                region: 0,
                to: B,
            }),
            DiffNotice::Differ(0),
            DiffNotice::Differ(1),
            DiffNotice::Differ(3),
            DiffNotice::Editing(C),
            DiffNotice::MergeEditsResult,
            DiffNotice::MergeKeepsSides,
            DiffNotice::NoRegionSelected,
            DiffNotice::NoSuchRegion,
            DiffNotice::NotAFile(A),
            DiffNotice::Refine(true),
            DiffNotice::Status(status),
            DiffNotice::Whitespace(false),
            DiffNotice::Wrote(PathBuf::from("a.rs")),
        ];
        for notice in every {
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
    fn test_the_status_names_only_what_applies() {
        let status = DiffStatus {
            at: None,
            case_left_out: false,
            count: 3,
            conflicts_left: None,
            whitespace_left_out: false,
        };
        assert_eq!(status.to_string(), "region none of 3");
    }
}
