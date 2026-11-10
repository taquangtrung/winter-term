//! One picker session: the query, the source, the matcher, the rows on
//! screen, and the selection, the action menu, the preview, saving, and
//! resuming over them.
//!
//! The session never waits: rows stream in from the source, matches come
//! back from the matcher, and both are taken in by `tick`. The selection
//! stays on its row as rows arrive, and on the best match until it is moved.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use winter_model::input::table::menu_entry;
use winter_model::input::{format_key, Key, KeyCode};
use winter_model::textedit::field::TextField;

use super::history::Recency;
use super::keys::{PickerAct, KEYS};
use super::matcher::{MatchReply, Matcher, Shown, Tracked};
use super::query::Query;
use super::row::{Highlights, Row, RowKey};
use super::source::{
    Action, Batch, Feed, Matching, Preview, PreviewKind, Resume, Source, SourceKey,
};

// ========================================================================
// Constants
// ========================================================================

/// What the count ends with while rows are still coming, and once the
/// source stopped at its cap.
const WORKING_MARK: &str = " …";
const CAPPED_MARK: &str = " (capped)";

/// How long the typing pauses before a source that matches by itself is
/// fed the query.
const QUERY_PAUSE: Duration = Duration::from_millis(150);

/// How long the selection rests before its row is previewed.
const PREVIEW_PAUSE: Duration = Duration::from_millis(50);

/// How many lines of rows a picker asks for before its host says how many
/// it shows: more than any screen holds, so no picker opens short.
const OPENING_LINES: usize = 100;

/// How many pages of rows are held above and below those on screen, so a
/// move or a page key scrolls without waiting on the matcher.
const MARGIN_PAGES: usize = 2;

/// How often [`Picker::settle`] looks for the first rows.
const SETTLE_POLL: Duration = Duration::from_micros(200);

/// What a picker with no rows showing says when asked to save them.
const NOTHING_TO_SAVE: &str = "no rows to save";

/// What stands between a saved row's label and its description.
const SAVED_DESCRIPTION_GAP: &str = "  ";

/// The key hints' names for what their keys do.
const HINT_ACTIONS: &str = "actions";
const HINT_SAVE: &str = "save";

// ========================================================================
// Data Structures
// ========================================================================

/// One picker session.
pub struct Picker<O> {
    /// The rows the matcher last sent: those on screen, and a margin of
    /// pages above and below them that a move scrolls through at once.
    shown: Vec<Shown>,
    /// The batches of a stream being fed, while it is.
    feed: Option<Receiver<Batch>>,
    /// The index among the matches of the first row on screen.
    first: usize,
    /// The index among the matches of the first row held in `shown`.
    held: usize,
    /// When the query is next fed to a source that matches by itself.
    due: Option<Instant>,
    /// Why the source stopped, when it failed.
    error: Option<String>,
    /// The query as typed.
    field: Box<TextField>,
    /// This picker's earlier queries, newest first, and which is recalled.
    history: Vec<String>,
    history_at: Option<usize>,
    /// The query being typed when the history was first stepped into.
    live_query: String,
    /// How many lines of rows the list shows.
    lines: usize,
    /// None for a picker whose query is the answer.
    matcher: Option<Matcher>,
    /// How many rows match, and how many there are in all.
    matched: usize,
    total: usize,
    /// The action menu, while it is open.
    menu: Option<ActionMenu>,
    /// Whether the selection was moved, so it stays on its row; else it
    /// stays on the best match.
    moved: bool,
    /// Whether the side panel is shown.
    panel: bool,
    /// When the selected row is previewed, and the row last previewed.
    preview_due: Option<Instant>,
    previewed: Option<RowKey>,
    /// The rows' use, for a source ranking recently used first.
    recency: Recency,
    /// Whether the matched rows were asked for, to save them.
    saving: bool,
    /// Whether the query is a seed, the word under the cursor it opened on:
    /// drawn selected, the first character typed (or a deletion) replaces it
    /// whole, and any other edit keeps it.
    seeded: bool,
    /// The index among the matches of the selected row.
    selected: usize,
    /// The selected row's key, once it has been on screen.
    selected_key: Option<RowKey>,
    source: Box<dyn Source<O>>,
    /// Where the rows stand.
    status: Status,
    /// Whether the matcher scores on the caller's thread.
    synchronous: bool,
}

/// A closed picker session kept to resume, its work stopped.
pub struct Parked<O>(Box<Picker<O>>);

/// The action menu over a picker: the selected row's actions, filtered.
struct ActionMenu {
    actions: Vec<Action>,
    filter: Box<TextField>,
    row: RowKey,
    selected: usize,
}

/// Where a picker session's rows stand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    /// The source stopped at its cap with rows left.
    Capped,
    /// Every row is in and matched.
    Done,
    /// Rows are still coming, or being matched.
    Working,
}

/// What an act or a key did.
#[derive(Debug, Eq, PartialEq)]
pub enum Reply<O> {
    /// Close the picker.
    Close,
    /// The picker took it.
    Consumed,
    /// The picker has no use for it.
    Ignored,
    /// Say this in the status bar.
    Notice(String),
    /// Carry this out and close the picker.
    Run(O),
}

/// What a tick took in.
#[derive(Debug)]
pub struct Tick<O> {
    /// When to tick again for work waiting on a pause.
    pub again: Option<Instant>,
    /// What saving the rows came to, where the source saves them its own
    /// way: carry it out and close the picker.
    pub outcome: Option<O>,
    /// The rows saved as lines of text, where the source saves them no way
    /// of its own: keep them in a buffer and close the picker.
    pub saved: Option<SavedLines>,
    /// What to preview, when the selection came to rest on another row.
    pub preview: Option<PreviewUpdate>,
    /// Whether the view changed.
    pub redraw: bool,
}

/// A picker's rows saved as lines of text, for a buffer of their own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedLines {
    /// Each row as it read: its label, and its description after it.
    pub lines: Vec<String>,
    /// What the picker is called (see `Source::name`).
    pub name: String,
    /// What the rows were filtered by; empty for every row.
    pub query: String,
}

/// A change to what the preview shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewUpdate {
    /// Show nothing: the row has nothing to preview, or the panel is hidden.
    Clear,
    /// Show this.
    Show(Preview),
}

