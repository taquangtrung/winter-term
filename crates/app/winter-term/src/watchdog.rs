//! The window's watchdog, in two parts.
//!
//! - **Hangs**, always: misty's watchdog thread tells [`hang_watch`] of a
//!   callback that has not returned after [`HANG_LIMIT`]. The first telling
//!   writes the report, the page that was drawing ([`drawing`]), the slow
//!   turns before it, and every thread's state and stack to the crash log
//!   ([`crate::diagnose::threads`]), so a window that froze leaves something
//!   to read. One still stuck after [`RESTART_AFTER`] starts Winter again
//!   ([`restart`]), its programs in mux sessions living on, unless it was
//!   itself started after a hang within [`RESTART_GUARD`], so a page that
//!   hangs each time cannot start Winter over and over.
//! - **Slow turns**, while profiling is on ([`set_watching`], from
//!   [`crate::diagnose::set_profiling`]): each turn of the event loop that
//!   runs past [`SLOW_TURN`] is kept (what it handled, how long, the tab it
//!   ended on), the last [`MAX_SLOW_TURNS`] of them, for a crash or `winter
//!   profile` to show what the window was doing; one past [`LOGGED_TURN`] is
//!   logged too. Off, marking a turn is one atomic read.
//!
//! Nothing is watched until the program starts it ([`start`]), so tests
//! never write to the user's crash log or start Winter again.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use misty::{HangReport, HangWatch};

// ========================================================================
// Constants
// ========================================================================

/// How long one callback may run before the window counts as stuck: far
/// past any frame, short of the user giving up on the window.
const HANG_LIMIT: Duration = Duration::from_secs(5);

/// How long the window stays stuck before Winter is started again: a load
/// that is only slow has long finished, and the user has long given up.
const RESTART_AFTER: Duration = Duration::from_secs(60);

/// How soon after a restart for a hang another hang ends in no restart:
/// the page that hung likely reopened and hung again.
const RESTART_GUARD: Duration = Duration::from_secs(10 * 60);

/// Set in the environment of a Winter started again after a hang, to the
/// seconds since the epoch it was started at.
const RESTARTED_VAR: &str = "WINTER_RESTARTED_AFTER_HANG";

/// How long a turn runs before it is kept as slow: several frames' worth,
/// a pause the user can feel.
const SLOW_TURN: Duration = Duration::from_millis(100);

/// How long a turn runs before it is logged too.
const LOGGED_TURN: Duration = Duration::from_secs(1);

/// How many slow turns are kept, the oldest dropped first.
const MAX_SLOW_TURNS: usize = 200;

/// How many kinds of work one turn records.
const MAX_KINDS: usize = 8;

/// What a report says of the turns while profiling is off.
const NOT_WATCHING: &str = "(profiling is off: turn it on with Profiling: Profile the Window (Toggle), \
                            or launch Winter with `winter --profile`)\n";

/// The watchdog, once the program starts it.
static WATCH: OnceLock<Arc<Watch>> = OnceLock::new();

/// The page drawing now, as [`winter_model::editor::Page::type_name`]
/// names it.
static DRAWING: Mutex<Option<&'static str>> = Mutex::new(None);

// ========================================================================
// Data Structures
// ========================================================================

/// The window's turns, shared with the thread a hang is told on.
struct Watch {
    /// Whether it watches: profiling is on.
    watching: AtomicBool,
    /// The turn under way and the slow ones kept.
    state: Mutex<State>,
}

/// What is done about a hang, as it is told again and again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HangAction {
    /// Write the report to the crash log.
    Report,
    /// Start Winter again.
    Restart,
    /// Nothing more: it was reported, and a restart is not due or not
    /// allowed.
    Wait,
}

/// The turn under way and the slow ones kept.
#[derive(Default)]
struct State {
    /// The slow turns kept, oldest first.
    slow: VecDeque<SlowTurn>,
    /// The turn under way.
    turn: Turn,
}

/// The window's turn under way, if any.
#[derive(Default)]
struct Turn {
    /// When it began, `None` while the window waits.
    began: Option<Instant>,
    /// What it has handled so far (`input: key`, `render`), each once.
    kinds: Vec<&'static str>,
}

/// A turn that ran past [`SLOW_TURN`].
struct SlowTurn {
    /// When it ended.
    at: DateTime<Local>,
    /// What the window showed as it ended: the focused tab.
    detail: String,
    /// What it handled.
    kinds: Vec<&'static str>,
    /// How long it ran.
    took: Duration,
}

// ========================================================================
// Watch Implementations
// ========================================================================

impl Watch {
    /// A watch with the window waiting.
    fn new() -> Self {
        Self {
            watching: AtomicBool::new(false),
            state: Mutex::new(State::default()),
        }
    }

