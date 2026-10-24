//! Slow work off the event-loop thread: an editor asks for it, a worker thread
//! does it, and the answer is collected on the next poll.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::editors::dir::ops;
use crate::model::editor::{
    CommandOutput, CommandRequest, FileOps, JobReply, JobRequest, SearchHit, SearchRequest,
    SearchResult,
};
use crate::model::input::{format_key, Key, KeyCode};
use crate::model::layout::TabId;

use crate::editors::proc::collect;
use crate::editors::sys;

use super::wake::Waker;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// The key that stops the running tasks of the editor tab it is pressed in,
/// while it has some. `C-g`, Emacs' and magit's quit: an editor tab has no shell
/// to send it to, and a terminal tab never sees it taken.
pub(crate) const STOP_TASKS_KEY: Key = Key {
    alt: false,
    code: KeyCode::Char('g'),
    ctrl: true,
    shift: false,
    sup: false,
};

/// How many jobs may run at once. Past this, a request is dropped rather than
/// queued: every caller asks for work about what is on screen, so a request
/// that waited long enough to run would be answering a stale question.
const MAX_IN_FLIGHT: usize = 16;

/// Matches a search reports before it stops. An editor shows a screenful at a
/// time, and a query loose enough to pass this is one to narrow, not to hold
/// every answer to.
const SEARCH_MAX_HITS: usize = 500;

/// Longest a hit's text is kept. One minified line can run to megabytes, which
/// no pane can show and nothing should hold five hundred of.
const SEARCH_MAX_LINE: usize = 300;

/// Largest file a search reads. Anything past this is generated, packed, or
/// both, and reading it costs more than the match is worth.
const SEARCH_MAX_FILE_BYTES: u64 = 1 << 20;

/// How deep a search walks, matching the size walk's own ceiling.
const SEARCH_MAX_DEPTH: usize = 32;

/// How long a command runs before the status bar offers to stop it. Most
/// finish inside this, and flashing a hint for each would only flicker.
const PROGRESS_DELAY: Duration = Duration::from_millis(400);

/// Longest command line the status bar shows before it is cut short.
const PROGRESS_LABEL_MAX_CHARS: usize = 60;

/// Directories a search never enters: version-control internals and build
/// output, which are derived rather than written and would swamp every result.
const SEARCH_SKIP_DIRS: [&str; 5] = [".git", ".hg", ".svn", "node_modules", "target"];

/// The program a search hands its walk to where it is installed: it reads the
/// tree's `.gitignore` files, and it is faster than any walk written here.
const RIPGREP: &str = "rg";

/// The byte ripgrep is asked to end each path with, since a path may hold the
/// `:` that would otherwise separate it from the line number.
const PATH_END: u8 = 0;

/// What a request turned away at capacity answers with.
pub(crate) const TOO_BUSY: &str = "too many tasks running; try again once some finish";

/// What a request answers with when no thread could be started to run it.
const NO_WORKER: &str = "could not start a worker thread";

/// How long the output of a command that has exited may take to reach its
/// end. A program that has ended has written everything it will, so its own
/// output arrives at once; what holds a pipe open past this is something it
/// left running (an ssh connection kept for reuse, a helper daemon), which
/// may never close it.
const STREAM_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// How much of a command's output is read at a time.
const STREAM_CHUNK: usize = 8 * 1024;

// ========================================================================
// Data Structures
// ========================================================================

/// The jobs currently running, and the channel their answers arrive on.
pub(crate) struct Jobs {
    next_id: u64,
    /// Answers given without running anything: requests turned away at
    /// capacity, or whose worker could not start. Each still answers the editor
    /// that asked, which would otherwise wait for ever on a reply.
    refused: Vec<(TabId, JobReply)>,
    /// The jobs whose answers are still wanted. A cancelled job leaves this
    /// map, so its answer is discarded when it eventually arrives.
    running: HashMap<u64, RunningJob>,
    rx: mpsc::Receiver<Finished>,
    tx: mpsc::Sender<Finished>,
    /// Wakes the event loop as each answer is sent, so it is delivered at
    /// once rather than on the next poll.
    waker: Waker,
}

