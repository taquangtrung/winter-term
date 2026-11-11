//! The window's side of the picker framework (misty's `misty::picker`): opening a
//! picker over a source, routing keys to it through its key tables, taking
//! in what its source and matcher sent, carrying out what a row chose, and
//! keeping the last picker session to resume.
//!
//! - [`outcome`]: what a chosen row has the window do.
//! - `preview`: a live picker's preview in its pane, and the side panel's.
//! - [`sources`]: one source per picker.

pub(crate) mod keys;
mod open;
pub(in crate::window) mod outcome;
pub(in crate::window) mod preview;
pub(in crate::window) mod sources;
mod view;

pub(in crate::window) use outcome::WindowOutcome;

pub(in crate::window) use preview::{FilePreview, PanelState, Preview};

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use misty::picker::{self, History, Parked, Picker, PreviewKind, Recency, Reply, Source};
use misty::{EventCx, QuickPick};
use winter_model::input::table::find;
use winter_model::input::Key;
use winter_model::layout::TabId;
use winter_model::vim::objects::keyword_at;

use crate::window::editor::ActivePick;
use crate::window::input::misty::misty_key;
use crate::window::input::overlay::{Overlay, PickerOverlay};
use crate::window::terminal::navigation::vim::line_chars;
use crate::window::App;
use keys::KEYS;
use sources::ScoutLines;

// ========================================================================
// Constants
// ========================================================================

/// How long a picker opening waits for its first rows before it is drawn
/// without them: two frames, so a list ready at once never flashes empty.
const OPENING_PATIENCE: Duration = Duration::from_millis(30);

/// Whether pickers match on the window's thread: under test, so a test never
/// waits on a matcher's thread.
const SYNCHRONOUS: bool = cfg!(test);

/// The size of the window a test types into a picker through.
#[cfg(test)]
const TEST_WINDOW: (u32, u32) = (800, 600);

/// Said when a picker is asked to save its rows and none match.
const NOTHING_TO_SAVE: &str = "no rows to save";

/// Said when there is no picker session to resume.
const NOTHING_TO_RESUME: &str = "no picker to resume";

// ========================================================================
// Data Structures
// ========================================================================

/// What the window keeps of its pickers: each picker's earlier queries and
/// the use of its rows (both saved across restarts), whether each shows its
/// side panel, and the last picker session to resume.
#[derive(Default)]
pub(in crate::window) struct PickerMemory {
    pub(in crate::window) history: History,
    /// Whether each picker's side panel is shown: hidden until `F6` shows it.
    shown_panels: HashMap<&'static str, bool>,
    /// The side panel's preview of the selected row, read off the window's
    /// thread.
    pub(in crate::window) panel: PanelState,
    parked: Option<Parked<WindowOutcome, Preview>>,
    pub(in crate::window) recency: Recency,
    /// The keys typed into the open picker's query and not yet handed to
    /// it, which takes them with the window's context at the end of the
    /// input they came in.
    typed: Vec<misty::Key>,
    /// The open picker as the last frame worked it out (where it goes, its
    /// rows' colors, its side panel), for misty's quick pick to draw.
    pub(in crate::window) drawn: Option<winter_render::PickerView>,
    /// What wakes misty's loop when a picker's matcher has news; `None`
    /// before the window is open, when pickers match on the window's
    /// thread.
    pub(in crate::window) waker: Option<misty::Waker<()>>,
    /// The editor lines Scout lists, while it is the picker last opened over
    /// an editor (see [`ScoutLines`]).
    pub(in crate::window) scout: Option<ScoutLines>,
}

// ========================================================================
// PickerMemory
// ========================================================================

impl PickerMemory {
    /// What the saved state left: each picker's queries and its rows' use.
    pub(in crate::window) fn new(history: History, recency: Recency) -> Self {
        Self {
            history,
            recency,
            ..Self::default()
        }
    }
}