    /// Watch from now on, or stop: a turn under way is let go, its end
    /// unreported.
    fn set_watching(&self, on: bool) {
        self.watching.store(on, Ordering::Relaxed);
        if !on && let Ok(mut state) = self.state.lock() {
            state.turn = Turn::default();
        }
    }

    /// Mark a turn handling `kind` begun, or `kind` added to the one under
    /// way.
    fn busy(&self, kind: &'static str) {
        if !self.watching.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let turn = &mut state.turn;
        if turn.began.is_none() {
            *turn = Turn {
                began: Some(Instant::now()),
                kinds: vec![kind],
            };
        } else if !turn.kinds.contains(&kind) && turn.kinds.len() < MAX_KINDS {
            turn.kinds.push(kind);
        }
    }

    /// Mark the window waiting, keeping the turn when it was slow, with
    /// `detail` asked for only then.
    fn idle(&self, detail: impl FnOnce() -> String) {
        if !self.watching.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let turn = std::mem::take(&mut state.turn);
        drop(state);
        let Some(began) = turn.began else {
            return;
        };
        let took = began.elapsed();
        if took < SLOW_TURN {
            return;
        }
        let slow = SlowTurn {
            at: Local::now(),
            detail: detail(),
            kinds: turn.kinds,
            took,
        };
        if took >= LOGGED_TURN {
            log::warn!("a slow turn of the window: {}", slow.describe());
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.slow.len() == MAX_SLOW_TURNS {
            state.slow.pop_front();
        }
        state.slow.push_back(slow);
    }

    /// The slow turns kept, oldest first, one per line, then the turn
    /// under way when it has run past [`SLOW_TURN`].
    fn describe(&self) -> String {
        let Ok(state) = self.state.lock() else {
            return "(the watchdog failed)\n".to_string();
        };
        let mut out = String::new();
        for slow in &state.slow {
            let _ = writeln!(out, "{}", slow.describe());
        }
        if let Some(began) = state.turn.began
            && began.elapsed() >= SLOW_TURN
        {
            let _ = writeln!(
                out,
                "now, for {:.2}s: {}",
                began.elapsed().as_secs_f64(),
                state.turn.kinds.join(", ")
            );
        }
        match out.is_empty() {
            true => "(none)\n".to_string(),
            false => out,
        }
    }
}

// ========================================================================
// SlowTurn Implementations
// ========================================================================

impl SlowTurn {
    /// One line: when it ended, how long it ran, what it handled, and the
    /// tab it ended on.
    fn describe(&self) -> String {
        format!(
            "{} {:>6.2}s {} (in {})",
            self.at.format("%H:%M:%S%.3f"),
            self.took.as_secs_f64(),
            self.kinds.join(", "),
            self.detail
        )
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Start the watchdog: the program calls it once as the window starts,
/// and nothing else does. Until then [`hang_watch`] watches nothing.
pub fn start() {
    let _ = WATCH.set(Arc::new(Watch::new()));
}

/// What misty does about a hang: report it, and start Winter again if it
/// lasts, as the module says; nothing before [`start`].
pub fn hang_watch() -> HangWatch {
    match WATCH.get() {
        Some(_) => HangWatch::new().limit(HANG_LIMIT).on_hang(on_hang),
        None => HangWatch::off(),
    }
}

/// Name `page` as the one drawing, or none once it is drawn: what a hang's
/// report says the window was doing.
pub(crate) fn drawing(page: Option<&'static str>) {
    if let Ok(mut drawing) = DRAWING.lock() {
        *drawing = page;
    }
}

/// Mark a turn of the window's event loop handling `kind` begun, or `kind`
/// added to the turn under way: called as each handler starts.
pub fn busy(kind: &'static str) {
    if let Some(watch) = WATCH.get() {
        watch.busy(kind);
    }
}

/// Mark the window waiting for its next event: called last before it
/// waits. `detail`, the focused tab, is asked for only for a slow turn.
pub fn idle(detail: impl FnOnce() -> String) {
    if let Some(watch) = WATCH.get() {
        watch.idle(detail);
    }
}

/// The slow turns kept and the one under way, for a report.
pub(crate) fn recent_turns() -> String {
    match WATCH.get() {
        Some(watch) if watch.watching.load(Ordering::Relaxed) => watch.describe(),
        Some(_) | None => NOT_WATCHING.to_string(),
    }
}

/// Watch the window's turns from now on, or stop: only turning profiling on
/// or off calls it.
pub(crate) fn set_watching(on: bool) {
    if let Some(watch) = WATCH.get() {
        watch.set_watching(on);
    }
}

/// Do what [`hang_action`] says of `report`, on misty's watchdog thread.
fn on_hang(report: &HangReport) {
    match hang_action(report, restarted_ago()) {
        HangAction::Report => report_hang(report),
        HangAction::Restart => restart(report),
        HangAction::Wait => {}
    }
}

/// What is done about `report`, in a Winter started again after a hang
/// `restarted_ago`, if it was: reported the first time it is told, Winter
/// started again once it has lasted [`RESTART_AFTER`], unless that would
/// be a second restart within [`RESTART_GUARD`].
fn hang_action(report: &HangReport, restarted_ago: Option<Duration>) -> HangAction {
    if report.repeats == 0 {
        return HangAction::Report;
    }
    let recently = restarted_ago.is_some_and(|ago| ago < RESTART_GUARD);
    match report.stuck_for >= RESTART_AFTER && !recently {
        true => HangAction::Restart,
        false => HangAction::Wait,
    }
}

/// Write `report` to the crash log: the page that was drawing, the slow
/// turns before it, and every thread's state and stack.
fn report_hang(report: &HangReport) {
    log::warn!("{report}; its threads go to the crash log");
    let page = DRAWING
        .lock()
        .ok()
        .and_then(|drawing| *drawing)
        .unwrap_or("(none)");
    let body = format!(
        "Drawing: {page}\nRecent slow turns:\n{}\nThreads:\n{}",
        recent_turns(),
        crate::diagnose::threads()
    );
    winter_base::crash_log::append_entry(&report.to_string(), &body);
}

/// How long ago this Winter was started again after a hang, if it was.
fn restarted_ago() -> Option<Duration> {
    let at: u64 = std::env::var(RESTARTED_VAR).ok()?.parse().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(now.saturating_sub(Duration::from_secs(at)))
}

/// Start Winter again after `report`'s hang, with the arguments this one
/// was given: on Unix by replacing this image, elsewhere by starting
/// another and ending this one. The stuck window's programs end with it;
/// those in mux sessions live on.
fn restart(report: &HangReport) {
    let heading = format!("{report}; starting Winter again");
    winter_base::crash_log::append_entry(&heading, "");
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs());
    let mut command = std::process::Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .env(RESTARTED_VAR, now.to_string());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        log::error!("Winter could not start again after a hang: {error}");
    }
    #[cfg(not(unix))]
    {
        if command.spawn().is_ok() {
            std::process::exit(1);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::thread;

    use misty::HangPhase;

    use super::*;

    /// A hang drawing for `stuck_for`, told `repeats` times before.
    fn hang(stuck_for: Duration, repeats: u32) -> HangReport {
        HangReport::new(HangPhase::Drawing, stuck_for, repeats)
    }

    #[test]
    fn test_a_hang_is_reported_once_then_restarts_winter_only_once_it_lasts() {
        assert_eq!(hang_action(&hang(HANG_LIMIT, 0), None), HangAction::Report);
        assert_eq!(
            hang_action(&hang(HANG_LIMIT * 2, 1), None),
            HangAction::Wait
        );
        assert_eq!(
            hang_action(&hang(RESTART_AFTER, 11), None),
            HangAction::Restart
        );
    }

    #[test]
    fn test_a_hang_soon_after_a_restart_for_one_is_reported_but_never_restarts() {
        let soon = Some(RESTART_GUARD / 2);
        assert_eq!(hang_action(&hang(HANG_LIMIT, 0), soon), HangAction::Report);
        assert_eq!(
            hang_action(&hang(RESTART_AFTER, 11), soon),
            HangAction::Wait
        );
        let later = Some(RESTART_GUARD * 2);
        assert_eq!(
            hang_action(&hang(RESTART_AFTER, 11), later),
            HangAction::Restart
        );
    }

    #[test]
    fn test_nothing_is_watched_before_the_program_starts_it() {
        // Tests never start it, so no test can write the crash log or
        // start Winter again.
        assert!(WATCH.get().is_none());
        assert_eq!(
            format!("{:?}", hang_watch()),
            format!("{:?}", HangWatch::off())
        );
    }

    #[test]
    fn test_nothing_is_kept_while_profiling_is_off() {
        let watch = Watch::new();
        watch.busy("input");
        thread::sleep(SLOW_TURN + SLOW_TURN / 4);
        watch.idle(String::new);
        assert_eq!(watch.describe(), "(none)\n", "no slow turn kept");
    }

    #[test]
    fn test_a_slow_turn_is_kept_with_what_it_handled_and_a_quick_one_never_asks_for_detail() {
        let watch = Watch::new();
        watch.set_watching(true);
        let asked = AtomicBool::new(false);
        watch.busy("input: key");
        watch.idle(|| {
            asked.store(true, Ordering::SeqCst);
            String::new()
        });
        assert!(
            !asked.load(Ordering::SeqCst),
            "a quick turn asks for nothing"
        );
        assert_eq!(watch.describe(), "(none)\n");
        watch.busy("input: key");
        watch.busy("render");
        watch.busy("input: key");
        thread::sleep(SLOW_TURN + SLOW_TURN / 4);
        watch.idle(|| "terminal running claude".to_string());
        let kept = watch.describe();
        assert!(
            kept.contains("input: key, render (in terminal running claude)"),
            "{kept}"
        );
    }
}
