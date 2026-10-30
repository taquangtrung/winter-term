//! Fixtures the app layer's tests share: an app holding tabs, a terminal
//! that sits idle, a pane whose shell has died.

use std::time::{Duration, Instant};

use portable_pty::CommandBuilder;
use winter_core::MAX_SCROLLBACK;

use crate::model::input::Keymap;
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::terminal::session::Terminal;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long a test waits for a child process to do what it was asked. Every
/// wait breaks out as soon as it happens, so the ceiling only has to survive
/// a machine under load.
pub(in crate::window) const PTY_WAIT: Duration = Duration::from_secs(5);

/// How long to sleep between polls, so waiting does not spin a core.
pub(in crate::window) const POLL_STEP: Duration = Duration::from_millis(20);

// ========================================================================
// Functions
// ========================================================================

/// An app whose one pane holds `n` empty tabs (no PTYs), `TabId(0)`
/// first in the strip and shown, and the rest in order after it.
pub(in crate::window) fn app_with_tabs(n: usize) -> App {
    let mut app = App::new();
    for i in 1..n {
        app.layout_mut().add_tab(TabId(i as u64));
    }
    app.layout_mut().focus(TabId(0));
    app.next_tab_id = app.next_tab_id.max(n as u64);
    app
}

/// Inserts a real, already-exited `Pane` at `id` so `reap_dead_panes` sees
/// it as dead. Waits for the child process to actually terminate rather
/// than assuming immediate exit.
pub(in crate::window) fn insert_dead_pane(app: &mut App, id: TabId) {
    let mut terminal =
        Terminal::with_command(20, 5, portable_pty::CommandBuilder::new("true"), 1000)
            .expect("test pane spawn");
    let deadline = Instant::now() + PTY_WAIT;
    while Instant::now() < deadline {
        if !terminal.is_alive() {
            break;
        }
        std::thread::sleep(POLL_STEP);
    }
    app.open_tabs.insert_terminal(id, terminal);
}

/// A terminal running a program that exits at once, for a tab whose mode
/// is all a test is about.
pub(in crate::window) fn idle_terminal() -> Terminal {
    Terminal::with_command(20, 5, portable_pty::CommandBuilder::new("true"), 1000)
        .expect("test terminal spawn")
}

/// Write numbered lines into `grid` until well past its height, the shape
/// `seq 1 200` used to produce.
pub(in crate::window) fn fill_scrollback(grid: &mut winter_core::Grid) {
    for line in 1..=200 {
        for ch in line.to_string().chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.carriage_return();
    }
}

/// A terminal holding more than a page of numbered lines of scrollback.
pub(in crate::window) fn pane_with_scrollback() -> Terminal {
    let mut terminal = Terminal::with_command(
        40,
        10,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("test pane spawn");
    fill_scrollback(terminal.grid_mut());
    assert!(
        terminal.grid().scrollback_len() > terminal.grid().rows(),
        "fixture needs more than a page of scrollback"
    );
    terminal
}

/// A pane with a real PTY running `cat`, mirroring how a full-screen app
/// holds the keyboard: a foreground process (cat) plus the alt screen.
/// An app whose one tab runs `cat` in Insert mode, with the default keymap so
/// the developer's own keybindings cannot leak into a test.
pub(in crate::window) fn app_pane_and_id() -> (App, TabId) {
    let mut app = App::new();
    app.keymap = Keymap::default();
    let id = app.layout().shown_tabs()[0];
    let terminal = Terminal::with_command(40, 10, CommandBuilder::new("cat"), MAX_SCROLLBACK)
        .expect("test pane spawn");
    app.open_tabs.insert_terminal(id, terminal);
    app.open_tabs.set_mode(&id, Mode::Insert);
    (app, id)
}
