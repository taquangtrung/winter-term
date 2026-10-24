//! System monitor: the machine as a whole, with processor load per core,
//! memory and swap, GPUs, and disks, kept current on a timer. The sibling of
//! the process monitor, which lists what is using it.
//!
//! - [`collect`]: taking a snapshot, the one side-effecting module.
//! - [`parse`]: reading each system's figures.
//! - [`usage`]: processor load worked out from two snapshots.
//! - [`view`]: laying a snapshot out as sections of rows.

pub mod collect;
pub mod parse;
pub mod usage;
pub mod view;

use std::time::{Duration, Instant};

use crate::editors::list::Viewport;
use crate::editors::refresh::RefreshTimer;
use crate::model::editor::{
    menu_groups, Editor, EditorMenuItem, EditorOutcome, JobReply, JobRequest,
};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{CursorMove, Key};
use crate::model::paint::{Frame, Row, Span, Style};
use crate::model::system::SystemSample;
use crate::model::vim::nav::{VimKey, VimNav};

use self::usage::{Usage, UsageTracker};
use self::view::sections;

// ========================================================================
// Constants
// ========================================================================

/// How long after a snapshot arrives the next is asked for. Long enough for a
/// processor share to mean something, short enough to feel live.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long a snapshot may be outstanding before it is given up on and asked
/// for again. The runner drops a request when it is busy.
const STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// What the monitor is called in the pane title.
const TITLE: &str = "System";

/// Rows above the scrolling body: the title line.
const HEADER_ROWS: usize = 1;

/// Rows a turn of the wheel moves the editor by.
const WHEEL_STEP: isize = 3;

/// The monitor's own keys. Refreshing is `C-r` in every editor; `r` stays as
/// the short key.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Refresh,
        keys: &["r", "C-r"],
        label: "Refresh Now",
    },
    Binding {
        action: Act::Pause,
        keys: &["p"],
        label: "Pause / Resume",
    },
    Binding {
        action: Act::NextSection,
        keys: &["n"],
        label: "Next Section",
    },
    Binding {
        action: Act::PreviousSection,
        keys: &["N"],
        label: "Previous Section",
    },
    Binding {
        action: Act::LineDown,
        keys: &["C-e"],
        label: "Scroll a Line Down",
    },
    Binding {
        action: Act::LineUp,
        keys: &["C-y"],
        label: "Scroll a Line Up",
    },
];

/// What the menu opened over the editor offers, in its order, a group at a
/// time: the readings, then moving between sections.
const MENU: [&[Act]; 2] = [
    &[Act::Refresh, Act::Pause],
    &[Act::NextSection, Act::PreviousSection],
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the monitor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    LineDown,
    LineUp,
    NextSection,
    Pause,
    PreviousSection,
    Refresh,
}

/// A system monitor.
#[derive(Debug)]
pub struct SysEditor {
    /// How tall the body was when last painted, so a motion knows how far a
    /// editor is and how far the end.
    body_len: usize,
    /// Why the last snapshot failed, until one succeeds.
    error: Option<String>,
    nav: VimNav,
    sample: Option<SystemSample>,
    /// The body row each section starts on, for jumping between them.
    section_starts: Vec<usize>,
    timer: RefreshTimer,
    tracker: UsageTracker,
    usage: Usage,
    /// The window over the body.
    view: Viewport,
}

// ========================================================================
// SysEditor
// ========================================================================

