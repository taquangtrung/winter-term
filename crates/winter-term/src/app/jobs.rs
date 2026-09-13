//! Slow work off the event-loop thread: a page asks for it, a worker thread
//! does it, and the answer is collected on the next poll.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::model::layout::PaneId;
use crate::model::page::{CommandOutput, CommandRequest, JobReply, JobRequest};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How many jobs may run at once. Past this, a request is dropped rather than
/// queued: every caller asks for work about what is on screen, so a request
/// that waited long enough to run would be answering a stale question.
const MAX_IN_FLIGHT: usize = 16;

// ========================================================================
// Data Structures
// ========================================================================

/// The jobs currently running, and the channel their answers arrive on.
pub(crate) struct Jobs {
    next_id: u64,
    /// The jobs whose answers are still wanted. A cancelled job leaves this
    /// map, so its answer is discarded when it eventually arrives.
    running: HashMap<u64, RunningJob>,
    rx: mpsc::Receiver<Finished>,
    tx: mpsc::Sender<Finished>,
}

/// A job in flight: who asked, and the flag that asks it to stop.
struct RunningJob {
    cancel: Arc<AtomicBool>,
    pane: PaneId,
}

/// A finished job: which page asked, and what came back.
struct Finished {
    id: u64,
    pane: PaneId,
    reply: JobReply,
}

// ========================================================================
// Jobs
// ========================================================================

impl Jobs {
    /// An idle runner.
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            next_id: 0,
            running: HashMap::new(),
            rx,
            tx,
        }
    }

    /// Whether any job is still running, so the event loop knows to keep
    /// polling rather than settling into its idle interval.
    pub(crate) fn is_busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// Start `request` on behalf of `pane`. Dropped when the runner is already
    /// at capacity.
    pub(crate) fn spawn(&mut self, pane: PaneId, request: JobRequest) {
        if self.running.len() >= MAX_IN_FLIGHT {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.insert(
            id,
            RunningJob {
                cancel: Arc::clone(&cancel),
                pane,
            },
        );

        let tx = self.tx.clone();
        // A detached worker: nothing joins it, and a cancelled job is left to
        // notice the flag and exit on its own rather than being killed.
        let _ = thread::Builder::new()
            .name("winter job".into())
            .spawn(move || {
                let reply = run(request, &cancel);
                let _ = tx.send(Finished { id, pane, reply });
            });
    }

    /// Ask every job started for `pane` to stop, and discard their answers.
    pub(crate) fn cancel_for(&mut self, pane: PaneId) {
        self.running.retain(|_, job| {
            if job.pane != pane {
                return true;
            }
            job.cancel.store(true, Ordering::Relaxed);
            false
        });
    }

    /// Every answer that has arrived since the last call.
    pub(crate) fn drain(&mut self) -> Vec<(PaneId, JobReply)> {
        let mut done = Vec::new();
        while let Ok(finished) = self.rx.try_recv() {
            // A job cancelled while it ran is no longer in the map, and its
            // answer is to a question nobody is asking any more.
            if self.running.remove(&finished.id).is_none() {
                continue;
            }
            done.push((finished.pane, finished.reply));
        }
        done
    }
}

// ========================================================================
// App: page jobs
// ========================================================================

impl App {
    /// Hand each finished job's answer to the page that asked for it.
    pub(crate) fn deliver_finished_jobs(&mut self) -> bool {
        let finished = self.jobs.drain();
        let mut any = false;
        for (pane_id, reply) in finished {
            let Some(slot) = self.pages.get_mut(&pane_id) else {
                continue;
            };
            let outcome = slot.page.on_job(reply);
            self.act_on_page_outcome(pane_id, outcome);
            any = true;
        }
        any
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Do the work a request asks for, checking `cancel` often enough that a
/// cancelled job stops within a directory or two rather than at the end.
fn run(request: JobRequest, cancel: &AtomicBool) -> JobReply {
    match request {
        JobRequest::Command(request) => JobReply::Command(run_command(request)),
        JobRequest::DirSize(path) => {
            let bytes = walk_size(&path, cancel, 0);
            JobReply::DirSize { bytes, path }
        }
    }
}

/// Run a program to completion and collect what it wrote. Not interruptible:
/// these are short commands, and killing one mid-write is how a repository ends
/// up with a half-applied change.
fn run_command(request: CommandRequest) -> CommandOutput {
    let output = match &request.stdin {
        Some(input) => piped_output(&request, input),
        None => std::process::Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.cwd)
            .output(),
    };
    match output {
        Ok(output) => CommandOutput {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            tag: request.tag,
        },
        Err(e) => CommandOutput {
            code: None,
            stderr: e.to_string(),
            stdout: String::new(),
            tag: request.tag,
        },
    }
}

/// Run a program that reads its payload from standard input. The handle is
/// dropped before waiting, since a program reading to end-of-input would
/// otherwise never see one.
fn piped_output(request: &CommandRequest, input: &str) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = std::process::Command::new(&request.program)
        .args(&request.args)
        .current_dir(&request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut handle) = child.stdin.take() {
        handle.write_all(input.as_bytes())?;
    }
    child.wait_with_output()
}

