//! System monitor: the machine as a whole, with processor load per core,
//! memory and swap, GPUs, and disks, kept current on a timer. The sibling of
//! the process monitor, which lists what is using it.
//!
//! - [`collect`]: taking a snapshot, the one side-effecting module.
//! - [`parse`]: reading each system's figures.
//! - [`usage`]: processor load worked out from two snapshots.
//! - [`view`]: laying a snapshot out as a dashboard of cards, drawn by
//!   misty as the page's view.

pub mod collect;
pub mod parse;
pub mod usage;
pub mod view;

use std::time::{Duration, Instant};

use misty::view::{badge, page, text};
use misty::{Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, ScrollStep};
use misty::{Theme, ThemeToken};

use winter_model::editor::refresh::RefreshTimer;
use winter_model::editor::{
    EditorMenuItem, EditorOutcome, JobReply, JobRequest, Page, PageView, menu_of,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{CursorMove, Key};
use winter_model::paint::{Frame, Row};
use winter_model::system::SystemSample;
use winter_model::vim::nav::{VimKey, VimNav};

use self::usage::{Usage, UsageTracker};
use self::view::{History, card_ids, dashboard, machine_line, tenths_of};

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
const TITLE: &str = "System Monitor";

/// The id of the page's scrolling body, which the keys scroll.
const BODY: &str = "sys.body";

/// What the heading says of the readings, and what the page says before
/// the first.
const LIVE: &str = "live (2s)";
const PAUSED: &str = "paused";
const READING: &str = "reading the machine…";

/// How many readings the history lines hold: two minutes' worth.
const HISTORY_LEN: usize = 60;

/// How far past a section's top the body must be for `N` to go back to
/// that section rather than the one before it, in pixels.
const SECTION_SLACK: f32 = 1.0;

/// A half of a screen, for the half-page motions.
const HALF_PAGE: f32 = 0.5;

/// The monitor's own keys. Refreshing is `C-r` in every editor; `r` stays as
/// the short key.
const KEYS: KeyTable<Act> = KeyTable {
    block: "sys",
    bindings: &[
        Binding {
            action: Act::Refresh,
            id: "refresh",
            keys: &["r", "C-r"],
            label: "Refresh Now",
        },
        Binding {
            action: Act::Pause,
            id: "pause",
            keys: &["p"],
            label: "Pause / Resume",
        },
        Binding {
            action: Act::NextSection,
            id: "next_section",
            keys: &["n"],
            label: "Next Section",
        },
        Binding {
            action: Act::PreviousSection,
            id: "previous_section",
            keys: &["N"],
            label: "Previous Section",
        },
        Binding {
            action: Act::LineDown,
            id: "line_down",
            keys: &["C-e"],
            label: "Scroll a Line Down",
        },
        Binding {
            action: Act::LineUp,
            id: "line_up",
            keys: &["C-y"],
            label: "Scroll a Line Up",
        },
    ],
    configurable: true,
    title: "System",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

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
    /// The keys pressed so far of a motion (`g g`).
    nav: VimNav,
    /// The page's view and what it shows.
    view: PageView<Machine>,
}

/// What the monitor knows of the machine: the state its view is built
/// from.
#[derive(Debug)]
struct Machine {
    /// Why the last snapshot failed, until one succeeds.
    error: Option<String>,
    /// The processor's and memory's recent shares.
    history: History,
    sample: Option<SystemSample>,
    timer: RefreshTimer,
    tracker: UsageTracker,
    usage: Usage,
}

// ========================================================================
// SysEditor
// ========================================================================

impl SysEditor {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        let machine = Machine {
            error: None,
            history: History::default(),
            sample: None,
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            tracker: UsageTracker::new(),
            usage: Usage::default(),
        };
        Self {
            nav: VimNav::new(),
            view: PageView::new(machine),
        }
    }

    /// Scroll the body by `step`.
    fn scroll(&mut self, step: ScrollStep) {
        self.view.scroll(BODY, step);
    }

    /// Interpret a motion of the shared Vim layer over the body.
    fn apply_motion(&mut self, motion: CursorMove) {
        match motion {
            CursorMove::Down => self.scroll(ScrollStep::Rows(1.0)),
            CursorMove::Up => self.scroll(ScrollStep::Rows(-1.0)),
            CursorMove::Top => self.scroll(ScrollStep::Start),
            CursorMove::Bottom => self.scroll(ScrollStep::End),
            CursorMove::HalfPageDown => self.scroll(ScrollStep::Pages(HALF_PAGE)),
            CursorMove::HalfPageUp => self.scroll(ScrollStep::Pages(-HALF_PAGE)),
            CursorMove::PageDown => self.scroll(ScrollStep::Pages(1.0)),
            CursorMove::PageUp => self.scroll(ScrollStep::Pages(-1.0)),
            // The column motions and the rest mean nothing over an editor.
            _ => {}
        }
    }

    /// Bring the next card to the top of the body, or the one before the
    /// card at the top.
    fn jump_section(&mut self, forward: bool) {
        let Some(ids) = self.view.state().sample.as_ref().map(card_ids) else {
            return;
        };
        let Some(at) = self.view.offset(BODY) else {
            return;
        };
        let tops: Vec<(&str, f32)> = ids
            .into_iter()
            .filter_map(|id| Some((id, self.view.top_of(id)?)))
            .collect();
        let target = match forward {
            true => tops
                .iter()
                .filter(|(_, top)| *top > at + SECTION_SLACK)
                .min_by(by_top),
            false => tops
                .iter()
                .filter(|(_, top)| *top < at - SECTION_SLACK)
                .max_by(by_top),
        };
        if let Some((id, _)) = target {
            let id = *id;
            self.view.reveal_top(id);
        }
    }
}

