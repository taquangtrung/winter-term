//! What the process monitor tells the user, kept typed until the window
//! takes it, and what it asks before signalling a process.
//!
//! The monitor stores a [`ProcNotice`] and turns it into a misty [`Notice`]
//! only in `take_notice`, and asks a [`ProcConfirmation`] as a misty
//! [`Confirmation`], so the wording and severity are decided in one `match`
//! each and tests compare variants rather than sentences.

use std::fmt;

use winter_model::editor::{Confirmation, Notice};
use winter_model::notice::SearchMiss;

use super::act::{NICE_RANGE, Signal};

// ========================================================================
// Constants
// ========================================================================

/// What a failed signal or priority change is reported as, its output
/// under it.
const COMMAND_FAILED: &str = "the command failed";

// ========================================================================
// Data Structures
// ========================================================================

/// A question the monitor asks before signalling a process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProcConfirmation {
    /// Sending `signal` to what `what` names (`vim (pid 42)`).
    Signal {
        /// The signal to send.
        signal: Signal,
        /// The process, named for the question.
        what: String,
    },
}

/// One message for the status bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProcNotice {
    /// A signal or priority change finished, saying what it did.
    Done(Acting),
    /// A command finished with nothing named to say it did.
    DoneQuietly,
    /// A signal or priority change failed, with what the command said.
    Failed(String),
    /// The process chosen has gone, or its pid now runs something else.
    Gone(u32),
    /// A signal or priority change that could not be asked for.
    Refused(ActRefusal),
    /// A search over the processes that moved nowhere.
    Search(SearchMiss),
}

/// A signal or priority change under way, as its report will name it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Acting {
    /// A priority being set.
    Reniced(Renice),
    /// A signal being sent.
    Signalled(Signalled),
}

/// A priority being set.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Renice {
    /// The process.
    pub(crate) pid: u32,
    /// The priority, as typed.
    pub(crate) value: String,
}

/// A signal being sent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Signalled {
    /// The process's name.
    pub(crate) name: String,
    /// The process.
    pub(crate) pid: u32,
    /// The signal.
    pub(crate) signal: Signal,
}

/// Why a signal or priority change could not be asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActRefusal {
    /// Windows has no way to suspend or resume a process.
    NoSuspendOnWindows,
    /// What was typed is no nice value in the range given.
    NotNiceValue(String),
    /// What was typed is no Windows priority class.
    NotPriorityClass(String),
}

// ========================================================================
// ProcConfirmation
// ========================================================================

impl ProcConfirmation {
    /// The question the window asks: the one place its wording and whether
    /// it warns are decided. A signal that ends the process warns.
    pub(crate) fn to_confirmation(&self) -> Confirmation {
        match self {
            Self::Signal { signal, what } => {
                let message = format!("{} {what}?", signal.verb());
                match signal.ends() {
                    true => Confirmation::destructive(signal.title(), message, signal.button()),
                    false => Confirmation::new(signal.title(), message, signal.button()),
                }
            }
        }
    }
}

// ========================================================================
// ProcNotice
// ========================================================================

impl ProcNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Done(acting) => Notice::info(acting.to_string()),
            Self::DoneQuietly => Notice::info("done"),
            Self::Failed(detail) => Notice::error(COMMAND_FAILED).with_detail(detail),
            Self::Gone(pid) => Notice::info(format!("pid {pid} is no longer the process chosen")),
            Self::Refused(refusal) => Notice::refusal(refusal.to_string()),
            Self::Search(miss) => miss.into_notice(),
        }
    }
}

// ========================================================================
// Acting
// ========================================================================

impl fmt::Display for Acting {
    /// `suspended server (10)`, or `set priority of 10 to 5`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reniced(Renice { pid, value }) => write!(f, "set priority of {pid} to {value}"),
            Self::Signalled(Signalled { name, pid, signal }) => {
                write!(f, "{} {name} ({pid})", signal.past_tense())
            }
        }
    }
}

// ========================================================================
// ActRefusal
// ========================================================================

impl fmt::Display for ActRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuspendOnWindows => {
                write!(f, "suspend and resume are not supported on Windows")
            }
            Self::NotNiceValue(value) => write!(
                f,
                "not a nice value: {value} (from {} to {})",
                NICE_RANGE.start(),
                NICE_RANGE.end()
            ),
            Self::NotPriorityClass(value) => write!(f, "not a priority class: {value}"),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        let every = [
            ProcNotice::Done(Acting::Reniced(Renice {
                pid: 10,
                value: "5".to_string(),
            })),
            ProcNotice::Done(Acting::Signalled(Signalled {
                name: "server".to_string(),
                pid: 10,
                signal: Signal::Stop,
            })),
            ProcNotice::DoneQuietly,
            ProcNotice::Failed("kill: not permitted".to_string()),
            ProcNotice::Gone(10),
            ProcNotice::Refused(ActRefusal::NoSuspendOnWindows),
            ProcNotice::Refused(ActRefusal::NotNiceValue("20".to_string())),
            ProcNotice::Refused(ActRefusal::NotPriorityClass("Fast".to_string())),
            ProcNotice::Search(SearchMiss::NoSearch),
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
    fn test_a_signal_is_reported_by_what_it_did_to_whom() {
        let acting = Acting::Signalled(Signalled {
            name: "server".to_string(),
            pid: 10,
            signal: Signal::Stop,
        });
        assert_eq!(acting.to_string(), "suspended server (10)");
    }
}