/// A job in flight: who asked, and the flag that asks it to stop.
struct RunningJob {
    cancel: Arc<AtomicBool>,
    /// The command line, for a job that runs a program; `None` for the rest,
    /// which are not worth announcing.
    label: Option<String>,
    tab: TabId,
    started: Instant,
}

/// A finished job: which editor asked, and what came back.
struct Finished {
    id: u64,
    tab: TabId,
    reply: JobReply,
}

/// One of a command's output streams, read on a thread of its own into a
/// buffer the runner can take from at any time: what has arrived so far is
/// there to answer with even while something still holds the stream open.
struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    finished: Arc<AtomicBool>,
}

// ========================================================================
// Jobs
// ========================================================================

impl Jobs {
    /// An idle runner.
    pub(crate) fn new(waker: Waker) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            next_id: 0,
            refused: Vec::new(),
            running: HashMap::new(),
            rx,
            tx,
            waker,
        }
    }

    /// The tabs with a job running for them, whatever the job is.
    pub(crate) fn busy_tabs(&self) -> HashSet<TabId> {
        self.running.values().map(|job| job.tab).collect()
    }

    /// Whether `tab` has a job running for it.
    pub(crate) fn is_busy_for(&self, tab: TabId) -> bool {
        self.running.values().any(|job| job.tab == tab)
    }

    /// Ask every job started for `tab` to stop, keeping their answers: a
    /// stopped command still answers (as stopped), so the editor that asked
    /// hears how it ended. Returns the command lines stopped.
    pub(crate) fn stop_for(&mut self, tab: TabId) -> Vec<String> {
        self.running
            .values()
            .filter(|job| job.tab == tab)
            .filter_map(|job| {
                job.cancel.store(true, Ordering::Relaxed);
                job.label.clone()
            })
            .collect()
    }

    /// The command lines of every program running, for an editor or not.
    pub(crate) fn labels(&self) -> Vec<String> {
        self.running
            .values()
            .filter_map(|job| job.label.clone())
            .collect()
    }

    /// Whether `tab` has a program running for it long enough to be worth
    /// mentioning.
    pub(crate) fn has_long_job_for(&self, tab: TabId) -> bool {
        self.running.values().any(|job| {
            job.tab == tab && job.label.is_some() && job.started.elapsed() >= PROGRESS_DELAY
        })
    }

    /// Start `request` on behalf of `pane`. Turned away when the runner is
    /// already at capacity, with an answer saying so.
    pub(crate) fn spawn(&mut self, tab: TabId, request: JobRequest) {
        if self.running.len() >= MAX_IN_FLIGHT {
            self.refused.push((tab, refusal(&request, TOO_BUSY)));
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.insert(
            id,
            RunningJob {
                cancel: Arc::clone(&cancel),
                label: progress_label(&request),
                tab,
                started: Instant::now(),
            },
        );

        let tx = self.tx.clone();
        let waker = self.waker.clone();
        // Kept to answer with if the worker cannot start, since the request
        // itself goes with the closure that fails to run.
        let fallback = refusal(&request, NO_WORKER);
        // A detached worker: nothing joins it, and a cancelled job is left to
        // notice the flag and exit on its own rather than being killed.
        let spawned = thread::Builder::new()
            .name("winter job".into())
            .spawn(move || {
                let reply = run(request, &cancel);
                let _ = tx.send(Finished { id, tab, reply });
                waker.wake();
            });
        if spawned.is_err() {
            // A job with no worker would read as running for ever.
            self.running.remove(&id);
            self.refused.push((tab, fallback));
        }
    }

    /// Ask every job started for `pane` to stop, and discard their answers.
    pub(crate) fn cancel_for(&mut self, tab: TabId) {
        self.running.retain(|_, job| {
            if job.tab != tab {
                return true;
            }
            job.cancel.store(true, Ordering::Relaxed);
            false
        });
    }

    /// Every answer that has arrived since the last call.
    pub(crate) fn drain(&mut self) -> Vec<(TabId, JobReply)> {
        let mut done = std::mem::take(&mut self.refused);
        while let Ok(finished) = self.rx.try_recv() {
            // A job cancelled while it ran is no longer in the map, and its
            // answer is to a question nobody is asking any more.
            if self.running.remove(&finished.id).is_none() {
                continue;
            }
            done.push((finished.tab, finished.reply));
        }
        done
    }
}

