//! Docker in Winter, as an extension shipped with it: the containers,
//! images, volumes, and networks the daemon knows, one kind at a time, with
//! what can be done to each a key away: start, stop, restart, pause, kill,
//! and remove a container, read its logs and processes, open a shell in it;
//! pull and remove an image; remove a volume or a network; prune what is
//! unused.
//!
//! - [`parse`]: reading what `docker` lists.
//!
//! Everything goes through the `docker` command, the one program its package
//! asks to run, so it needs no more right to the daemon than the shell
//! Winter was started from. A shell, an attach, and a log that follows open
//! in a terminal tab of their own; what is only read (an inspect, a page of
//! logs, a container's processes) fills the view in place of the list until
//! `q`. The list is read again as the view comes into sight, after each
//! action, on `r`, and every [`MIN_TIMER`] while it stays in sight.

mod parse;

use std::sync::{Mutex, MutexGuard, PoisonError};

use winter_ext_api::{
    Event, Extension, Host, Icon, Line, MIN_TIMER, PickItem, ProgramOutput, Ran, Registry,
    RunError, Styled, Tone, View, ViewKey, pick,
};

use self::parse::{Item, Section};

// ========================================================================
// Constants
// ========================================================================

/// The extension's id.
const ID: &str = "docker";

/// The view's name among the extension's, and what its tab is called.
const VIEW: &str = "list";
const TITLE: &str = "Docker";

/// The program every request runs.
const DOCKER: &str = "docker";

/// The file whose icon the view and its command are drawn with.
const ICON_FILE: &str = "Dockerfile";

/// What a failed docker command is reported as, its output after it.
const DOCKER_FAILED: &str = "docker failed";

/// The tags an action on an object and output to read are run under, past
/// the sections' listings (see [`Section::list_tag`]).
const TAG_ACT: u64 = 10;
const TAG_READ: u64 = 11;

/// The timer that lists the section showing again.
const REFRESH_TIMER: &str = "refresh";

/// Rows above the first object: the summary and the column headings; above
/// the first line of output: its title.
const HEADER_ROWS: usize = 2;
const OUTPUT_HEADER_ROWS: usize = 1;

/// How wide a column may grow to fit what it holds, past which it is cut.
const MAX_COLUMN_WIDTH: usize = 40;

/// How many lines of a container's log the view reads.
const LOG_LINES: &str = "500";

/// The shell a container is entered with: bash where the image has it, else
/// the `sh` every image has.
const SHELL: &str = "command -v bash >/dev/null && exec bash || exec sh";

/// The mark on a field too long for its column.
const ELLIPSIS: char = '\u{2026}';

