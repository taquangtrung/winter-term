//! Startup monitor: what starts on its own at boot or login (systemd units,
//! desktop autostart entries, launch agents, crontab jobs, the files every
//! new shell reads), each with the file that sets it up a key away, and a
//! user's own services turned on and off in place.
//!
//! - [`collect`]: reading each system's list, the one side-effecting module.
//!
//! It is read when first shown and again on `r`: what starts at login
//! changes when someone changes it, not on a clock.

pub mod collect;
mod notice;

use std::path::PathBuf;
use std::time::Instant;

use misty::ThemeToken;
use misty::view::{LazySource, Space, badge, column, divider, lazy_column, page, row};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, Theme};

use winter_model::editor::{
    CommandRequest, EditorMenuItem, EditorOutcome, JobReply, JobRequest, Notice, OpenTarget,
    OwnView, Page, PromptMode, PromptReply, PromptRequest, SearchReport, ViewSlot, menu_of,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{Key, format_key};
use winter_model::list::{ListView, Viewport};
use winter_model::monitor::{MonitorAnswer, MonitorQuery, StartupEntry, StartupKind, StartupScope};
use winter_model::paint::{Frame, Row};
use winter_model::vim::nav::{VimKey, VimNav};

use crate::table::{cell, fit, heading_row, marked, table_row, text};

use self::notice::StartupNotice;

// ========================================================================
// Constants
// ========================================================================

/// What the monitor is called in the pane title.
const TITLE: &str = "Startup Programs";

/// The questions the monitor asks.
const ASK_FILTER: &str = "startup-filter";
const ASK_SEARCH: &str = "startup-search";

/// The tag of a service turned on or off.
const TAG_TOGGLE: &str = "startup-toggle";

/// The pane height below which the footer describing the entry is left out.
const MIN_ROWS_FOR_DETAIL: usize = 16;

/// Column widths, in cells. A column is followed by one space.
const STATE_WIDTH: usize = 4;
const KIND_WIDTH: usize = 9;
const SCOPE_WIDTH: usize = 7;
const NAME_WIDTH: usize = 32;

/// The monitor's own keys. Reading again is `C-r` in every editor; `r`
/// stays as the short key.
const KEYS: KeyTable<Act> = KeyTable {
    block: "startup",
    bindings: &[
        Binding {
            action: Act::Refresh,
            id: "refresh",
            keys: &["r", "C-r", "M-S-r"],
            label: "Read Again",
        },
        Binding {
            action: Act::Open,
            id: "open",
            keys: &["RET"],
            label: "Open Its File",
        },
        Binding {
            action: Act::Toggle,
            id: "toggle",
            keys: &["t"],
            label: "Turn On / Off",
        },
        Binding {
            action: Act::Search,
            id: "search",
            keys: &["/"],
            label: "Search",
        },
        Binding {
            action: Act::NextMatch,
            id: "next_match",
            keys: &["n"],
            label: "Next Match",
        },
        Binding {
            action: Act::PreviousMatch,
            id: "previous_match",
            keys: &["N"],
            label: "Previous Match",
        },
        Binding {
            action: Act::Filter,
            id: "filter",
            keys: &["f"],
            label: "Filter…",
        },
        Binding {
            action: Act::ClearFilter,
            id: "clear_filter",
            keys: &["ESC"],
            label: "Clear the Filter",
        },
        Binding {
            action: Act::CopyName,
            id: "copy_name",
            keys: &["y"],
            label: "Copy Name",
        },
        Binding {
            action: Act::CopyCommand,
            id: "copy_command",
            keys: &["Y"],
            label: "Copy Command",
        },
    ],
    configurable: true,
    title: "Startup Programs",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

/// What the menu opened over an entry offers, a group at a time.
const MENU: [&[Act]; 3] = [
    &[Act::Open, Act::Toggle],
    &[Act::Refresh],
    &[Act::CopyName, Act::CopyCommand],
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the monitor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    ClearFilter,
    CopyCommand,
    CopyName,
    Filter,
    NextMatch,
    Open,
    PreviousMatch,
    Refresh,
    Search,
    Toggle,
}

/// A startup monitor.
#[derive(Debug)]
pub struct StartupEditor {
    /// Whether the pane is tall enough for the footer.
    detail: bool,
    entries: Vec<StartupEntry>,
    /// Why the last reading failed, until one succeeds.
    error: Option<String>,
    filter: String,
    /// The cursor over the rows, keyed by each entry's kind and name.
    list: ListView<String>,
    /// Whether a reading has arrived, so an empty list can say why.
    loaded: bool,
    nav: VimNav,
    notice: Option<StartupNotice>,
    /// Whether a reading is outstanding, so a second is not asked for.
    reading: bool,
    /// The entries shown, as indexes into `entries`, in order.
    rows: Vec<usize>,
    /// The service being turned on or off, and which.
    toggling: Option<(String, bool)>,
    view: ViewSlot<StartupEditor>,
}

// ========================================================================
// StartupEditor
// ========================================================================

impl StartupEditor {
    /// A monitor that reads nothing until it is first shown.
    pub fn new() -> Self {
        Self {
            detail: false,
            entries: Vec::new(),
            error: None,
            filter: String::new(),
            list: ListView::new(),
            loaded: false,
            nav: VimNav::new(),
            notice: None,
            reading: false,
            rows: Vec::new(),
            toggling: None,
            view: ViewSlot::new(),
        }
    }

    /// Ask for the list, unless it is being read already.
    fn read(&mut self) -> EditorOutcome {
        if self.reading {
            return EditorOutcome::Consumed;
        }
        self.reading = true;
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Startup))
    }

    /// Take in a reading that came back, or why none did.
    fn take(&mut self, result: Result<Vec<StartupEntry>, String>) {
        self.reading = false;
        match result {
            Ok(mut entries) => {
                entries.sort_by(|a, b| {
                    (a.kind, a.scope, a.name.to_lowercase()).cmp(&(
                        b.kind,
                        b.scope,
                        b.name.to_lowercase(),
                    ))
                });
                self.entries = entries;
                self.loaded = true;
                self.error = None;
                self.rebuild();
            }
            Err(why) => self.error = Some(why),
        }
    }

    /// Filter the list again, keeping the cursor on the same entry.
    fn rebuild(&mut self) {
        let filter = self.filter.to_lowercase();
        self.rows = (0..self.entries.len())
            .filter(|at| {
                filter.is_empty() || line_of(&self.entries[*at]).to_lowercase().contains(&filter)
            })
            .collect();
        let keys = self
            .rows
            .iter()
            .map(|at| key_of_entry(&self.entries[*at]))
            .collect();
        self.list.set_keys(keys);
    }

    /// The entry the cursor is on.
    fn selected(&self) -> Option<&StartupEntry> {
        self.entries.get(*self.rows.get(self.list.cursor())?)
    }

    /// Turn the selected service on or off: a user's own systemd unit, the
    /// one kind changed here; for the rest, say how.
    fn toggle(&mut self) -> EditorOutcome {
        let Some(entry) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let refusal = match (entry.kind, entry.scope) {
            (StartupKind::Systemd, StartupScope::User) => None,
            (StartupKind::Systemd, StartupScope::System) => Some(StartupNotice::NeedsRoot {
                name: entry.name.clone(),
                on: !entry.enabled,
            }),
            _ => Some(StartupNotice::NotHere),
        };
        if let Some(refusal) = refusal {
            self.notice = Some(refusal);
            return EditorOutcome::Consumed;
        }
        let (name, on) = (entry.name.clone(), !entry.enabled);
        let verb = if on { "enable" } else { "disable" };
        self.toggling = Some((name.clone(), on));
        EditorOutcome::Job(JobRequest::Command(CommandRequest {
            args: vec!["--user".to_string(), verb.to_string(), name],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            id: 0,
            program: "systemctl".to_string(),
            stdin: None,
            tag: TAG_TOGGLE,
        }))
    }

    /// What a search reads over, one label per row.
    fn search_labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|at| line_of(&self.entries[*at]))
            .collect()
    }

    /// Do what one of the monitor's own keys asks. `None` for a key with
    /// nothing to do now, which then reaches the motion layer.
    fn run(&mut self, act: Act) -> Option<EditorOutcome> {
        let outcome = match act {
            Act::Refresh => self.read(),
            Act::Open => match self.selected().and_then(|entry| entry.source.clone()) {
                Some(path) => EditorOutcome::OpenPath(OpenTarget {
                    column: None,
                    line: None,
                    path,
                }),
                None => {
                    self.notice = Some(StartupNotice::NoFile);
                    EditorOutcome::Consumed
                }
            },
            Act::Toggle => self.toggle(),
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::NextMatch | Act::PreviousMatch => {
                let labels = self.search_labels();
                if let Some(miss) = self.list.search_step(&labels, act == Act::NextMatch) {
                    self.notice = Some(StartupNotice::Search(miss));
                }
                EditorOutcome::Consumed
            }
            Act::Filter => EditorOutcome::Prompt(PromptRequest {
                initial: self.filter.clone(),
                label: "filter: ".to_string(),
                mode: PromptMode::Text,
                tag: ASK_FILTER,
                title: None,
            }),
            Act::ClearFilter if self.filter.is_empty() => return None,
            Act::ClearFilter => {
                self.filter.clear();
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::CopyName => match self.selected() {
                Some(entry) => EditorOutcome::Yank(entry.name.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::CopyCommand => match self.selected() {
                Some(entry) => EditorOutcome::Yank(entry.command.clone()),
                None => EditorOutcome::Consumed,
            },
        };
        Some(outcome)
    }

    // ====================================================================
    // Drawing
    // ====================================================================

    /// The summary at the heading's right: how many entries and how many on.
    fn summary(&self) -> Vec<Element<'_, Self>> {
        let on = self.entries.iter().filter(|entry| entry.enabled).count();
        let count = match self.loaded {
            false => "reading…".to_string(),
            true if self.filter.is_empty() => format!("{} entries, {on} on", self.entries.len()),
            true => format!("{} entries ({} shown)", self.entries.len(), self.rows.len()),
        };
        let mut parts = vec![text(count).muted().into()];
        if !self.filter.is_empty() {
            parts.push(
                badge(format!("filter: {}", self.filter), ThemeToken::Accent).tooltip(format!(
                    "Only entries matching {}; {} shows them all",
                    self.filter,
                    key_of(Act::ClearFilter)
                )),
            );
        }
        if let Some(why) = &self.error {
            parts.push(text(why.as_str()).color(ThemeToken::Danger).into());
        }
        parts
    }

    /// The column headings.
    fn heading_view(&self) -> Element<'_, Self> {
        let head = |label: &'static str, about: &'static str, width: Option<usize>| {
            cell(text(label).muted(), width, false).tooltip(about)
        };
        heading_row(vec![
            head("ON", "Whether it is set to start", Some(STATE_WIDTH)),
            head("KIND", "What starts it", Some(KIND_WIDTH)),
            head(
                "SCOPE",
                "Whether it is yours or the whole machine's",
                Some(SCOPE_WIDTH),
            ),
            head("NAME", "Its name", Some(NAME_WIDTH)),
            head("COMMAND", "What it runs", None),
        ])
    }

    /// Row `at` as the table shows it, the cursor's marked; a click puts the
    /// cursor on it.
    fn entry_view(&self, at: usize) -> Element<'_, Self> {
        let entry = &self.entries[self.rows[at]];
        let state = match entry.enabled {
            true => text("on").color(ThemeToken::Accent),
            false => text("off").muted(),
        };
        let line = table_row(vec![
            cell(state, Some(STATE_WIDTH), false),
            cell(text(kind_name(entry.kind)).muted(), Some(KIND_WIDTH), false),
            cell(
                text(scope_name(entry.scope)).muted(),
                Some(SCOPE_WIDTH),
                false,
            ),
            cell(text(fit(&entry.name, NAME_WIDTH)), Some(NAME_WIDTH), false),
            cell(text(entry.command.as_str()).muted(), None, false),
        ])
        .on_pointer_press(move |editor: &mut Self, _| {
            editor.list.view_mut().release();
            editor.list.set_cursor(at);
        });
        marked(line, at == self.list.cursor())
    }

    /// The footer, describing the entry the cursor is on.
    fn detail_view(&self) -> Option<Element<'_, Self>> {
        let entry = self.selected()?;
        let about = format!(
            "{} {} {}",
            if entry.enabled { "on" } else { "off" },
            scope_name(entry.scope),
            kind_name(entry.kind)
        );
        let mut lines = vec![
            row([
                text(entry.name.as_str()).emphasized().into(),
                badge(about, ThemeToken::Muted),
            ])
            .gap(Space::Cells(1.0)),
        ];
        if let Some(source) = &entry.source {
            lines.push(text(source.display().to_string()).muted().into());
        }
        lines.push(text(entry.command.as_str()).into());
        Some(column(lines).gap(Space::Cells(0.2)))
    }
}

