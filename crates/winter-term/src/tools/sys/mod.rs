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

use crate::model::input::{CursorMove, Key, KeyCode, Mods};
use crate::model::paint::{Frame, Row, Span, Style};
use crate::model::system::SystemSample;
use crate::model::tool::{JobReply, JobRequest, Tool, ToolMenuItem, ToolOutcome};
use crate::model::vim::nav::{VimKey, VimNav};
use crate::tools::refresh::RefreshTimer;

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

/// The one-line key reminder after the title.
const HINTS: &str = "  r refresh  p pause  n/N section";

/// Rows above the scrolling body: the title line.
const HEADER_ROWS: usize = 1;

/// Rows a turn of the wheel moves the tool by.
const WHEEL_STEP: isize = 3;

/// What the menu opened over the tool calls each of the entries it offers.
const LABEL_NEXT: &str = "Next Section";
const LABEL_PAUSE: &str = "Pause / Resume";
const LABEL_PREVIOUS: &str = "Previous Section";
const LABEL_REFRESH: &str = "Refresh Now";

// ========================================================================
// Data Structures
// ========================================================================

/// A system monitor.
#[derive(Debug)]
pub struct SysTool {
    /// How tall the body was when last painted, so a motion knows how far a
    /// tool is and how far the end.
    body_len: usize,
    /// Why the last snapshot failed, until one succeeds.
    error: Option<String>,
    nav: VimNav,
    sample: Option<SystemSample>,
    scroll: usize,
    /// The body row each section starts on, for jumping between them.
    section_starts: Vec<usize>,
    timer: RefreshTimer,
    tracker: UsageTracker,
    usage: Usage,
    /// Rows the body had room for when last painted.
    viewport: usize,
}

// ========================================================================
// SysTool
// ========================================================================

impl SysTool {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        Self {
            body_len: 0,
            error: None,
            nav: VimNav::new(),
            sample: None,
            scroll: 0,
            section_starts: Vec::new(),
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            tracker: UsageTracker::new(),
            usage: Usage::default(),
            viewport: 0,
        }
    }

    /// Scroll by `delta` rows, stopping at either end.
    fn scroll_by(&mut self, delta: isize) {
        self.scroll_to(self.scroll.saturating_add_signed(delta));
    }

    /// Scroll to row `at`, or the last position that still fills the pane.
    fn scroll_to(&mut self, at: usize) {
        self.scroll = at.min(self.body_len.saturating_sub(self.viewport));
    }

    /// Interpret a motion of the shared Vim layer over the body.
    fn apply_motion(&mut self, motion: CursorMove) {
        let half = (self.viewport / 2).max(1) as isize;
        let screen = self.viewport.max(1) as isize;
        match motion {
            CursorMove::Down => self.scroll_by(1),
            CursorMove::Up => self.scroll_by(-1),
            CursorMove::Top => self.scroll_to(0),
            CursorMove::Bottom => self.scroll_to(usize::MAX),
            CursorMove::HalfPageDown => self.scroll_by(half),
            CursorMove::HalfPageUp => self.scroll_by(-half),
            CursorMove::PageDown => self.scroll_by(screen),
            CursorMove::PageUp => self.scroll_by(-screen),
            // The column motions and the rest mean nothing over a tool.
            _ => {}
        }
    }

    /// Jump to the start of the next section, or the previous one.
    fn jump_section(&mut self, forward: bool) {
        let target = if forward {
            self.section_starts
                .iter()
                .copied()
                .find(|start| *start > self.scroll)
        } else {
            self.section_starts
                .iter()
                .rev()
                .copied()
                .find(|start| *start < self.scroll)
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
        match &self.error {
            Some(report) => row.push(Span::new(Style::Accent, format!("  {report}"))),
            None => row.push(Span::new(Style::Dim, HINTS)),
        }
        row
    }
}