/// Every action of the view: what it does, its id, the keys it starts on,
/// and what the help list says of it.
const ACTS: [(Act, &str, &[&str], &str); 28] = [
    (Act::Down, "down", &["j", "<down>"], "Next row"),
    (Act::Up, "up", &["k", "<up>"], "Previous row"),
    (Act::First, "first", &["g g", "<home>"], "First row"),
    (Act::Last, "last", &["G", "<end>"], "Last row"),
    (
        Act::NextSection,
        "next_section",
        &["TAB"],
        "Next section: containers, images, volumes, networks",
    ),
    (
        Act::PreviousSection,
        "previous_section",
        &["S-TAB"],
        "Previous section",
    ),
    (Act::Inspect, "inspect", &["RET"], "Inspect"),
    (Act::Refresh, "refresh", &["r"], "Refresh Now"),
    (Act::Find, "find", &["/"], "Find a row"),
    (Act::CopyName, "copy_name", &["y"], "Copy Name"),
    (Act::CopyId, "copy_id", &["i"], "Copy ID"),
    (Act::Remove, "remove", &["D"], "Remove"),
    (
        Act::Prune,
        "prune",
        &["C"],
        "Prune: remove every unused one",
    ),
    (Act::Start, "start", &["s"], "Start a container"),
    (Act::Stop, "stop", &["x"], "Stop a container"),
    (Act::Restart, "restart", &["R"], "Restart a container"),
    (Act::Pause, "pause", &["p"], "Pause / unpause a container"),
    (Act::Kill, "kill", &["K"], "Kill a container"),
    (
        Act::Shell,
        "shell",
        &["e"],
        "Open a shell in a container, in a new tab",
    ),
    (
        Act::Attach,
        "attach",
        &["a"],
        "Attach to a container, in a new tab",
    ),
    (
        Act::FollowLogs,
        "follow_logs",
        &["L"],
        "Follow a container's logs, in a new tab",
    ),
    (Act::Logs, "logs", &["o"], "Read a container's last logs"),
    (
        Act::Processes,
        "processes",
        &["t"],
        "A container's processes",
    ),
    (
        Act::Stats,
        "stats",
        &["S"],
        "A container's CPU, memory, and I/O",
    ),
    (Act::Pull, "pull", &["P"], "Pull an image again"),
    (Act::History, "history", &["H"], "An image's layers"),
    (Act::Back, "back", &["q", "ESC"], "Back to the list"),
    (Act::Copy, "copy_line", &["Y"], "Copy the line of output"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// The extension.
pub struct Docker;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Docker);

/// The view, drawing what [`STATE`] holds.
struct Board;

/// What one of the view's actions does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Attach,
    Back,
    Copy,
    CopyId,
    CopyName,
    Down,
    Find,
    First,
    FollowLogs,
    History,
    Inspect,
    Kill,
    Last,
    Logs,
    NextSection,
    Pause,
    PreviousSection,
    Processes,
    Prune,
    Pull,
    Refresh,
    Remove,
    Restart,
    Shell,
    Start,
    Stats,
    Stop,
    Up,
}

/// What the extension knows of the daemon, and where the view stands.
struct State {
    /// What the action in flight is reported as once it is done.
    acting: Option<String>,
    /// The row the cursor is on, and the first row shown.
    cursor: Cursor,
    /// Why the last listing failed, until one succeeds: no daemon, or no
    /// `docker` at all.
    error: Option<String>,
    items: Vec<Item>,
    /// Whether the section showing has been listed since it was chosen, so
    /// an empty one can say so rather than look like it is loading.
    loaded: bool,
    output: Option<Output>,
    section: Section,
}

/// A cursor over rows, and the first row shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Cursor {
    at: usize,
    top: usize,
}

/// What fills the view in place of the list: the command that read it, its
/// lines, the cursor over them, and what it is of.
struct Output {
    args: Vec<String>,
    cursor: Cursor,
    lines: Vec<String>,
    title: String,
}

// ========================================================================
// Statics
// ========================================================================

static STATE: Mutex<State> = Mutex::new(State::new());

// ========================================================================
// Act
// ========================================================================

impl Act {
    /// The action whose id is `id`.
    fn from_id(id: &str) -> Option<Self> {
        ACTS.iter()
            .find(|(_, listed, _, _)| *listed == id)
            .map(|(act, _, _, _)| *act)
    }
}

// ========================================================================
// Board
// ========================================================================

impl View for Board {
    fn render(&mut self, rows: usize, _columns: usize) -> Vec<Line> {
        state().lines(rows)
    }

    fn on_action(&mut self, host: &mut dyn Host, action: &str) {
        if let Some(act) = Act::from_id(action) {
            run_act(host, act);
        }
    }

    fn cursor_line(&self) -> Option<usize> {
        state().cursor_line()
    }

    fn on_click(&mut self, _host: &mut dyn Host, line: usize, _column: usize) -> bool {
        state().click(line)
    }

    fn on_scroll(&mut self, lines: isize) -> bool {
        state().scroll(lines);
        true
    }
}

// ========================================================================
// Cursor
// ========================================================================

impl Cursor {
    /// Move by `by` rows among `len`, held to the first and the last.
    fn step(&mut self, by: isize, len: usize) {
        let last = len.saturating_sub(1);
        self.at = self.at.saturating_add_signed(by).min(last);
    }

