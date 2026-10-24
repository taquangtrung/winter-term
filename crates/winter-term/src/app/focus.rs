//! Which tab has the keyboard, and telling the programs that asked (DECSET
//! 1004) when they gain or lose it: on a pane switch as on a window switch,
//! as tmux does, so an editor in a pane left behind hears that it was left.
//! The tab that takes it also flashes its cursor's line, fading out, so the
//! eye finds where typing goes now; so does the focused tab when the window
//! itself gains the keyboard.

use std::time::{Duration, Instant};

use crate::model::layout::TabId;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// What a program in focus-event mode is sent when its tab gains the keyboard.
const FOCUS_IN: &[u8] = b"\x1b[I";

/// What it is sent when its tab loses the keyboard.
const FOCUS_OUT: &[u8] = b"\x1b[O";

/// How long a focus flash takes to fade out: long enough to be seen from
/// across a split window, short enough to be gone before typing starts.
const FLASH_DURATION: Duration = Duration::from_millis(500);

/// How often a fading flash is drawn again: about a display's refresh.
const FLASH_FRAME: Duration = Duration::from_millis(16);

// ========================================================================
// Data Structures
// ========================================================================

/// The line lit in the tab that just took the keyboard, while it fades.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FocusFlash {
    started: Instant,
    pub(crate) tab: TabId,
}

/// Where the keyboard is: the focused tab, and whether the window itself has
/// it. A tab only has the keyboard while its window does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FocusSpot {
    pub(crate) tab: TabId,
    pub(crate) window: bool,
}

/// The reports one move of the keyboard calls for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FocusReports {
    /// The tab that has the keyboard now and did not before.
    gained: Option<TabId>,
    /// The tab that had the keyboard and no longer does.
    lost: Option<TabId>,
}

// ========================================================================
// FocusFlash Implementations
// ========================================================================

impl FocusFlash {
    /// A flash in tab `tab`, lit at `now`.
    pub(crate) fn new(tab: TabId, now: Instant) -> Self {
        Self { started: now, tab }
    }

    /// How strongly the line is lit at `now`: 1 as it lights, easing out to
    /// nothing over [`FLASH_DURATION`]. `None` once it has faded.
    pub(crate) fn strength(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.started);
        let t = elapsed.as_secs_f32() / FLASH_DURATION.as_secs_f32();
        (t < 1.0).then(|| (1.0 - t).powi(2))
    }
}

// ========================================================================
// FocusSpot Implementations
// ========================================================================

impl FocusSpot {
    /// The tab holding the keyboard here, if the window has it.
    fn holder(self) -> Option<TabId> {
        self.window.then_some(self.tab)
    }
}

// ========================================================================
// App: focus
// ========================================================================

impl App {
    /// The pane drawn as the active one: the focused tab, unless a popup holds
    /// the keyboard over the panes, when every pane is drawn inactive.
    pub(crate) fn active_pane(&self) -> Option<TabId> {
        match &self.overlay {
            None => Some(self.layout().focused()),
            // A search's popup leaves the pane it searches lit: that pane is
            // still what is being read, and its matches show as they are typed.
            Some(super::Overlay::EditorPrompt(prompt)) if prompt.typed_search().is_some() => {
                Some(prompt.tab)
            }
            Some(_) => None,
        }
    }

    /// Give the keyboard to tab `tab`, telling the programs on either side.
    /// Returns whether the layout holds the tab to focus.
    pub(crate) fn set_focus(&mut self, tab: TabId) -> bool {
        let focused = self.layout_mut().focus(tab);
        self.sync_focus_reports();
        focused
    }

    /// Tell the programs that asked for focus events where the keyboard went
    /// since this was last called, however it got there: a pane switch, a
    /// closed tab, the window losing or gaining the keyboard.
    pub(crate) fn sync_focus_reports(&mut self) {
        let now = FocusSpot {
            tab: self.layout().focused(),
            window: self.window_focused,
        };
        let reports = focus_reports(self.focus_reported, now);
        // A tab taking the keyboard from another flashes, and so does the
        // focused tab when the window gains the keyboard, so the eye finds
        // the line on coming back from another window. The first tab of a
        // window opening does not.
        let flashes = self
            .focus_reported
            .is_some_and(|last| last.tab != now.tab || (now.window && !last.window));
        if flashes {
            self.focus_flash = Some(FocusFlash::new(now.tab, Instant::now()));
            self.dirty = true;
        }
        self.focus_reported = Some(now);
        if let Some(tab) = reports.lost {
            self.send_focus_report(tab, FOCUS_OUT);
        }
        if let Some(tab) = reports.gained {
            self.send_focus_report(tab, FOCUS_IN);
        }
    }

