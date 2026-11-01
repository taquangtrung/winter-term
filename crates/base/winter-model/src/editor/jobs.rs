//! The work an editor asks the host to run, and what the host hands back.

use std::path::PathBuf;

use crate::process::ProcessSample;
use crate::system::SystemSample;

use super::search::{SearchRequest, SearchResult};

// ========================================================================
// Data Structures
// ========================================================================

/// Slow work an editor is asking the host to do off the event-loop thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobRequest {
    /// Run a program and capture what it wrote.
    Command(CommandRequest),
    /// Total the bytes under this directory.
    DirSize(PathBuf),
    /// Take a snapshot of every process on the machine.
    Processes,
    /// Read these files, skipping the ones that are not there. For the small
    /// state files a program keeps beside its data, where "not there" is an
    /// answer rather than a failure.
    ReadFiles(Vec<PathBuf>),
    /// Find the lines under a directory that hold some text.
    Search(SearchRequest),
    /// Take a snapshot of the machine's CPU, memory, disks, and GPUs.
    System,
    /// Copy, move, delete, or trash files. A large tree takes long, and the
    /// window must not wait on it; the stop key can end it between entries.
    FileOps(FileOps),
}

/// The file work a [`JobRequest::FileOps`] does, its paths already resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileOps {
    /// Copy each `(from, to)`.
    Copy(Vec<(PathBuf, PathBuf)>),
    /// Delete each path, a directory with everything in it.
    Delete(Vec<PathBuf>),
    /// Move each `(from, to)`.
    Move(Vec<(PathBuf, PathBuf)>),
    /// Move each path to the trash.
    Trash(Vec<PathBuf>),
}

/// A program to run on an editor's behalf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// Variables set for the program on top of the app's own environment.
    pub env: Vec<(String, String)>,
    /// Which request of its tag this is, handed back in the reply: an editor
    /// keeping what it asked each request for finds it again by this. Nought
    /// for one that keeps nothing.
    pub id: u64,
    /// The program.
    pub program: String,
    /// What to write to the program's standard input, for a command that reads
    /// its payload rather than taking it as an argument.
    pub stdin: Option<String>,
    /// Which request this is, so the editor knows what finished.
    pub tag: &'static str,
}

/// The answer to a [`JobRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobReply {
    /// What a program wrote, and how it exited.
    Command(CommandOutput),
    /// What a directory holds, with unreadable parts skipped rather than
    /// reported as an error: a total is worth more than a failure here.
    DirSize {
        /// Bytes under the directory.
        bytes: u64,
        /// The directory that was totalled.
        path: PathBuf,
    },
    /// What the files that could be read hold, keyed by the path asked for.
    /// A file that is missing or unreadable is simply absent.
    Files(Vec<(PathBuf, String)>),
    /// Every process, or why the system would not list them.
    Processes(Result<ProcessSample, String>),
    /// The lines a search found.
    Search(SearchResult),
    /// The machine, or why the system would not describe it.
    System(Result<SystemSample, String>),
    /// What a [`JobRequest::FileOps`] did (`"copied 3"`), or where it stopped
    /// and why, with how many it had done by then.
    FileOps(Result<String, String>),
}

/// What running a program produced. A program that could not be started at all
/// reports no code and its reason on `stderr`, so one path covers both.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOutput {
    /// Exit status, absent when the program never ran.
    pub code: Option<i32>,
    /// The id of the request this answers (see [`CommandRequest::id`]).
    pub id: u64,
    /// What it wrote to standard error.
    pub stderr: String,
    /// What it wrote to standard output.
    pub stdout: String,
    /// The tag of the request this answers.
    pub tag: &'static str,
}

impl CommandOutput {
    /// Whether the program ran and reported success.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// The first line of the failure, for a one-line report.
    pub fn failure(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        text.lines().next().unwrap_or("failed").to_string()
    }
}

/// A command an editor wants run in a real terminal rather than captured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// The program.
    pub program: String,
}

/// A build an editor is about to start, as the host knows it by.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuildStart {
    /// The shell command line.
    pub command: String,
    /// Where it runs.
    pub dir: PathBuf,
    /// The project it belongs to.
    pub root: PathBuf,
}