    /// The first row to show, `rows` of `len` in sight, so the cursor is in
    /// sight: kept where it was while the cursor stays inside.
    fn place(&mut self, rows: usize, len: usize) -> usize {
        self.at = self.at.min(len.saturating_sub(1));
        if self.at < self.top {
            self.top = self.at;
        } else if rows > 0 && self.at >= self.top + rows {
            self.top = self.at + 1 - rows;
        }
        self.top = self.top.min(len.saturating_sub(rows.max(1)));
        self.top
    }
}

// ========================================================================
// Extension
// ========================================================================

impl Extension for Docker {
    fn id(&self) -> &'static str {
        ID
    }

    fn activate(&self, registry: &mut Registry) {
        registry.command_with_icon(
            "open",
            "Docker: Containers, Images, Volumes, Networks",
            Icon::File(ICON_FILE.to_string()),
            |host| host.open_view(VIEW),
        );
        let keys = ACTS
            .iter()
            .map(|(_, id, keys, label)| ViewKey::new(id, label, keys))
            .collect();
        let icon = Icon::File(ICON_FILE.to_string());
        registry.view_with_icon(VIEW, TITLE, keys, icon, || Box::new(Board));
        registry.on_event(hear);
    }
}

// ========================================================================
// State
// ========================================================================

impl State {
    const fn new() -> Self {
        Self {
            acting: None,
            cursor: Cursor { at: 0, top: 0 },
            error: None,
            items: Vec::new(),
            loaded: false,
            output: None,
            section: Section::Containers,
        }
    }

    /// The object under the cursor.
    fn selected(&self) -> Option<&Item> {
        self.items.get(self.cursor.at)
    }

    /// Show `section` instead, as yet unlisted.
    fn show_section(&mut self, section: Section) {
        self.section = section;
        self.items.clear();
        self.cursor = Cursor::default();
        self.loaded = false;
        self.error = None;
    }

    /// Take a listing of the section showing: its objects, the cursor kept
    /// on the one it was on, or what went wrong. Whether it was the
    /// section's: one of another, asked for before `Tab` moved on, is left.
    fn take_listing(&mut self, ran: &Ran) -> bool {
        if ran.tag != self.section.list_tag() {
            return false;
        }
        match &ran.outcome {
            Ok(output) if output.is_success() => {
                let was = self.selected().map(|item| item.id.clone());
                self.items = parse::parse(self.section, &output.stdout);
                self.error = None;
                self.loaded = true;
                if let Some(at) = was.and_then(|id| self.items.iter().position(|i| i.id == id)) {
                    self.cursor.at = at;
                }
            }
            Ok(output) => self.error = Some(failure(output)),
            Err(error) => self.error = Some(run_failure(error)),
        }
        true
    }

    /// The lines of the view, `rows` high.
    fn lines(&mut self, rows: usize) -> Vec<Line> {
        if let Some(output) = self.output.as_mut() {
            let mut lines = vec![vec![Styled::new(Tone::Accent, &output.title)]];
            if output.lines.is_empty() {
                lines.push(vec![Styled::new(Tone::Dim, "reading…")]);
                return lines;
            }
            let shown = rows.saturating_sub(OUTPUT_HEADER_ROWS);
            let top = output.cursor.place(shown, output.lines.len());
            lines.extend(
                output.lines[top..]
                    .iter()
                    .take(shown)
                    .map(|line| vec![Styled::new(Tone::Normal, line)]),
            );
            return lines;
        }
        let widths = self.widths();
        let mut lines = vec![self.summary(), self.heading(&widths)];
        if let Some(error) = &self.error {
            lines.push(vec![Styled::new(Tone::Error, format!("docker: {error}"))]);
            return lines;
        }
        if self.items.is_empty() {
            let note = match self.loaded {
                true => format!("no {}", self.section.label().to_lowercase()),
                false => String::new(),
            };
            lines.push(vec![Styled::new(Tone::Dim, note)]);
            return lines;
        }
        let shown = rows.saturating_sub(HEADER_ROWS);
        let top = self.cursor.place(shown, self.items.len());
        lines.extend(
            self.items[top..]
                .iter()
                .take(shown)
                .map(|item| self.item_line(item, &widths)),
        );
        lines
    }

