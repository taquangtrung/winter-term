//! What a process monitor reads from the system: one snapshot of every
//! process, before any rate has been worked out from it.

use std::time::Instant;

// ========================================================================
// Data Structures
// ========================================================================

/// How much CPU a process has used, in whichever form the system reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuReading {
    /// A rate the system already worked out, in tenths of a percent (`ps`
    /// reports one, averaged over the process's whole life).
    Rate(u32),
    /// CPU time used so far in milliseconds, from which a rate needs two
    /// snapshots to work out.
    Time(u64),
}

/// One process as the system reported it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawProcess {
    /// The full command line, or the name when the system hides arguments.
    pub command: String,
    /// CPU use.
    pub cpu: CpuReading,
    /// The directory it runs in, where the system tells.
    pub cwd: Option<String>,
    /// The executable's full path, where the system tells.
    pub exec_path: Option<String>,
    /// Video memory it holds in kilobytes, where a GPU tool reports it.
    pub gpu_kb: Option<u64>,
    /// The executable's name, without a path.
    pub name: String,
    /// The process id.
    pub pid: u32,
    /// The parent's id, where the system tells.
    pub ppid: Option<u32>,
    /// Resident memory in kilobytes.
    pub rss_kb: u64,
    /// What it is doing.
    pub state: ProcState,
    /// The owner's name, where the system tells.
    pub user: Option<String>,
}

/// Every process at one moment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSample {
    /// The processes, in no particular order.
    pub entries: Vec<RawProcess>,
    /// Whether a GPU tool answered, so a process missing from its list holds
    /// no video memory rather than an unknown amount.
    pub has_gpu: bool,
    /// When the snapshot was started, so two of them give an interval.
    pub taken: Instant,
    /// Physical memory in kilobytes, or zero when the system would not say.
    pub total_mem_kb: u64,
}

/// What a process is doing, as far as the systems Winter runs on agree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcState {
    /// On a CPU or waiting for one.
    Running,
    /// Waiting for something to happen.
    Sleeping,
    /// Stopped by a signal.
    Stopped,
    /// The system could not say, or has no such notion.
    Unknown,
    /// Finished, waiting for its parent to collect it.
    Zombie,
}