impl LazySource<StartupEditor> for StartupEditor {
    fn count(&self) -> usize {
        self.rows.len()
    }

    fn row(&self, index: usize) -> Element<'_, StartupEditor> {
        self.entry_view(index)
    }
}

impl HostedView for StartupEditor {
    /// The summary over the headings and the entries, misty building those
    /// that show, and the selected entry in the footer where there is room.
    fn view(&self) -> Element<'_, Self> {
        let mut table = vec![self.heading_view(), divider()];
        if self.rows.is_empty() && self.loaded {
            table.push(text("nothing starts on its own").muted().into());
        }
        table.push(
            lazy_column(self, self.list.cursor())
                .on_shown(|editor: &mut Self, shown| editor.list.view_mut().set_shown(shown))
                .into(),
        );
        let mut heading = page(TITLE);
        for part in self.summary() {
            heading = heading.action(part);
        }
        let mut shown = heading.filled(column(table).gap(Space::Cells(0.0)));
        if let Some(detail) = self.detail.then(|| self.detail_view()).flatten() {
            shown = shown.footer(detail);
        }
        shown.into()
    }
}

impl OwnView for StartupEditor {
    fn view_slot(&mut self) -> &mut ViewSlot<Self> {
        &mut self.view
    }
}

impl Default for StartupEditor {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Page for StartupEditor {
    /// Read the first time it is shown, and not again until asked.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        match shown && !self.loaded {
            true => self.read(),
            false => EditorOutcome::Consumed,
        }
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn visit_viewports(&mut self, visit: &mut dyn FnMut(&mut Viewport)) {
        visit(self.list.view_mut());
    }

