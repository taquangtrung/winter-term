//! One build running under a pty of its own, out of sight.
//!
//! A pty rather than pipes, so the tools behave as they do in a terminal:
//! colored, with their progress lines, flushing as they go rather than in
//! blocks once a pipe's buffer fills. What they write is read on a thread
//! and handed over as it comes, waking the window, so the output streams in
//! while the build runs.
//!
//! The command's process group is what stops: the shell, the compiler it
//! ran, and whatever that ran in turn. A `SIGTERM` first, so a build can
//! clean up after itself, then a `SIGKILL` for what is still there after
//! [`STOP_GRACE`].

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

#[cfg(unix)]
use crate::process::signal_group;
use crate::wake::wake_window;

// ========================================================================
// Constants
// ========================================================================

/// How wide the build's terminal is. Wide, since the lines are folded to
/// the pane when they are painted: a narrow terminal would have the tools
/// fold or cut them first.
pub const COLUMNS: u16 = 200;

/// How tall the build's terminal is, for the tools that size a display to it.
const ROWS: u16 = 50;

/// What the build is told its terminal is: one that takes colors.
const TERM: &str = "xterm-256color";

/// The pager a build's tools are handed: none, since a pager waits for keys
/// the hidden terminal never sends, and the tab already scrolls.
const PAGER: &str = "cat";

/// How much of the output is read at a time.
const READ_CHUNK: usize = 16 * 1024;

/// How long the output is still read for once the build has exited: a
/// process it left in the background may hold the terminal open for ever.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// How long a build asked to stop has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// How often a stopping build is looked at to see whether it has gone.
const STOP_POLL: Duration = Duration::from_millis(20);

// ========================================================================
// Data Structures
// ========================================================================

/// A build in progress.
pub struct Runner {
    /// What the build sends, in the order it happens.
    events: mpsc::Receiver<Event>,
    /// Set once the build has exited.
    exited: Arc<AtomicBool>,
    /// What kills the build where it leads no group to signal.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Where what is typed for the build goes, as a terminal's keys would.
    input: Option<Box<dyn Write + Send>>,
    /// The pty's own side, held for as long as the build runs: closing it
    /// hangs the build up.
    _master: Box<dyn MasterPty + Send>,
    /// The process group the build leads.
    #[cfg(unix)]
    group: Option<libc::pid_t>,
}

/// What a running build sends.
#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    /// The build has exited, and all it wrote has been sent.
    Exited(ExitStatus),
    /// Some of what it wrote.
    Output(Vec<u8>),
}

/// How a build ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    /// The exit code, which a build ended by a signal has none of.
    pub code: Option<u32>,
    /// The signal that ended it, by name.
    pub signal: Option<String>,
}