    /// The line the cursor is on, among those [`Self::lines`] gave.
    fn cursor_line(&self) -> Option<usize> {
        match &self.output {
            Some(output) if !output.lines.is_empty() => {
                Some(OUTPUT_HEADER_ROWS + output.cursor.at - output.cursor.top)
            }
            Some(_) => None,
            None if self.error.is_none() && !self.items.is_empty() => {
                Some(HEADER_ROWS + self.cursor.at - self.cursor.top)
            }
            None => None,
        }
    }

    /// Put the cursor on the object clicked, at `line` of the view; whether
    /// one was.
    fn click(&mut self, line: usize) -> bool {
        if self.output.is_some() || line < HEADER_ROWS {
            return false;
        }
        let at = self.cursor.top + line - HEADER_ROWS;
        let on_one = at < self.items.len();
        if on_one {
            self.cursor.at = at;
        }
        on_one
    }

    /// Move the rows shown by `lines`, positive upwards, as the wheel turned.
    fn scroll(&mut self, lines: isize) {
        let (cursor, len) = match self.output.as_mut() {
            Some(output) => (&mut output.cursor, output.lines.len()),
            None => (&mut self.cursor, self.items.len()),
        };
        cursor.step(-lines, len);
    }

    /// The cursor the motions move, and how many rows it moves over.
    fn moving(&mut self) -> (&mut Cursor, usize) {
        match self.output.as_mut() {
            Some(output) => (&mut output.cursor, output.lines.len()),
            None => (&mut self.cursor, self.items.len()),
        }
    }

