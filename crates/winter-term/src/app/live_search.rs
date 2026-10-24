//! The project searched as you type (`C-M-g`), in the palette: each pause in
//! the typing runs ripgrep over the focused tab's project for the pattern,
//! and the lines it finds stream into the list as it finds them. `Enter`
//! opens the file on the line. The approach follows magic-vscode's search.
//!
//! A pattern is a literal unless it has a `*` in it, which reads as "anything
//! here" and makes it a regex; case is ignored unless the pattern has a
//! capital in it. ` @ <filter>` after the pattern keeps the lines of the files
//! whose paths match it, and changing only that refilters what was found
//! rather than searching again.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::model::editor::OpenTarget;
use crate::model::paint::IconKind;
use crate::model::palette::{split_live_query, Palette, PaletteEntry, PaletteMode};
use crate::model::path::abbreviate_home;
use crate::process;

use super::project_files::project_root;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long the typing has to pause before the pattern is searched for.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The shortest pattern searched for: one character matches nearly every
/// line of a project.
const MIN_PATTERN_CHARS: usize = 2;

/// The most lines kept: past this ripgrep is stopped, since a list that long
/// is narrowed by typing more, not by reading it.
const MAX_RESULTS: usize = 10_000;

/// How many lines ripgrep finds before they are handed to the list, and how
/// long it may go before handing over what it has anyway.
const BATCH_LINES: usize = 200;
const BATCH_EVERY: Duration = Duration::from_millis(50);

/// The longest line kept, in characters: a minified file's one line would
/// otherwise be the whole row.
const MAX_LINE_CHARS: usize = 300;

/// The program searched with.
const RIPGREP: &str = "rg";

/// Said when ripgrep is not there to search with.
const NO_RIPGREP: &str = "searching needs ripgrep (rg) installed";

/// What the title shows while a search is still running.
const RUNNING_MARK: &str = " \u{2026}";

/// What a `*` in a pattern stands for in the regex it becomes.
const WILDCARD_REGEX: &str = ".*";

// ========================================================================
// Data Structures
// ========================================================================

/// The search behind the open palette: where it looks, what it last looked
/// for, and the line to the thread doing the looking.
pub(crate) struct LiveSearch {
    batches: Receiver<Batch>,
    /// The run whose lines are wanted; a run that sees this move on stops.
    current: Arc<AtomicU64>,
    /// When the pattern typed is next searched for, once the typing pauses.
    due: Option<Instant>,
    /// The pattern the rows are for.
    pattern: String,
    root: PathBuf,
    /// Whether the current run is still finding lines.
    running: bool,
    sender: Sender<Batch>,
    /// How many lines the current run has found.
    total: usize,
}

/// Lines one run found since the last batch it sent.
struct Batch {
    done: bool,
    hits: Vec<Hit>,
    run: u64,
}

/// One line found.
#[derive(Debug, Eq, PartialEq)]
struct Hit {
    line: usize,
    /// From the project's root, `/`-separated.
    path: String,
    text: String,
}

// ========================================================================
// LiveSearch Implementations
// ========================================================================

impl Drop for LiveSearch {
    fn drop(&mut self) {
        // A run still going sees it is no longer wanted and stops ripgrep.
        self.current.fetch_add(1, Ordering::Relaxed);
    }
}

// ========================================================================
// App: live search
// ========================================================================

impl App {
    /// Open the project search over the focused tab's project, or put it
    /// away when it is the one showing.
    pub(crate) fn open_live_search(&mut self) {
        let showing = self
            .palette()
            .is_some_and(|palette| palette.mode == PaletteMode::LiveSearch);
        if showing {
            self.dismiss_overlay();
            return;
        }
        let root = project_root(&self.focused_start_dir());
        let (sender, batches) = mpsc::channel();
        let search = LiveSearch {
            batches: self.waker.relay(batches),
            current: Arc::new(AtomicU64::new(0)),
            due: None,
            pattern: String::new(),
            root,
            running: false,
            sender,
            total: 0,
        };
        let palette = Palette::open_live_search(title(&search, 0))
            .with_query_history(self.palette_history.clone());
        self.live_search = Some(search);
        self.open_palette(palette);
        self.dirty = true;
    }

    /// Take in an edit of the query: a new pattern is searched for once the
    /// typing pauses, and a new filter only refilters what was found.
    pub(crate) fn on_live_query(&mut self, palette: &mut Palette) {
        let Some(search) = self.live_search.as_mut() else {
            return;
        };
        let pattern = split_live_query(&palette.query).0.to_string();
        if pattern != search.pattern {
            search.current.fetch_add(1, Ordering::Relaxed);
            search.running = false;
            search.total = 0;
            palette.clear_entries();
            search.due = (pattern.trim().chars().count() >= MIN_PATTERN_CHARS)
                .then(|| Instant::now() + DEBOUNCE);
            search.pattern = pattern;
        }
        palette.set_title(title(search, palette.filtered.len()));
    }

