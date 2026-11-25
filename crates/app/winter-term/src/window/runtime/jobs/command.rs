//! Running an editor's command on a worker thread: the child spawned in its own
//! process group, its output captured as it streams, and the whole group
//! stopped when the job is cancelled. A command asked to report its progress
//! has each step of it reported as it comes, and left out of its answer.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use winter_base::bytes_text;
#[cfg(unix)]
use winter_base::process::signal_group;
use winter_model::editor::{CommandOutput, CommandRequest};
use winter_vcs::exec::PROGRESS;

// ========================================================================
// Constants
// ========================================================================

/// How long the output of a command that has exited may take to reach its
/// end. A program that has ended has written everything it will, so its own
/// output arrives at once; what holds a pipe open past this is something it
/// left running (an ssh connection kept for reuse, a helper daemon), which
/// may never close it.
pub(super) const STREAM_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// How much of a command's output is read at a time.
const STREAM_CHUNK: usize = 8 * 1024;

/// How often a running command is checked for having finished, or for
/// having been asked to stop.
const COMMAND_POLL: Duration = Duration::from_millis(5);

/// How long a stopped command's process group has to end on SIGTERM before
/// it is killed outright. Git removes its lock files on SIGTERM, so it is
/// given the chance rather than killed mid-write.
#[cfg(unix)]
const STOP_GRACE: Duration = Duration::from_secs(2);

/// What a stopped command answers with, in place of what it would have
/// written to standard error.
pub(in crate::window) const STOPPED: &str = "stopped";

/// How much of the end of standard error is looked through for the latest
/// step of a command's progress: a few of git's lines.
const PROGRESS_TAIL: usize = 1024;

/// What a remote's own progress lines start with, ahead of the step.
const REMOTE_PREFIX: &str = "remote: ";

/// What stands between a step's name and how far it has got.
const STEP_SEPARATOR: &str = ": ";

// ========================================================================
// Data Structures
// ========================================================================

/// One of a command's output streams, read on a thread of its own into a
/// buffer the runner can take from at any time: what has arrived so far is
/// there to answer with even while something still holds the stream open.
pub(super) struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    finished: Arc<AtomicBool>,
}

// ========================================================================
// Capture Implementations
// ========================================================================