// ========================================================================
// Capture
// ========================================================================

impl Capture {
    /// Start reading `pipe` to its end. A missing pipe reads as finished and
    /// empty.
    fn spawn(pipe: Option<Box<dyn Read + Send>>) -> Self {
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
// App: editor jobs
// ========================================================================

impl App {
    /// Stop every task running for `tab`, saying what is being stopped. Each
    /// still answers its editor, as stopped, once it has ended.
    pub(crate) fn stop_tasks(&mut self, tab: TabId) {
        let stopped = self.jobs.stop_for(tab);
        match stopped.is_empty() {
            true => self.set_notice("nothing running to stop"),
            false => self.set_notice(format!("stopping {}", stopped.join(", "))),
        }
    }

    /// What the status bar says while the focused tab has a task running: how
    /// to stop it. The strip's spinner already shows that it runs.
    pub(crate) fn job_progress_line(&self, focused: TabId) -> Option<String> {
        self.jobs
            .has_long_job_for(focused)
            .then(|| format!("{} to stop", format_key(&STOP_TASKS_KEY)))
    }

    /// Hand each finished job's answer to the editor that asked for it.
    pub(crate) fn deliver_finished_jobs(&mut self) -> bool {
        let finished = self.jobs.drain();
        let mut any = false;
        for (tab_id, reply) in finished {
            let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
                continue;
            };
            let outcome = slot.editor.on_job(reply);
            self.act_on_editor_outcome(tab_id, outcome);
            any = true;
        }
        any
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The answer a request gets when it is not run at all, saying `why` where
/// the reply has room for it. Every kind gets one, so no editor is left waiting
/// on a reply that will never come.
fn refusal(request: &JobRequest, why: &str) -> JobReply {
    match request {
        JobRequest::Command(command) => JobReply::Command(CommandOutput {
            code: None,
            stderr: why.to_string(),
            stdout: String::new(),
            tag: command.tag,
        }),
        JobRequest::DirSize(path) => JobReply::DirSize {
            bytes: 0,
            path: path.clone(),
        },
        JobRequest::FileOps(_) => JobReply::FileOps(Err(why.to_string())),
        JobRequest::Processes => JobReply::Processes(Err(why.to_string())),
        JobRequest::ReadFiles(_) => JobReply::Files(Vec::new()),
        JobRequest::Search(search) => JobReply::Search(SearchResult {
            query: search.query.clone(),
            ..SearchResult::default()
        }),
        JobRequest::System => JobReply::System(Err(why.to_string())),
    }
}

/// The command line a program-running request shows while it runs, cut to fit
/// the status bar. `None` for requests that run no program.
fn progress_label(request: &JobRequest) -> Option<String> {
    if let JobRequest::FileOps(work) = request {
        return Some(file_ops_label(work));
    }
    let JobRequest::Command(command) = request else {
        return None;
    };
    let line = std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= PROGRESS_LABEL_MAX_CHARS {
        return Some(line);
    }
    let kept: String = line.chars().take(PROGRESS_LABEL_MAX_CHARS).collect();
    Some(format!("{kept}..."))
}

/// What the status bar says while `work` runs: `copying 3 items`.
fn file_ops_label(work: &FileOps) -> String {
    let (verb, count) = match work {
        FileOps::Copy(pairs) => ("copying", pairs.len()),
        FileOps::Move(pairs) => ("moving", pairs.len()),
        FileOps::Delete(paths) => ("deleting", paths.len()),
        FileOps::Trash(paths) => ("trashing", paths.len()),
    };
    let items = if count == 1 { "item" } else { "items" };
    format!("{verb} {count} {items}")
}

/// Do the work a request asks for, checking `cancel` often enough that a
/// cancelled job stops within a directory or two rather than at the end.
fn run(request: JobRequest, cancel: &AtomicBool) -> JobReply {
    match request {
        JobRequest::Command(request) => JobReply::Command(run_command(request, cancel)),
        JobRequest::DirSize(path) => {
            let bytes = walk_size(&path, cancel, 0);
            JobReply::DirSize { bytes, path }
        }
        JobRequest::Processes => JobReply::Processes(collect::collect(cancel)),
        JobRequest::ReadFiles(paths) => JobReply::Files(read_files(&paths)),
        JobRequest::Search(request) => JobReply::Search(search_tree(&request, cancel)),
        JobRequest::System => JobReply::System(sys::collect::collect()),
        JobRequest::FileOps(work) => JobReply::FileOps(ops::run_file_ops(work, cancel)),
    }
}

/// Read what each of `paths` holds, dropping the ones that are not there or
/// do not read as text. A caller asking for a program's state files expects
/// most of them to be missing most of the time, so a missing file is left out
/// rather than reported.
fn read_files(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    paths
        .iter()
        .filter_map(|path| {
            std::fs::read_to_string(path)
                .ok()
                .map(|text| (path.clone(), text))
        })
        .collect()
}

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
pub(crate) const STOPPED: &str = "stopped";

/// Run a program to completion and collect what it wrote. There is no time
/// limit: a push over a slow link or a long hook takes what it takes. A
/// command asked to stop through `cancel` is stopped, with everything it
/// started, and answers [`STOPPED`].
fn run_command(request: CommandRequest, cancel: &AtomicBool) -> CommandOutput {
    let tag = request.tag;
    let (code, stdout, stderr) = match run_until_stopped(&request, cancel) {
        Ok(Some(output)) => (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ),
        Ok(None) => (None, String::new(), STOPPED.to_string()),
        Err(e) => (None, String::new(), e.to_string()),
    };
    CommandOutput {
        code,
        stderr,
        stdout,
        tag,
    }
}

/// Run `request`, feeding it its standard input if it has any, until it ends
/// or `cancel` is set: `None` once it has been stopped.
fn run_until_stopped(
    request: &CommandRequest,
    cancel: &AtomicBool,
) -> std::io::Result<Option<std::process::Output>> {
    let mut command = Command::new(&request.program);
    // A group of its own, so stopping it reaches what it started too: the
    // ssh a push runs, a hook, a credential helper.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .args(&request.args)
        .current_dir(&request.cwd)
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
            let _ = handle.write_all(input.as_bytes());
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
fn await_captures(captures: &[&Capture], grace: Duration, cancel: &AtomicBool) -> bool {
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

/// Send `signal` to every process in the group `group` leads.
#[cfg(unix)]
#[allow(unsafe_code)]
fn signal_group(group: libc::pid_t, signal: libc::c_int) {
    // SAFETY: `kill` takes no pointers and touches no memory of ours; a
    // negative pid names the group, which `process_group(0)` made the child
    // the leader of.
    unsafe { libc::kill(-group, signal) };
}

/// Stop `child`, which on this platform leads no group of its own.
#[cfg(not(unix))]
fn stop_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Total bytes under `dir`, skipping what cannot be read and links rather than
/// following them, the way `du` does. Returns what it summed so far when
/// cancelled, which the caller discards.
fn walk_size(dir: &Path, cancel: &AtomicBool, depth: usize) -> u64 {
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

/// Every line under the requested root holding its query, ignoring case. An
/// empty query matches nothing rather than everything.
fn search_tree(request: &SearchRequest, cancel: &AtomicBool) -> SearchResult {
    let needle = request.query.to_lowercase();
    let mut found = SearchResult {
        query: request.query.clone(),
        ..SearchResult::default()
    };
    if needle.is_empty() {
        return found;
    }
    if let Some(ripgrep) = search_with_ripgrep(request, cancel) {
        return ripgrep;
    }
    search_dir(&request.root, &needle, cancel, 0, &mut found);
    found
}

/// The same search run by ripgrep, under the same rules as the walk below:
/// any case, the query taken literally, hidden files read, the same
/// directories skipped, the same caps, files in sorted order. `None` where
/// ripgrep is not installed, so the walk runs instead.
fn search_with_ripgrep(request: &SearchRequest, cancel: &AtomicBool) -> Option<SearchResult> {
    let mut command = Command::new(RIPGREP);
    command
        .args(["--line-number", "--with-filename", "--no-heading", "--null"])
        .args([
            "--color",
            "never",
            "--ignore-case",
            "--fixed-strings",
            "--hidden",
        ])
        .args(["--sort", "path", "--no-require-git"])
        .arg(format!("--max-filesize={SEARCH_MAX_FILE_BYTES}"))
        .arg(format!("--max-depth={SEARCH_MAX_DEPTH}"));
    for skipped in SEARCH_SKIP_DIRS {
        command.arg(format!("--glob=!{skipped}"));
    }
    let mut child = command
        .arg("--regexp")
        .arg(&request.query)
        .arg(&request.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut found = SearchResult {
        query: request.query.clone(),
        ..SearchResult::default()
    };
    let stdout = child.stdout.take()?;
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if found.hits.len() >= SEARCH_MAX_HITS {
            found.truncated = true;
            break;
        }
        if let Some(hit) = ripgrep_hit(&line) {
            found.hits.push(hit);
        }
    }
    // Whatever is left unread is not wanted: a capped or cancelled search
    // stops ripgrep rather than letting it walk the rest of the tree.
    let _ = child.kill();
    let _ = child.wait();
    Some(found)
}

/// One line of ripgrep's output, `path NUL line:text`, as a hit.
fn ripgrep_hit(line: &[u8]) -> Option<SearchHit> {
    let split = line.iter().position(|byte| *byte == PATH_END)?;
    let path = PathBuf::from(String::from_utf8_lossy(&line[..split]).to_string());
    let rest = String::from_utf8_lossy(&line[split + 1..]);
    let (number, text) = rest.split_once(':')?;
    Some(SearchHit {
        line: number.parse().ok()?,
        path,
        text: text.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
    })
}

/// Walk `dir`, adding what its files hold. Entries are taken in sorted order,
/// so running the same search twice reads the same way.
fn search_dir(
    dir: &Path,
    needle: &str,
    cancel: &AtomicBool,
    depth: usize,
    found: &mut SearchResult,
) {
    if depth >= SEARCH_MAX_DEPTH || found.truncated || cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = reader.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if found.truncated || cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // A link is never followed, so a cycle cannot make the walk endless.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if !is_skipped_dir(&path) {
                search_dir(&path, needle, cancel, depth + 1, found);
            }
        } else if meta.len() <= SEARCH_MAX_FILE_BYTES {
            search_file(&path, needle, found);
        }
    }
}

/// Whether a directory is one searches leave alone.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SEARCH_SKIP_DIRS.contains(&name))
}

/// Add each line of `path` holding `needle`. A file that will not read as text
/// is skipped, since searching one prints a screen of control bytes.
fn search_file(path: &Path, needle: &str, found: &mut SearchResult) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (index, line) in text.lines().enumerate() {
        if !line.to_lowercase().contains(needle) {
            continue;
        }
        if found.hits.len() >= SEARCH_MAX_HITS {
            found.truncated = true;
            return;
        }
        found.hits.push(SearchHit {
            line: index + 1,
            path: path.to_path_buf(),
            text: line.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
        });
    }
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
    ) -> Vec<(TabId, JobReply)> {
        let deadline = Instant::now() + timeout;
        let mut collected = Vec::new();
        while collected.len() < count && Instant::now() < deadline {
            collected.extend(jobs.drain());
            std::thread::sleep(POLL_STEP);
        }
        collected
    }