    /// Start the search the typing paused on, and take in the lines found
    /// since the last call. Returns when to be called again for a search
    /// still waiting on its pause.
    pub(crate) fn tick_live_search(&mut self, now: Instant) -> Option<Instant> {
        let open = self
            .palette()
            .is_some_and(|palette| palette.mode == PaletteMode::LiveSearch);
        if !open {
            self.live_search = None;
            return None;
        }
        let search = self.live_search.as_mut()?;
        if search.due.is_some_and(|due| due <= now) {
            search.due = None;
            if let Err(reason) = start_run(search) {
                self.set_error(reason);
                return None;
            }
        }
        let search = self.live_search.as_mut()?;
        let mut rows = Vec::new();
        let mut changed = false;
        while let Ok(batch) = search.batches.try_recv() {
            if batch.run != search.current.load(Ordering::Relaxed) {
                continue;
            }
            changed = true;
            search.total += batch.hits.len();
            search.running &= !batch.done;
            rows.extend(batch.hits.into_iter().map(|hit| row(&search.root, hit)));
        }
        let due = search.due;
        if changed {
            let search = self.live_search.take()?;
            if let Some(palette) = self.palette_mut() {
                palette.append_entries(rows);
                palette.set_title(title(&search, palette.filtered.len()));
            }
            self.live_search = Some(search);
            self.dirty = true;
        }
        due
    }