// ========================================================================
// App: pickers
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Opening
    // --------------------------------------------------------------------

    /// Open a picker over `source`, in place of whatever holds the keyboard.
    pub(in crate::window) fn open_picker(
        &mut self,
        source: Box<dyn Source<WindowOutcome, Preview>>,
    ) {
        self.open_picker_answering(source, None);
    }

    /// Open a picker over `source` that answers an editor's question, which
    /// is told when it goes unanswered.
    pub(in crate::window) fn open_picker_answering(
        &mut self,
        source: Box<dyn Source<WindowOutcome, Preview>>,
        pick: Option<ActivePick>,
    ) {
        let id = source.id();
        let history = self.pickers.history.of(id).to_vec();
        let panel = self
            .pickers
            .shown_panels
            .get(id)
            .copied()
            .unwrap_or_default();
        let recency = self.pickers.recency.clone();
        // A search starts on the word under the cursor, for typing to
        // replace; a list an editor asked for is its own question.
        let seeds = pick.is_none() && self.config.picker_word_at_point;
        let seed = (seeds && source.takes_word_at_point())
            .then(|| self.word_at_point(self.layout().focused()))
            .flatten();
        let memory = picker::PickerMemory {
            history,
            panel,
            recency,
        };
        let picker = match (&self.pickers.waker, SYNCHRONOUS) {
            (Some(waker), false) => Picker::open(source, memory, waker.clone()),
            (None, _) | (Some(_), true) => Picker::synchronous(source, memory),
        };
        let mut picker = Box::new(picker);
        if let Some(word) = seed {
            picker.seed(&word, Instant::now());
        }
        picker.settle(OPENING_PATIENCE);
        self.pickers.panel.clear();
        // Each picker opens where its placement puts it: a drag or resize
        // belongs to the picker it was done to.
        let quick = Box::new(QuickPick::new(*picker).without_default_keys());
        self.open_overlay(Overlay::Picker(Box::new(PickerOverlay { pick, quick })));
    }

    /// The word under the cursor of tab `tab`: an editor's, or a terminal's
    /// Normal-mode cursor's. `None` off a word.
    fn word_at_point(&self, tab: TabId) -> Option<String> {
        if let Some(slot) = self.open_tabs.page(&tab) {
            return slot.page.word_at_cursor();
        }
        let (row, col) = self.nav_cursor(tab)?;
        let terminal = self.open_tabs.terminal(&tab)?;
        keyword_at(&line_chars(terminal.grid(), row), col)
    }

    /// Open the picker `id` builds, or put it away when it is the one open:
    /// the chord that opens a picker closes it.
    pub(in crate::window) fn toggle_picker(
        &mut self,
        id: &str,
        build: impl FnOnce(&mut App) -> Option<Box<dyn Source<WindowOutcome, Preview>>>,
    ) {
        if self.is_picker_open(id) {
            self.dismiss_overlay();
            return;
        }
        if let Some(source) = build(self) {
            self.open_picker(source);
        }
    }

    /// Whether the picker open is `id`.
    pub(in crate::window) fn is_picker_open(&self, id: &str) -> bool {
        self.chrome.picker().is_some_and(|picker| picker.id() == id)
    }

    /// Open the last picker session closed, as it was.
    pub(in crate::window) fn resume_picker(&mut self) {
        let Some(parked) = self.pickers.parked.take() else {
            self.set_notice(NOTHING_TO_RESUME);
            return;
        };
        let mut picker = Box::new(match sources::rebuild(self, parked.id()) {
            Some(fresh) => parked.resume_with(fresh),
            None => parked.resume(),
        });
        picker.settle(OPENING_PATIENCE);
        self.pickers.panel.clear();
        let quick = Box::new(QuickPick::new(*picker).without_default_keys());
        self.open_overlay(Overlay::Picker(Box::new(PickerOverlay {
            pick: None,
            quick,
        })));
    }

    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    /// Route `key` to the open picker: its own table first, then the table
    /// of its source's actions, then the source and the query.
    pub(in crate::window) fn press_picker_key(&mut self, key: &Key, focused: TabId) {
        let Some(quick) = self.chrome.quick_pick_mut() else {
            return;
        };
        let reply = match find(&KEYS, key) {
            // The quick pick knows the lines it shows, for the page keys.
            Some(act) => quick.act(act),
            None => match sources::action_for(quick.picker().id(), key) {
                Some(action) => quick.picker_mut().run_action(action),
                None => {
                    // Typed into the query with the window's context, at
                    // the end of the input (see `type_into_picker`).
                    self.pickers.typed.push(misty_key(key));
                    Reply::Consumed
                }
            },
        };
        self.answer_reply(reply, focused);
    }

    /// Hand the keys typed into the open picker's query since the input
    /// began to it, with the window's context `cx` (its clipboard).
    pub(in crate::window) fn type_into_picker(&mut self, cx: &mut EventCx<'_>) {
        if self.pickers.typed.is_empty() {
            return;
        }
        let typed = std::mem::take(&mut self.pickers.typed);
        let focused = self.layout().focused();
        let now = Instant::now();
        for key in typed {
            let Some(picker) = self.chrome.picker_mut() else {
                return;
            };
            let reply = picker.key(key, cx, now);
            self.answer_reply(reply, focused);
        }
    }

    /// Text pasted while a picker is open goes to its query.
    pub(in crate::window) fn paste_into_picker(&mut self, text: &str) {
        if let Some(picker) = self.chrome.picker_mut() {
            picker.paste(text, Instant::now());
            self.dirty = true;
        }
    }

    // --------------------------------------------------------------------
    // Ticking
    // --------------------------------------------------------------------

    /// Take in what the open picker's source and matcher sent; when to be
    /// called again for work waiting on a pause.
    pub(in crate::window) fn tick_picker(&mut self, now: Instant) -> Option<Instant> {
        let panel_again = self.take_panel_preview();
        let focused = self.layout().focused();
        let picker = self.chrome.picker_mut()?;
        let kind = picker.preview_kind();
        let query = picker.query().to_string();
        let tick = picker.tick(now);
        if tick.redraw {
            self.dirty = true;
        }
        if let Some(update) = tick.preview {
            match kind {
                PreviewKind::Live => self.live_preview(update, &query, focused),
                PreviewKind::Panel => self.panel_preview(update, &query),
                PreviewKind::None => {}
            }
        }
        if let Some(outcome) = tick.outcome {
            self.finish_picker(outcome, focused);
        }
        if let Some(saved) = tick.saved {
            self.finish_picker(WindowOutcome::SaveLines(saved), focused);
        }
        [tick.again, panel_again].into_iter().flatten().min()
    }

    // --------------------------------------------------------------------
    // Closing
    // --------------------------------------------------------------------

    /// The picker `overlay` held went without a choice: a live picker's pane
    /// goes back to where it was, and the session is kept to resume. The
    /// question it answered, if any, has been told already.
    pub(in crate::window) fn picker_dismissed(&mut self, overlay: PickerOverlay) {
        if overlay.quick.picker().preview_kind() == PreviewKind::Live {
            let focused = self.layout().focused();
            self.cancel_jump(focused);
        }
        self.keep_picker(overlay.quick.into_picker());
    }

    /// Do what a key or a tick's reply asks.
    fn answer_reply(&mut self, reply: Reply<WindowOutcome>, focused: TabId) {
        match reply {
            Reply::Close => self.dismiss_overlay(),
            Reply::Consumed => {}
            Reply::Ignored => return,
            Reply::NothingToSave => self.set_notice(NOTHING_TO_SAVE),
            Reply::Run(outcome) => self.finish_picker(outcome, focused),
        }
        self.dirty = true;
    }

    /// The picker chose `outcome`: remember the query and the row, close
    /// the picker keeping its session, and carry the outcome out, an answer
    /// to whoever asked.
    fn finish_picker(&mut self, outcome: WindowOutcome, focused: TabId) {
        let Some(overlay) = self.chrome.take_picker_overlay() else {
            return;
        };
        self.remember_choice(&overlay);
        let PickerOverlay { pick, quick } = overlay;
        self.keep_picker(quick.into_picker());
        match outcome {
            WindowOutcome::AnswerEditor(choice) => {
                if let Some(pick) = pick {
                    self.answer_pick(pick, Some(choice));
                }
            }
            outcome => {
                // A question the picker held but did not answer is told so,
                // as one put away is.
                if let Some(pick) = pick {
                    self.answer_pick(pick, None);
                }
                self.carry_out(outcome, focused);
            }
        }
        self.dirty = true;
    }

    /// Remember the query `overlay`'s picker ran with, and the row chosen,
    /// for this picker's history and its ranking by use, here and on disk.
    fn remember_choice(&mut self, overlay: &PickerOverlay) {
        let picker = overlay.quick.picker();
        let id = picker.id();
        self.pickers.history.record(id, picker.query());
        let used = picker
            .selected_row()
            .and_then(|row| picker.recency_name(row));
        if let Some(name) = used {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default();
            self.pickers.recency.record(id, &name, now);
        }
        let (history, recency) = (self.pickers.history.clone(), self.pickers.recency.clone());
        self.update_app_state(|state| {
            state.picker_history = history;
            state.picker_recency = recency;
        });
    }

    /// Keep `overlay`'s picker session to resume, and whether its panel
    /// shows, for the next time it opens.
    fn keep_picker(&mut self, picker: Picker<WindowOutcome, Preview>) {
        if picker.preview_kind() == PreviewKind::Panel {
            self.pickers
                .shown_panels
                .insert(picker.id(), picker.is_panel_shown());
        }
        self.pickers.panel.clear();
        if let Some(parked) = picker.park() {
            self.pickers.parked = Some(parked);
        }
    }
}