    fn command(program: &str, args: &[&str]) -> JobRequest {
        JobRequest::Command(CommandRequest {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: PathBuf::from("."),
            env: Vec::new(),
            program: program.to_string(),
            stdin: None,
            tag: "test",
        })
    }

    #[test]
    fn test_progress_label_joins_the_program_and_its_arguments() {
        let label = progress_label(&command("git", &["push", "origin", "main"]));
        assert_eq!(label.as_deref(), Some("git push origin main"));
    }

    #[test]
    fn test_progress_label_cuts_a_long_command_line() {
        let long = "x".repeat(PROGRESS_LABEL_MAX_CHARS * 2);
        let label = progress_label(&command("git", &[&long])).expect("a command has a label");
        assert_eq!(label.chars().count(), PROGRESS_LABEL_MAX_CHARS + 3);
        assert!(label.ends_with("..."));
    }

    #[test]
    fn test_progress_label_ignores_jobs_that_run_no_program() {
        assert_eq!(
            progress_label(&JobRequest::DirSize(PathBuf::from("."))),
            None
        );
    }

    #[test]
    fn test_a_long_job_is_mentioned_only_for_its_own_tab() {
        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(TabId(1), command("sleep", &["5"]));
        assert!(
            !jobs.has_long_job_for(TabId(1)),
            "a command that just started is not announced"
        );

        for job in jobs.running.values_mut() {
            job.started = Instant::now() - PROGRESS_DELAY;
        }
        assert!(jobs.has_long_job_for(TabId(1)));
        assert!(!jobs.has_long_job_for(TabId(2)), "another tab's job");
        jobs.cancel_for(TabId(1));
        assert!(
            !jobs.has_long_job_for(TabId(1)),
            "a cancelled job is no longer reported"
        );
    }

