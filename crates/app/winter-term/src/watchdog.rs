//! The window's watchdog. Each turn of the event loop that runs past
//! [`SLOW_TURN`] is kept (what it handled, how long, the tab it ended on),
//! the last [`MAX_SLOW_TURNS`] of them, for a freeze, a crash, or `winter
//! profile` to show what the window was doing; one past [`LOGGED_TURN`] is
//! logged too. One past [`STALL`] writes every thread's state and stack to
//! the crash log ([`crate::diagnose::threads`]), so a window that froze
//! leaves something to read after it is closed; and when it answers again,
//! how long it was stuck.
//!
//! It watches only while profiling is on ([`set_watching`], from
//! [`crate::diagnose::set_profiling`]); off, marking a turn is one atomic
//! read. It is passive. The window marks each turn busy as it begins and
//! idle as it goes to wait ([`busy`], [`idle`]); while idle the watchdog's
//! thread sleeps until woken, so a quiet window costs nothing, and while
//! busy it wakes at most once per [`STALL`]. Nothing runs until the program
//! starts it ([`start`]), so tests never write to the user's crash log.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

// ========================================================================
// Constants
// ========================================================================

/// How long one turn of the event loop may run before the window counts as
/// stuck: far past any frame, short of the user giving up on the window.
const STALL: Duration = Duration::from_secs(5);

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

/// The watchdog's thread name, as `/proc` and a debugger show it.
const THREAD_NAME: &str = "winter-watchdog";

/// The watchdog, once the program starts it.
static WATCH: OnceLock<Arc<Watch>> = OnceLock::new();

// ========================================================================
// Data Structures
// ========================================================================