impl SysEditor {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        Self {
            body_len: 0,
            error: None,
            nav: VimNav::new(),
            sample: None,
            section_starts: Vec::new(),
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            tracker: UsageTracker::new(),
            usage: Usage::default(),
            view: Viewport::new(),
        }
    }

    /// Scroll by `delta` rows, stopping at either end.
    fn scroll_by(&mut self, delta: isize) {
        self.view.scroll_by(delta, self.body_len);
    }

    /// Scroll to row `at`, or the last position that still fills the pane.
    fn scroll_to(&mut self, at: usize) {
        self.view.scroll_to(at, self.body_len);
    }

    /// Interpret a motion of the shared Vim layer over the body.
    fn apply_motion(&mut self, motion: CursorMove) {
        let half = (self.view.rows() / 2).max(1) as isize;
        let screen = self.view.rows().max(1) as isize;
        match motion {
            CursorMove::Down => self.scroll_by(1),
            CursorMove::Up => self.scroll_by(-1),
            CursorMove::Top => self.scroll_to(0),
            CursorMove::Bottom => self.scroll_to(usize::MAX),
            CursorMove::HalfPageDown => self.scroll_by(half),
            CursorMove::HalfPageUp => self.scroll_by(-half),
            CursorMove::PageDown => self.scroll_by(screen),
            CursorMove::PageUp => self.scroll_by(-screen),
            // The column motions and the rest mean nothing over an editor.
            _ => {}
        }
    }

    /// Jump to the start of the next section, or the previous one.
    fn jump_section(&mut self, forward: bool) {
        let target = if forward {
            self.section_starts
                .iter()
                .copied()
                .find(|start| *start > self.view.scroll())
        } else {
            self.section_starts
                .iter()
                .rev()
                .copied()
                .find(|start| *start < self.view.scroll())
        };
        if let Some(start) = target {
            self.scroll_to(start);
        }
    }

    /// The rows under the title, with the row each section starts on.
    fn body(&self, cols: usize) -> (Vec<Row>, Vec<usize>) {
        let Some(sample) = &self.sample else {
            let note = self.error.as_deref().unwrap_or("reading the machine...");
            return (vec![vec![Span::new(Style::Dim, note)]], vec![0]);
        };
        let mut rows = Vec::new();
        let mut starts = Vec::new();
        for section in sections(sample, &self.usage, cols) {
            starts.push(rows.len());
            rows.push(vec![Span::new(Style::HeaderAccent, section.title)]);
            rows.extend(section.rows);
            rows.push(Row::new());
        }
        (rows, starts)
    }

    /// The first row: the title, and what the last key or snapshot said.
    fn title_row(&self) -> Row {
        let mut title = TITLE.to_string();
        if self.timer.is_paused() {
            title.push_str("  paused");
        }
        let mut row = vec![Span::new(Style::Header, title)];
        if let Some(report) = &self.error {
            row.push(Span::new(Style::Accent, format!("  {report}")));
        }
        row
    }
}

impl Default for SysEditor {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Editor for SysEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.view.set_rows(rows.saturating_sub(HEADER_ROWS));
        let (body, starts) = self.body(cols);
        self.body_len = body.len();
        self.section_starts = starts;
        self.scroll_to(self.view.scroll());
        let mut frame_rows = vec![self.title_row()];
        frame_rows.extend(
            body.into_iter()
                .skip(self.view.scroll())
                .take(self.view.rows()),
        );
        Frame::new(frame_rows)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.nav.in_sequence() || key.alt {
            return self.on_motion_key(key);
        }
        match table::find(KEYS, key) {
            Some(command) => self.run(command),
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.scroll_by(-lines * WHEEL_STEP);
        EditorOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        if let JobReply::System(result) = reply {
            self.timer.on_answer(Instant::now());
            match result {
                Ok(sample) => {
                    self.usage = self.tracker.usage(&sample.cpu);
                    self.sample = Some(sample);
                    self.error = None;
                }
                Err(reason) => self.error = Some(reason),
            }
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if !self.timer.on_tick(now) {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(JobRequest::System)
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // Work asked for before the editor was covered was cancelled with it.
        self.timer.restart(Instant::now());
        EditorOutcome::Consumed
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        menu_groups(MENU.into_iter().map(|group| {
            group
                .iter()
                .filter_map(|&command| table::menu_entry(KEYS, command))
                .map(|(key, label)| EditorMenuItem::new(key, label))
                .collect()
        }))
    }
}

// ========================================================================
// Helpers
// ========================================================================

impl SysEditor {
    /// Do what one of the monitor's own keys asks.
    fn run(&mut self, command: Act) -> EditorOutcome {
        match command {
            Act::Refresh => {
                self.timer.request_now(Instant::now());
                return EditorOutcome::Job(JobRequest::System);
            }
            Act::Pause => self.timer.toggle_pause(Instant::now()),
            Act::NextSection => self.jump_section(true),
            Act::PreviousSection => self.jump_section(false),
            Act::LineDown => self.scroll_by(1),
            Act::LineUp => self.scroll_by(-1),
        }
        EditorOutcome::Consumed
    }

    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the body.
    fn on_motion_key(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