    /// A program's command, as the runner takes it.
    fn command_request(program: &str, args: &[&str]) -> CommandRequest {
        match command(program, args) {
            JobRequest::Command(request) => request,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    /// Whether a process `pid` is still alive.
    #[cfg(unix)]
    fn is_alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn test_a_command_runs_until_it_ends_and_answers_with_what_it_wrote() {
        let echo = CommandRequest {
            stdin: Some("in".to_string()),
            ..command_request("sh", &["-c", "cat; echo err >&2"])
        };
        let out = run_until_stopped(&echo, &AtomicBool::new(false))
            .expect("runs")
            .expect("ends on its own");
        assert_eq!(out.stdout, b"in");
        assert_eq!(out.stderr, b"err\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_stopping_a_command_ends_it_and_everything_it_started() {
        // A command that starts a long sleep of its own, as a push starts
        // ssh, and writes down the sleep's pid.
        let dir = std::env::temp_dir().join(format!("winter-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pid_file = dir.join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let request = command_request("sh", &["-c", &script]);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopper = {
            let cancel = Arc::clone(&cancel);
            let pid_file = pid_file.clone();
            thread::spawn(move || {
                while !pid_file.exists() {
                    thread::sleep(POLL_STEP);
                }
                cancel.store(true, Ordering::Relaxed);
            })
        };
        let started = Instant::now();
        let out = run_until_stopped(&request, &cancel).expect("runs");
        stopper.join().expect("stopper");
        assert!(out.is_none(), "stopped, not finished");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stopped promptly"
        );
        let pid = std::fs::read_to_string(&pid_file).expect("pid written");
        assert!(!is_alive(pid.trim()), "the sleep it started went with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_stopped_command_answers_its_editor_as_stopped() {
        let mut jobs = Jobs::new(Waker::default());
        let tab = TabId(1);
        jobs.spawn(tab, command("sleep", &["30"]));
        assert!(jobs.is_busy_for(tab));
        assert_eq!(jobs.stop_for(tab), vec!["sleep 30".to_string()]);
        let answers = wait_for_replies(&mut jobs, 1, ARRIVES);
        match answers.as_slice() {
            [(answered, JobReply::Command(output))] => {
                assert_eq!(*answered, tab);
                assert_eq!(output.stderr, STOPPED);
            }
            other => panic!("expected one stopped answer, got {other:?}"),
        }
        assert!(!jobs.is_busy_for(tab));
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

        let mut jobs = Jobs::new(Waker::default());
        let tab = TabId(7);
        jobs.spawn(tab, JobRequest::DirSize(tree.0.clone()));
        assert!(!jobs.running.is_empty());

        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        assert_eq!(replies.len(), 1, "the walk reported back");
        let (got_pane, reply) = &replies[0];
        assert_eq!(*got_pane, tab, "answers go to whoever asked");
        let JobReply::DirSize { bytes, .. } = reply else {
            panic!("expected a directory total");
        };
        assert_eq!(*bytes, 150, "nested files count toward the total");
        assert!(jobs.running.is_empty(), "and the job is no longer running");
    }

    #[test]
    fn test_a_cancelled_job_is_not_reported_back() {
        // The worker may already be finishing when cancel arrives, so the
        // answer has to be discarded on arrival, not merely asked to stop.
        let tree = TempTree::new("cancel");
        let mut jobs = Jobs::new(Waker::default());
        let tab = TabId(1);
        jobs.spawn(tab, JobRequest::DirSize(tree.0.clone()));
        jobs.cancel_for(tab);

        assert!(
            jobs.running.is_empty(),
            "a cancelled job is no longer waited on"
        );
        let replies = wait_for_replies(&mut jobs, 1, NOTHING_COMING);
        assert!(replies.is_empty(), "its answer is dropped");
    }

    #[test]
    fn test_cancelling_one_pane_leaves_another_pane_running() {
        let tree = TempTree::new("cancel-one");
        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(TabId(1), JobRequest::DirSize(tree.0.clone()));
        jobs.spawn(TabId(2), JobRequest::DirSize(tree.0.clone()));

        jobs.cancel_for(TabId(1));
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].0, TabId(2));
    }

    #[test]
    fn test_requests_past_the_limit_are_dropped_rather_than_queued() {
        // A queued request would be answering a question about a screen the
        // user has long since left.
        let tree = TempTree::new("limit");
        let mut jobs = Jobs::new(Waker::default());
        for _ in 0..MAX_IN_FLIGHT + 5 {
            jobs.spawn(TabId(1), JobRequest::DirSize(tree.0.clone()));
        }
        assert!(jobs.running.len() <= MAX_IN_FLIGHT);
    }

    /// A runner whose every slot is taken by a job that never answers.
    fn full_runner() -> Jobs {
        let mut jobs = Jobs::new(Waker::default());
        for id in 0..MAX_IN_FLIGHT as u64 {
            jobs.running.insert(
                id,
                RunningJob {
                    cancel: Arc::new(AtomicBool::new(false)),
                    label: None,
                    started: Instant::now(),
                    tab: TabId(9),
                },
            );
        }
        jobs.next_id = MAX_IN_FLIGHT as u64;
        jobs
    }

    #[test]
    fn test_a_request_turned_away_at_capacity_still_answers_its_editor() {
        // Regression: a dropped git read left the editor waiting on a reply
        // that never came, its view stuck as "watching".
        let mut jobs = full_runner();
        jobs.spawn(TabId(1), command("git", &["status"]));
        jobs.spawn(TabId(2), JobRequest::ReadFiles(vec![PathBuf::from("x")]));
        match jobs.drain().as_slice() {
            [(TabId(1), JobReply::Command(output)), (TabId(2), JobReply::Files(files))] => {
                assert_eq!(output.stderr, TOO_BUSY);
                assert_eq!(output.tag, "test", "answered under the tag it asked with");
                assert!(!output.succeeded());
                assert!(files.is_empty());
            }
            other => panic!("expected both requests answered, got {other:?}"),
        }
        assert_eq!(jobs.running.len(), MAX_IN_FLIGHT, "neither was started");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_command_answers_once_it_ends_though_what_it_left_holds_its_output() {
        // Regression: an ssh kept running for reuse holds the pipes open, and
        // waiting for their end kept the job running for ever.
        let request = command_request("sh", &["-c", "sleep 4 & echo done"]);
        let started = Instant::now();
        let out = run_until_stopped(&request, &AtomicBool::new(false))
            .expect("runs")
            .expect("ends on its own");
        assert_eq!(out.stdout, b"done\n");
        assert!(out.status.success());
        assert!(
            started.elapsed() < STREAM_DRAIN_GRACE + Duration::from_secs(2),
            "answered within the grace, not when the sleep ends"
        );
    }

    #[test]
    fn test_waiting_on_a_held_stream_still_stops_on_the_stop_key() {
        let (reader, _held_open) = std::io::pipe().expect("pipe");
        let capture = Capture::spawn(Some(Box::new(reader)));
        let cancel = Arc::new(AtomicBool::new(false));
        let stopper = {
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                thread::sleep(NOTHING_COMING);
                cancel.store(true, Ordering::Relaxed);
            })
        };
        let started = Instant::now();
        let wanted = await_captures(&[&capture], ARRIVES * 4, &cancel);
        stopper.join().expect("stopper");
        assert!(!wanted, "a stopped job's answer is not wanted");
        assert!(
            started.elapsed() < ARRIVES,
            "stopped without waiting out the grace"
        );
    }
    #[test]
    fn test_a_search_reports_the_lines_that_hold_the_query() {
        let tree = TempTree::new("search");
        std::fs::write(tree.0.join("a.txt"), "first line\nholds NEEDLE here\n").expect("write");
        std::fs::write(tree.0.join("b.txt"), "nothing\n").expect("write");

        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(
            TabId(1),
            JobRequest::Search(SearchRequest {
                query: "needle".to_string(),
                root: tree.0.clone(),
            }),
        );
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        let JobReply::Search(found) = &replies[0].1 else {
            panic!("expected a search answer, got {:?}", replies[0].1);
        };
        assert_eq!(found.hits.len(), 1, "one line matched: {:?}", found.hits);
        assert_eq!(found.hits[0].line, 2, "line numbers count from one");
        assert_eq!(found.hits[0].text, "holds NEEDLE here");
        assert!(!found.truncated);
    }

    #[test]
    fn test_a_search_stays_out_of_build_output_and_repository_internals() {
        // A hit inside `.git` or `target` answers a question nobody asked, and
        // walking them is what makes a search feel broken on a real checkout.
        let tree = TempTree::new("search-skip");
        for skipped in [".git", "target"] {
            let dir = tree.0.join(skipped);
            std::fs::create_dir_all(&dir).expect("temp subdir");
            std::fs::write(dir.join("c.txt"), "needle\n").expect("write");
        }
        std::fs::write(tree.0.join("d.txt"), "needle\n").expect("write");

        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(
            TabId(1),
            JobRequest::Search(SearchRequest {
                query: "needle".to_string(),
                root: tree.0.clone(),
            }),
        );
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        let JobReply::Search(found) = &replies[0].1 else {
            panic!("expected a search answer");
        };
        let paths: Vec<String> = found
            .hits
            .iter()
            .map(|hit| hit.path.to_string_lossy().to_string())
            .collect();
        assert_eq!(paths.len(), 1, "only the tracked file matched: {paths:?}");
        assert!(paths[0].ends_with("d.txt"), "{paths:?}");
    }

    #[test]
    fn test_a_search_leaves_out_what_gitignore_names_when_ripgrep_runs_it() {
        let tree = TempTree::new("search-ignore");
        std::fs::write(tree.0.join(".gitignore"), "generated.txt\n").expect("write");
        std::fs::write(tree.0.join("generated.txt"), "needle\n").expect("write");
        std::fs::write(tree.0.join("kept.txt"), "needle\n").expect("write");
        let request = SearchRequest {
            query: "needle".to_string(),
            root: tree.0.clone(),
        };
        let Some(found) = search_with_ripgrep(&request, &AtomicBool::new(false)) else {
            // No ripgrep here: the walk that runs instead knows no ignore files.
            return;
        };
        let names: Vec<String> = found
            .hits
            .iter()
            .filter_map(|hit| {
                hit.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .collect();
        assert_eq!(names, ["kept.txt"]);
    }

    #[test]
    fn test_a_search_stops_at_its_cap_and_says_the_list_is_partial() {
        let tree = TempTree::new("search-cap");
        let lines = "needle\n".repeat(SEARCH_MAX_HITS + 20);
        std::fs::write(tree.0.join("many.txt"), lines).expect("write");

        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(
            TabId(1),
            JobRequest::Search(SearchRequest {
                query: "needle".to_string(),
                root: tree.0.clone(),
            }),
        );
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        let JobReply::Search(found) = &replies[0].1 else {
            panic!("expected a search answer");
        };
        assert_eq!(found.hits.len(), SEARCH_MAX_HITS);
        assert!(found.truncated, "a capped list has to say it was cut short");
    }

    #[test]
    fn test_an_empty_query_finds_nothing_rather_than_everything() {
        let tree = TempTree::new("search-empty");
        std::fs::write(tree.0.join("a.txt"), "anything\n").expect("write");

        let mut jobs = Jobs::new(Waker::default());
        jobs.spawn(
            TabId(1),
            JobRequest::Search(SearchRequest {
                query: String::new(),
                root: tree.0.clone(),
            }),
        );
        let replies = wait_for_replies(&mut jobs, 1, ARRIVES);
        let JobReply::Search(found) = &replies[0].1 else {
            panic!("expected a search answer");
        };
        assert!(found.hits.is_empty());
    }
}
