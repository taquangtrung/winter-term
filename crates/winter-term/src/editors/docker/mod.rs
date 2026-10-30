//! Docker: the containers, images, volumes, and networks the daemon knows,
//! one kind at a time, kept current on a timer, with what magic-vscode's
//! Docker view does to each a key away: start, stop, restart, pause, kill,
//! and remove a container, read its logs and processes, open a shell in it;
//! pull and remove an image; remove a volume or a network; prune what is
//! unused.
//!
//! - [`parse`]: reading what `docker` lists.
//!
//! Everything goes through the `docker` command, so nothing is linked in and
//! Winter needs no more right to the daemon than the shell it was started
//! from. A shell, an attach, and a log that follows open in a terminal tab of
//! their own; what is only read (an inspect, a page of logs, a container's
//! processes) fills this view in place of the list until `q`.

pub mod parse;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::editors::list::{ListPlace, ListView};
use crate::editors::refresh::RefreshTimer;
use crate::model::editor::{
    menu_groups, CommandOutput, CommandRequest, Editor, EditorMenuItem, EditorOutcome, JobReply,
    JobRequest, Notice, PromptReply, PromptRequest, SearchReport, SpawnRequest,
};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::paint::{Frame, Point, Row, Span, Style};
use crate::model::vim::nav::{VimKey, VimNav};

use self::parse::{Item, Section};

// ========================================================================
// Constants
// ========================================================================

/// The program every request runs.
const DOCKER: &str = "docker";

/// The questions the view asks.
const ASK_CONFIRM: &str = "docker-confirm";
const ASK_SEARCH: &str = "docker-search";

/// Tags naming what a finished command was, past a section's listing (see
/// [`Section::list_tag`]): an action on an object, and output to read.
const TAG_ACT: &str = "docker-act";
const TAG_READ: &str = "docker-read";

/// Rows above the first object: the summary and the column headings.
const HEADER_ROWS: usize = 2;

/// How often the section showing is listed again, and how long a listing
/// may be out before it is asked for again.
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// How wide a column may grow to fit what it holds, past which it is cut.
const MAX_COLUMN_WIDTH: usize = 40;

/// How many lines of a container's log the view reads.
const LOG_LINES: &str = "500";

/// What the view is called.
const TITLE: &str = "Docker";

/// The shell a container is entered with: bash where the image has it, else
/// the `sh` every image has.
const SHELL: &str = "command -v bash >/dev/null && exec bash || exec sh";

/// The mark on a field too long for its column.
const ELLIPSIS: char = '\u{2026}';

/// The keys of every section, ahead of the shared motions.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::NextSection,
        keys: &["TAB"],
        label: "Next section: containers, images, volumes, networks",
    },
    Binding {
        action: Act::PreviousSection,
        keys: &["S-TAB"],
        label: "Previous section",
    },
    Binding {
        action: Act::Inspect,
        keys: &["RET"],
        label: "Inspect",
    },
    Binding {
        action: Act::Refresh,
        keys: &["r", "C-r"],
        label: "Refresh Now",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Search",
    },
    Binding {
        action: Act::NextMatch,
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::PreviousMatch,
        keys: &["N"],
        label: "Previous Match",
    },
    Binding {
        action: Act::CopyName,
        keys: &["y"],
        label: "Copy Name",
    },
    Binding {
        action: Act::CopyId,
        keys: &["Y"],
        label: "Copy ID",
    },
    Binding {
        action: Act::Remove,
        keys: &["D"],
        label: "Remove",
    },
    Binding {
        action: Act::Prune,
        keys: &["C"],
        label: "Prune: remove every unused one",
    },
];