/// What the window draws of a picker session.
#[derive(Clone, Debug)]
pub struct SessionView {
    /// The caret's place in the query, in chars.
    pub caret: usize,
    /// Why the source stopped, when it failed.
    pub error: Option<String>,
    /// The keys the picker offers now, for the hint line.
    pub hints: Vec<KeyHint>,
    /// How many rows match, and how many there are.
    pub matched: usize,
    /// The action menu, while it is open.
    pub menu: Option<MenuView>,
    /// Whether the side panel is shown.
    pub panel: bool,
    /// What the query line says, dimmed, while nothing is typed.
    pub placeholder: String,
    /// Where the selected row stands among the matches, from zero; `None`
    /// with nothing matching.
    pub position: Option<usize>,
    /// The kind of preview the picker has.
    pub preview_kind: PreviewKind,
    /// The query as typed.
    pub query: String,
    /// The rows on screen.
    pub rows: Vec<ViewRow>,
    /// Whether the query is a seed typing replaces, drawn selected.
    pub seeded: bool,
    /// The index in `rows` of the selected row, when it is on screen.
    pub selected: Option<usize>,
    /// Where the rows stand.
    pub status: Status,
    /// What the picker is headed by.
    pub title: String,
    /// How many rows there are in all.
    pub total: usize,
}

/// One row as drawn.
#[derive(Clone, Debug)]
pub struct ViewRow {
    /// The row's actions shown as buttons.
    pub buttons: Vec<Action>,
    /// The letters lit in each field.
    pub lit: Highlights,
    /// The row.
    pub row: Row,
}

/// The action menu as drawn.
#[derive(Clone, Debug)]
pub struct MenuView {
    /// The actions the filter keeps.
    pub actions: Vec<Action>,
    /// What is typed to filter them.
    pub filter: String,
    /// The index of the selected action.
    pub selected: usize,
}

/// One key the picker offers, and what it does, for the hint line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyHint {
    /// The key, as `keybindings.kdl` writes it.
    pub key: String,
    /// What it does, in a word.
    pub label: String,
}

// ========================================================================
// Picker
// ========================================================================

impl<O> Picker<O> {
    // --------------------------------------------------------------------
    // Opening
    // --------------------------------------------------------------------

    /// Open over `source`, with its picker's earlier queries, the use of
    /// rows, and whether its side panel shows; matching on a thread unless
    /// `synchronous`.
    pub fn open(
        source: Box<dyn Source<O>>,
        history: Vec<String>,
        recency: Recency,
        panel: bool,
        synchronous: bool,
    ) -> Self {
        let mut picker = Self {
            due: None,
            error: None,
            feed: None,
            field: Box::new(TextField::line("")),
            first: 0,
            history,
            held: 0,
            history_at: None,
            lines: OPENING_LINES,
            live_query: String::new(),
            matched: 0,
            matcher: None,
            menu: None,
            moved: false,
            panel,
            preview_due: None,
            previewed: None,
            recency,
            saving: false,
            seeded: false,
            selected: 0,
            selected_key: None,
            shown: Vec::new(),
            source,
            status: Status::Working,
            synchronous,
            total: 0,
        };
        picker.matcher = picker.new_matcher();
        picker.start_feed();
        picker.request_rows();
        picker
    }

