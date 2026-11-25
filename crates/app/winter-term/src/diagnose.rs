//! Profiling, off by default, and what a running window says about itself.
//!
//! Profiling is turned on for a window by launching it with [`PROFILE_FLAG`]
//! or by the command that toggles it ([`set_profiling`]): the watchdog then
//! keeps the window's slow turns and writes its threads when it freezes
//! ([`crate::watchdog`]), and warnings go to `winter.log` too
//! ([`crate::logging`]), and the window counts its frames, how long each
//! took to draw, and which tabs' output asked for them ([`note_frame`],
//! [`note_output`]). Off, none of it does any work.
//!
//! What a running window says about itself for `winter profile`: its version,
//! GPU, the programs under it, its recent slow turns, and every thread's
//! state and stack. All of it is read off the window's thread, on the
//! control listener's ([`crate::control`]), so a frozen window still
//! answers. [`report`] lays it out with the user's files for an issue.
//!
//! Thread states and the programs come from `/proc`; the stacks from `gdb`,
//! when it is installed, let attach to this process alone for the moment it
//! takes (`PR_SET_PTRACER`), which a system that only lets a process trace
//! its own children otherwise forbids. Off Linux there are neither yet.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ========================================================================
// Constants
// ========================================================================

/// How many of a log's last lines a report carries.
const LOG_TAIL_LINES: usize = 200;

/// The flag that launches a window with profiling on.
pub const PROFILE_FLAG: &str = "--profile";

/// Whether profiling is on.
static PROFILING: AtomicBool = AtomicBool::new(false);

/// The GPU the window draws with, once its renderer has opened it.
static GPU: OnceLock<String> = OnceLock::new();

/// How far back the frame counts reach.
const FRAME_WINDOW: Duration = Duration::from_secs(10);

/// The frames drawn and the output that asked for them, while profiling.
static FRAMES: Mutex<FrameStats> = Mutex::new(FrameStats {
    drawn: VecDeque::new(),
    output: VecDeque::new(),
});

// ========================================================================
// Data Structures
// ========================================================================

/// A running window's account of itself.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Diagnosis {
    /// The GPU it draws with, `None` before its renderer opened one.
    pub gpu: Option<String>,
    /// The programs running under it, by name, as a tree.
    pub processes: String,
    /// Each thread's state and kernel wait, then every stack.
    pub threads: String,
    /// Its frames of the last seconds and the output that asked for them,
    /// while profiling; empty from a window too old to count them.
    #[serde(default)]
    pub frames: String,
    /// Its recent slow turns of the event loop, and the one under way.
    pub turns: String,
    /// Its version.
    pub version: String,
}

/// The frames drawn in the last [`FRAME_WINDOW`], and the tabs whose output
/// came in it.
struct FrameStats {
    /// When each frame was drawn, and how long drawing it took.
    drawn: VecDeque<(Instant, Duration)>,
    /// When a tab's output was read, and the tab's name.
    output: VecDeque<(Instant, String)>,
}

/// The user's files a report carries, by where they are.
pub struct ReportFiles {
    /// The crash log, when there is a state directory.
    pub crash_log: Option<PathBuf>,
    /// The log file, when there is a state directory.
    pub log: Option<PathBuf>,
    /// `settings.kdl`.
    pub settings: PathBuf,
}

// ========================================================================
// Functions
// ========================================================================

/// Turn profiling on or off for the rest of the process, or until turned
/// again: the watchdog's record of slow turns and freezes, and the log file.
pub fn set_profiling(on: bool) {
    PROFILING.store(on, Ordering::Relaxed);
    crate::logging::set_file(on);
    crate::watchdog::set_watching(on);
}

/// Whether profiling is on.
pub fn profiling() -> bool {
    PROFILING.load(Ordering::Relaxed)
}

/// This window's account of itself, now. Blocks while `gdb` walks the
/// stacks, up to a minute, so only the listener's thread calls it.
pub(crate) fn take() -> Diagnosis {
    Diagnosis {
        gpu: GPU.get().cloned(),
        processes: system::processes(),
        frames: frames_text(Instant::now()),
        threads: threads(),
        turns: crate::watchdog::recent_turns(),
        version: env!("WINTER_BUILD_VERSION").to_string(),
    }
}