/// The window's turns, shared with the watchdog's thread.
struct Watch {
    /// Whether it watches: profiling is on.
    watching: AtomicBool,
    /// The turn under way and the slow ones kept.
    state: Mutex<State>,
    /// Wakes the watchdog's thread when a turn begins, or a stuck one ends.
    changed: Condvar,
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
    /// Whether it was reported stuck, so its end is worth waking for.
    stuck: bool,
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
            changed: Condvar::new(),
        }
    }

    /// Watch from now on, or stop: a turn under way is let go, its end
    /// unreported.
    fn set_watching(&self, on: bool) {
        self.watching.store(on, Ordering::Relaxed);
        if !on && let Ok(mut state) = self.state.lock() {
            state.turn = Turn::default();
            self.changed.notify_one();
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
                stuck: false,
            };
            self.changed.notify_one();
        } else if !turn.kinds.contains(&kind) && turn.kinds.len() < MAX_KINDS {
            turn.kinds.push(kind);
        }
    }

    /// Mark the window waiting, keeping the turn when it was slow, with
    /// `detail` asked for only then. The watchdog is woken only when it
    /// waits on a stuck turn to end: an ordinary turn's end costs nothing.
    fn idle(&self, detail: impl FnOnce() -> String) {
        if !self.watching.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let turn = std::mem::take(&mut state.turn);
        if turn.stuck {
            self.changed.notify_one();
        }
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

    /// Watch turns for good: `stuck` with how long the window has been on
    /// one turn once it passes `stall`, then `answered` with how long it was
    /// stuck once that turn ends.
    fn run(&self, stall: Duration, stuck: impl Fn(Duration), answered: impl Fn(Duration)) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        loop {
            // Asleep until a turn begins.
            let Some(began) = state.turn.began else {
                let Ok(woken) = self.changed.wait(state) else {
                    return;
                };
                state = woken;
                continue;
            };
            let left = stall.saturating_sub(began.elapsed());
            if !left.is_zero() {
                let Ok((woken, _)) = self.changed.wait_timeout(state, left) else {
                    return;
                };
                state = woken;
                continue;
            }
            // The same turn, past the stall: report it once, unlocked, so
            // the window can end its turn meanwhile.
            state.turn.stuck = true;
            drop(state);
            stuck(began.elapsed());
            let Ok(mut held) = self.state.lock() else {
                return;
            };
            while held.turn.began == Some(began) {
                let Ok(woken) = self.changed.wait(held) else {
                    return;
                };
                held = woken;
            }
            answered(began.elapsed());
            state = held;
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

/// Start the watchdog on its own thread: the program calls it once as the
/// window starts, and nothing else does.
pub fn start() {
    let watch = Arc::new(Watch::new());
    if WATCH.set(Arc::clone(&watch)).is_err() {
        return;
    }
    let spawned = thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || watch.run(STALL, report_stuck, report_answered));
    if let Err(error) = spawned {
        log::warn!("the window's watchdog did not start: {error}");
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

/// Write the window found stuck for `held` to the crash log, with the slow
/// turns before it and every thread's state and stack.
fn report_stuck(held: Duration) {
    log::warn!(
        "the window has not answered for {:.1}s; its threads go to the crash log",
        held.as_secs_f64()
    );
    let heading = format!("the window has not answered for {:.1}s", held.as_secs_f64());
    let body = format!(
        "Recent slow turns:\n{}\nThreads:\n{}",
        recent_turns(),
        crate::diagnose::threads()
    );
    winter_base::crash_log::append_entry(&heading, &body);
}

/// Write the window answering again after `held` to the crash log.
fn report_answered(held: Duration) {
    let heading = format!("the window answered again after {:.1}s", held.as_secs_f64());
    winter_base::crash_log::append_entry(&heading, "");
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    /// How long a test's stall is.
    const TEST_STALL: Duration = Duration::from_millis(40);

    /// A watch running on its own thread with `TEST_STALL`, and how many
    /// times it reported the window stuck and answering.
    fn running() -> (Arc<Watch>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let watch = Arc::new(Watch::new());
        watch.set_watching(true);
        let stuck = Arc::new(AtomicUsize::new(0));
        let answered = Arc::new(AtomicUsize::new(0));
        let (runner, stuck_count, answered_count) = (
            Arc::clone(&watch),
            Arc::clone(&stuck),
            Arc::clone(&answered),
        );
        thread::spawn(move || {
            runner.run(
                TEST_STALL,
                |_| {
                    stuck_count.fetch_add(1, Ordering::SeqCst);
                },
                |_| {
                    answered_count.fetch_add(1, Ordering::SeqCst);
                },
            );
        });
        (watch, stuck, answered)
    }

    #[test]
    fn test_nothing_is_watched_while_profiling_is_off() {
        let (watch, stuck, _) = running();
        watch.set_watching(false);
        watch.busy("input");
        thread::sleep(TEST_STALL * 3);
        watch.idle(String::new);
        assert_eq!(stuck.load(Ordering::SeqCst), 0, "no stall seen");
        assert_eq!(watch.describe(), "(none)\n", "no slow turn kept");
    }

    #[test]
    fn test_short_turns_are_never_reported() {
        let (watch, stuck, answered) = running();
        for _ in 0..10 {
            watch.busy("input");
            thread::sleep(TEST_STALL / 8);
            watch.idle(String::new);
            thread::sleep(TEST_STALL / 8);
        }
        thread::sleep(TEST_STALL * 2);
        assert_eq!(stuck.load(Ordering::SeqCst), 0);
        assert_eq!(answered.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_a_stuck_turn_is_reported_once_and_its_end_once() {
        let (watch, stuck, answered) = running();
        watch.busy("input");
        thread::sleep(TEST_STALL * 4);
        watch.busy("render");
        assert_eq!(stuck.load(Ordering::SeqCst), 1, "once, however long");
        assert_eq!(answered.load(Ordering::SeqCst), 0, "still stuck");
        watch.idle(String::new);
        thread::sleep(TEST_STALL * 2);
        assert_eq!(answered.load(Ordering::SeqCst), 1);
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
