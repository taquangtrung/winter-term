//! What the tests of pickers build on: a source whose rows, batches,
//! failures, actions, and previews a test scripts, and acts and keys fed to a
//! picker in order, on a clock the test moves, so no test waits on a thread.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winter_model::input::table::{find, keys};

use super::keys::KEYS;
use super::row::{Highlights, MatchOn, Row, RowKey};
use super::session::{Picker, Reply};
use super::source::{
    Action, ActionId, Batch, Feed, Matching, Preview, PreviewKind, Resume, Source,
};

// ========================================================================
// Constants
// ========================================================================

/// The fake source's one action unless a test gives it more.
pub const PICK: ActionId = ActionId("pick");

/// How far one `tick` step of a script moves its clock: past every pause a
/// picker waits.
const TICK_STEP: Duration = Duration::from_secs(1);

/// How many lines a script's picker shows.
const SCRIPT_LINES: usize = 10;

// ========================================================================
// Data Structures
// ========================================================================

/// A source a test scripts.
pub struct FakeSource {
    actions: Vec<Action>,
    can_save: bool,
    /// The rows of a fixed list, or the batches of a stream sent through
    /// the feed handed back by `stream`.
    fixed: Option<Vec<Row>>,
    matching: Matching,
    preview: PreviewKind,
    resume: Resume,
    stream: Option<mpsc::Receiver<Batch>>,
    log: Arc<Mutex<FakeLog>>,
}

/// What a fake source was asked, for a test to read after the picker took it.
#[derive(Debug, Default)]
pub struct FakeLog {
    /// The queries fed, in order.
    pub fed: Vec<String>,
    /// How many times it was refreshed.
    pub refreshed: usize,
}

/// The line a test sends a streaming fake source's batches down.
pub struct FakeFeed(Sender<Batch>);

/// What a fake source's rows come to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FakeOutcome {
    /// The query typed, as an answer.
    Answered(String),
    /// An action run on a row.
    Picked(Picked),
    /// The rows saved, in order.
    Saved(Vec<RowKey>),
}

/// An action run on a row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picked {
    /// The action.
    pub action: ActionId,
    /// The row.
    pub row: RowKey,
}

/// Acts, keys, and ticks fed to a picker in order.
pub struct Script(Vec<Step>);

/// One step of a script.
enum Step {
    Key(winter_model::input::Key),
    Tick,
}

/// A clock a test moves.
pub struct Clock(Instant);

// ========================================================================
// FakeSource
// ========================================================================

impl FakeSource {
    /// A fixed list of `labels`, keyed by their places.
    pub fn rows(labels: &[&str]) -> Self {
        Self::with(Some(rows(labels)), None)
    }

    /// A stream whose batches the test sends down the feed.
    pub fn stream() -> (Self, FakeFeed) {
        let (send, receive) = mpsc::channel();
        (Self::with(None, Some(receive)), FakeFeed(send))
    }

    /// The same source, its rows matched as `matching` says.
    pub fn matching(self, matching: Matching) -> Self {
        Self { matching, ..self }
    }

    /// The same source, resumed as `resume` says.
    pub fn resume(self, resume: Resume) -> Self {
        Self { resume, ..self }
    }

    /// The same source, previewing as `kind` says: each row its label.
    pub fn preview(self, kind: PreviewKind) -> Self {
        Self {
            preview: kind,
            ..self
        }
    }

