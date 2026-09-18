//! Slow work off the event-loop thread: a page asks for it, a worker thread
//! does it, and the answer is collected on the next poll.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::model::layout::PaneId;
use crate::model::page::{
    CommandOutput, CommandRequest, JobReply, JobRequest, SearchHit, SearchRequest, SearchResult,
};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How many jobs may run at once. Past this, a request is dropped rather than
/// queued: every caller asks for work about what is on screen, so a request
/// that waited long enough to run would be answering a stale question.
const MAX_IN_FLIGHT: usize = 16;

/// Matches a search reports before it stops. A page shows a screenful at a
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

/// Directories a search never enters: version-control internals and build
/// output, which are derived rather than written and would swamp every result.
const SEARCH_SKIP_DIRS: [&str; 5] = [".git", ".hg", ".svn", "node_modules", "target"];

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
        JobRequest::ReadFiles(paths) => JobReply::Files(read_files(&paths)),
        JobRequest::Search(request) => JobReply::Search(search_tree(&request, cancel)),
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
    search_dir(&request.root, &needle, cancel, 0, &mut found);
    found
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
    #[test]
    fn test_a_search_reports_the_lines_that_hold_the_query() {
        let tree = TempTree::new("search");
        std::fs::write(tree.0.join("a.txt"), "first line\nholds NEEDLE here\n").expect("write");
        std::fs::write(tree.0.join("b.txt"), "nothing\n").expect("write");

        let mut jobs = Jobs::new();
        jobs.spawn(
            PaneId(1),
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

        let mut jobs = Jobs::new();
        jobs.spawn(
            PaneId(1),
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
    fn test_a_search_stops_at_its_cap_and_says_the_list_is_partial() {
        let tree = TempTree::new("search-cap");
        let lines = "needle\n".repeat(SEARCH_MAX_HITS + 20);
        std::fs::write(tree.0.join("many.txt"), lines).expect("write");

        let mut jobs = Jobs::new();
        jobs.spawn(
            PaneId(1),
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

        let mut jobs = Jobs::new();
        jobs.spawn(
            PaneId(1),
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
