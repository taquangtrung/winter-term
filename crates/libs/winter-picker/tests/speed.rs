//! The design's speed targets, held to numbers: run with
//! `cargo test -p winter-picker --release -- --ignored speed`. Ignored in an
//! ordinary run, since a debug build is slower than any target.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use winter_picker::{
    Action, ActionId, Batch, Feed, MatchOn, MatchReply, Matcher, Matching, Picker, PickerAct,
    Recency, Resume, Row, RowKey, Source,
};

// ========================================================================
// Constants
// ========================================================================

/// One frame at 60 Hz.
const FRAME: Duration = Duration::from_millis(16);

/// How long a key may take to be handled while a batch is scored.
const KEY_BUDGET: Duration = Duration::from_millis(1);

/// How many files a large project lists.
const FILES: usize = 50_000;

/// How many lines a long scrollback holds.
const LINES: usize = 1_000_000;

/// How long a check waits for an answer before failing.
const PATIENCE: Duration = Duration::from_secs(30);

// ========================================================================
// Data Structures
// ========================================================================

/// A source streaming `rows`, all in one batch, as soon as it is fed.
struct Streaming(Option<Vec<Row>>);

impl Source<()> for Streaming {
    fn id(&self) -> &'static str {
        "speed"
    }

    fn title(&self) -> String {
        "Speed".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::Words(MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Never
    }

    fn feed(&mut self, _query: &str) -> Feed {
        let (send, receive) = mpsc::channel();
        let rows = self.0.take().unwrap_or_default();
        thread::spawn(move || {
            let _ = send.send(Batch {
                done: true,
                rows,
                ..Batch::default()
            });
        });
        Feed::Stream(receive)
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        Vec::new()
    }

    fn run(&mut self, _action: ActionId, _row: RowKey, _query: &str) -> Option<()> {
        None
    }
}

// ========================================================================
// Functions
// ========================================================================

fn paths(count: usize) -> Vec<Row> {
    (0..count)
        .map(|n| {
            let label = format!("crates/winter-{}/src/module{}/file{n}.rs", n % 17, n % 311);
            Row::new(RowKey(n as u64), label)
        })
        .collect()
}

fn lines(count: usize) -> Vec<Row> {
    (0..count)
        .map(|n| {
            let text = format!(
                "{n:>7}  cargo test --workspace -p crate{} -- line {n} output",
                n % 97
            );
            Row::new(RowKey(n as u64), format!("{n}")).with_description(text)
        })
        .collect()
}

/// The answer once every command given has been applied.
fn settle(matcher: &mut Matcher) -> MatchReply {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(reply) = matcher.poll().filter(|reply| !reply.working) {
            return reply;
        }
        assert!(Instant::now() < deadline, "the matcher never settled");
        thread::yield_now();
    }
}

/// How long from a query to its answer, over a matcher holding `rows`.
fn answer_time(rows: Vec<Row>, on: MatchOn, query: &str) -> Duration {
    let mut matcher = Matcher::threaded(on);
    matcher.push(rows);
    matcher.request_slice(0, 30);
    settle(&mut matcher);
    let start = Instant::now();
    matcher.set_query(query);
    settle(&mut matcher);
    start.elapsed()
}

// ========================================================================
// Tests
// ========================================================================

#[test]
#[ignore = "speed: run with --release -- --ignored speed"]
fn speed_a_picker_opens_within_a_frame() {
    let source = Box::new(Streaming(Some(paths(FILES))));
    let start = Instant::now();
    let picker = Picker::open(source, Vec::new(), Recency::default(), false, false);
    let took = start.elapsed();
    drop(picker);
    assert!(took < FRAME, "opening took {took:?}");
}

#[test]
#[ignore = "speed: run with --release -- --ignored speed"]
fn speed_a_key_over_50_000_files_answers_within_a_frame() {
    let took = answer_time(paths(FILES), MatchOn::LABEL, "m");
    assert!(took < FRAME, "the answer took {took:?}");
}

#[test]
#[ignore = "speed: run with --release -- --ignored speed"]
fn speed_a_key_over_a_million_lines_answers_within_a_frame() {
    let took = answer_time(lines(LINES), MatchOn::DESCRIPTION, "c");
    assert!(took < FRAME, "the answer took {took:?}");
}

#[test]
#[ignore = "speed: run with --release -- --ignored speed"]
fn speed_a_key_is_never_held_up_by_a_batch_being_scored() {
    let source = Box::new(Streaming(Some(paths(FILES * 4))));
    let mut picker = Picker::open(source, Vec::new(), Recency::default(), false, false);
    let mut slowest = Duration::ZERO;
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        let start = Instant::now();
        picker.tick(Instant::now());
        picker.act(PickerAct::Next, 10, Instant::now());
        slowest = slowest.max(start.elapsed());
    }
    assert!(slowest < KEY_BUDGET, "a key took {slowest:?}");
}