    /// Open the file a row of the search found, on its line.
    pub(crate) fn open_live_search_row(&mut self, action: &str) {
        if let Some((line, path)) = action.split_once(':') {
            let line = line.parse().unwrap_or(1);
            self.open_path_editor(OpenTarget::at_line(PathBuf::from(path), line));
        }
        self.live_search = None;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The palette's title: where is searched, how many of the lines found are
/// shown, and whether more may come.
fn title(search: &LiveSearch, shown: usize) -> String {
    let mut title = format!("Search: {}", abbreviate_home(&search.root));
    if !search.pattern.is_empty() {
        title.push_str(&format!(" [{shown}/{}]", search.total));
    }
    if search.running || search.due.is_some() {
        title.push_str(RUNNING_MARK);
    }
    title
}

/// A row for `hit`: the line itself, with where it is on the right. Its
/// action is `<line>:<full path>`.
fn row(root: &Path, hit: Hit) -> PaletteEntry {
    let name = hit.path.rsplit('/').next().unwrap_or(&hit.path).to_string();
    let path = root.join(&hit.path);
    PaletteEntry::new(
        format!("{}:{}", hit.line, path.display()),
        hit.text,
        format!("{}:{}", hit.path, hit.line),
    )
    .with_icon(IconKind::File { name })
}

/// Start ripgrep on `search`'s pattern, on a thread of its own.
fn start_run(search: &mut LiveSearch) -> Result<(), String> {
    let run = search.current.load(Ordering::Relaxed);
    let mut child = process::command(RIPGREP)
        .args(rg_args(&search.pattern))
        .current_dir(&search.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| NO_RIPGREP.to_string())?;
    let stdout = child.stdout.take().ok_or(NO_RIPGREP)?;
    let current = Arc::clone(&search.current);
    let sender = search.sender.clone();
    search.running = true;
    thread::Builder::new()
        .name("winter live search".into())
        .spawn(move || {
            stream(BufReader::new(stdout), run, &current, &sender);
            // Whatever is left unread is not wanted.
            let _ = child.kill();
            let _ = child.wait();
        })
        .map(|_| ())
        .map_err(|e| format!("could not start the search: {e}"))
}

/// Hand what ripgrep prints on to the list in batches, until it is done, the
/// cap is reached, or the run is no longer the one wanted.
fn stream(output: impl BufRead, run: u64, current: &AtomicU64, sender: &Sender<Batch>) {
    let mut hits = Vec::new();
    let mut sent_at = Instant::now();
    let mut found = 0;
    for line in output.split(b'\n').map_while(Result::ok) {
        if current.load(Ordering::Relaxed) != run {
            return;
        }
        let Some(hit) = parse_hit(&line) else {
            continue;
        };
        hits.push(hit);
        found += 1;
        if found >= MAX_RESULTS {
            break;
        }
        if hits.len() >= BATCH_LINES || sent_at.elapsed() >= BATCH_EVERY {
            let batch = Batch {
                done: false,
                hits: std::mem::take(&mut hits),
                run,
            };
            if sender.send(batch).is_err() {
                return;
            }
            sent_at = Instant::now();
        }
    }
    let _ = sender.send(Batch {
        done: true,
        hits,
        run,
    });
}

/// What ripgrep is run with for `pattern`: one line per hit, its path ended
/// by a NUL so a `:` in a name cannot be misread, hidden files searched but
/// version control's own directories not, and case ignored unless the
/// pattern has a capital in it. A `*` makes the pattern a regex, each one
/// standing for anything; otherwise it is searched for as it is.
fn rg_args(pattern: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "--line-number",
        "--no-heading",
        "--with-filename",
        "--null",
        "--color=never",
        "--smart-case",
        "--hidden",
        "--glob=!.git",
        "--glob=!.hg",
    ]
    .map(String::from)
    .to_vec();
    match pattern.contains('*') {
        true => {
            let regex: Vec<String> = pattern.split('*').map(regex::escape).collect();
            args.push("--regexp".into());
            args.push(regex.join(WILDCARD_REGEX));
        }
        false => {
            args.push("--fixed-strings".into());
            args.push("--regexp".into());
            args.push(pattern.to_string());
        }
    }
    args
}

/// One line of ripgrep's output: `path NUL line:text`.
fn parse_hit(line: &[u8]) -> Option<Hit> {
    let split = line.iter().position(|byte| *byte == 0)?;
    let path = String::from_utf8_lossy(&line[..split]);
    let path = path.strip_prefix("./").unwrap_or(&path).replace('\\', "/");
    let rest = String::from_utf8_lossy(&line[split + 1..]);
    let (number, text) = rest.split_once(':')?;
    Some(Hit {
        line: number.parse().ok()?,
        path,
        text: text.trim().chars().take(MAX_LINE_CHARS).collect(),
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_pattern_is_literal_unless_a_star_makes_it_a_regex() {
        let literal = rg_args("a.b(c");
        assert!(literal.contains(&"--fixed-strings".to_string()));
        assert_eq!(literal.last().map(String::as_str), Some("a.b(c"));

        let wild = rg_args("fn *(x");
        assert!(!wild.contains(&"--fixed-strings".to_string()));
        assert_eq!(wild.last().map(String::as_str), Some(r"fn .*\(x"));
    }

    #[test]
    fn test_a_line_of_output_names_its_file_and_line() {
        assert_eq!(
            parse_hit(b"./src/a:b.rs\x0012:    let x = 1;   "),
            Some(Hit {
                line: 12,
                path: "src/a:b.rs".into(),
                text: "let x = 1;".into(),
            }),
            "a colon in the name is the name's"
        );
        assert_eq!(parse_hit(b"no separator"), None);
    }

    #[test]
    fn test_a_run_no_longer_wanted_sends_nothing_more() {
        let (sender, batches) = mpsc::channel();
        let current = AtomicU64::new(1);
        let output: &[u8] = b"a\x001:x\nb\x002:y\n";
        stream(output, 0, &current, &sender);
        assert!(batches.try_recv().is_err(), "run 0 was replaced by run 1");

        stream(output, 1, &current, &sender);
        let batch = batches.try_recv().expect("the wanted run's lines");
        assert!(batch.done);
        assert_eq!(batch.hits.len(), 2);
    }

    #[test]
    fn test_a_paused_pattern_streams_this_repository_s_lines_into_the_list() {
        if process::command(RIPGREP).arg("--version").output().is_err() {
            return;
        }
        let mut app = App::new();
        app.open_live_search();
        // Spelled apart so this file's own copy of it is the only match.
        let needle = format!("{}{}", "MIN_PATTERN_", "CHARS: usize");
        let mut palette = app.palette().cloned().expect("the search");
        palette.query = needle;
        app.on_live_query(&mut palette);
        *app.palette_mut().expect("still open") = palette;

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut now = Instant::now() + DEBOUNCE;
        while app.palette().is_some_and(|p| p.entries.is_empty()) && Instant::now() < deadline {
            app.tick_live_search(now);
            thread::sleep(Duration::from_millis(20));
            now = Instant::now() + DEBOUNCE;
        }
        let palette = app.palette().expect("still open");
        assert_eq!(palette.entries.len(), 1, "{:?}", palette.entries);
        assert!(palette.entries[0].shortcut.ends_with(&format!(
            "src/app/live_search.rs:{}",
            palette.entries[0]
                .action
                .split(':')
                .next()
                .unwrap_or_default()
        )));
    }

    #[test]
    fn test_the_search_starts_after_the_typing_pauses_and_not_for_one_letter() {
        let mut app = App::new();
        app.open_live_search();
        let mut palette = app.palette().cloned().expect("the search");
        palette.query = "x".into();
        app.on_live_query(&mut palette);
        assert!(app.live_search.as_ref().is_some_and(|s| s.due.is_none()));

        palette.query = "xy".into();
        app.on_live_query(&mut palette);
        let due = app.live_search.as_ref().and_then(|s| s.due);
        assert!(
            due.is_some_and(|due| due > Instant::now()),
            "waits for a pause"
        );
    }
}