/// A container's keys.
const CONTAINER_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Start,
        keys: &["s"],
        label: "Start",
    },
    Binding {
        action: Act::Stop,
        keys: &["x"],
        label: "Stop",
    },
    Binding {
        action: Act::Restart,
        keys: &["R"],
        label: "Restart",
    },
    Binding {
        action: Act::Pause,
        keys: &["p"],
        label: "Pause / Unpause",
    },
    Binding {
        action: Act::Kill,
        keys: &["K"],
        label: "Kill",
    },
    Binding {
        action: Act::Shell,
        keys: &["e"],
        label: "Open a shell in it, in a new tab",
    },
    Binding {
        action: Act::Attach,
        keys: &["a"],
        label: "Attach to it, in a new tab",
    },
    Binding {
        action: Act::FollowLogs,
        keys: &["L"],
        label: "Follow its logs, in a new tab",
    },
    Binding {
        action: Act::Logs,
        keys: &["o"],
        label: "Read its last logs",
    },
    Binding {
        action: Act::Top,
        keys: &["t"],
        label: "Its processes",
    },
    Binding {
        action: Act::Stats,
        keys: &["S"],
        label: "Its CPU, memory, and I/O",
    },
];

/// An image's keys.
const IMAGE_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Pull,
        keys: &["P"],
        label: "Pull it again",
    },
    Binding {
        action: Act::History,
        keys: &["H"],
        label: "Its layers",
    },
];

/// The keys while output fills the view.
const OUTPUT_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Back,
        keys: &["q", "ESC"],
        label: "Back to the list",
    },
    Binding {
        action: Act::Refresh,
        keys: &["r", "C-r"],
        label: "Read it again",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Search",
    },
    Binding {
        action: Act::NextMatch,
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::PreviousMatch,
        keys: &["N"],
        label: "Previous Match",
    },
];

/// What the menu over a container offers, a group at a time.
const CONTAINER_MENU: [&[Act]; 4] = [
    &[Act::Start, Act::Stop, Act::Restart, Act::Pause, Act::Kill],
    &[Act::Shell, Act::Attach, Act::FollowLogs],
    &[Act::Inspect, Act::Logs, Act::Top, Act::Stats],
    &[Act::CopyName, Act::CopyId, Act::Remove],
];

/// What the menu over an image offers.
const IMAGE_MENU: [&[Act]; 3] = [
    &[Act::Inspect, Act::History],
    &[Act::Pull],
    &[Act::CopyName, Act::CopyId, Act::Remove],
];

/// What the menu over a volume or a network offers.
const OBJECT_MENU: [&[Act]; 2] = [&[Act::Inspect], &[Act::CopyName, Act::CopyId, Act::Remove]];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the view's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Attach,
    Back,
    CopyId,
    CopyName,
    FollowLogs,
    History,
    Inspect,
    Kill,
    Logs,
    NextMatch,
    NextSection,
    Pause,
    PreviousMatch,
    PreviousSection,
    Prune,
    Pull,
    Refresh,
    Remove,
    Restart,
    Search,
    Shell,
    Start,
    Stats,
    Stop,
    Top,
}

/// A command waiting on the answer to "are you sure", and what it is.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Pending {
    args: Vec<String>,
    what: String,
}

/// What fills the view in place of the list: what it is of, its lines, the
/// command that read them, and the cursor over them.
#[derive(Debug)]
struct Output {
    args: Vec<String>,
    lines: Vec<String>,
    list: ListView<usize>,
    title: String,
}

/// The Docker view.
#[derive(Debug)]
pub struct DockerEditor {
    /// What the action in flight is reported as once it is done.
    acting: Option<String>,
    /// Why the last listing failed, until one succeeds: no daemon, or no
    /// `docker` at all.
    error: Option<String>,
    items: Vec<Item>,
    /// The cursor over the objects, keyed by id, so a refresh that reorders
    /// them leaves it on the same one.
    list: ListView<String>,
    /// Whether the section showing has been listed since it was chosen, so
    /// an empty one can say so rather than look like it is loading.
    loaded: bool,
    nav: VimNav,
    notice: Option<Notice>,
    output: Option<Output>,
    pending: Option<Pending>,
    section: Section,
    timer: RefreshTimer,
}

// ========================================================================
// DockerEditor
// ========================================================================

impl Default for DockerEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerEditor {
    /// The view, on its containers, listing them at its first tick.
    pub fn new() -> Self {
        Self {
            acting: None,
            error: None,
            items: Vec::new(),
            list: ListView::new(),
            loaded: false,
            nav: VimNav::new(),
            notice: None,
            output: None,
            pending: None,
            section: Section::default(),
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
        }
    }