/// Count a frame drawn now that took `took`, while profiling.
pub(crate) fn note_frame(took: Duration) {
    if !profiling() {
        return;
    }
    let now = Instant::now();
    let mut stats = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    stats.drawn.push_back((now, took));
    stats.forget_before(now);
}

/// Count output read now from the tab named `tab`, while profiling.
pub(crate) fn note_output(tab: impl FnOnce() -> String) {
    if !profiling() {
        return;
    }
    let now = Instant::now();
    let mut stats = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    stats.output.push_back((now, tab()));
    stats.forget_before(now);
}

/// The frames of the last [`FRAME_WINDOW`] before `now`, while profiling.
fn frames_text(now: Instant) -> String {
    let mut stats = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    stats.forget_before(now);
    match profiling() {
        true => stats.describe(),
        false => "(profiling is off)".to_string(),
    }
}

impl FrameStats {
    /// How many frames a second were drawn, how long they took, and the
    /// tabs whose output came most, most first.
    fn describe(&self) -> String {
        let count = self.drawn.len();
        let seconds = FRAME_WINDOW.as_secs_f64();
        let total: Duration = self.drawn.iter().map(|(_, took)| *took).sum();
        let slowest = self
            .drawn
            .iter()
            .map(|(_, took)| *took)
            .max()
            .unwrap_or_default();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mean = match count {
            0 => 0.0,
            n => ms(total) / n as f64,
        };
        let mut out = format!(
            "last {seconds:.0} s: {count} frames ({:.1} a second), drawn in {mean:.2} ms on \
             average, {:.2} ms at most ({:.1}% of the time drawing)\n",
            count as f64 / seconds,
            ms(slowest),
            ms(total) / (seconds * 1000.0) * 100.0
        );
        let mut by_tab: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, tab) in &self.output {
            *by_tab.entry(tab.as_str()).or_default() += 1;
        }
        let mut ranked: Vec<(&str, usize)> = by_tab.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (tab, reads) in ranked {
            let _ = writeln!(out, "  output read {reads} times from {tab}");
        }
        out
    }

    /// Forget what is older than [`FRAME_WINDOW`] before `now`.
    fn forget_before(&mut self, now: Instant) {
        let keep = |at: &Instant| now.saturating_duration_since(*at) <= FRAME_WINDOW;
        while self.drawn.front().is_some_and(|(at, _)| !keep(at)) {
            self.drawn.pop_front();
        }
        while self.output.front().is_some_and(|(at, _)| !keep(at)) {
            self.output.pop_front();
        }
    }
}

/// Keep `gpu`, the adapter the renderer opened, for every diagnosis after.
pub(crate) fn note_gpu(gpu: String) {
    let _ = GPU.set(gpu);
}

/// Each thread's state and kernel wait, then every stack.
pub(crate) fn threads() -> String {
    let mut out = system::thread_states();
    out.push('\n');
    out.push_str(&system::stacks());
    out
}

/// What `winter profile stacks` prints of `diagnosis`: the slow turns, then
/// every thread.
pub fn stacks_text(diagnosis: &Diagnosis) -> String {
    format!(
        "Recent slow turns:\n{}\nThreads:\n{}",
        diagnosis.turns, diagnosis.threads
    )
}

/// A file name for what was gathered now, led by `prefix`:
/// `winter-report-20261122-164930.txt`.
pub fn stamped_name(prefix: &str) -> String {
    format!(
        "{prefix}-{}.txt",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    )
}

/// A report for an issue: what this command and the system are, the
/// window's `diagnosis` (or why none came), and the user's `files`. It
/// holds `settings.kdl` whole, for the user to look over before sharing.
pub fn report(diagnosis: Result<Diagnosis, String>, files: &ReportFiles) -> String {
    let mut out = format!(
        "Winter report, {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z")
    );
    let _ = writeln!(
        out,
        "winter (this command): {}",
        env!("WINTER_BUILD_VERSION")
    );
    let _ = writeln!(out, "system: {}", system::describe());
    match diagnosis {
        Ok(window) => {
            let _ = writeln!(out, "window: {}", window.version);
            let gpu = window.gpu.as_deref().unwrap_or("(not opened yet)");
            let _ = writeln!(out, "GPU: {gpu}");
            section(&mut out, "Programs under the window", &window.processes);
            section(&mut out, "Frames", &window.frames);
            section(&mut out, "Recent slow turns", &window.turns);
            section(&mut out, "Threads", &window.threads);
        }
        Err(why) => {
            let _ = writeln!(out, "window: no running window answered: {why}");
        }
    }
    section(&mut out, "settings.kdl", &read_or_say(&files.settings));
    for (name, path) in [("winter.log", &files.log), ("crash.log", &files.crash_log)] {
        let body = match path {
            Some(path) => tail(&read_or_say(path), LOG_TAIL_LINES),
            None => "(no state directory)".to_string(),
        };
        section(
            &mut out,
            &format!("{name}, last {LOG_TAIL_LINES} lines"),
            &body,
        );
    }
    out
}