/// Total bytes under `dir`, skipping what cannot be read and links rather than
/// following them, the way `du` does. Returns what it summed so far when
/// cancelled, which the caller discards.
fn walk_size(dir: &std::path::Path, cancel: &AtomicBool, depth: usize) -> u64 {
    const MAX_DEPTH: usize = 64;
    if depth >= MAX_DEPTH || cancel.load(Ordering::Relaxed) {
        return 0;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in reader.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return total;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            total += walk_size(&entry.path(), cancel, depth + 1);
        } else {
            total += meta.len();
        }
    }
    total
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// Poll until `count` answers arrive or `timeout` passes. A test expecting
    /// nothing passes a short timeout, since it can only ever wait it out.
    fn wait_for_replies(
        jobs: &mut Jobs,
        count: usize,
        timeout: Duration,
    ) -> Vec<(PaneId, JobReply)> {
        let deadline = Instant::now() + timeout;
        let mut collected = Vec::new();
        while collected.len() < count && Instant::now() < deadline {
            collected.extend(jobs.drain());
            std::thread::sleep(POLL_STEP);
        }
        collected
    }

    /// How long to sleep between polls, so waiting does not spin a core.
    const POLL_STEP: Duration = Duration::from_millis(5);

    /// Long enough for a walk of a handful of files on a loaded machine.
    const ARRIVES: Duration = Duration::from_secs(5);

    /// Long enough to show nothing is coming, short enough not to pad the run.
    const NOTHING_COMING: Duration = Duration::from_millis(300);

    /// A temporary tree that removes itself on drop.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-jobs-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn test_a_directory_walk_totals_what_is_under_it() {
        let tree = TempTree::new("walk");
        std::fs::write(tree.0.join("a.bin"), vec![0u8; 100]).expect("a");
        std::fs::create_dir_all(tree.0.join("sub")).expect("sub");
        std::fs::write(tree.0.join("sub").join("b.bin"), vec![0u8; 50]).expect("b");

        let mut jobs = Jobs::new();
        let pane = PaneId(7);
        jobs.spawn(pane, JobRequest::DirSize(tree.0.clone()));
        assert!(jobs.is_busy());

        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        assert_eq!(replies.len(), 1, "the walk reported back");
        let (got_pane, reply) = &replies[0];
        assert_eq!(*got_pane, pane, "answers go to whoever asked");
        let JobReply::DirSize { bytes, .. } = reply else {
            panic!("expected a directory total");
        };
        assert_eq!(*bytes, 150, "nested files count toward the total");
        assert!(!jobs.is_busy(), "and the job is no longer running");
    }

    #[test]
    fn test_a_cancelled_job_is_not_reported_back() {
        // The worker may already be finishing when cancel arrives, so the
        // answer has to be discarded on arrival, not merely asked to stop.
        let tree = TempTree::new("cancel");
        let mut jobs = Jobs::new();
        let pane = PaneId(1);
        jobs.spawn(pane, JobRequest::DirSize(tree.0.clone()));
        jobs.cancel_for(pane);

        assert!(!jobs.is_busy(), "a cancelled job is no longer waited on");
        let replies = wait_for_replies(&mut jobs, 1, NOTHING_COMING);
        assert!(replies.is_empty(), "its answer is dropped");
    }

    #[test]
    fn test_cancelling_one_pane_leaves_another_pane_running() {
        let tree = TempTree::new("cancel-one");
        let mut jobs = Jobs::new();
        jobs.spawn(PaneId(1), JobRequest::DirSize(tree.0.clone()));
        jobs.spawn(PaneId(2), JobRequest::DirSize(tree.0.clone()));

        jobs.cancel_for(PaneId(1));
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].0, PaneId(2));
    }

    #[test]
    fn test_requests_past_the_limit_are_dropped_rather_than_queued() {
        // A queued request would be answering a question about a screen the
        // user has long since left.
        let tree = TempTree::new("limit");
        let mut jobs = Jobs::new();
        for _ in 0..MAX_IN_FLIGHT + 5 {
            jobs.spawn(PaneId(1), JobRequest::DirSize(tree.0.clone()));
        }
        assert!(jobs.running.len() <= MAX_IN_FLIGHT);
    }
}