    /// The object under the cursor.
    fn selected(&self) -> Option<&Item> {
        self.items.get(self.list.cursor())
    }

    /// The keys of the section showing, past the ones every section has.
    fn section_keys(&self) -> &'static [Binding<Act>] {
        match self.section {
            Section::Containers => CONTAINER_KEYS,
            Section::Images => IMAGE_KEYS,
            Section::Networks | Section::Volumes => &[],
        }
    }

    /// List the section showing again, now.
    fn list_now(&mut self) -> EditorOutcome {
        self.timer.request_now(Instant::now());
        self.listing()
    }

    /// The request listing the section showing.
    fn listing(&self) -> EditorOutcome {
        let request = request(self.section.list_args(), self.section.list_tag());
        EditorOutcome::Job(JobRequest::Command(request))
    }

    /// Show `section` instead, listing it at once.
    fn show_section(&mut self, section: Section) -> EditorOutcome {
        self.section = section;
        self.items.clear();
        self.list.reset_keys(Vec::new());
        self.loaded = false;
        self.error = None;
        self.list_now()
    }

    /// A finished listing: the objects, keeping the cursor on the one it was
    /// on, or what went wrong. A listing of another section than the one
    /// showing is left unread.
    fn on_listed(&mut self, output: &CommandOutput) {
        if output.tag != self.section.list_tag() {
            return;
        }
        self.timer.on_answer(Instant::now());
        if !output.succeeded() {
            self.error = Some(output.failure());
            return;
        }
        self.error = None;
        self.loaded = true;
        self.items = parse::parse(self.section, &output.stdout);
        self.list
            .set_keys(self.items.iter().map(|item| item.id.clone()).collect());
    }

    /// Do `args` to the object under the cursor: the docker words before the
    /// object's name, reported as `what` once done.
    fn act_on_selected(&mut self, words: &[&str], verb: &str) -> EditorOutcome {
        let Some(item) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let mut args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        args.push(item.handle.clone());
        let what = format!("{verb} {}", item.handle);
        self.run_action(args, what)
    }

    /// Run an action, saying in the status bar what it is as it starts.
    fn run_action(&mut self, args: Vec<String>, what: String) -> EditorOutcome {
        self.notice = Some(Notice::info(format!("{what}...")).unlogged());
        self.acting = Some(what);
        EditorOutcome::Job(JobRequest::Command(request(args, TAG_ACT)))
    }

    /// Ask before `args`, which cannot be taken back.
    fn confirm(&mut self, args: Vec<String>, what: String) -> EditorOutcome {
        let question = format!("{what}?");
        self.pending = Some(Pending { args, what });
        EditorOutcome::Prompt(PromptRequest::confirm(ASK_CONFIRM, &question))
    }

    /// Ask before removing the object under the cursor.
    fn confirm_remove(&mut self) -> EditorOutcome {
        let Some(item) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let object = self.section.object();
        let args = vec![object.to_string(), "rm".to_string(), item.handle.clone()];
        let what = format!("remove {object} {}", item.handle);
        self.confirm(args, what)
    }

    /// Ask before removing every unused object of the section showing.
    fn confirm_prune(&mut self) -> EditorOutcome {
        let object = self.section.object();
        let args = vec![object.to_string(), "prune".to_string(), "--force".to_string()];
        let what = match self.section {
            Section::Containers => "remove every stopped container".to_string(),
            Section::Images => "remove every dangling image".to_string(),
            section => format!("remove every unused {}", section.object()),
        };
        self.confirm(args, what)
    }

    /// Ask before killing the container under the cursor.
    fn confirm_kill(&mut self) -> EditorOutcome {
        let Some(item) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let args = vec!["kill".to_string(), item.handle.clone()];
        let what = format!("kill container {}", item.handle);
        self.confirm(args, what)
    }

    /// Pause the container under the cursor, or unpause it if it is paused.
    fn toggle_pause(&mut self) -> EditorOutcome {
        let paused = self
            .selected()
            .is_some_and(|item| item.columns.get(1).map(String::as_str) == Some("paused"));
        match paused {
            true => self.act_on_selected(&["unpause"], "unpause"),
            false => self.act_on_selected(&["pause"], "pause"),
        }
    }

    /// A finished action: what it did, or why it could not, and a listing
    /// soon, since the list is about to be wrong.
    fn on_acted(&mut self, output: &CommandOutput) {
        let done = self.acting.take();
        self.notice = Some(match (output.succeeded(), done) {
            (true, Some(what)) => Notice::info(what),
            (true, None) => Notice::info("done"),
            (false, _) => Notice::error(format!("failed: {}", output.failure())),
        });
        self.timer.refresh_soon(Instant::now());
    }

    /// Read `args`'s output into the view, under `title`.
    fn read(&mut self, args: Vec<String>, title: String) -> EditorOutcome {
        self.output = Some(Output {
            args: args.clone(),
            lines: Vec::new(),
            list: ListView::new(),
            title,
        });
        EditorOutcome::Job(JobRequest::Command(request(args, TAG_READ)))
    }

    /// Read the docker words `words` before the selected object's name.
    fn read_selected(&mut self, words: &[&str], title: &str) -> EditorOutcome {
        let Some(item) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let handle = item.handle.clone();
        let mut args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        args.push(handle.clone());
        self.read(args, format!("{title}: {handle}"))
    }

    /// Output that was read: its lines, stdout then stderr, since `docker
    /// logs` writes a container's error stream to its own.
    fn on_read(&mut self, output: &CommandOutput) {
        let Some(shown) = self.output.as_mut() else {
            return;
        };
        if !output.succeeded() {
            self.notice = Some(Notice::error(format!("failed: {}", output.failure())));
        }
        shown.lines = output
            .stdout
            .lines()
            .chain(output.stderr.lines())
            .map(str::to_string)
            .collect();
        shown.list.reset_keys((0..shown.lines.len()).collect());
    }

    /// Open a terminal tab running `docker` with `args`.
    fn spawn(&self, args: Vec<String>) -> EditorOutcome {
        EditorOutcome::Spawn(SpawnRequest {
            args,
            cwd: crate::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            program: DOCKER.to_string(),
        })
    }

    /// Open a terminal tab running `docker` with `words` before the selected
    /// container's name, and `after` it.
    fn spawn_on_selected(&self, words: &[&str], after: &[&str]) -> EditorOutcome {
        let Some(item) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let mut args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        args.push(item.handle.clone());
        args.extend(after.iter().map(|word| word.to_string()));
        self.spawn(args)
    }

    /// Do what one of the view's own keys asks. `None` for a key with
    /// nothing to do, which then reaches the motions.
    fn run(&mut self, act: Act) -> Option<EditorOutcome> {
        let outcome = match act {
            Act::NextSection => self.show_section(self.section.next()),
            Act::PreviousSection => {
                let back = Section::ALL.len() - 1;
                let section = (0..back).fold(self.section, |section, _| section.next());
                self.show_section(section)
            }
            Act::Refresh => self.list_now(),
            Act::Search => ask_search(),
            Act::NextMatch => self.search_step(true),
            Act::PreviousMatch => self.search_step(false),
            Act::CopyName => match self.selected() {
                Some(item) => EditorOutcome::Yank(item.handle.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::CopyId => match self.selected() {
                Some(item) => EditorOutcome::Yank(item.id.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::Inspect => {
                let object = self.section.object();
                self.read_selected(&[object, "inspect"], "Inspect")
            }
            Act::Remove => self.confirm_remove(),
            Act::Prune => self.confirm_prune(),
            Act::Start => self.act_on_selected(&["start"], "start"),
            Act::Stop => self.act_on_selected(&["stop"], "stop"),
            Act::Restart => self.act_on_selected(&["restart"], "restart"),
            Act::Pause => self.toggle_pause(),
            Act::Kill => self.confirm_kill(),
            Act::Shell => self.spawn_on_selected(&["exec", "-it"], &["sh", "-c", SHELL]),
            Act::Attach => self.spawn_on_selected(&["attach"], &[]),
            Act::FollowLogs => {
                self.spawn_on_selected(&["logs", "--follow", "--tail", LOG_LINES], &[])
            }
            Act::Logs => self.read_selected(&["logs", "--tail", LOG_LINES], "Logs"),
            Act::Top => self.read_selected(&["top"], "Processes"),
            Act::Stats => self.read_selected(&["stats", "--no-stream"], "Stats"),
            Act::History => self.read_selected(&["image", "history"], "History"),
            Act::Pull => self.act_on_selected(&["pull"], "pull"),
            Act::Back => return None,
        };
        Some(outcome)
    }

    /// A key while output fills the view.
    fn on_output_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(OUTPUT_KEYS, key) {
            Some(Act::Back) => {
                self.output = None;
                self.list_now()
            }
            Some(Act::Refresh) => {
                let Some(shown) = self.output.as_ref() else {
                    return EditorOutcome::Consumed;
                };
                let (args, title) = (shown.args.clone(), shown.title.clone());
                self.read(args, title)
            }
            Some(Act::Search) => ask_search(),
            Some(Act::NextMatch) => self.search_step(true),
            Some(Act::PreviousMatch) => self.search_step(false),
            _ => self.on_motion_key(key),
        }
    }

    /// Offer a key to the shared motions, over the output or the list.
    fn on_motion_key(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                match self.output.as_mut() {
                    Some(shown) => shown.list.apply_motion(motion),
                    None => self.list.apply_motion(motion),
                };
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    /// What a search reads over: the output's lines, or one label per object.
    fn search_labels(&self) -> Vec<String> {
        match &self.output {
            Some(shown) => shown.lines.clone(),
            None => self
                .items
                .iter()
                .map(|item| format!("{} {} {}", item.handle, item.id, item.columns.join(" ")))
                .collect(),
        }
    }

    fn search_for(&mut self, query: &str) -> EditorOutcome {
        let labels = self.search_labels();
        let list = match self.output.as_mut() {
            Some(shown) => shown.list.search_for(&labels, query.trim()),
            None => self.list.search_for(&labels, query.trim()),
        };
        if let Some(notice) = list {
            self.notice = Some(notice);
        }
        EditorOutcome::Consumed
    }

    fn search_step(&mut self, forward: bool) -> EditorOutcome {
        let labels = self.search_labels();
        let found = match self.output.as_mut() {
            Some(shown) => shown.list.search_step(&labels, forward),
            None => self.list.search_step(&labels, forward),
        };
        if let Some(notice) = found {
            self.notice = Some(notice);
        }
        EditorOutcome::Consumed
    }

    /// The summary over the list: the sections, the one showing in the
    /// accent color, and how many objects it has.
    fn summary_row(&self) -> Row {
        let mut row = Vec::new();
        for section in Section::ALL {
            let style = match section == self.section {
                true => Style::HeaderAccent,
                false => Style::Dim,
            };
            row.push(Span::new(style, section.label()));
            row.push(Span::plain("  "));
        }
        let count = match (self.loaded, &self.error) {
            (_, Some(_)) => String::new(),
            (true, None) => format!("{} listed", self.items.len()),
            (false, None) => "reading...".to_string(),
        };
        row.push(Span::new(Style::Dim, count));
        row
    }

    /// How wide each column is: wide enough for its heading and for what it
    /// holds, up to [`MAX_COLUMN_WIDTH`], with the name first.
    fn widths(&self) -> Vec<usize> {
        let fit = |heading: &str, cells: Vec<usize>| {
            cells
                .into_iter()
                .chain([heading.chars().count()])
                .max()
                .unwrap_or_default()
                .min(MAX_COLUMN_WIDTH)
        };
        let names = self.items.iter().map(|item| item.handle.chars().count()).collect();
        let mut widths = vec![fit("NAME", names)];
        for (at, heading) in self.section.headings().iter().enumerate() {
            let cells = self
                .items
                .iter()
                .map(|item| item.columns.get(at).map_or(0, |cell| cell.chars().count()))
                .collect();
            widths.push(fit(heading, cells));
        }
        widths
    }

    /// One object's row, cut to `cols`.
    fn item_row(&self, item: &Item, widths: &[usize], cols: usize) -> Row {
        let mut row = vec![Span::new(Style::Normal, pad(&item.handle, widths[0]))];
        for (at, cell) in item.columns.iter().enumerate() {
            let Some(&width) = widths.get(at + 1) else {
                break;
            };
            let style = match (self.section, at) {
                (Section::Containers, 1) if item.running => Style::ChangeAdded,
                (Section::Containers, 1) => Style::Dim,
                _ => Style::Normal,
            };
            row.push(Span::new(style, pad(cell, width)));
        }
        clip_row(row, cols)
    }

    /// The column headings, cut to `cols`.
    fn heading_row(&self, widths: &[usize], cols: usize) -> Row {
        let headings = std::iter::once("NAME").chain(self.section.headings().iter().copied());
        let row = headings
            .zip(widths)
            .map(|(heading, width)| Span::new(Style::Header, pad(heading, *width)))
            .collect();
        clip_row(row, cols)
    }

    /// The output filling the view, a line to a row.
    fn output_frame(&mut self, rows: usize, cols: usize) -> Option<Frame> {
        let shown = self.output.as_mut()?;
        let viewport = rows.saturating_sub(1);
        shown.list.view_mut().set_rows(viewport);
        let placed = shown.list.place(viewport, |_| 1);
        let window = placed.window;
        let mut frame_rows = vec![clip_row(vec![Span::new(Style::HeaderAccent, shown.title.clone())], cols)];
        if shown.lines.is_empty() {
            frame_rows.push(vec![Span::new(Style::Dim, "reading...")]);
            return Some(Frame::new(frame_rows));
        }
        frame_rows.extend(
            shown.lines[window.start..window.start + window.count]
                .iter()
                .map(|line| clip_row(vec![Span::plain(line.clone())], cols)),
        );
        Some(placed.apply(Frame::new(frame_rows), 1))
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Editor for DockerEditor {
    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.list.saved_place()))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(place) = state.downcast::<ListPlace<String>>() {
            self.list.restore_place(*place);
        }
    }

    fn title(&self) -> String {
        match &self.output {
            Some(shown) => format!("{TITLE}: {}", shown.title),
            None => format!("{TITLE}: {}", self.section.label()),
        }
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        if let Some(frame) = self.output_frame(rows, cols) {
            return frame;
        }
        let widths = self.widths();
        let mut frame_rows = vec![
            clip_row(self.summary_row(), cols),
            self.heading_row(&widths, cols),
        ];
        if let Some(error) = &self.error {
            frame_rows.push(vec![Span::new(Style::Failed, format!("docker: {error}"))]);
            return Frame::new(frame_rows);
        }
        if self.items.is_empty() {
            let note = match self.loaded {
                true => format!("no {}", self.section.label().to_lowercase()),
                false => String::new(),
            };
            frame_rows.push(vec![Span::new(Style::Dim, note)]);
            return Frame::new(frame_rows);
        }
        let viewport = rows.saturating_sub(HEADER_ROWS);
        self.list.view_mut().set_rows(viewport);
        let placed = self.list.place(viewport, |_| 1);
        let window = placed.window;
        frame_rows.extend(
            self.items[window.start..window.start + window.count]
                .iter()
                .map(|item| self.item_row(item, &widths, cols)),
        );
        placed.apply(Frame::new(frame_rows), HEADER_ROWS)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if self.output.is_some() {
            return self.on_output_key(key);
        }
        if self.nav.in_sequence() {
            return self.on_motion_key(key);
        }
        let act = table::find(KEYS, key).or_else(|| table::find(self.section_keys(), key));
        match act.and_then(|act| self.run(act)) {
            Some(outcome) => outcome,
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = match self.output.is_some() {
            true => table::help(OUTPUT_KEYS),
            false => {
                let mut rows = table::help(KEYS);
                rows.extend(table::help(self.section_keys()));
                rows
            }
        };
        rows.extend(VimNav::help());
        rows
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag || self.output.is_some() || at.row < HEADER_ROWS {
            return EditorOutcome::Ignored;
        }
        let index = self.list.view().scroll() + at.row - HEADER_ROWS;
        if index >= self.items.len() {
            return EditorOutcome::Ignored;
        }
        self.list.set_cursor(index);
        EditorOutcome::Consumed
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.list.view_mut().set_margin(rows);
        if let Some(shown) = self.output.as_mut() {
            shown.list.view_mut().set_margin(rows);
        }
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        match self.output.as_mut() {
            Some(shown) => shown.list.view_mut().wheel(lines),
            None => self.list.view_mut().wheel(lines),
        }
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        match reply.tag {
            ASK_CONFIRM => match (reply.answer, self.pending.take()) {
                (Some(_), Some(pending)) => self.run_action(pending.args, pending.what),
                _ => EditorOutcome::Consumed,
            },
            ASK_SEARCH => match reply.answer {
                Some(query) => self.search_for(&query),
                None => EditorOutcome::Consumed,
            },
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        match tag == ASK_SEARCH && !query.trim().is_empty() {
            true => self.search_for(query),
            false => EditorOutcome::Ignored,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            JobReply::Command(output) => match output.tag {
                TAG_ACT => self.on_acted(&output),
                TAG_READ => self.on_read(&output),
                _ => self.on_listed(&output),
            },
            JobReply::DirSize { .. }
            | JobReply::Files(_)
            | JobReply::FileOps(_)
            | JobReply::Processes(_)
            | JobReply::Search(_)
            | JobReply::System(_) => {}
        }
        EditorOutcome::Consumed
    }

    fn set_shown(&mut self, shown: bool) {
        self.timer.set_hidden(!shown, Instant::now());
    }

    fn next_tick(&self) -> Option<Instant> {
        // Output being read is not listed under: the list is not showing.
        match self.output.is_some() {
            true => None,
            false => self.timer.next_tick(),
        }
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if self.output.is_some() || !self.timer.on_tick(now) {
            return EditorOutcome::Consumed;
        }
        self.listing()
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // What was asked for before the view was covered went with it.
        self.timer.restart(Instant::now());
        EditorOutcome::Consumed
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        match self.output.as_mut() {
            Some(shown) => shown.list.take_search(),
            None => self.list.take_search(),
        }
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        if self.output.is_some() || self.selected().is_none() {
            return Vec::new();
        }
        let groups: &[&[Act]] = match self.section {
            Section::Containers => &CONTAINER_MENU,
            Section::Images => &IMAGE_MENU,
            Section::Networks | Section::Volumes => &OBJECT_MENU,
        };
        let entry = |act: &Act| {
            table::menu_entry(KEYS, *act)
                .or_else(|| table::menu_entry(self.section_keys(), *act))
                .map(|(key, label)| EditorMenuItem::new(key, label))
        };
        menu_groups(
            groups
                .iter()
                .map(|group| group.iter().filter_map(entry).collect()),
        )
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A request running `docker` with `args`, from wherever the app is.
fn request(args: Vec<String>, tag: &'static str) -> CommandRequest {
    CommandRequest {
        id: 0,
        args,
        cwd: PathBuf::from("."),
        env: Vec::new(),
        program: DOCKER.to_string(),
        stdin: None,
        tag,
    }
}

/// The search question.
fn ask_search() -> EditorOutcome {
    EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH))
}

/// `text` in a column `width` cells wide and the space after it: cut with a
/// mark where it is too long, padded where it is short.
fn pad(text: &str, width: usize) -> String {
    let length = text.chars().count();
    let fitted = match length > width {
        true => {
            let kept: String = text.chars().take(width.saturating_sub(1)).collect();
            format!("{kept}{ELLIPSIS}")
        }
        false => format!("{text}{}", " ".repeat(width - length)),
    };
    format!("{fitted} ")
}

/// `row` cut to `cols` cells, so it takes one screen row.
fn clip_row(row: Row, cols: usize) -> Row {
    let mut room = cols;
    let mut clipped = Vec::with_capacity(row.len());
    for span in row {
        if room == 0 {
            break;
        }
        let text: String = span.text.chars().take(room).collect();
        room -= text.chars().count();
        clipped.push(Span::new(span.style, text));
    }
    clipped
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
