//! What each git request in flight was asked for, kept by its id until the
//! reply comes back. A reply carries only its tag and its id, and the jobs
//! run several at once in no set order: kept in one slot per kind instead, a
//! second request of the same kind overwrote the first's, and the first's
//! reply was read as the second's (a diff filed under the wrong file, a
//! resolved conflict staging the wrong path).

use std::collections::HashMap;

use crate::editors::git_exec::Rev;
use crate::model::editor::JobRequest;

use super::rows::FileRow;

// ========================================================================
// Data Structures
// ========================================================================

/// What a request was asked for, which its reply needs to be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Asked {
    /// A blame of the file at this path.
    Blame(String),
    /// The diff of an open file, asked under the status read counted as
    /// `under`: an answer to an older read is dropped.
    Diff(DiffAsked),
    /// `HEAD`'s copy of the file at this path, for the diff tool.
    Head(String),
    /// One side of the conflicted file at this path taken whole, which the
    /// answer stages.
    Resolve(String),
    /// A branch spun off from `from`, which the answer puts back where
    /// `upstream` is.
    SpinOff(SpinOffAsked),
    /// A fixup of this commit, which the answer folds in.
    Squash(Rev),
}

/// The diff of `file`, asked under status read `under`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DiffAsked {
    pub(super) file: FileRow,
    pub(super) under: u64,
}

/// The branch a spin-off left, and the upstream it goes back to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SpinOffAsked {
    pub(super) from: Rev,
    pub(super) upstream: Rev,
}

/// The requests in flight that something waits on, by id.
#[derive(Debug, Default)]
pub(super) struct InFlight {
    asked: HashMap<u64, Asked>,
    /// The last id handed out; ids start at one, nought being every request
    /// no one waits on.
    last: u64,
}

// ========================================================================
// InFlight
// ========================================================================

impl InFlight {
    /// `job`, numbered so its reply finds `asked` again. A job that is no
    /// command has no reply to number, and goes as it is.
    pub(super) fn ask(&mut self, job: JobRequest, asked: Asked) -> JobRequest {
        let JobRequest::Command(mut command) = job else {
            return job;
        };
        self.last += 1;
        command.id = self.last;
        self.asked.insert(self.last, asked);
        JobRequest::Command(command)
    }

    /// What the request numbered `id` was asked for, no longer waited on.
    pub(super) fn answer(&mut self, id: u64) -> Option<Asked> {
        self.asked.remove(&id)
    }

    /// The id of the oldest request in flight asked for something `matches`
    /// holds, for a test to answer it as the job runner would.
    #[cfg(test)]
    pub(super) fn oldest(&self, matches: impl Fn(&Asked) -> bool) -> Option<u64> {
        self.asked.iter().filter(|(_, asked)| matches(asked)).map(|(id, _)| *id).min()
    }

    /// Whether a request in flight was asked for something `matches` holds.
    pub(super) fn is_asking(&self, matches: impl Fn(&Asked) -> bool) -> bool {
        self.asked.values().any(matches)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::editor::CommandRequest;

    fn command(tag: &'static str) -> JobRequest {
        JobRequest::Command(CommandRequest {
            args: Vec::new(),
            cwd: PathBuf::from("/repo"),
            env: Vec::new(),
            id: 0,
            program: "git".to_string(),
            stdin: None,
            tag,
        })
    }

    fn id_of(job: &JobRequest) -> u64 {
        match job {
            JobRequest::Command(command) => command.id,
            other => panic!("not a command: {other:?}"),
        }
    }

    #[test]
    fn test_two_requests_of_one_kind_each_find_their_own_answer() {
        // One slot per kind gave the second's path to the first's reply,
        // whichever came back first.
        let mut in_flight = InFlight::default();
        let first = in_flight.ask(command("resolve"), Asked::Resolve("a.rs".to_string()));
        let second = in_flight.ask(command("resolve"), Asked::Resolve("b.rs".to_string()));
        assert_ne!(id_of(&first), id_of(&second));
        assert_eq!(in_flight.answer(id_of(&second)), Some(Asked::Resolve("b.rs".to_string())));
        assert!(in_flight.is_asking(|asked| matches!(asked, Asked::Resolve(_))));
        assert_eq!(in_flight.answer(id_of(&first)), Some(Asked::Resolve("a.rs".to_string())));
        assert_eq!(in_flight.answer(id_of(&first)), None, "answered once");
        assert!(!in_flight.is_asking(|_| true));
    }
}