impl Capture {
    /// Start reading `pipe` to its end. A missing pipe reads as finished and
    /// empty.
    pub(super) fn spawn(pipe: Option<Box<dyn Read + Send>>) -> Self {
        let capture = Self {
            bytes: Arc::new(Mutex::new(Vec::new())),
            finished: Arc::new(AtomicBool::new(false)),
        };
        let Some(mut pipe) = pipe else {
            capture.finished.store(true, Ordering::Relaxed);
            return capture;
        };
        let bytes = Arc::clone(&capture.bytes);
        let finished = Arc::clone(&capture.finished);
        let reader = thread::Builder::new()
            .name("winter job output".into())
            .spawn(move || {
                let mut chunk = [0u8; STREAM_CHUNK];
                while let Ok(read) = pipe.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    if let Ok(mut held) = bytes.lock() {
                        held.extend_from_slice(&chunk[..read]);
                    }
                }
                finished.store(true, Ordering::Relaxed);
            });
        if reader.is_err() {
            // Nothing will read this stream, so nothing is waited for.
            capture.finished.store(true, Ordering::Relaxed);
        }
        capture
    }

    /// How much has arrived so far.
    fn len(&self) -> usize {
        self.bytes.lock().map_or(0, |held| held.len())
    }

    /// The last `max` bytes of what has arrived, leaving it all there.
    fn tail(&self, max: usize) -> Vec<u8> {
        self.bytes
            .lock()
            .map(|held| held[held.len().saturating_sub(max)..].to_vec())
            .unwrap_or_default()
    }

    /// Whether the stream has reached its end.
    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// What has arrived so far.
    fn take(&self) -> Vec<u8> {
        self.bytes
            .lock()
            .map(|mut held| std::mem::take(&mut *held))
            .unwrap_or_default()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Run a program to completion and collect what it wrote. There is no time
/// limit: a push over a slow link or a long hook takes what it takes. A
/// command asked to stop through `cancel` is stopped, with everything it
/// started, and answers [`STOPPED`]. Each step of the progress a command
/// asked to report is given to `report` as it comes.
pub(super) fn run_command(
    request: CommandRequest,
    cancel: &AtomicBool,
    report: &dyn Fn(&str),
) -> CommandOutput {
    let (id, tag) = (request.id, request.tag);
    let reports_progress = asks_for_progress(&request);
    let (code, stdout, stderr) = match run_until_stopped(&request, cancel, report) {
        Ok(Some(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            (
                output.status.code(),
                // Every byte kept, so a name or a patch that is not UTF-8 goes
                // back to the program as it came (see [`bytes_text`]).
                bytes_text::decode(&output.stdout),
                // The steps were shown as they came; what is left is what
                // the command had to say, its failure first.
                match reports_progress {
                    true => without_progress(&stderr),
                    false => stderr.into_owned(),
                },
            )
        }
        Ok(None) => (None, String::new(), STOPPED.to_string()),
        Err(e) => (None, String::new(), e.to_string()),
    };
    CommandOutput {
        code,
        id,
        stderr,
        stdout,
        tag,
    }
}

/// Run `request`, feeding it its standard input if it has any, until it ends
/// or `cancel` is set: `None` once it has been stopped. A request asking
/// for its progress has each new step of it given to `report`.
pub(super) fn run_until_stopped(
    request: &CommandRequest,
    cancel: &AtomicBool,
    report: &dyn Fn(&str),
) -> std::io::Result<Option<std::process::Output>> {
    let mut command = Command::new(&request.program);
    // A group of its own, so stopping it reaches what it started too: the
    // ssh a push runs, a hook, a credential helper.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut command = command;
    command.args(request.args.iter().map(|arg| bytes_text::to_os(arg)));
    if !request.cwd.as_os_str().is_empty() {
        command.current_dir(&request.cwd);
    }
    let mut child = command
        .envs(request.env.iter().map(|(name, value)| (name, value)))
        .stdin(match request.stdin {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // The pipes are filled and drained on threads of their own: a payload or
    // an answer bigger than a pipe holds would otherwise stall the command
    // against a runner that is not reading yet. The handle is dropped once
    // written, since a program reading to end-of-input would never see one
    // otherwise.
    if let (Some(mut handle), Some(input)) = (child.stdin.take(), request.stdin.clone()) {
        thread::spawn(move || {
            let _ = handle.write_all(&bytes_text::encode(&input));
        });
    }
    let stdout = Capture::spawn(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = Capture::spawn(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );

    let reports_progress = asks_for_progress(request);
    let mut heard = 0;
    let mut last_step = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if reports_progress && stderr.len() != heard {
            heard = stderr.len();
            let step = latest_progress(&stderr.tail(PROGRESS_TAIL));
            if let Some(step) = step.filter(|step| last_step.as_ref() != Some(step)) {
                report(&step);
                last_step = Some(step);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            // The readers are left to end by themselves: a grandchild holding
            // the pipes open would keep a join waiting.
            stop_child(&mut child);
            return Ok(None);
        }
        thread::sleep(COMMAND_POLL);
    };
    // The program has ended, but what it left running may hold its pipes for
    // ever: the answer is what arrived within the grace, and the stop key
    // still works while it is waited for.
    if !await_captures(&[&stdout, &stderr], STREAM_DRAIN_GRACE, cancel) {
        return Ok(None);
    }
    Ok(Some(std::process::Output {
        status,
        stdout: stdout.take(),
        stderr: stderr.take(),
    }))
}

/// Wait up to `grace` for every capture in `captures` to reach the end of its
/// stream. `false` when `cancel` was set first, and the answer is not wanted.
pub(super) fn await_captures(captures: &[&Capture], grace: Duration, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + grace;
    while !captures.iter().all(|capture| capture.is_finished()) && Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(COMMAND_POLL);
    }
    !cancel.load(Ordering::Relaxed)
}

/// Whether `request` asks the program to report its progress.
fn asks_for_progress(request: &CommandRequest) -> bool {
    request.args.iter().any(|arg| arg == PROGRESS)
}

/// The latest step of progress `stderr` ends with, such as `Writing
/// objects:  45% (9/20)`. A step is written over by the next with a carriage
/// return, so only those ended by one, or by a newline, are whole.
fn latest_progress(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    let mut segments: Vec<&str> = text.split(['\r', '\n']).collect();
    // What follows the last ending is still being written.
    segments.pop();
    segments
        .into_iter()
        .rev()
        .find(|segment| is_progress(segment))
        .map(|segment| segment.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `stderr` with the steps of its progress left out, leaving what the
/// command had to say: an error, a hint, where it pushed to.
fn without_progress(stderr: &str) -> String {
    stderr
        .split('\n')
        .filter_map(|line| {
            let kept: Vec<&str> = line
                .split('\r')
                .filter(|segment| !is_progress(segment))
                .collect();
            let kept = kept.concat();
            (!kept.trim().is_empty()).then_some(kept)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `segment` is a step of git's progress: a capitalized name, then
/// a count, as in `Counting objects: 100% (5/5), done.` or a remote's
/// `remote: Compressing objects:  50% (1/2)`.
fn is_progress(segment: &str) -> bool {
    let step = segment
        .strip_prefix(REMOTE_PREFIX)
        .unwrap_or(segment)
        .trim_start();
    let Some((name, count)) = step.split_once(STEP_SEPARATOR) else {
        return false;
    };
    name.starts_with(char::is_uppercase)
        && name.chars().all(|c| c.is_alphabetic() || c == ' ')
        && count.trim_start().starts_with(|c: char| c.is_ascii_digit())
}

/// Stop `child` and the process group it leads: SIGTERM, so git can clean
/// up after itself, then SIGKILL for whatever is still there after
/// [`STOP_GRACE`].
#[cfg(unix)]
fn stop_child(child: &mut std::process::Child) {
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    signal_group(group, libc::SIGTERM);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        thread::sleep(COMMAND_POLL);
    }
    // Whatever of the group outlived SIGTERM goes now, the leader included
    // if it ignored it.
    signal_group(group, libc::SIGKILL);
    let _ = child.wait();
}

/// Stop `child`, which on this platform leads no group of its own.
#[cfg(not(unix))]
fn stop_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}