/// Append a section titled `title` holding `body` to `out`.
fn section(out: &mut String, title: &str, body: &str) {
    let _ = write!(out, "\n== {title}\n{}", body.trim_end());
    out.push('\n');
}

/// The file at `path`, or a line saying why it could not be read.
fn read_or_say(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| format!("({} could not be read: {error})", path.display()))
}

/// The last `lines` lines of `text`.
fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

// ========================================================================
// System
// ========================================================================

#[cfg(target_os = "linux")]
mod system {
    //! Threads, programs, and stacks, read from `/proc` and `gdb`.

    use std::fmt::Write as _;
    use std::fs::{File, OpenOptions};
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    /// How long `gdb` may take to load symbols and walk every stack.
    const GDB_TIMEOUT: Duration = Duration::from_secs(60);

    /// The shell's status when it found no `gdb` or `timeout` to run.
    const NOT_FOUND: i32 = 127;

    /// The shell's status when `timeout` killed `gdb` (128 + `SIGKILL`).
    const KILLED: i32 = 137;

    /// The stacks' file is readable and writable by this user alone.
    const PRIVATE_MODE: u32 = 0o600;

    /// Held through each walk of the stacks, so two never attach at once.
    static WALK: Mutex<()> = Mutex::new(());

    /// The most programs a tree lists, so a fork bomb under a terminal does
    /// not make a report of it.
    const MAX_PROCESSES: usize = 500;

    /// The system's name, release, and kernel.
    pub(super) fn describe() -> String {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let name = release
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map_or("Linux", |name| name.trim_matches('"'))
            .to_string();
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
        format!(
            "{name}, kernel {}, {}",
            kernel.trim(),
            std::env::consts::ARCH
        )
    }