    /// The summary over the list: the sections, the one showing in the
    /// accent color, and how many objects it has.
    fn summary(&self) -> Line {
        let mut line = Vec::new();
        for section in Section::ALL {
            let tone = match section == self.section {
                true => Tone::Accent,
                false => Tone::Dim,
            };
            line.push(Styled::new(tone, section.label()));
            line.push(Styled::new(Tone::Normal, "  "));
        }
        let count = match (self.loaded, &self.error) {
            (_, Some(_)) => String::new(),
            (true, None) => format!("{} listed", self.items.len()),
            (false, None) => "reading…".to_string(),
        };
        line.push(Styled::new(Tone::Dim, count));
        line
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
        let names = self
            .items
            .iter()
            .map(|item| item.handle.chars().count())
            .collect();
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

    /// The column headings.
    fn heading(&self, widths: &[usize]) -> Line {
        std::iter::once("NAME")
            .chain(self.section.headings().iter().copied())
            .zip(widths)
            .map(|(heading, width)| Styled::new(Tone::Heading, pad(heading, *width)))
            .collect()
    }

    /// One object's line: a running container's state in the info hue, a
    /// stopped one's dimmed.
    fn item_line(&self, item: &Item, widths: &[usize]) -> Line {
        let mut line = vec![Styled::new(Tone::Normal, pad(&item.handle, widths[0]))];
        for (at, cell) in item.columns.iter().enumerate() {
            let Some(&width) = widths.get(at + 1) else {
                break;
            };
            let tone = match (self.section, at) {
                (Section::Containers, 1) if item.running => Tone::Info,
                (Section::Containers, 1) => Tone::Dim,
                _ => Tone::Normal,
            };
            line.push(Styled::new(tone, pad(cell, width)));
        }
        line
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the extension does on hearing `event`: shown, or its timer rung,
/// it lists the section showing; a program done, it takes what it printed.
fn hear(host: &mut dyn Host, event: &Event) {
    match event {
        Event::Shown | Event::Timer(_) => list(host),
        Event::Ran(ran) => take_ran(host, ran),
        _ => {}
    }
}

/// List the section showing, unless output fills the view.
fn list(host: &mut dyn Host) {
    let state = state();
    if state.output.is_some() {
        return;
    }
    let section = state.section;
    drop(state);
    host.run(section.list_tag(), DOCKER, &section.list_args());
}

/// Take what a program run printed: a listing, an action's outcome, or
/// output to read.
fn take_ran(host: &mut dyn Host, ran: &Ran) {
    match ran.tag {
        TAG_ACT => {
            let done = state().acting.take();
            match (&ran.outcome, done) {
                (Ok(output), Some(what)) if output.is_success() => host.notify(&what),
                (Ok(output), _) if output.is_success() => host.notify("done"),
                (Ok(output), _) => host.notify(&format!("{DOCKER_FAILED}: {}", failure(output))),
                (Err(error), _) => host.notify(&format!("{DOCKER_FAILED}: {}", run_failure(error))),
            }
            list(host);
        }
        TAG_READ => {
            let mut state = state();
            let Some(shown) = state.output.as_mut() else {
                return;
            };
            match &ran.outcome {
                // Stdout then stderr: `docker logs` writes a container's
                // error stream to its own.
                Ok(output) => {
                    shown.lines = output
                        .stdout
                        .lines()
                        .chain(output.stderr.lines())
                        .map(str::to_string)
                        .collect();
                }
                Err(error) => shown.lines = vec![run_failure(error)],
            }
        }
        _ => {
            if state().take_listing(ran) {
                host.set_timer(REFRESH_TIMER, MIN_TIMER);
            }
        }
    }
}

/// Do what the action `act` asks.
fn run_act(host: &mut dyn Host, act: Act) {
    let mut state = state();
    match act {
        Act::Down | Act::Up | Act::First | Act::Last => {
            let (cursor, len) = state.moving();
            match act {
                Act::Down => cursor.step(1, len),
                Act::Up => cursor.step(-1, len),
                Act::First => cursor.at = 0,
                _ => cursor.at = len.saturating_sub(1),
            }
        }
        Act::Back => {
            state.output = None;
            drop(state);
            list(host);
        }
        Act::Copy => {
            if let Some(line) = state
                .output
                .as_ref()
                .and_then(|output| output.lines.get(output.cursor.at))
            {
                host.copy_text(line);
            }
        }
        Act::Find => {
            drop(state);
            find(host);
        }
        Act::Refresh => match state.output.as_ref() {
            Some(output) => {
                let args = output.args.clone();
                drop(state);
                host.run(TAG_READ, DOCKER, &args);
            }
            None => {
                drop(state);
                list(host);
            }
        },
        // The rest act on the list, not on output filling the view.
        _ if state.output.is_some() => {}
        Act::NextSection | Act::PreviousSection => {
            let section = match act {
                Act::NextSection => state.section.next(),
                _ => (0..Section::ALL.len() - 1).fold(state.section, |s, _| s.next()),
            };
            state.show_section(section);
            drop(state);
            list(host);
        }
        _ => {
            let Some(item) = state.selected().cloned() else {
                return;
            };
            let section = state.section;
            drop(state);
            act_on(host, act, section, &item);
        }
    }
}

/// Do `act` to `item`, an object of `section`.
fn act_on(host: &mut dyn Host, act: Act, section: Section, item: &Item) {
    let handle = item.handle.as_str();
    let object = section.object();
    let container = section == Section::Containers;
    let image = section == Section::Images;
    match act {
        Act::CopyName => host.copy_text(handle),
        Act::CopyId => host.copy_text(&item.id),
        Act::Inspect => read(
            host,
            &[object, "inspect", handle],
            format!("Inspect: {handle}"),
        ),
        Act::Remove => confirm(
            host,
            words(&[object, "rm", handle]),
            format!("remove {object} {handle}"),
            "Remove",
        ),
        Act::Prune => {
            let what = match section {
                Section::Containers => "remove every stopped container".to_string(),
                Section::Images => "remove every dangling image".to_string(),
                _ => format!("remove every unused {object}"),
            };
            confirm(
                host,
                words(&[object, "prune", "--force"]),
                what,
                "Remove All",
            );
        }
        Act::Start if container => act_with(host, &["start", handle], format!("start {handle}")),
        Act::Stop if container => act_with(host, &["stop", handle], format!("stop {handle}")),
        Act::Restart if container => {
            act_with(host, &["restart", handle], format!("restart {handle}"))
        }
        Act::Pause if container => {
            let paused = item.columns.get(1).map(String::as_str) == Some("paused");
            let verb = match paused {
                true => "unpause",
                false => "pause",
            };
            act_with(host, &[verb, handle], format!("{verb} {handle}"));
        }
        Act::Kill if container => confirm(
            host,
            words(&["kill", handle]),
            format!("kill container {handle}"),
            "Kill",
        ),
        Act::Shell if container => {
            host.open_terminal(DOCKER, &words(&["exec", "-it", handle, "sh", "-c", SHELL]))
        }
        Act::Attach if container => host.open_terminal(DOCKER, &words(&["attach", handle])),
        Act::FollowLogs if container => host.open_terminal(
            DOCKER,
            &words(&["logs", "--follow", "--tail", LOG_LINES, handle]),
        ),
        Act::Logs if container => read(
            host,
            &["logs", "--tail", LOG_LINES, handle],
            format!("Logs: {handle}"),
        ),
        Act::Processes if container => read(host, &["top", handle], format!("Processes: {handle}")),
        Act::Stats if container => read(
            host,
            &["stats", "--no-stream", handle],
            format!("Stats: {handle}"),
        ),
        Act::Pull if image => act_with(host, &["pull", handle], format!("pull {handle}")),
        Act::History if image => read(
            host,
            &["image", "history", handle],
            format!("History: {handle}"),
        ),
        _ => {}
    }
}

/// Run `args`, an action, saying what it is as it starts and once done.
fn act_with(host: &mut dyn Host, args: &[&str], what: String) {
    host.notify(&format!("{what}…"));
    state().acting = Some(what);
    host.run(TAG_ACT, DOCKER, &words(args));
}

/// Ask before `args`, which cannot be taken back: `Cancel` first, so a
/// stray `RET` does nothing.
fn confirm(host: &mut dyn Host, args: Vec<String>, what: String, yes: &str) {
    let choices = vec![(PickItem::new("Cancel"), false), (PickItem::new(yes), true)];
    pick(
        host,
        &format!("Docker: {what}?"),
        choices,
        move |host, sure| {
            if sure {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                act_with(host, &args, what);
            }
        },
    );
}

/// Read what `args` prints into the view, under `title`.
fn read(host: &mut dyn Host, args: &[&str], title: String) {
    let args = words(args);
    state().output = Some(Output {
        args: args.clone(),
        cursor: Cursor::default(),
        lines: Vec::new(),
        title,
    });
    host.run(TAG_READ, DOCKER, &args);
}

/// Offer the rows showing in the picker, and put the cursor on the one
/// chosen.
fn find(host: &mut dyn Host) {
    let shown = state();
    let labels: Vec<String> = match &shown.output {
        Some(output) => output.lines.clone(),
        None => shown
            .items
            .iter()
            .map(|item| format!("{}  {}", item.handle, item.columns.join("  ")))
            .collect(),
    };
    drop(shown);
    let choices = labels
        .into_iter()
        .enumerate()
        .map(|(at, label)| (PickItem::new(label), at))
        .collect();
    pick(host, "Docker: Find", choices, |_, at| {
        let mut state = state();
        state.moving().0.at = at;
    });
}

/// `words` as a program's arguments.
fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

/// What a docker command that failed said, its error stream first.
fn failure(output: &ProgramOutput) -> String {
    let said = match output.stderr.trim() {
        "" => output.stdout.trim(),
        stderr => stderr,
    };
    match said {
        "" => format!("exit code {}", output.code.unwrap_or(-1)),
        said => said.lines().next().unwrap_or(said).to_string(),
    }
}

/// Why docker could not be run.
fn run_failure(error: &RunError) -> String {
    match error {
        RunError::Failed(why) => format!("cannot run docker: {why}"),
        RunError::NotAllowed => "not allowed to run docker".to_string(),
        // A reason added after this extension was built.
        other => format!("cannot run docker: {other:?}"),
    }
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

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(section: Section, stdout: &str) -> Ran {
        Ran::new(
            section.list_tag(),
            Ok(ProgramOutput::new(Some(0), stdout, "")),
        )
    }

    const TWO: &str = "a1\tweb\tnginx\trunning\tUp\t\t\nb2\tdb\tpostgres\texited\tExited\t\t\n";

    #[test]
    fn test_a_relisting_keeps_the_cursor_on_the_same_object() {
        let mut state = State::new();
        assert!(state.take_listing(&listing(Section::Containers, TWO)));
        state.cursor.at = 1;
        let reordered = "c3\tcache\tredis\trunning\tUp\t\t\nb2\tdb\tpostgres\texited\tExited\t\t\na1\tweb\tnginx\trunning\tUp\t\t\n";
        state.take_listing(&listing(Section::Containers, reordered));
        assert_eq!(
            state.selected().map(|item| item.handle.as_str()),
            Some("db")
        );
    }

    #[test]
    fn test_a_listing_of_another_section_is_left() {
        // Asked for before `Tab` moved on, it filled the images with containers.
        let mut state = State::new();
        state.show_section(Section::Images);
        assert!(!state.take_listing(&listing(Section::Containers, TWO)));
        assert!(state.items.is_empty() && !state.loaded);
    }

    #[test]
    fn test_a_failed_listing_says_why_from_docker_s_error_stream() {
        let mut state = State::new();
        let refused = ProgramOutput::new(Some(1), "", "Cannot connect to the Docker daemon\nmore");
        state.take_listing(&Ran::new(Section::Containers.list_tag(), Ok(refused)));
        assert_eq!(
            state.error.as_deref(),
            Some("Cannot connect to the Docker daemon")
        );
        let lines = state.lines(10);
        assert_eq!(lines.len(), HEADER_ROWS + 1, "the error, no rows");
        assert_eq!(state.cursor_line(), None, "no cursor over an error");
    }

    #[test]
    fn test_the_cursor_stays_in_sight_and_its_line_counts_the_headings() {
        let mut cursor = Cursor { at: 9, top: 0 };
        assert_eq!(
            cursor.place(4, 20),
            6,
            "scrolled so the cursor is the last shown"
        );
        cursor.step(-1, 20);
        assert_eq!(cursor.place(4, 20), 6, "kept while it stays inside");
        cursor.step(100, 20);
        assert_eq!(cursor.at, 19, "held to the last row");
        let mut state = State::new();
        state.take_listing(&listing(Section::Containers, TWO));
        state.cursor.at = 1;
        state.lines(10);
        assert_eq!(state.cursor_line(), Some(HEADER_ROWS + 1));
        assert!(state.click(HEADER_ROWS), "on the first object");
        assert_eq!(state.cursor.at, 0);
        assert!(!state.click(HEADER_ROWS + 5), "below the last");
    }

    #[test]
    fn test_a_long_field_is_cut_with_a_mark_and_a_short_one_padded() {
        assert_eq!(pad("abcdef", 4), "abc\u{2026} ");
        assert_eq!(pad("ab", 4), "ab   ");
    }

    #[test]
    fn test_every_action_id_names_one_action() {
        for (act, id, _, _) in ACTS {
            assert_eq!(Act::from_id(id), Some(act), "{id}");
        }
        assert_eq!(Act::from_id("no-such"), None);
    }
}