    /// The picker's name, its history's and its key table's.
    pub fn id(&self) -> &'static str {
        self.source.id()
    }

    /// The query as typed.
    pub fn query(&self) -> &str {
        self.field.text()
    }

    /// The kind of preview the picker has: where the window draws it.
    pub fn preview_kind(&self) -> PreviewKind {
        self.source.preview_kind()
    }

    /// Whether the side panel is shown, to remember for the next opening.
    pub fn is_panel_shown(&self) -> bool {
        self.panel
    }

    /// The name a chosen row's use is remembered by.
    pub fn recency_name(&self, row: RowKey) -> Option<String> {
        self.source
            .ranks_recent()
            .then(|| self.source.recency_name(row))
            .flatten()
    }

    /// The selected row's key, when there is one.
    pub fn selected_row(&self) -> Option<RowKey> {
        self.selected_key
    }

    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    /// Do `act`, given how many lines of rows the list shows.
    pub fn act(&mut self, act: PickerAct, lines: usize, now: Instant) -> Reply<O> {
        self.lines = lines.max(1);
        if self.menu.is_some() {
            return self.act_in_menu(act);
        }
        match act {
            PickerAct::Accept => self.accept(),
            PickerAct::ActionMenu => self.open_menu(),
            PickerAct::Close => Reply::Close,
            PickerAct::HistoryNext => self.step_history(false, now),
            PickerAct::HistoryPrevious => self.step_history(true, now),
            PickerAct::Next => self.move_selection(1),
            PickerAct::PageDown => self.move_selection(self.fit() as isize),
            PickerAct::PageUp => self.move_selection(-(self.fit() as isize)),
            PickerAct::Previous => self.move_selection(-1),
            PickerAct::Save => self.save(),
            PickerAct::TogglePanel => self.toggle_panel(now),
        }
    }

    /// Say how many lines of rows the list shows, as the host last drew it.
    pub fn set_lines(&mut self, lines: usize) {
        let lines = lines.max(1);
        if lines != self.lines {
            self.lines = lines;
            self.request_rows();
        }
    }

    /// A key no table binds: the source's own, else the query's.
    pub fn key(&mut self, key: &Key, now: Instant) -> Reply<O> {
        if let Some(menu) = &mut self.menu {
            menu.filter.on_key(key, 1);
            menu.selected = 0;
            return Reply::Consumed;
        }
        if let Some(reply) = self.source_key(key) {
            return reply;
        }
        let before = self.query().to_string();
        let seeded = std::mem::take(&mut self.seeded);
        // What the key does to an empty query says what it is: one that
        // types something replaces the seed, one that deletes takes the seed
        // whole, and a move keeps it to edit.
        let typed = seeded
            .then(|| {
                let mut field = TextField::line("");
                field.on_key(key, 1);
                field
            })
            .filter(|field| !field.text().is_empty());
        match typed {
            Some(field) => *self.field = field,
            None => {
                self.field.on_key(key, 1);
                if seeded && self.query().len() < before.len() {
                    self.field.set_text("");
                }
            }
        }
        if self.query() == before {
            return Reply::Ignored;
        }
        self.history_at = None;
        self.query_changed(now);
        Reply::Consumed
    }

    /// Text pasted into the query, in place of a seed.
    pub fn paste(&mut self, text: &str, now: Instant) {
        if std::mem::take(&mut self.seeded) {
            self.field.set_text("");
        }
        self.field.paste(text);
        self.query_changed(now);
    }

    /// Start the query as `text`, the word under the cursor, for the first
    /// character typed to replace (drawn selected). A query already
    /// there, a resumed session's, is kept.
    pub fn seed(&mut self, text: &str, now: Instant) {
        if text.is_empty() || !self.query().is_empty() {
            return;
        }
        self.field.set_text(text);
        self.seeded = true;
        self.query_changed(now);
    }

    /// Run a source action bound to a key on the selected row, with no menu.
    pub fn run_action(&mut self, action: super::source::ActionId) -> Reply<O> {
        let Some(row) = self.selected_key else {
            return Reply::Ignored;
        };
        self.run(action, row)
    }

    /// A click on a button of the row `at` on screen.
    pub fn click_button(&mut self, at: usize, action: super::source::ActionId) -> Reply<O> {
        let Some(shown) = self.on_screen().get(at) else {
            return Reply::Ignored;
        };
        let row = shown.row.key;
        self.run(action, row)
    }

    /// Start selected on `row` rather than the best match: a live picker on
    /// the line its pane's cursor is on.
    pub fn preselect(&mut self, row: RowKey) {
        self.moved = true;
        self.selected_key = Some(row);
        self.request_rows();
    }

    /// Select the row `at` on screen, as a click does: the action menu's
    /// while it is open over the list.
    pub fn select_shown(&mut self, at: usize) {
        if let Some(menu) = &mut self.menu {
            if at < menu.kept().len() {
                menu.selected = at;
            }
            return;
        }
        if at < self.on_screen().len() {
            self.select(self.first + at);
        }
    }

    // --------------------------------------------------------------------
    // Ticking
    // --------------------------------------------------------------------

    /// Take in what the source and the matcher sent, and do what a pause
    /// was waited for.
    pub fn tick(&mut self, now: Instant) -> Tick<O> {
        let mut redraw = false;
        if self.due.is_some_and(|due| due <= now) {
            self.due = None;
            self.refeed(true);
            redraw = true;
        }
        redraw |= self.take_batches();
        redraw |= self.take_matches();
        let (outcome, saved) = match self.take_saved() {
            Some(Ok(outcome)) => (Some(outcome), None),
            Some(Err(lines)) => (None, Some(lines)),
            None => (None, None),
        };
        let preview = self.take_preview(now);
        Tick {
            again: [self.due, self.preview_due].into_iter().flatten().min(),
            outcome,
            preview,
            redraw,
            saved,
        }
    }

    /// Wait up to `patience` for the first rows, so a picker whose rows come
    /// at once is first drawn with them rather than empty. Says whether they
    /// came, or the source had none, in time.
    pub fn settle(&mut self, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        loop {
            self.take_batches();
            let Some(matcher) = &mut self.matcher else {
                return true;
            };
            if let Some(reply) = matcher.poll() {
                // Idle with no more rows to come: the source had none.
                let idle = !reply.working && self.feed.is_none();
                self.apply(reply);
                if !self.shown.is_empty() || idle {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// What to draw.
    pub fn view(&self) -> SessionView {
        let query = self.query().to_string();
        let by_source = matches!(self.source.matching(), Matching::BySource(_));
        let on_screen = self.on_screen();
        let rows = on_screen
            .iter()
            .map(|shown| ViewRow {
                buttons: self
                    .source
                    .actions(shown.row.key)
                    .into_iter()
                    .filter(|action| action.button.is_some())
                    .collect(),
                // A source matching by itself lights its own matches; one
                // matched by words may light more of its own (the Todo
                // picker's keyword) beside the words typed.
                lit: match by_source {
                    true => self.source.highlights(&shown.row, &query),
                    false => merged(&shown.lit, self.source.highlights(&shown.row, &query)),
                },
                row: shown.row.clone(),
            })
            .collect();
        SessionView {
            caret: self.field.col(),
            error: self.error.clone(),
            hints: self.hints(),
            matched: self.matched,
            menu: self.menu.as_ref().map(ActionMenu::view),
            panel: self.panel,
            placeholder: self.source.placeholder(),
            position: (self.matched > 0).then_some(self.selected),
            preview_kind: self.source.preview_kind(),
            query,
            rows,
            seeded: self.seeded,
            selected: self
                .selected
                .checked_sub(self.first)
                .filter(|at| *at < on_screen.len()),
            status: self.status,
            title: self.source.title(),
            total: self.total,
        }
    }

    // --------------------------------------------------------------------
    // Resuming
    // --------------------------------------------------------------------

    /// Stop the work and keep the session to resume; `None` for a session
    /// that answered a question.
    pub fn park(mut self) -> Option<Parked<O>> {
        if self.source.resume() == Resume::Never {
            return None;
        }
        self.feed = None;
        self.due = None;
        self.menu = None;
        self.preview_due = None;
        self.previewed = None;
        self.saving = false;
        Some(Parked(Box::new(self)))
    }

    // --------------------------------------------------------------------
    // Internals
    // --------------------------------------------------------------------

    fn new_matcher(&self) -> Option<Matcher> {
        let on = match self.source.matching() {
            Matching::Answer => return None,
            Matching::BySource(on) | Matching::Words(on) => on,
        };
        let mut matcher = match self.synchronous {
            true => Matcher::synchronous(on),
            false => Matcher::threaded(on),
        };
        if matches!(self.source.matching(), Matching::BySource(_)) {
            matcher.unscored();
        }
        Some(matcher)
    }

    /// Feed the source from the start, for the query as it is.
    fn start_feed(&mut self) {
        if self.matcher.is_none() {
            self.status = Status::Done;
            return;
        }
        let query = self.query().to_string();
        self.status = Status::Working;
        self.error = None;
        match self.source.feed(&query) {
            Feed::Failed(reason) => {
                self.error = Some(reason);
                self.status = Status::Done;
            }
            Feed::Fixed(rows) => {
                self.push(rows);
                self.status = Status::Done;
            }
            Feed::Stream(batches) => self.feed = Some(batches),
        }
        self.pin_query_row();
    }

    /// Feed the source again: from no rows when `clear`, else after those
    /// held, which the matcher keeps once.
    fn refeed(&mut self, clear: bool) {
        if clear {
            if let Some(matcher) = &mut self.matcher {
                matcher.clear();
            }
            self.first = 0;
            self.held = 0;
            self.selected = 0;
        }
        self.feed = None;
        self.start_feed();
        self.request_rows();
    }

    /// Hand `rows` to the matcher, with their ranks by use when the source
    /// ranks by it; otherwise as they are, with no work per row here.
    fn push(&mut self, rows: Vec<Row>) {
        let Some(matcher) = &mut self.matcher else {
            return;
        };
        if !self.source.ranks_recent() {
            matcher.push(rows);
            return;
        }
        let id = self.source.id();
        let ranks = rows
            .iter()
            .map(|row| {
                self.source
                    .recency_name(row.key)
                    .map(|name| self.recency.rank(id, &name))
                    .unwrap_or_default()
            })
            .collect();
        matcher.push_ranked(rows, ranks);
    }

    /// Take in the batches the source sent; whether any came.
    fn take_batches(&mut self) -> bool {
        let Some(feed) = self.feed.take() else {
            return false;
        };
        let mut came = false;
        loop {
            match feed.try_recv() {
                Ok(batch) => {
                    came = true;
                    self.push(batch.rows);
                    self.pin_query_row();
                    if let Some(row) = batch.select.filter(|_| !self.moved) {
                        self.preselect(row);
                    }
                    if batch.error.is_some() {
                        self.error = batch.error;
                    }
                    if batch.done || batch.capped {
                        self.status = match batch.capped {
                            true => Status::Capped,
                            false => Status::Done,
                        };
                        self.request_rows();
                        return true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = Status::Done;
                    self.request_rows();
                    return true;
                }
            }
        }
        self.feed = Some(feed);
        if came {
            self.request_rows();
        }
        came
    }

    /// Take in the matcher's newest answer; whether one came.
    fn take_matches(&mut self) -> bool {
        let Some(reply) = self.matcher.as_mut().and_then(Matcher::poll) else {
            return false;
        };
        self.apply(reply);
        true
    }

    fn apply(&mut self, reply: MatchReply) {
        self.matched = reply.matched;
        self.total = reply.total;
        if reply.working && self.status == Status::Done {
            self.status = Status::Working;
        }
        if !reply.working && self.status == Status::Working && self.feed.is_none() {
            self.status = Status::Done;
        }
        // Where the selection sits on screen, kept as the rows move under it.
        let on_screen = self.selected.saturating_sub(self.first);
        match reply.tracked.filter(|_| self.moved) {
            // A reply to a move made before the latest says where the
            // selection was; the latest one's reply places it.
            Some(_) if !reply.current => {}
            Some(at) => {
                self.selected = at;
                self.first = at.saturating_sub(on_screen);
            }
            None if !self.moved => {
                self.selected = 0;
                self.first = 0;
            }
            None => {}
        }
        self.held = reply.first;
        self.shown = reply.rows;
        self.selected = self.selected.min(self.matched.saturating_sub(1));
        self.first = self.first.min(self.selected);
        self.selected_key = self.held_row(self.selected).map(|shown| shown.row.key);
    }

    /// The rows asked to be saved, once the matcher handed them over: as
    /// the source's own outcome where it has one, else as lines of text.
    fn take_saved(&mut self) -> Option<Result<O, SavedLines>> {
        if !self.saving {
            return None;
        }
        let rows = self.matcher.as_mut()?.take_matched()?;
        self.saving = false;
        let query = self.field.text();
        let keys: Vec<RowKey> = rows.iter().map(|row| row.key).collect();
        Some(self.source.export(&keys, query).ok_or_else(|| SavedLines {
            lines: rows.iter().map(saved_line).collect(),
            name: self.source.name(),
            query: query.trim().to_string(),
        }))
    }

    fn take_preview(&mut self, now: Instant) -> Option<PreviewUpdate> {
        let kind = self.source.preview_kind();
        let wanted = match kind {
            PreviewKind::Live => true,
            PreviewKind::None => false,
            PreviewKind::Panel => self.panel,
        };
        if !wanted || self.selected_key == self.previewed {
            self.preview_due = None;
            return None;
        }
        let due = *self.preview_due.get_or_insert(now + PREVIEW_PAUSE);
        if due > now {
            return None;
        }
        self.preview_due = None;
        self.previewed = self.selected_key;
        Some(
            match self.selected_key.and_then(|row| self.source.preview(row)) {
                Some(preview) => PreviewUpdate::Show(preview),
                None => PreviewUpdate::Clear,
            },
        )
    }

    /// The query changed: match it, or wait for the pause to feed it.
    fn query_changed(&mut self, now: Instant) {
        self.moved = false;
        let query = self.query().to_string();
        self.pin_query_row();
        match self.source.matching() {
            Matching::Answer => {}
            Matching::BySource(_) => self.due = Some(now + QUERY_PAUSE),
            Matching::Words(_) => {
                if let Some(matcher) = &mut self.matcher {
                    matcher.set_query(&query);
                }
                self.first = 0;
                self.selected = 0;
                self.request_rows();
            }
        }
    }

    /// Pin the row the query stands for after the matches, as the source
    /// makes it of the query and the rows it has now.
    fn pin_query_row(&mut self) {
        let row = self.source.query_row(self.query());
        if let Some(matcher) = &mut self.matcher {
            matcher.pin(row);
        }
    }

    /// Ask the matcher for the rows on screen and the margin around them,
    /// tracking the selected row.
    fn request_rows(&mut self) {
        let fit = self.fit();
        let margin = fit * MARGIN_PAGES;
        let start = self.first.saturating_sub(margin);
        let tracked = self.selected_key.filter(|_| self.moved).map(|key| Tracked {
            key,
            offset: self.selected.saturating_sub(start),
        });
        if let Some(matcher) = &mut self.matcher {
            matcher.track(tracked);
            matcher.request_slice(start, (self.first - start) + fit + 1 + margin);
        }
    }

    /// The rows on screen, from those held.
    fn on_screen(&self) -> &[Shown] {
        let Some(start) = self.first.checked_sub(self.held) else {
            return &[];
        };
        let start = start.min(self.shown.len());
        let end = (start + self.fit() + 1).min(self.shown.len());
        &self.shown[start..end]
    }

    /// The row at `at` among the matches, when it is held.
    fn held_row(&self, at: usize) -> Option<&Shown> {
        self.shown.get(at.checked_sub(self.held)?)
    }

    /// How many rows fit: the list's lines, over the lines each row takes.
    fn fit(&self) -> usize {
        let row_lines = self
            .shown
            .first()
            .map(|shown| shown.row.lines())
            .unwrap_or(1);
        (self.lines / row_lines).max(1)
    }

    fn move_selection(&mut self, by: isize) -> Reply<O> {
        if self.matched == 0 {
            return Reply::Consumed;
        }
        let last = self.matched - 1;
        let to = self.selected.saturating_add_signed(by).min(last);
        self.select(to);
        Reply::Consumed
    }

    /// Select the match `to`, scrolling to keep it in view. Its key is read
    /// from the rows on screen before the scroll, which are what it is
    /// among; a row past them is found by the matcher.
    fn select(&mut self, to: usize) {
        self.moved = true;
        self.selected = to;
        self.selected_key = self.held_row(to).map(|shown| shown.row.key);
        let fit = self.fit();
        if to < self.first {
            self.first = to;
        } else if to >= self.first + fit {
            self.first = to + 1 - fit;
        }
        // Held rows scroll at once; the matcher's answer only moves the
        // margin along.
        self.request_rows();
    }

    fn accept(&mut self) -> Reply<O> {
        if matches!(self.source.matching(), Matching::Answer) {
            let query = self.query().to_string();
            return match self.source.answer(&query) {
                Some(outcome) => Reply::Run(outcome),
                None => Reply::Consumed,
            };
        }
        let Some(row) = self.selected_key else {
            return Reply::Consumed;
        };
        // A source may give `Enter` a meaning of its own on a row: the
        // browser steps up from its `../` rather than opening it.
        if let Some(reply) = self.source_key(&Key::plain(KeyCode::Enter)) {
            return reply;
        }
        let Some(first) = self.source.actions(row).into_iter().next() else {
            return Reply::Consumed;
        };
        self.run(first.id, row)
    }

    /// What the source makes of `key` on the selected row, when it takes it.
    fn source_key(&mut self, key: &Key) -> Option<Reply<O>> {
        let query = self.query().to_string();
        match self.source.key(key, self.selected_key, &query)? {
            SourceKey::Done(outcome) => Some(Reply::Run(outcome)),
            SourceKey::Refeed => {
                self.refeed(true);
                Some(Reply::Consumed)
            }
            SourceKey::Reset => {
                self.seeded = false;
                self.field.set_text("");
                self.moved = false;
                if let Some(matcher) = &mut self.matcher {
                    matcher.set_query("");
                }
                self.refeed(true);
                Some(Reply::Consumed)
            }
            SourceKey::Stay => Some(Reply::Consumed),
        }
    }

    fn run(&mut self, action: super::source::ActionId, row: RowKey) -> Reply<O> {
        let query = self.query().to_string();
        match self.source.run(action, row, &query) {
            Some(outcome) => Reply::Run(outcome),
            None => Reply::Consumed,
        }
    }

    fn open_menu(&mut self) -> Reply<O> {
        let Some(row) = self.selected_key else {
            return Reply::Consumed;
        };
        let actions = self.source.actions(row);
        if actions.is_empty() {
            return Reply::Consumed;
        }
        self.menu = Some(ActionMenu {
            actions,
            filter: Box::new(TextField::line("")),
            row,
            selected: 0,
        });
        Reply::Consumed
    }

    fn act_in_menu(&mut self, act: PickerAct) -> Reply<O> {
        let Some(menu) = &mut self.menu else {
            return Reply::Ignored;
        };
        let count = menu.kept().len();
        match act {
            PickerAct::Accept => {
                let chosen = menu.kept().get(menu.selected).map(|action| action.id);
                let row = menu.row;
                self.menu = None;
                match chosen {
                    Some(action) => self.run(action, row),
                    None => Reply::Consumed,
                }
            }
            PickerAct::Close | PickerAct::ActionMenu => {
                self.menu = None;
                Reply::Consumed
            }
            PickerAct::Next | PickerAct::PageDown => {
                menu.selected = (menu.selected + 1).min(count.saturating_sub(1));
                Reply::Consumed
            }
            PickerAct::Previous | PickerAct::PageUp => {
                menu.selected = menu.selected.saturating_sub(1);
                Reply::Consumed
            }
            PickerAct::HistoryNext
            | PickerAct::HistoryPrevious
            | PickerAct::Save
            | PickerAct::TogglePanel => Reply::Ignored,
        }
    }

    fn step_history(&mut self, back: bool, now: Instant) -> Reply<O> {
        if self.history.is_empty() {
            return Reply::Consumed;
        }
        let at = match (self.history_at, back) {
            (None, true) => {
                self.live_query = self.query().to_string();
                Some(0)
            }
            (None, false) => None,
            (Some(at), true) => Some((at + 1).min(self.history.len() - 1)),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
        self.history_at = at;
        self.seeded = false;
        let text = match at {
            Some(at) => self.history[at].clone(),
            None => self.live_query.clone(),
        };
        self.field.set_text(&text);
        self.query_changed(now);
        Reply::Consumed
    }

    fn save(&mut self) -> Reply<O> {
        if self.matched == 0 {
            return Reply::Notice(NOTHING_TO_SAVE.to_string());
        }
        if let Some(matcher) = &mut self.matcher {
            matcher.request_matched();
            self.saving = true;
        }
        Reply::Consumed
    }

    fn toggle_panel(&mut self, now: Instant) -> Reply<O> {
        if self.source.preview_kind() == PreviewKind::Panel {
            self.panel = !self.panel;
            self.previewed = None;
            self.preview_due = Some(now);
        }
        Reply::Consumed
    }

    /// The keys the picker offers now: the action menu where the selected
    /// row has more than one action, the panel where it has one, saving
    /// where it can.
    fn hints(&self) -> Vec<KeyHint> {
        let has_menu = self
            .selected_key
            .is_some_and(|row| self.source.actions(row).len() > 1);
        let has_panel = self.source.preview_kind() == PreviewKind::Panel;
        [
            (has_menu, PickerAct::ActionMenu, Some(HINT_ACTIONS)),
            // Said by the command's own name, as the Keys editor lists it.
            (has_panel, PickerAct::TogglePanel, None),
            (self.matched > 0, PickerAct::Save, Some(HINT_SAVE)),
        ]
        .into_iter()
        .filter(|(offered, _, _)| *offered)
        .filter_map(|(_, act, hint)| {
            menu_entry(&KEYS, act).map(|(key, name)| KeyHint {
                key: format_key(&key),
                label: hint.unwrap_or(name).to_string(),
            })
        })
        .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The letters lit in either of `a` and `b`, each once, in order.
/// `row` as a line of a saved buffer: its label, then its description (a
/// file's directory, a buffer's kind) where it has one.
fn saved_line(row: &Row) -> String {
    match row.description.is_empty() {
        true => row.label.clone(),
        false => format!("{}{SAVED_DESCRIPTION_GAP}{}", row.label, row.description),
    }
}

fn merged(a: &Highlights, b: Highlights) -> Highlights {
    let join = |one: &[usize], mut two: Vec<usize>| {
        two.extend_from_slice(one);
        two.sort_unstable();
        two.dedup();
        two
    };
    Highlights {
        description: join(&a.description, b.description),
        detail: join(&a.detail, b.detail),
        label: join(&a.label, b.label),
    }
}

// ========================================================================
// SessionView
// ========================================================================

impl SessionView {
    /// What the picker is headed by, and why its source failed, if it did.
    pub fn heading(&self) -> String {
        match &self.error {
            Some(error) => format!("{}: {error}", self.title),
            None => self.title.clone(),
        }
    }

    /// Where the selected row stands among the matches, `2/10`, and whether
    /// more are coming or the cap stopped them: what a host shows beside the
    /// heading, apart from it so a long heading never cuts it short. Empty
    /// for a picker with no rows at all.
    pub fn count(&self) -> String {
        let mut count = String::new();
        if self.total > 0 {
            let at = self.position.map_or(0, |at| at + 1);
            count.push_str(&format!("{at}/{}", self.matched));
        }
        match self.status {
            Status::Capped => count.push_str(CAPPED_MARK),
            Status::Done => {}
            Status::Working => count.push_str(WORKING_MARK),
        }
        count
    }
}

// ========================================================================
// Parked
// ========================================================================

impl<O> Parked<O> {
    /// The picker's name.
    pub fn id(&self) -> &'static str {
        self.0.source.id()
    }

    /// The session as it was closed, over `source` in place of its own: a
    /// listing of what the host holds (tabs, buffers) is built afresh by the
    /// host, which alone can see it, and read with the query and selection
    /// the session had.
    pub fn resume_with(self, source: Box<dyn Source<O>>) -> Picker<O> {
        let mut parked = self;
        parked.0.source = source;
        parked.resume()
    }

    /// The session as it was closed: a listing reads its rows again and
    /// selects the same row if it is still there; a search goes on past the
    /// rows it has.
    pub fn resume(self) -> Picker<O> {
        let mut picker = *self.0;
        picker.moved = true;
        match picker.source.resume() {
            Resume::Continue => {
                picker.source.refresh();
                picker.refeed(false);
            }
            Resume::Never | Resume::Reread => picker.refeed(true),
        }
        picker
    }
}

// ========================================================================
// ActionMenu
// ========================================================================

impl ActionMenu {
    /// The actions the filter keeps, in order.
    fn kept(&self) -> Vec<Action> {
        let query = Query::parse(self.filter.text());
        self.actions
            .iter()
            .filter(|action| query.score_text(&action.title).is_some())
            .cloned()
            .collect()
    }

    fn view(&self) -> MenuView {
        MenuView {
            actions: self.kept(),
            filter: self.filter.text().to_string(),
            selected: self.selected,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{batch, open, rows, Clock, FakeOutcome, FakeSource, Picked, Script, PICK};
    use crate::source::ActionId;
    use crate::MatchOn;

    fn selected(picker: &Picker<FakeOutcome>) -> Option<String> {
        let view = picker.view();
        view.selected.map(|at| view.rows[at].row.label.clone())
    }

    fn run(
        picker: &mut Picker<FakeOutcome>,
        clock: &mut Clock,
        script: &str,
    ) -> Vec<Reply<FakeOutcome>> {
        Script::parse(script).run(picker, clock)
    }

    /// A picker matching on its own thread, as the window opens one.
    fn open_threaded(source: FakeSource) -> Picker<FakeOutcome> {
        Picker::open(
            Box::new(source),
            Vec::new(),
            Recency::default(),
            false,
            false,
        )
    }

    #[test]
    fn test_a_move_past_the_screen_shows_the_next_rows_before_the_matcher_answers() {
        let labels: Vec<String> = (0..100).map(|at| format!("row {at:02}")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut picker = open(FakeSource::rows(&labels));
        let now = Instant::now();
        picker.set_lines(10);
        picker.tick(now);
        for _ in 0..12 {
            picker.act(PickerAct::Next, 10, now);
        }
        // No tick: what is drawn now comes from the rows already held.
        let view = picker.view();
        assert_eq!(selected(&picker).as_deref(), Some("row 12"));
        assert_eq!(view.rows[0].row.label, "row 03", "scrolled by the move");
        assert_eq!(
            view.selected,
            Some(9),
            "the selection at the bottom of the screen"
        );
        picker.tick(now);
        assert_eq!(
            selected(&picker).as_deref(),
            Some("row 12"),
            "the answer agrees"
        );
        assert_eq!(picker.view().rows[0].row.label, "row 03");
    }

    #[test]
    fn test_the_count_says_where_the_selected_row_stands_among_the_matches() {
        let mut picker = open(FakeSource::rows(&["apple", "apricot", "banana"]));
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "tick");
        assert_eq!(picker.view().count(), "1/3");
        picker.act(PickerAct::Next, 10, clock.now());
        picker.tick(clock.now());
        assert_eq!(picker.view().count(), "2/3", "moves with the selection");
        run(&mut picker, &mut clock, "a p tick");
        assert_eq!(
            picker.view().count(),
            "1/2",
            "out of the matches, not every row"
        );
        run(&mut picker, &mut clock, "z z tick");
        assert_eq!(picker.view().count(), "0/0", "nothing matches");
    }

    #[test]
    fn test_the_count_says_more_rows_are_coming_and_none_for_no_rows() {
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "tick");
        assert_eq!(picker.view().count(), " …", "no rows yet, more coming");
        feed.send_batch(batch(&["a"], 0));
        run(&mut picker, &mut clock, "tick");
        assert_eq!(picker.view().count(), "1/1 …");
    }

    #[test]
    fn test_settling_waits_for_the_first_rows_of_a_threaded_matcher() {
        let mut picker = open_threaded(FakeSource::rows(&["a", "b", "c"]));
        assert!(picker.settle(Duration::from_secs(5)));
        assert_eq!(
            picker.view().rows.len(),
            3,
            "drawn first with its rows, not empty"
        );
    }

    #[test]
    fn test_settling_a_picker_with_no_rows_does_not_wait_out_its_patience() {
        let started = Instant::now();
        let mut picker = open_threaded(FakeSource::rows(&[]));
        assert!(picker.settle(Duration::from_secs(5)));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the idle answer ends it"
        );
    }

    #[test]
    fn test_settling_a_stream_that_gives_nothing_yet_gives_up_at_its_patience() {
        let (source, _feed) = FakeSource::stream();
        let mut picker = open_threaded(source);
        assert!(!picker.settle(Duration::from_millis(20)));
        assert!(picker.view().rows.is_empty());
    }

    #[test]
    fn test_a_picker_opens_before_its_rows_arrive() {
        let (source, _feed) = FakeSource::stream();
        let picker = open(source);
        let view = picker.view();
        assert_eq!(view.title, "Fake");
        assert_eq!(view.status, Status::Working);
        assert!(view.rows.is_empty());
    }

    #[test]
    fn test_the_selection_stays_on_its_row_as_rows_arrive() {
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        let mut clock = Clock::new();
        feed.send(&["xa", "ya"], 0);
        run(&mut picker, &mut clock, "a C-n");
        assert_eq!(selected(&picker).as_deref(), Some("ya"));
        let line = picker.view().selected;
        feed.send(&["a"], 5);
        run(&mut picker, &mut clock, "tick");
        assert_eq!(
            picker.view().matched,
            3,
            "the better match came in above it"
        );
        assert_eq!(selected(&picker).as_deref(), Some("ya"));
        assert_eq!(
            picker.view().selected,
            line,
            "and the view did not move under it"
        );
    }

    #[test]
    fn test_before_moving_the_selection_stays_on_the_best_match() {
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        let mut clock = Clock::new();
        feed.send(&["xa", "ya"], 0);
        run(&mut picker, &mut clock, "a");
        feed.send(&["a"], 5);
        run(&mut picker, &mut clock, "tick");
        assert_eq!(selected(&picker).as_deref(), Some("a"));
    }

    #[test]
    fn test_accept_runs_the_selected_rows_first_action() {
        let mut picker = open(FakeSource::rows(&["one", "two"]));
        let replies = run(&mut picker, &mut Clock::new(), "C-n RET");
        let picked = Picked {
            action: PICK,
            row: RowKey(1),
        };
        assert_eq!(
            replies,
            [Reply::Consumed, Reply::Run(FakeOutcome::Picked(picked))]
        );
    }

    #[test]
    fn test_the_action_menu_runs_the_action_chosen_and_esc_goes_back() {
        let source = FakeSource::rows(&["one"]).actions(&["open", "beside", "kill"]);
        let mut picker = open(source);
        let mut clock = Clock::new();
        let replies = run(&mut picker, &mut clock, "F4 C-n RET");
        let picked = Picked {
            action: ActionId("beside"),
            row: RowKey(0),
        };
        assert_eq!(replies[2], Reply::Run(FakeOutcome::Picked(picked)));
        run(&mut picker, &mut clock, "F4 ESC");
        assert!(
            picker.view().menu.is_none(),
            "Esc closes the menu, not the picker"
        );
        let picked = Picked {
            action: ActionId("open"),
            row: RowKey(0),
        };
        assert_eq!(
            run(&mut picker, &mut clock, "RET"),
            [Reply::Run(FakeOutcome::Picked(picked))]
        );
    }

    #[test]
    fn test_a_click_selects_in_the_action_menu_while_it_is_open() {
        let source = FakeSource::rows(&["one", "two"]).actions(&["open", "beside", "kill"]);
        let mut picker = open(source);
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "F4");
        picker.select_shown(2);
        let picked = Picked {
            action: ActionId("kill"),
            row: RowKey(0),
        };
        assert_eq!(
            run(&mut picker, &mut clock, "RET"),
            [Reply::Run(FakeOutcome::Picked(picked))],
            "the menu's third entry, on the row the menu was opened over"
        );
    }

    #[test]
    fn test_typing_filters_the_action_menu() {
        let source = FakeSource::rows(&["one"]).actions(&["open", "beside", "kill"]);
        let mut picker = open(source);
        let replies = run(&mut picker, &mut Clock::new(), "F4 k RET");
        let picked = Picked {
            action: ActionId("kill"),
            row: RowKey(0),
        };
        assert_eq!(replies[2], Reply::Run(FakeOutcome::Picked(picked)));
    }

    #[test]
    fn test_a_picker_whose_query_is_the_answer_hands_it_on() {
        let mut picker = open(FakeSource::rows(&[]).matching(Matching::Answer));
        let replies = run(&mut picker, &mut Clock::new(), "h i RET");
        assert_eq!(
            replies[2],
            Reply::Run(FakeOutcome::Answered("hi".to_string()))
        );
    }

    #[test]
    fn test_a_source_matching_by_itself_is_fed_once_the_typing_pauses() {
        let source = FakeSource::rows(&["a"]).matching(Matching::BySource(MatchOn::LABEL));
        let log = source.log();
        let mut picker = open(source);
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "x y");
        assert_eq!(log.lock().unwrap().fed, [""], "not while typing");
        run(&mut picker, &mut clock, "tick");
        assert_eq!(log.lock().unwrap().fed, ["", "xy"]);
    }

    #[test]
    fn test_save_hands_over_the_matched_rows_in_order() {
        let mut picker = open(FakeSource::rows(&["xb", "b", "c"]).saving());
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "b");
        picker.act(PickerAct::Save, 10, clock.now());
        let tick = picker.tick(clock.now());
        assert_eq!(
            tick.outcome,
            Some(FakeOutcome::Saved(vec![RowKey(1), RowKey(0)]))
        );
    }

    #[test]
    fn test_a_picker_with_no_saving_of_its_own_saves_its_rows_as_lines() {
        let mut picker = open(FakeSource::rows(&["xb", "b", "c"]));
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "b");
        picker.act(PickerAct::Save, 10, clock.now());
        let tick = picker.tick(clock.now());
        assert_eq!(tick.outcome, None);
        assert_eq!(
            tick.saved,
            Some(SavedLines {
                lines: vec!["b".to_string(), "xb".to_string()],
                name: "Fake".to_string(),
                query: "b".to_string(),
            })
        );
    }

    #[test]
    fn test_a_seed_is_replaced_by_typing_and_cleared_by_a_deletion() {
        let now = Instant::now();
        let typed = |code: KeyCode| Key::plain(code);

        let mut picker = open(FakeSource::rows(&["foo", "bar"]));
        picker.seed("foo", now);
        assert_eq!((picker.query(), picker.view().seeded), ("foo", true));
        picker.key(&typed(KeyCode::Char('b')), now);
        assert_eq!((picker.query(), picker.view().seeded), ("b", false));

        let mut picker = open(FakeSource::rows(&["foo"]));
        picker.seed("foo", now);
        picker.key(&typed(KeyCode::Backspace), now);
        assert_eq!(picker.query(), "", "the whole seed goes");
    }

    #[test]
    fn test_moving_in_a_seed_keeps_it_to_edit() {
        let now = Instant::now();
        let mut picker = open(FakeSource::rows(&["foo"]));
        picker.seed("foo", now);
        picker.key(&Key::plain(KeyCode::Left), now);
        picker.key(&Key::plain(KeyCode::Char('x')), now);
        assert_eq!(picker.query(), "foxo");
        assert!(!picker.view().seeded);
    }

    #[test]
    fn test_a_seed_never_replaces_a_query_already_there() {
        let now = Instant::now();
        let mut picker = open(FakeSource::rows(&["foo"]));
        picker.key(&Key::plain(KeyCode::Char('b')), now);
        picker.seed("foo", now);
        assert_eq!((picker.query(), picker.view().seeded), ("b", false));
    }

    #[test]
    fn test_a_picker_with_no_rows_showing_has_nothing_to_save() {
        let mut picker = open(FakeSource::rows(&["a"]));
        let replies = run(&mut picker, &mut Clock::new(), "z z z tick C-S-s");
        assert_eq!(
            replies.last(),
            Some(&Reply::Notice(NOTHING_TO_SAVE.to_string()))
        );
    }

    #[test]
    fn test_f6_shows_and_hides_the_panel_and_a_live_picker_keeps_its_preview() {
        let mut panel = open(FakeSource::rows(&["a"]).preview(PreviewKind::Panel));
        let mut clock = Clock::new();
        assert!(panel.view().panel);
        run(&mut panel, &mut clock, "F6");
        assert!(!panel.view().panel);
        let mut live = open(FakeSource::rows(&["a"]).preview(PreviewKind::Live));
        run(&mut live, &mut clock, "tick");
        run(&mut live, &mut clock, "F6");
        assert_eq!(
            live.tick(clock.now()).preview,
            None,
            "F6 changes nothing in a live picker"
        );
    }

    #[test]
    fn test_the_selected_row_is_previewed_once_the_selection_rests() {
        let mut picker = open(FakeSource::rows(&["a", "b"]).preview(PreviewKind::Panel));
        let mut clock = Clock::new();
        assert_eq!(
            picker.tick(clock.now()).preview,
            None,
            "not before the pause"
        );
        clock.advance(PREVIEW_PAUSE);
        let shown = PreviewUpdate::Show(Preview::Text("row 0".to_string()));
        assert_eq!(picker.tick(clock.now()).preview, Some(shown));
        assert_eq!(picker.tick(clock.now()).preview, None, "once per row");
    }

    #[test]
    fn test_a_failing_source_keeps_its_rows_and_says_why() {
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        feed.send(&["a", "b"], 0);
        feed.send_batch(Batch {
            done: true,
            error: Some("rg missing".to_string()),
            ..Batch::default()
        });
        run(&mut picker, &mut Clock::new(), "tick");
        let view = picker.view();
        assert_eq!(view.error.as_deref(), Some("rg missing"));
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.status, Status::Done);
    }

    #[test]
    fn test_paging_moves_by_the_rows_that_fit_in_the_lines() {
        let two_lines: Vec<Row> = rows(&["a"; 20])
            .into_iter()
            .zip(0..)
            .map(|(row, key)| Row {
                key: RowKey(key),
                ..row.with_detail("text")
            })
            .collect();
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        let mut clock = Clock::new();
        feed.send_batch(Batch {
            rows: two_lines,
            ..Batch::default()
        });
        run(&mut picker, &mut clock, "tick");
        picker.act(PickerAct::PageDown, 10, clock.now());
        picker.tick(clock.now());
        assert_eq!(
            picker.view().selected.map(|at| at + picker.first),
            Some(5),
            "ten lines hold five"
        );
    }

    #[test]
    fn test_a_resumed_listing_reads_again_and_selects_the_same_row() {
        let source = FakeSource::rows(&["a", "b", "c"]);
        let log = source.log();
        let mut picker = open(source);
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "C-n C-n");
        let mut picker = picker.park().map(Parked::resume).unwrap();
        picker.tick(clock.now());
        picker.tick(clock.now());
        assert_eq!(log.lock().unwrap().fed.len(), 2);
        assert_eq!(selected(&picker).as_deref(), Some("c"));
    }

    #[test]
    fn test_a_resumed_search_keeps_its_rows_and_goes_on() {
        let (source, feed) = FakeSource::stream();
        let source = source.resume(Resume::Continue);
        let log = source.log();
        let mut picker = open(source);
        let mut clock = Clock::new();
        feed.send_batch(batch(&["a", "b"], 0));
        run(&mut picker, &mut clock, "tick");
        let mut picker = picker.park().map(Parked::resume).unwrap();
        picker.tick(clock.now());
        assert_eq!(log.lock().unwrap().refreshed, 1);
        assert_eq!(picker.view().total, 2, "the rows it had are kept");
    }

    #[test]
    fn test_a_session_resumed_over_a_fresh_source_reads_it_with_its_query() {
        let mut picker = open(FakeSource::rows(&["xa", "ya"]));
        let mut clock = Clock::new();
        run(&mut picker, &mut clock, "a C-n");
        let parked = picker.park().unwrap();
        let fresh = FakeSource::rows(&["xa", "ya", "za"]);
        let mut picker = parked.resume_with(Box::new(fresh));
        picker.tick(clock.now());
        picker.tick(clock.now());
        assert_eq!(picker.query(), "a");
        assert_eq!(picker.view().matched, 3);
        assert_eq!(selected(&picker).as_deref(), Some("ya"));
    }

    #[test]
    fn test_a_picker_preselected_on_a_row_starts_on_it() {
        let mut picker = open(FakeSource::rows(&["a", "b", "c"]));
        picker.preselect(RowKey(2));
        picker.tick(Instant::now());
        assert_eq!(selected(&picker).as_deref(), Some("c"));
    }

    #[test]
    fn test_a_batch_may_say_which_row_to_start_on() {
        let (source, feed) = FakeSource::stream();
        let mut picker = open(source);
        feed.send_batch(Batch {
            select: Some(RowKey(2)),
            ..batch(&["a", "b", "c"], 0)
        });
        picker.tick(Instant::now());
        picker.tick(Instant::now());
        assert_eq!(selected(&picker).as_deref(), Some("c"));
    }

    #[test]
    fn test_a_session_that_answered_cannot_be_parked() {
        let picker = open(FakeSource::rows(&["a"]).resume(Resume::Never));
        assert!(picker.park().is_none());
    }

    #[test]
    fn test_the_key_hints_name_only_what_the_picker_offers() {
        let mut bare = open(FakeSource::rows(&["a"]));
        bare.tick(Instant::now());
        let keys: Vec<String> = bare.view().hints.into_iter().map(|hint| hint.key).collect();
        assert_eq!(keys, ["F3"], "every picker with rows saves them");
        let full = FakeSource::rows(&["a"])
            .actions(&["open", "beside"])
            .preview(PreviewKind::Panel)
            .saving();
        let mut full = open(full);
        full.tick(Instant::now());
        let hints: Vec<(String, String)> = full
            .view()
            .hints
            .into_iter()
            .map(|hint| (hint.key, hint.label))
            .collect();
        let hints: Vec<(&str, &str)> = hints
            .iter()
            .map(|(k, l)| (k.as_str(), l.as_str()))
            .collect();
        assert_eq!(
            hints,
            [("F4", "actions"), ("F6", "Toggle Preview"), ("F3", "save")]
        );
    }
}