impl Default for SysTool {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tool
// ========================================================================

impl Tool for SysTool {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.viewport = rows.saturating_sub(HEADER_ROWS);
        let (body, starts) = self.body(cols);
        self.body_len = body.len();
        self.section_starts = starts;
        self.scroll = self.scroll.min(self.body_len.saturating_sub(self.viewport));
        let mut frame_rows = vec![self.title_row()];
        frame_rows.extend(body.into_iter().skip(self.scroll).take(self.viewport));
        Frame::new(frame_rows)
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        if self.nav.in_sequence() || key.alt {
            return self.on_motion_key(key);
        }
        if key.ctrl {
            return match (key.mods(), key.glyph()) {
                (Mods::CTRL, KeyCode::Char('e')) => {
                    self.scroll_by(1);
                    ToolOutcome::Consumed
                }
                (Mods::CTRL, KeyCode::Char('y')) => {
                    self.scroll_by(-1);
                    ToolOutcome::Consumed
                }
                _ => self.on_motion_key(key),
            };
        }
        match key.glyph() {
            KeyCode::Char('n') => {
                self.jump_section(true);
                ToolOutcome::Consumed
            }
            KeyCode::Char('N') => {
                self.jump_section(false);
                ToolOutcome::Consumed
            }
            KeyCode::Char('r') => {
                self.timer.request_now(Instant::now());
                ToolOutcome::Job(JobRequest::System)
            }
            KeyCode::Char('p') => {
                self.timer.toggle_pause(Instant::now());
                ToolOutcome::Consumed
            }
            _ => self.on_motion_key(key),
        }
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        self.scroll_by(-lines * WHEEL_STEP);
        ToolOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> ToolOutcome {
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
        ToolOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> ToolOutcome {
        if !self.timer.on_tick(now) {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Job(JobRequest::System)
    }

    fn on_resume(&mut self) -> ToolOutcome {
        // Work asked for before the tool was covered was cancelled with it.
        self.timer.restart(Instant::now());
        ToolOutcome::Consumed
    }

    fn context_items(&self) -> Vec<ToolMenuItem> {
        [
            (KeyCode::Char('r'), LABEL_REFRESH),
            (KeyCode::Char('p'), LABEL_PAUSE),
            (KeyCode::Char('n'), LABEL_NEXT),
            (KeyCode::Char('N'), LABEL_PREVIOUS),
        ]
        .into_iter()
        .map(|(code, label)| ToolMenuItem::new(Key::plain(code), label))
        .collect()
    }
}

// ========================================================================
// Helpers
// ========================================================================

impl SysTool {
    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the body.
    fn on_motion_key(&mut self, key: &Key) -> ToolOutcome {
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                ToolOutcome::Consumed
            }
            VimKey::Pending => ToolOutcome::Consumed,
            VimKey::Unhandled => ToolOutcome::Ignored,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::model::paint::row_text;
    use crate::model::system::{
        CoreLoad, CoreReading, CpuSample, CpuTicks, DiskReading, MemoryReading, Overview,
    };

    use super::*;

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    fn sample(busy: u64, total: u64) -> SystemSample {
        SystemSample {
            cpu: CpuSample {
                cores: vec![CoreReading {
                    load: CoreLoad::Ticks(CpuTicks { busy, total }),
                    mhz: 0,
                }],
                load: None,
                model: "Test CPU".to_string(),
                total: CoreLoad::Ticks(CpuTicks { busy, total }),
            },
            disks: vec![DiskReading {
                available_kb: 1,
                fs_type: None,
                mount: "/".to_string(),
                total_kb: 2,
                used_kb: 1,
            }],
            gpus: Vec::new(),
            memory: MemoryReading {
                available_kb: 1,
                swap_total_kb: 0,
                swap_used_kb: 0,
                total_kb: 2,
            },
            overview: Overview {
                arch: "x86_64".to_string(),
                hostname: "box".to_string(),
                os: "Linux".to_string(),
                release: "6.8".to_string(),
                uptime_secs: 60,
            },
        }
    }

    fn loaded_tool() -> SysTool {
        let mut tool = SysTool::new();
        tool.on_job(JobReply::System(Ok(sample(250, 1000))));
        tool
    }

    fn lines(content: &Frame) -> Vec<String> {
        content.rows.iter().map(row_text).collect()
    }

    #[test]
    fn test_the_first_tick_asks_for_a_snapshot() {
        let mut tool = SysTool::new();
        assert_eq!(
            tool.on_tick(Instant::now()),
            ToolOutcome::Job(JobRequest::System)
        );
    }

    #[test]
    fn test_a_snapshot_fills_the_tool_with_its_sections() {
        let mut tool = loaded_tool();
        let text = lines(&tool.content(40, 120, false)).join("\n");
        for heading in [
            "System Overview",
            "CPU Usage",
            "Memory & Swap",
            "Storage / Disks",
        ] {
            assert!(text.contains(heading), "missing {heading:?} in {text:?}");
        }
        assert!(
            text.contains("25.0%"),
            "the first reading is the share since boot"
        );
    }

    #[test]
    fn test_the_second_snapshot_shows_the_interval_not_the_whole_uptime() {
        let mut tool = loaded_tool();
        tool.on_job(JobReply::System(Ok(sample(260, 1100))));
        let text = lines(&tool.content(40, 120, false)).join("\n");
        // Ten busy ticks in a hundred; since boot it would still read 23.6%.
        assert!(text.contains("10.0%"), "got {text:?}");
    }

    #[test]
    fn test_a_failed_snapshot_is_shown_and_retried() {
        let mut tool = SysTool::new();
        tool.on_tick(Instant::now());
        tool.on_job(JobReply::System(Err("df: not found".to_string())));
        let content = tool.content(10, 80, false);
        assert!(lines(&content).join("\n").contains("df: not found"));
        assert!(tool.next_tick().is_some());
    }

    #[test]
    fn test_sections_are_jumped_between_with_n_and_back_with_capital_n() {
        let mut tool = loaded_tool();
        // A pane too short for the whole tool, so there is somewhere to go.
        tool.content(6, 120, false);
        assert!(tool.section_starts.len() >= 3);
        tool.on_key(&press(KeyCode::Char('n')));
        assert_eq!(
            tool.scroll,
            tool.section_starts[1].min(tool.body_len - tool.viewport)
        );
        let after_first = tool.scroll;
        tool.on_key(&press(KeyCode::Char('n')));
        assert!(tool.scroll > after_first);
        tool.on_key(&press(KeyCode::Char('N')));
        assert_eq!(tool.scroll, after_first);
    }

    #[test]
    fn test_scrolling_stops_at_the_last_row_that_still_fills_the_pane() {
        let mut tool = loaded_tool();
        tool.content(6, 120, false);
        tool.on_key(&press(KeyCode::Char('G')));
        assert_eq!(tool.scroll, tool.body_len - tool.viewport);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(tool.scroll, tool.body_len - tool.viewport);
        tool.on_key(&press(KeyCode::Char('g')));
        tool.on_key(&press(KeyCode::Char('g')));
        assert_eq!(tool.scroll, 0);
    }

    #[test]
    fn test_a_pane_taller_than_the_tool_never_scrolls() {
        let mut tool = loaded_tool();
        tool.content(80, 120, false);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(tool.scroll, 0);
    }

    #[test]
    fn test_refresh_asks_at_once_and_pause_stops_the_asking() {
        let mut tool = loaded_tool();
        assert_eq!(
            tool.on_key(&press(KeyCode::Char('r'))),
            ToolOutcome::Job(JobRequest::System)
        );
        tool.on_key(&press(KeyCode::Char('p')));
        assert_eq!(tool.on_tick(Instant::now()), ToolOutcome::Consumed);
        assert!(row_text(&tool.title_row()).contains("paused"));
    }

    #[test]
    fn test_no_plain_key_closes_the_tool() {
        // `q` and `Esc` sit too close to the keys a reader reaches for to be
        // ways out; the window's close chord is.
        let mut tool = loaded_tool();
        assert_ne!(tool.on_key(&press(KeyCode::Char('q'))), ToolOutcome::Close);
        assert_ne!(tool.on_key(&press(KeyCode::Escape)), ToolOutcome::Close);
    }
}