    /// The same source, its rows doing `actions`.
    pub fn actions(self, ids: &[&'static str]) -> Self {
        let actions = ids
            .iter()
            .map(|id| Action {
                button: None,
                id: ActionId(id),
                title: id.to_string(),
            })
            .collect();
        Self { actions, ..self }
    }

    /// The same source, able to save its rows.
    pub fn saving(self) -> Self {
        Self {
            can_save: true,
            ..self
        }
    }

    /// What the source was asked, readable once the picker owns it.
    pub fn log(&self) -> Arc<Mutex<FakeLog>> {
        Arc::clone(&self.log)
    }

    fn with(fixed: Option<Vec<Row>>, stream: Option<mpsc::Receiver<Batch>>) -> Self {
        Self {
            actions: vec![Action {
                button: None,
                id: PICK,
                title: PICK.0.to_string(),
            }],
            can_save: false,
            fixed,
            log: Arc::default(),
            matching: Matching::Words(MatchOn::LABEL),
            preview: PreviewKind::None,
            resume: Resume::Reread,
            stream,
        }
    }
}

impl FakeSource {
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeLog> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Source<FakeOutcome> for FakeSource {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn title(&self) -> String {
        "Fake".to_string()
    }

    fn matching(&self) -> Matching {
        self.matching
    }

    fn resume(&self) -> Resume {
        self.resume
    }

    fn preview_kind(&self) -> PreviewKind {
        self.preview
    }

    fn feed(&mut self, query: &str) -> Feed {
        self.lock().fed.push(query.to_string());
        match (&self.fixed, self.stream.take()) {
            (_, Some(stream)) => Feed::Stream(stream),
            (Some(rows), None) => Feed::Fixed(rows.clone()),
            (None, None) => Feed::Fixed(Vec::new()),
        }
    }

    fn highlights(&self, _row: &Row, _query: &str) -> Highlights {
        Highlights::default()
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        self.actions.clone()
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<FakeOutcome> {
        Some(FakeOutcome::Picked(Picked { action, row }))
    }

    fn answer(&mut self, query: &str) -> Option<FakeOutcome> {
        Some(FakeOutcome::Answered(query.to_string()))
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        Some(Preview::Text(format!("row {}", row.0)))
    }

    fn can_save(&self) -> bool {
        self.can_save
    }

    fn refresh(&mut self) {
        self.lock().refreshed += 1;
    }

    fn export(&self, rows: &[RowKey], _query: &str) -> Option<FakeOutcome> {
        Some(FakeOutcome::Saved(rows.to_vec()))
    }
}

// ========================================================================
// FakeFeed
// ========================================================================

impl FakeFeed {
    /// Send `labels` as one batch, keyed from `first`.
    pub fn send(&self, labels: &[&str], first: u64) {
        self.send_batch(batch(labels, first));
    }

    /// Send a batch as it is.
    pub fn send_batch(&self, batch: Batch) {
        // A picker gone has nothing to take the batch.
        let _ = self.0.send(batch);
    }

    /// Send the last batch.
    pub fn finish(&self) {
        self.send_batch(Batch {
            done: true,
            ..Batch::default()
        });
    }
}

// ========================================================================
// Script
// ========================================================================

impl Script {
    /// Keys written as `keybindings.kdl` writes them, separated by spaces;
    /// `tick` lets the clock pass every pause.
    pub fn parse(script: &str) -> Self {
        Self(
            script
                .split_whitespace()
                .flat_map(|word| match word {
                    "tick" => vec![Step::Tick],
                    spec => keys(spec)
                        .unwrap_or_else(|| panic!("not a key: {spec}"))
                        .into_iter()
                        .map(Step::Key)
                        .collect(),
                })
                .collect(),
        )
    }

    /// Feed the steps to `picker` on `clock`, ticking after each; the
    /// replies of the keys, in order.
    pub fn run<O>(&self, picker: &mut Picker<O>, clock: &mut Clock) -> Vec<Reply<O>> {
        let mut replies = Vec::new();
        for step in &self.0 {
            picker.tick(clock.now());
            match step {
                Step::Key(key) => {
                    let reply = match find(&KEYS, key) {
                        Some(act) => picker.act(act, SCRIPT_LINES, clock.now()),
                        None => picker.key(key, clock.now()),
                    };
                    replies.push(reply);
                }
                Step::Tick => clock.advance(TICK_STEP),
            }
            picker.tick(clock.now());
        }
        replies
    }
}

// ========================================================================
// Clock
// ========================================================================

impl Clock {
    /// A clock at now.
    pub fn new() -> Self {
        Self(Instant::now())
    }

    /// The time on it.
    pub fn now(&self) -> Instant {
        self.0
    }

    /// Move it on by `by`.
    pub fn advance(&mut self, by: Duration) {
        self.0 += by;
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A picker over `source`, matching on the calling thread, with no history.
pub fn open(source: FakeSource) -> Picker<FakeOutcome> {
    Picker::open(
        Box::new(source),
        Vec::new(),
        super::history::Recency::default(),
        true,
        true,
    )
}

/// Rows of `labels`, keyed by their places.
pub fn rows(labels: &[&str]) -> Vec<Row> {
    keyed(labels, 0)
}

/// A batch of `labels`, keyed from `first`.
pub fn batch(labels: &[&str], first: u64) -> Batch {
    Batch {
        rows: keyed(labels, first),
        ..Batch::default()
    }
}

fn keyed(labels: &[&str], first: u64) -> Vec<Row> {
    labels
        .iter()
        .zip(first..)
        .map(|(label, key)| Row::new(RowKey(key), *label))
        .collect()
}