    /// Fade the focus flash a step, and say when it next needs drawing; put
    /// it out once it has faded.
    pub(crate) fn tick_focus_flash(&mut self, now: Instant) -> Option<Instant> {
        let flash = self.focus_flash?;
        self.dirty = true;
        if flash.strength(now).is_none() {
            self.focus_flash = None;
            return None;
        }
        Some(now + FLASH_FRAME)
    }

    /// Write `report` to tab `tab`'s program, if it asked for focus events.
    fn send_focus_report(&mut self, tab: TabId, report: &[u8]) {
        if let Some(terminal) = self.open_tabs.terminal_mut(&tab) {
            if terminal.focus_event() {
                terminal.write(report);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The reports moving the keyboard from `last` to `now` calls for. Nothing is
/// reported the first time, when no program can have asked yet.
fn focus_reports(last: Option<FocusSpot>, now: FocusSpot) -> FocusReports {
    let Some(last) = last else {
        return FocusReports::default();
    };
    let (before, after) = (last.holder(), now.holder());
    if before == after {
        return FocusReports::default();
    }
    FocusReports {
        gained: after,
        lost: before,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(tab: u64, window: bool) -> FocusSpot {
        FocusSpot {
            tab: TabId(tab),
            window,
        }
    }

    #[test]
    fn test_a_pane_switch_tells_the_pane_left_and_the_pane_reached() {
        // Regression: only a window switch was reported, so a program in a
        // pane left behind never heard it had lost the keyboard.
        assert_eq!(
            focus_reports(Some(spot(1, true)), spot(2, true)),
            FocusReports {
                gained: Some(TabId(2)),
                lost: Some(TabId(1)),
            }
        );
    }

    #[test]
    fn test_a_window_switch_tells_only_the_focused_pane() {
        assert_eq!(
            focus_reports(Some(spot(1, true)), spot(1, false)),
            FocusReports {
                gained: None,
                lost: Some(TabId(1)),
            }
        );
        assert_eq!(
            focus_reports(Some(spot(1, false)), spot(1, true)),
            FocusReports {
                gained: Some(TabId(1)),
                lost: None,
            }
        );
    }

    #[test]
    fn test_a_pane_switch_in_a_window_without_the_keyboard_tells_nobody() {
        assert_eq!(
            focus_reports(Some(spot(1, false)), spot(2, false)),
            FocusReports::default()
        );
        assert_eq!(focus_reports(None, spot(1, true)), FocusReports::default());
    }

    #[test]
    fn test_a_flash_fades_from_full_to_nothing() {
        let now = Instant::now();
        let flash = FocusFlash::new(TabId(1), now);
        assert_eq!(flash.strength(now), Some(1.0));
        let early = flash.strength(now + FLASH_DURATION / 4).expect("lit");
        let late = flash.strength(now + FLASH_DURATION * 3 / 4).expect("lit");
        assert!(early > late && late > 0.0, "fading: {early} then {late}");
        assert_eq!(flash.strength(now + FLASH_DURATION), None, "gone");
    }

    #[test]
    fn test_a_tab_taking_the_keyboard_or_the_window_regaining_it_flashes() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.window_focused = true;

        app.sync_focus_reports();
        assert_eq!(app.focus_flash, None, "the window's first tab");

        app.focus_reported = Some(FocusSpot {
            tab: focused,
            window: false,
        });
        app.sync_focus_reports();
        assert_eq!(
            app.focus_flash.map(|flash| flash.tab),
            Some(focused),
            "the window coming back to it"
        );

        app.focus_flash = None;
        app.window_focused = false;
        app.sync_focus_reports();
        assert_eq!(app.focus_flash, None, "the window losing the keyboard");
        app.window_focused = true;

        app.focus_reported = Some(FocusSpot {
            tab: TabId(focused.0 + 1),
            window: true,
        });
        app.sync_focus_reports();
        assert_eq!(app.focus_flash.map(|flash| flash.tab), Some(focused));
    }

    #[test]
    fn test_a_flash_is_drawn_again_while_it_fades_and_put_out_after() {
        let mut app = App::new();
        let now = Instant::now();
        app.focus_flash = Some(FocusFlash::new(TabId(1), now));
        assert_eq!(app.tick_focus_flash(now), Some(now + FLASH_FRAME));
        assert_eq!(app.tick_focus_flash(now + FLASH_DURATION), None);
        assert_eq!(app.focus_flash, None);
        assert!(app.dirty, "the faded line is drawn away");
    }
}