    /// One line per thread: its id, name, state (`R` running, `S` asleep,
    /// `D` in an uninterruptible wait), and what the kernel says it waits
    /// on.
    pub(super) fn thread_states() -> String {
        let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
            return "(no thread list)\n".to_string();
        };
        let mut out = String::new();
        for task in tasks.flatten() {
            let dir = task.path();
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            let _ = writeln!(
                out,
                "{:>8} {:<16} {} {}",
                task.file_name().to_string_lossy(),
                read("comm").trim(),
                state_of(&read("stat")),
                read("wchan").trim(),
            );
        }
        out
    }

    /// The programs running under this process, one per line by name,
    /// indented under the program that started it.
    pub(super) fn processes() -> String {
        let mut out = String::new();
        let mut listed = 0;
        children_into(Path::new("/proc/self"), 0, &mut out, &mut listed);
        match out.is_empty() {
            true => "(none)\n".to_string(),
            false => out,
        }
    }

    /// List the children of the process at `dir`, `depth` deep, into `out`,
    /// counting them in `listed`.
    fn children_into(dir: &Path, depth: usize, out: &mut String, listed: &mut usize) {
        let Ok(tasks) = std::fs::read_dir(dir.join("task")) else {
            return;
        };
        let mut children: Vec<u32> = tasks
            .flatten()
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .flat_map(|ids| {
                ids.split_whitespace()
                    .filter_map(|id| id.parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect();
        children.sort_unstable();
        for pid in children {
            if *listed >= MAX_PROCESSES {
                return;
            }
            *listed += 1;
            let child = Path::new("/proc").join(pid.to_string());
            let name = std::fs::read_to_string(child.join("comm")).unwrap_or_default();
            let stat = std::fs::read_to_string(child.join("stat")).unwrap_or_default();
            let indent = "  ".repeat(depth);
            let _ = writeln!(out, "{indent}{} {}", name.trim(), state_of(&stat));
            children_into(&child, depth + 1, out, listed);
        }
    }

    /// The state letter in a `stat` line: it follows the name, which is in
    /// parentheses and may hold spaces itself.
    fn state_of(stat: &str) -> &str {
        stat.rsplit_once(") ")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .unwrap_or("?")
    }

    /// Every thread's stack from `gdb`, or why there is none, one walk at a
    /// time.
    ///
    /// While `gdb` is attached this whole process is stopped, the thread
    /// waiting on it too, so nothing here may depend on this process to
    /// finish: `gdb` writes to a file rather than a pipe it could fill, and
    /// `timeout`, outside the process, ends a `gdb` that runs too long, the
    /// shell around it waking this process after in case `gdb` left it
    /// stopped.
    pub(super) fn stacks() -> String {
        let _one = WALK.lock().unwrap_or_else(PoisonError::into_inner);
        let pid = std::process::id();
        let path = private_file_path(pid);
        let opened = create_private(&path).and_then(|out| Ok((out.try_clone()?, out)));
        let (out, err) = match opened {
            Ok(files) => files,
            Err(error) => return format!("(no stacks: {error})\n"),
        };
        let script = format!(
            "timeout --signal=KILL {} gdb -q -nx; status=$?; kill -CONT {pid}; exit $status",
            GDB_TIMEOUT.as_secs()
        );
        let spawned = Command::new("sh")
            .args(["-c", &script])
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(err)
            .spawn();
        let mut shell = match spawned {
            Ok(shell) => shell,
            Err(error) => return format!("(no stacks: {error})\n"),
        };
        // The shell and what it starts may trace this process.
        allow_tracer(shell.id());
        // Attach only once it may: the commands wait on stdin until now.
        let commands = format!(
            "set confirm off\nset pagination off\nattach {pid}\nthread apply all bt\ndetach\nquit\n"
        );
        if let Some(mut stdin) = shell.stdin.take() {
            let _ = stdin.write_all(commands.as_bytes());
        }
        let status = shell.wait();
        allow_tracer(0);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        match status.ok().and_then(|status| status.code()) {
            Some(NOT_FOUND) => "(no stacks: gdb is not installed)\n".to_string(),
            Some(KILLED) => format!(
                "(gdb took longer than {}s; what it found)\n{text}",
                GDB_TIMEOUT.as_secs()
            ),
            _ => text,
        }
    }

    /// A path in the temporary directory, shared with other users, for this
    /// process's stacks: hard to guess, so none can be laid in wait.
    fn private_file_path(pid: u32) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        std::env::temp_dir().join(format!("winter-stacks-{pid}-{nanos}.txt"))
    }

    /// Create the file at `path` for this user alone, refusing one already
    /// there and a symbolic link: another user could have put either there
    /// to have the stacks written where they choose, or to read them.
    fn create_private(path: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(PRIVATE_MODE)
            .open(path)
    }

    /// Let process `pid` trace this one, `0` taking that back.
    fn allow_tracer(pid: u32) {
        // SAFETY: `prctl` with `PR_SET_PTRACER` takes a process id, touches
        // no memory of ours, and fails harmlessly where Yama is absent.
        #[allow(unsafe_code)]
        unsafe {
            libc::prctl(libc::PR_SET_PTRACER, libc::c_ulong::from(pid), 0, 0, 0);
        }
    }

    // ====================================================================
    // Tests
    // ====================================================================

    #[cfg(test)]
    mod tests {
        use std::os::unix::fs::PermissionsExt as _;

        use super::*;

        #[test]
        fn test_the_thread_list_names_each_thread_and_its_state() {
            let name = "diagnose-probe";
            let (ready, wait) = std::sync::mpsc::channel();
            let (done, finish) = std::sync::mpsc::channel::<()>();
            let probe = std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    let _ = ready.send(());
                    let _ = finish.recv();
                })
                .expect("spawned");
            wait.recv().expect("started");
            let listed = thread_states();
            let _ = done.send(());
            probe.join().expect("joined");
            let line = listed
                .lines()
                .find(|line| line.contains(name))
                .expect("the probe listed");
            assert!(line.contains(" S "), "asleep on its channel: {line}");
        }

        #[test]
        fn test_the_programs_under_the_process_are_listed_by_name() {
            let mut child = Command::new("sleep").arg("30").spawn().expect("spawned");
            let listed = processes();
            let _ = child.kill();
            let _ = child.wait();
            assert!(
                listed
                    .lines()
                    .any(|line| line.trim_start().starts_with("sleep ")),
                "{listed}"
            );
        }

        #[test]
        fn test_the_stacks_file_is_private_and_never_follows_what_was_laid_there() {
            let path = private_file_path(std::process::id());
            let file = create_private(&path).expect("created");
            let mode = file.metadata().expect("read").permissions().mode();
            assert!(
                create_private(&path).is_err(),
                "one already there is refused"
            );
            let _ = std::fs::remove_file(&path);
            assert_eq!(mode & 0o777, PRIVATE_MODE);
            let target = path.with_extension("target");
            std::os::unix::fs::symlink(&target, &path).expect("linked");
            let followed = create_private(&path);
            let _ = std::fs::remove_file(&path);
            assert!(followed.is_err(), "a symbolic link is refused");
            assert!(!target.exists(), "and nothing written where it pointed");
        }

        #[test]
        #[ignore = "needs gdb installed"]
        fn test_gdb_may_attach_and_walks_every_thread() {
            let stacks = stacks();
            assert!(stacks.contains("Thread 1"), "{stacks}");
            assert!(stacks.contains("stacks"), "this very function: {stacks}");
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod system {
    //! Nothing read off Linux yet but the system's name.

    /// The system's name and architecture.
    pub(super) fn describe() -> String {
        format!("{}, {}", std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Why there are no thread states.
    pub(super) fn thread_states() -> String {
        "(no thread states on this system)\n".to_string()
    }

    /// Why there is no program tree.
    pub(super) fn processes() -> String {
        "(no program list on this system)\n".to_string()
    }

    /// Why there are no stacks.
    pub(super) fn stacks() -> String {
        "(no stacks on this system)\n".to_string()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frames_count_the_last_seconds_and_rank_the_tabs_by_output() {
        let now = Instant::now();
        let long_ago = now - FRAME_WINDOW - Duration::from_secs(1);
        let mut stats = FrameStats {
            drawn: VecDeque::from([
                (long_ago, Duration::from_millis(50)),
                (now, Duration::from_millis(4)),
                (now, Duration::from_millis(6)),
            ]),
            output: VecDeque::from([
                (long_ago, "tab 9 (old)".to_string()),
                (now, "tab 1 (shell)".to_string()),
                (now, "tab 2 (claude)".to_string()),
                (now, "tab 2 (claude)".to_string()),
            ]),
        };
        stats.forget_before(now);
        let text = stats.describe();
        assert!(
            text.starts_with("last 10 s: 2 frames (0.2 a second)"),
            "{text}"
        );
        assert!(
            text.contains("5.00 ms on average, 6.00 ms at most"),
            "{text}"
        );
        let claude = text
            .find("2 times from tab 2 (claude)")
            .expect("claude counted");
        let shell = text
            .find("1 times from tab 1 (shell)")
            .expect("shell counted");
        assert!(claude < shell, "the busiest first: {text}");
        assert!(
            !text.contains("old"),
            "what is older than the window is forgotten"
        );
    }

    #[test]
    fn test_a_report_without_a_window_still_carries_the_settings_and_the_logs_last_lines() {
        let dir = std::env::temp_dir().join(format!("winter-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let settings = dir.join("settings.kdl");
        std::fs::write(&settings, "font-size 14\n").expect("written");
        let log = dir.join("winter.log");
        let lines: Vec<String> = (0..LOG_TAIL_LINES + 50)
            .map(|n| format!("line {n}"))
            .collect();
        std::fs::write(&log, lines.join("\n")).expect("written");
        let files = ReportFiles {
            crash_log: Some(dir.join("crash.log")),
            log: Some(log),
            settings,
        };
        let report = report(Err("connection refused".to_string()), &files);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(report.contains("no running window answered: connection refused"));
        assert!(report.contains("font-size 14"));
        assert!(
            report.contains("line 249") && !report.contains("line 49\n"),
            "the tail only"
        );
        assert!(
            report.contains("crash.log could not be read"),
            "a missing log says so"
        );
    }
}