    /// Misty draws the page; the grid's height says whether the footer fits.
    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        self.detail = rows >= MIN_ROWS_FOR_DETAIL;
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.draw_own_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        self.own_view_input(input, cx)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        self.own_access_action(action)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.own_view_about_to_wait(cx)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(StartupNotice::into_notice)
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence()
            && let Some(outcome) = table::find(&KEYS, key).and_then(|act| self.run(act))
        {
            return outcome;
        }
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(&KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.list.view_mut().release();
        match (reply.tag, reply.answer) {
            (ASK_FILTER, Some(answer)) => {
                self.filter = answer.trim().to_string();
                self.rebuild();
            }
            (ASK_SEARCH, Some(answer)) => {
                let labels = self.search_labels();
                if let Some(miss) = self.list.search_for(&labels, answer.trim()) {
                    self.notice = Some(StartupNotice::Search(miss));
                }
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag != ASK_FILTER {
            return EditorOutcome::Ignored;
        }
        self.filter = query.trim().to_string();
        self.rebuild();
        EditorOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            JobReply::Monitor(MonitorAnswer::Startup(result)) => self.take(result),
            JobReply::Command(output) if output.tag == TAG_TOGGLE => {
                let Some((name, on)) = self.toggling.take() else {
                    return EditorOutcome::Consumed;
                };
                self.notice = Some(StartupNotice::Toggled {
                    failure: (!output.succeeded()).then(|| output.failure_detail()),
                    name,
                    on,
                });
                return self.read();
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // A reading asked for before the editor was covered was cancelled
        // with it: one never answered is asked for again.
        let unanswered = self.reading;
        self.reading = false;
        match unanswered {
            true => self.read(),
            false => EditorOutcome::Consumed,
        }
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        match self.selected() {
            Some(_) => menu_of(&KEYS, &MENU),
            None => Vec::new(),
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.selected()
            .and_then(|entry| entry.source.as_ref())
            .and_then(|source| source.parent())
            .map(PathBuf::from)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A kind as the table names it.
fn kind_name(kind: StartupKind) -> &'static str {
    match kind {
        StartupKind::Autostart => "autostart",
        StartupKind::Cron => "cron",
        StartupKind::Launchd => "launchd",
        StartupKind::Shell => "shell",
        StartupKind::Systemd => "systemd",
    }
}

/// A scope as the table names it.
fn scope_name(scope: StartupScope) -> &'static str {
    match scope {
        StartupScope::System => "machine",
        StartupScope::User => "user",
    }
}

/// An entry in one line: what a search and a filter read.
fn line_of(entry: &StartupEntry) -> String {
    format!(
        "{} {} {} {}",
        kind_name(entry.kind),
        scope_name(entry.scope),
        entry.name,
        entry.command
    )
}

/// What keeps the cursor on an entry across a reading.
fn key_of_entry(entry: &StartupEntry) -> String {
    format!(
        "{}:{}:{}",
        kind_name(entry.kind),
        scope_name(entry.scope),
        entry.name
    )
}

/// The key the monitor's own `act` is typed by, for a tooltip to name.
fn key_of(act: Act) -> String {
    table::menu_entry(&KEYS, act).map_or_else(String::new, |(key, _)| format_key(&key))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