// ========================================================================
// Runner
// ========================================================================

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("exited", &self.exited.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Runner {
    /// Start `program` with `args` in `dir`, with `env` set over Winter's
    /// own environment.
    pub fn start(
        program: &str,
        args: &[String],
        dir: &Path,
        env: &[(String, String)],
    ) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLUMNS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("cannot open a terminal for the build: {e}"))?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(dir);
        command.env("TERM", TERM);
        command.env("COLORTERM", "truecolor");
        command.env("WINTER", "1");
        command.env("PAGER", PAGER);
        command.env("GIT_PAGER", PAGER);
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        // Only the build may hold the terminal's far side, so that its
        // output ends when the build does.
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("cannot read the build's output: {e}"))?;
        let killer = child.clone_killer();
        let input = pair.master.take_writer().ok();
        #[cfg(unix)]
        let group = child
            .process_id()
            .and_then(|pid| libc::pid_t::try_from(pid).ok());
        let (send, events) = mpsc::channel();
        let (drained, drained_wait) = mpsc::channel::<()>();
        let output = send.clone();
        spawn_named("winter compile output", move || {
            let mut buffer = vec![0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if output.send(Event::Output(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                        wake_window();
                    }
                }
            }
            let _ = drained.send(());
        })?;
        let exited = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&exited);
        spawn_named("winter compile wait", move || {
            let status = child.wait();
            flag.store(true, Ordering::Relaxed);
            // What the build wrote last goes out before word that it has
            // gone, unless something it left behind keeps the terminal open.
            let _ = drained_wait.recv_timeout(DRAIN_GRACE);
            let status = match status {
                Ok(status) => ExitStatus {
                    code: status.signal().is_none().then(|| status.exit_code()),
                    signal: status.signal().map(str::to_string),
                },
                Err(_) => ExitStatus {
                    code: None,
                    signal: None,
                },
            };
            let _ = send.send(Event::Exited(status));
            wake_window();
        })?;
        Ok(Self {
            events,
            exited,
            input,
            killer,
            _master: pair.master,
            #[cfg(unix)]
            group,
        })
    }

    /// Type `text` to the build, as though at its terminal. Whether it could
    /// be written.
    pub fn send(&mut self, text: &[u8]) -> bool {
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        input.write_all(text).and_then(|()| input.flush()).is_ok()
    }

    /// What the build has sent since this was last asked, in order.
    pub fn take_events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Whether the build has exited, whether or not that has been taken yet.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    /// Ask the build to stop, and kill it if it has not within
    /// [`STOP_GRACE`]. Returns at once: the stop is seen through on a thread
    /// of its own, and the build's [`Event::Exited`] says when it is done.
    pub fn stop(&mut self) {
        if self.has_exited() {
            return;
        }
        #[cfg(unix)]
        if let Some(group) = self.group {
            signal_group(group, libc::SIGTERM);
            let exited = Arc::clone(&self.exited);
            let _ = spawn_named("winter compile stop", move || {
                let mut waited = Duration::ZERO;
                while waited < STOP_GRACE && !exited.load(Ordering::Relaxed) {
                    thread::sleep(STOP_POLL);
                    waited += STOP_POLL;
                }
                if !exited.load(Ordering::Relaxed) {
                    signal_group(group, libc::SIGKILL);
                }
            });
            return;
        }
        let _ = self.killer.kill();
    }
}

impl Drop for Runner {
    /// A build whose tab has gone is not left running.
    fn drop(&mut self) {
        self.stop();
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Start `work` on a thread called `name`.
fn spawn_named(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .map(|_| ())
        .map_err(|e| format!("cannot start a thread for the build: {e}"))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Long enough for a loaded machine to start and end a shell.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// Everything a build sends until it exits.
    fn run_to_end(runner: &Runner) -> (Vec<u8>, ExitStatus) {
        let deadline = Instant::now() + PATIENCE;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            for event in runner.take_events() {
                match event {
                    Event::Output(bytes) => output.extend(bytes),
                    Event::Exited(status) => return (output, status),
                }
            }
            thread::sleep(STOP_POLL);
        }
        panic!(
            "the build did not end: {}",
            String::from_utf8_lossy(&output)
        );
    }

    fn shell(command: &str) -> Runner {
        Runner::start(
            "sh",
            &["-c".to_string(), command.to_string()],
            Path::new("/"),
            &[("WINTER_TEST".to_string(), "set".to_string())],
        )
        .expect("starts the shell")
    }

    #[test]
    fn test_a_builds_output_and_exit_code_arrive_in_order() {
        let runner = shell("printf 'one\\n'; printf \"$WINTER_TEST\"; exit 3");
        let (output, status) = run_to_end(&runner);
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("one\r\nset"), "{text:?}");
        assert_eq!(status.code, Some(3));
    }

    #[test]
    fn test_what_is_typed_reaches_the_build() {
        let mut runner = shell("read line; echo \"got $line\"");
        assert!(runner.send(b"hello\r"));
        let (output, status) = run_to_end(&runner);
        assert!(String::from_utf8_lossy(&output).contains("got hello"));
        assert_eq!(status.code, Some(0));
    }

    #[test]
    fn test_the_build_sees_a_terminal() {
        let runner = shell("test -t 1 && echo tty");
        let (output, status) = run_to_end(&runner);
        assert!(String::from_utf8_lossy(&output).contains("tty"));
        assert_eq!(status.code, Some(0));
    }

    #[test]
    fn test_stopping_ends_the_build_and_what_it_started() {
        let mut runner = shell("sleep 30 & sleep 30; echo never");
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        runner.stop();
        let (output, status) = run_to_end(&runner);
        assert!(started.elapsed() < PATIENCE);
        assert!(!String::from_utf8_lossy(&output).contains("never"));
        assert_eq!(status.code, None);
    }
}