// ========================================================================
// Test support
// ========================================================================

#[cfg(test)]
impl App {
    /// Tick the open picker past every pause it waits on, until its source's
    /// work is done or a deadline passes; an extension's picker's queries are
    /// told it each tick, as the event loop tells them.
    pub(in crate::window) fn settle_picker(&mut self) {
        let deadline = Instant::now() + SETTLE_PATIENCE;
        loop {
            self.tick_picker(Instant::now() + SETTLE_PAST);
            self.tell_extension_queries();
            let working = self
                .chrome
                .picker()
                .is_some_and(|picker| picker.view().status == misty::picker::Status::Working);
            if !working || Instant::now() > deadline {
                // Once more, past the preview's pause the last tick started.
                self.tick_picker(Instant::now() + SETTLE_PAST * 2);
                return;
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// Press the keys of `spec`, written as `keybindings.kdl` writes them and
    /// separated by spaces, into the open picker, settling it after each.
    pub(in crate::window) fn press_in_picker(&mut self, spec: &str) {
        let focused = self.layout().focused();
        let mut window = misty::testing::TestWindow::new(TEST_WINDOW.0, TEST_WINDOW.1);
        for word in spec.split_whitespace() {
            let keys = winter_model::input::table::keys(word).expect("a key spec");
            for key in keys {
                self.press_picker_key(&key, focused);
                self.type_into_picker(&mut window.cx());
                self.settle_picker();
            }
        }
    }

    /// The labels of the open picker's rows on screen.
    pub(in crate::window) fn picker_labels(&self) -> Vec<String> {
        self.chrome
            .picker()
            .map(|picker| {
                picker
                    .view()
                    .rows
                    .into_iter()
                    .map(|row| row.row.label)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// How far past now a settling tick looks, beyond every pause a picker waits.
#[cfg(test)]
const SETTLE_PAST: std::time::Duration = std::time::Duration::from_secs(1);

/// How long settling waits on a source's work, and how often it looks.
#[cfg(test)]
const SETTLE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
const SETTLE_POLL: std::time::Duration = std::time::Duration::from_millis(5);
