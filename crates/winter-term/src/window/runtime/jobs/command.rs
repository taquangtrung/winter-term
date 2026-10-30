//! Running an editor's command on a worker thread: the child spawned in its own
//! process group, its output captured as it streams, and the whole group
//! stopped when the job is cancelled.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::bytes_text;
use crate::model::editor::{CommandOutput, CommandRequest};
#[cfg(unix)]
use crate::process::signal_group;

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
/// started, and answers [`STOPPED`].
pub(super) fn run_command(request: CommandRequest, cancel: &AtomicBool) -> CommandOutput {
    let (id, tag) = (request.id, request.tag);
    let (code, stdout, stderr) = match run_until_stopped(&request, cancel) {
        Ok(Some(output)) => (
            output.status.code(),
            // Every byte kept, so a name or a patch that is not UTF-8 goes
            // back to the program as it came (see [`bytes_text`]).
            bytes_text::decode(&output.stdout),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ),
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
/// or `cancel` is set: `None` once it has been stopped.
pub(super) fn run_until_stopped(
    request: &CommandRequest,
    cancel: &AtomicBool,
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

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
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