/// Two cards in the order they stand down the body.
fn by_top(a: &&(&str, f32), b: &&(&str, f32)) -> std::cmp::Ordering {
    a.1.total_cmp(&b.1)
}

impl Machine {
    /// Take a snapshot that came back, or why none did.
    fn take(&mut self, result: Result<SystemSample, String>) {
        match result {
            Ok(sample) => {
                self.usage = self.tracker.usage(&sample.cpu);
                let memory = &sample.memory;
                let used = memory.total_kb.saturating_sub(memory.available_kb);
                push_capped(&mut self.history.cpu, tenths_of_percent(self.usage.total));
                push_capped(
                    &mut self.history.memory,
                    tenths_of_percent(tenths_of(used, memory.total_kb)),
                );
                self.sample = Some(sample);
                self.error = None;
            }
            Err(reason) => self.error = Some(reason),
        }
    }
}

impl HostedView for Machine {
    /// The heading saying whether the readings are live and any error,
    /// over the dashboard (or what the page waits on before the first).
    fn view(&self) -> Element<'_, Self> {
        let state = match self.timer.is_paused() {
            true => badge(PAUSED, ThemeToken::Warning),
            false => badge(LIVE, ThemeToken::Muted),
        };
        let mut shown = page(TITLE).scroll_id(BODY);
        if let Some(sample) = &self.sample {
            shown = shown.action(machine_line(sample));
        }
        shown = shown.action(state);
        if let Some(report) = &self.error {
            shown = shown.action(
                text(format!("\u{26a0} {report}"))
                    .color(ThemeToken::Danger)
                    .proportional(),
            );
        }
        match &self.sample {
            Some(sample) => shown.content(dashboard(sample, &self.usage, &self.history)),
            None => shown.content(
                text(self.error.as_deref().unwrap_or(READING))
                    .muted()
                    .proportional(),
            ),
        }
        .into()
    }
}

/// `value` after the others in `history`, the oldest let go past
/// [`HISTORY_LEN`].
fn push_capped(history: &mut Vec<f32>, value: f32) {
    if history.len() >= HISTORY_LEN {
        history.remove(0);
    }
    history.push(value);
}

/// A share in tenths of a percent as a percentage.
fn tenths_of_percent(tenths: u32) -> f32 {
    tenths as f32 / 10.0
}

impl Default for SysEditor {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Page for SysEditor {
    /// Not shown, the list stops refreshing; shown again, it refreshes at
    /// once.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.view
            .state_mut()
            .timer
            .set_hidden(!shown, Instant::now());
        EditorOutcome::Consumed
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty.
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        self.view
            .view_input(input, cx)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_access_action(&mut self, action: misty::AccessAction) -> Option<EditorOutcome> {
        self.view
            .access_action(action)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<std::time::Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.nav.in_sequence() {
            return self.on_motion_key(key);
        }
        match table::find(&KEYS, key) {
            Some(command) => self.run(command),
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(&KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        if let JobReply::System(result) = reply {
            let machine = self.view.state_mut();
            machine.timer.on_answer(Instant::now());
            machine.take(result);
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.view.state().timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if !self.view.state_mut().timer.on_tick(now) {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(JobRequest::System)
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // Work asked for before the editor was covered was cancelled with it.
        self.view.state_mut().timer.restart(Instant::now());
        EditorOutcome::Consumed
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        menu_of(&KEYS, &MENU)
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
                self.view.state_mut().timer.request_now(Instant::now());
                return EditorOutcome::Job(JobRequest::System);
            }
            Act::Pause => self.view.state_mut().timer.toggle_pause(Instant::now()),
            Act::NextSection => self.jump_section(true),
            Act::PreviousSection => self.jump_section(false),
            Act::LineDown => self.scroll(ScrollStep::Rows(1.0)),
            Act::LineUp => self.scroll(ScrollStep::Rows(-1.0)),
        }
        EditorOutcome::Consumed
    }

    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the body.
    fn on_motion_key(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.row_key(key) {
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
