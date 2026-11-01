//! A picker session's rows and their scoring, on a worker thread of their
//! own so the window never waits on them, or on the caller's thread for
//! tests that must not.
//!
//! Every query has a generation. Work on a query a newer one replaced is
//! dropped, and an answer of an older generation is never handed out, so a
//! slow match on "fo" never lands over the one for "foo". A query that only
//! narrows the last scores only the rows that matched it; a new batch is
//! scored alone and merged in. Rows that score the same keep the order they
//! came in, after those used more and more lately. A row pinned for the
//! query is listed after every match.

use std::cmp::Reverse;
use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use winter_base::wake::wake_window;

use super::query::{narrows, prepare, Prepared, Query};
use super::row::{Highlights, MatchOn, Row, RowKey};

// ========================================================================
// Constants
// ========================================================================

/// How many rows are scored between looks for a newer query.
const CHUNK: usize = 4096;

/// How often scoring across threads looks for a newer query.
const STOP_POLL: Duration = Duration::from_micros(100);

/// The most threads one query is scored on: past this, starting them costs
/// more than they save, the work being bound by memory.
const MAX_THREADS: usize = 16;

/// How many scores a match can have: the buckets matches are ordered in.
const SCORES: usize = 4;

/// The worker thread's name.
const THREAD_NAME: &str = "picker-matcher";

// ========================================================================
// Data Structures
// ========================================================================

/// The rows on screen and the counts, as of one generation.
#[derive(Clone, Debug)]
pub struct MatchReply {
    /// Whether it answers every command given, not only some of them: a
    /// reply to a selection moved since says where it was, not where it is.
    pub current: bool,
    /// The index among the matches of the first row given.
    pub first: usize,
    /// The query's generation.
    pub generation: u64,
    /// How many rows match.
    pub matched: usize,
    /// The rows asked for, with their highlights.
    pub rows: Vec<Shown>,
    /// Where the row being tracked is among the matches, when it matches.
    pub tracked: Option<usize>,
    /// How many rows there are in all.
    pub total: usize,
    /// Whether work the matcher was given is still to be done.
    pub working: bool,
}

/// One matching row and where it matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    /// The letters lit in each field.
    pub lit: Highlights,
    /// The row.
    pub row: Row,
}

/// A picker session's rows and their scoring.
pub struct Matcher {
    /// The current query's generation.
    generation: u64,
    inner: Inner,
    /// The keys of the rows matched, newest asked for, once they came.
    keys: Option<Vec<RowKey>>,
    /// The newest slice of the current generation, not yet polled.
    newest: Option<MatchReply>,
    /// How many commands were given; a reply that has applied fewer is
    /// still working.
    sent: u64,
}

/// Where the scoring runs.
enum Inner {
    Synchronous(Box<SyncState>),
    Threaded(Threaded),
}

/// Scoring on the caller's thread, and the answers it gave.
struct SyncState {
    answers: VecDeque<Answer>,
    worker: Worker,
}

/// The line to a worker thread.
struct Threaded {
    answers: Receiver<Answer>,
    commands: Sender<Command>,
}

/// What the matcher is asked.
enum Command {
    Clear,
    Keys,
    Pin(Option<Row>),
    Push(PushCommand),
    Query(QueryCommand),
    Slice(Window),
    Track(Option<Tracked>),
    Unscored,
}

/// Rows to add, and their ranks by use when the source ranks by it.
struct PushCommand {
    ranks: Option<Vec<u64>>,
    rows: Vec<Row>,
}

/// A new query, and its generation.
struct QueryCommand {
    generation: u64,
    query: Query,
}

/// A row whose place among the matches the answers follow, and the line
/// of the list it is kept on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tracked {
    /// The row.
    pub key: RowKey,
    /// How many rows of the list are above it.
    pub offset: usize,
}

/// The rows asked for: from the `first` match, `count` of them.
#[derive(Clone, Copy, Debug, Default)]
struct Window {
    count: usize,
    first: usize,
}

/// What the matcher answers.
enum Answer {
    Keys(Vec<RowKey>),
    Slice(Box<SliceAnswer>),
}

/// A slice of the matches, and how many commands it has applied.
struct SliceAnswer {
    applied: u64,
    reply: MatchReply,
}

/// The scoring state, whichever thread runs it.
struct Worker {
    applied: u64,
    engine: Engine,
    generation: u64,
    tracked: Option<Tracked>,
    window: Window,
}

/// One row as the matcher holds it: its place in arrival order is its
/// index among the candidates.
struct Candidate {
    prepared: Prepared,
    rank: u64,
    row: Row,
}

/// One match: the candidate's index, and its score. Where it matched is
/// found only for the rows on screen.
#[derive(Clone, Copy)]
struct Hit {
    at: usize,
    score: u8,
}

/// The rows a query is scored over: some of them by index, or all.
#[derive(Clone, Copy)]
struct Pool<'a> {
    /// The rows' indices, or `None` for every row.
    ats: Option<&'a [usize]>,
    len: usize,
}

/// Matches by score, lowest first.
type Buckets = [Vec<Hit>; SCORES];

/// The rows, the matches, and the query they are for.
struct Engine {
    candidates: Vec<Candidate>,
    /// The keys of every row held, so a row given again is not held twice.
    held: HashSet<RowKey>,
    hits: Vec<Hit>,
    /// Whether the hits are complete for the query: false while scoring
    /// was cut short, until the newer query or the clearing that cut it.
    complete: bool,
    on: MatchOn,
    /// The row listed after every match, whatever the query: the one the
    /// query itself stands for.
    pinned: Option<Row>,
    /// The query rows are matched against.
    query: Query,
    /// Whether any row has a rank by use, so ranks must be sorted by.
    ranked: bool,
    /// Whether the rows are kept as they came, matched by their source.
    unscored: bool,
}

// ========================================================================
// Matcher
// ========================================================================

impl Matcher {
    /// A matcher on a worker thread for rows matched on the fields `on`
    /// names; its answers wake the window.
    pub fn threaded(on: MatchOn) -> Self {
        let (commands, inbox) = mpsc::channel();
        let (outbox, answers) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || run_worker(Worker::new(on), &inbox, &outbox));
        // A thread that cannot start leaves a matcher that never answers,
        // the picker showing no rows rather than taking the window down.
        drop(spawned);
        Self::with(Inner::Threaded(Threaded { answers, commands }))
    }

    /// A matcher scoring on the calling thread, each call done before it
    /// returns: for tests, which must not wait on a thread.
    pub fn synchronous(on: MatchOn) -> Self {
        Self::with(Inner::Synchronous(Box::new(SyncState {
            answers: VecDeque::new(),
            worker: Worker::new(on),
        })))
    }

    /// Keep rows as they come, unscored: for a source that matches them.
    pub fn unscored(&mut self) {
        self.send(Command::Unscored);
    }

    /// Score against `query` from now on; its generation.
    pub fn set_query(&mut self, query: &str) -> u64 {
        self.generation += 1;
        let generation = self.generation;
        self.send(Command::Query(QueryCommand {
            generation,
            query: Query::parse(query),
        }));
        generation
    }

    /// Add `rows`, scored against the current query and merged in. The
    /// rows go to the worker as they are, so the caller does no work per row.
    pub fn push(&mut self, rows: Vec<Row>) {
        self.send(Command::Push(PushCommand { ranks: None, rows }));
    }

    /// Add `rows`, each ranked by its use as `ranks` says, row for row.
    pub fn push_ranked(&mut self, rows: Vec<Row>, ranks: Vec<u64>) {
        self.send(Command::Push(PushCommand {
            ranks: Some(ranks),
            rows,
        }));
    }

    /// List `row` after every match from now on, in place of the one pinned
    /// before; `None` lists none.
    pub fn pin(&mut self, row: Option<Row>) {
        self.send(Command::Pin(row));
    }

    /// Drop every row, for a source giving them again.
    pub fn clear(&mut self) {
        self.send(Command::Clear);
    }

    /// Ask for `count` matching rows from the `first`.
    pub fn request_slice(&mut self, first: usize, count: usize) {
        self.send(Command::Slice(Window { count, first }));
    }

    /// Follow a row in every answer: where it is among the matches, and the
    /// slice moved so it stays `offset` rows down the list, so a selection
    /// stays on its row, and its row in its place, as rows arrive.
    pub fn track(&mut self, tracked: Option<Tracked>) {
        self.send(Command::Track(tracked));
    }

    /// Ask for the keys of every matching row, in order, for saving them.
    pub fn request_keys(&mut self) {
        self.send(Command::Keys);
    }

    /// The newest answer of the current generation, if one came since.
    pub fn poll(&mut self) -> Option<MatchReply> {
        self.drain();
        self.newest.take()
    }

    /// The keys asked for with `request_keys`, once they came.
    pub fn take_keys(&mut self) -> Option<Vec<RowKey>> {
        self.drain();
        self.keys.take()
    }

    fn with(inner: Inner) -> Self {
        Self {
            generation: 0,
            inner,
            keys: None,
            newest: None,
            sent: 0,
        }
    }

    fn send(&mut self, command: Command) {
        self.sent += 1;
        match &mut self.inner {
            Inner::Synchronous(state) => {
                let SyncState { answers, worker } = state.as_mut();
                worker.apply(command, &mut || false, &mut |answer| {
                    answers.push_back(answer)
                });
                worker.answer_slice(&mut |answer| answers.push_back(answer));
            }
            Inner::Threaded(threaded) => {
                // A worker gone has nothing to say; the picker keeps what it has.
                let _ = threaded.commands.send(command);
            }
        }
    }

    fn next_answer(&mut self) -> Option<Answer> {
        match &mut self.inner {
            Inner::Synchronous(state) => state.answers.pop_front(),
            Inner::Threaded(threaded) => threaded.answers.try_recv().ok(),
        }
    }

    /// Take in every answer waiting: keys kept for `take_keys`, the newest
    /// slice of the current generation for `poll`, older ones dropped.
    fn drain(&mut self) {
        while let Some(answer) = self.next_answer() {
            match answer {
                Answer::Keys(keys) => self.keys = Some(keys),
                Answer::Slice(slice) if slice.reply.generation == self.generation => {
                    let mut reply = slice.reply;
                    reply.current = slice.applied == self.sent;
                    reply.working |= !reply.current;
                    self.newest = Some(reply);
                }
                Answer::Slice(_) => {}
            }
        }
    }
}

// ========================================================================
// Worker
// ========================================================================

impl Worker {
    fn new(on: MatchOn) -> Self {
        Self {
            applied: 0,
            engine: Engine::new(on),
            generation: 0,
            tracked: None,
            window: Window::default(),
        }
    }

    /// Do `command`; `stop` says whether a newer query is waiting, which
    /// cuts scoring short.
    fn apply(
        &mut self,
        command: Command,
        stop: &mut dyn FnMut() -> bool,
        answer: &mut dyn FnMut(Answer),
    ) {
        self.applied += 1;
        match command {
            Command::Clear => self.engine.clear(),
            Command::Keys => answer(Answer::Keys(self.engine.keys())),
            Command::Pin(row) => self.engine.pinned = row,
            Command::Push(push) => self.engine.push(push),
            Command::Query(command) => {
                self.generation = command.generation;
                self.engine.set_query(command.query, stop);
            }
            Command::Slice(window) => self.window = window,
            Command::Track(key) => self.tracked = key,
            Command::Unscored => self.engine.unscored = true,
        }
    }

    /// Answer with the rows asked for, as they stand.
    fn answer_slice(&self, answer: &mut dyn FnMut(Answer)) {
        let tracked = self
            .tracked
            .and_then(|tracked| Some((self.engine.position(tracked.key)?, tracked.offset)));
        let window = match tracked {
            Some((at, offset)) => Window {
                count: self.window.count,
                first: at.saturating_sub(offset),
            },
            None => self.window,
        };
        let (rows, matched) = self.engine.slice(window);
        answer(Answer::Slice(Box::new(SliceAnswer {
            applied: self.applied,
            reply: MatchReply {
                current: false,
                first: window.first,
                generation: self.generation,
                matched,
                rows,
                total: self.engine.candidates.len() + usize::from(self.engine.pinned.is_some()),
                tracked: tracked.map(|(at, _)| at),
                working: !self.engine.complete,
            },
        })));
    }
}

/// The worker thread: take every command waiting, apply them in order, then
/// answer once; a newer query arriving mid-scoring cuts the old one short.
fn run_worker(mut worker: Worker, inbox: &Receiver<Command>, outbox: &Sender<Answer>) {
    let mut waiting: VecDeque<Command> = VecDeque::new();
    loop {
        if waiting.is_empty() {
            match inbox.recv() {
                Ok(command) => waiting.push_back(command),
                Err(_) => return,
            }
        }
        waiting.extend(inbox.try_iter());
        while let Some(command) = waiting.pop_front() {
            let superseded = matches!(command, Command::Query(_))
                && waiting
                    .iter()
                    .any(|later| matches!(later, Command::Query(_)));
            if superseded {
                worker.applied += 1;
                continue;
            }
            let mut stop = || {
                waiting.extend(inbox.try_iter());
                waiting
                    .iter()
                    .any(|later| matches!(later, Command::Query(_) | Command::Clear))
            };
            let mut send = |answer| {
                let _ = outbox.send(answer);
            };
            worker.apply(command, &mut stop, &mut send);
        }
        let mut send = |answer| {
            let _ = outbox.send(answer);
        };
        worker.answer_slice(&mut send);
        wake_window();
    }
}

// ========================================================================
// Engine
// ========================================================================

impl Engine {
    fn new(on: MatchOn) -> Self {
        Self {
            candidates: Vec::new(),
            complete: true,
            held: HashSet::new(),
            hits: Vec::new(),
            on,
            pinned: None,
            query: Query::parse(""),
            ranked: false,
            unscored: false,
        }
    }

    /// Score the rows against `query`: only the last matches where it
    /// narrows the last query, else every row, across the machine's cores,
    /// cut short when `stop` says a newer query or a clearing waits.
    fn set_query(&mut self, query: Query, stop: &mut dyn FnMut() -> bool) {
        let narrowing = self.complete && narrows(&self.query, &query);
        self.query = query;
        if self.unscored {
            return;
        }
        let narrowed: Option<Vec<usize>> = narrowing.then(|| {
            let mut ats: Vec<usize> = self.hits.iter().map(|hit| hit.at).collect();
            ats.sort_unstable();
            ats
        });
        let pool = Pool {
            ats: narrowed.as_deref(),
            len: narrowed.as_ref().map_or(self.candidates.len(), Vec::len),
        };
        self.complete = false;
        let Some(scored) = self.score_all(pool, stop) else {
            return;
        };
        self.hits = match self.ranked {
            true => self.order(scored),
            false => scored,
        };
        self.complete = true;
    }

    /// The matches among the rows at `pool`, highest score first and in
    /// `pool`'s order within a score, scored on as many threads as help;
    /// `None` when `stop` cut it short.
    fn score_all(&self, pool: Pool, stop: &mut dyn FnMut() -> bool) -> Option<Vec<Hit>> {
        let threads = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_THREADS)
            .min(pool.len / CHUNK)
            .max(1);
        if threads == 1 {
            let mut buckets: Buckets = Default::default();
            for start in (0..pool.len).step_by(CHUNK) {
                if stop() {
                    return None;
                }
                self.score_into(pool, start..(start + CHUNK).min(pool.len), &mut buckets);
            }
            return Some(join_buckets(std::slice::from_ref(&buckets)));
        }
        let cancel = AtomicBool::new(false);
        let part = pool.len.div_ceil(threads);
        let parts: Vec<Buckets> = thread::scope(|scope| {
            let handles: Vec<_> = (0..pool.len)
                .step_by(part)
                .map(|start| {
                    let range = start..(start + part).min(pool.len);
                    let cancel = &cancel;
                    scope.spawn(move || self.score_part(pool, range, cancel))
                })
                .collect();
            while !handles.iter().all(|handle| handle.is_finished()) {
                if stop() {
                    cancel.store(true, Ordering::Relaxed);
                }
                thread::sleep(STOP_POLL);
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_default())
                .collect()
        });
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // Each part's buckets in pool order, high scores first: a bucket's
        // parts joined in order keep arrival order within a score.
        Some(join_buckets(&parts))
    }

    /// One thread's part of the scoring, its matches by score, stopping
    /// early once `cancel` is set.
    fn score_part(&self, pool: Pool, range: Range<usize>, cancel: &AtomicBool) -> Buckets {
        let mut buckets: Buckets = Default::default();
        for start in range.clone().step_by(CHUNK) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            self.score_into(pool, start..(start + CHUNK).min(range.end), &mut buckets);
        }
        buckets
    }

    /// Score the rows at `range` of `pool` into `buckets`.
    fn score_into(&self, pool: Pool, range: Range<usize>, buckets: &mut Buckets) {
        for hit in range.filter_map(|k| self.score(pool.at(k))) {
            buckets[bucket_of(hit.score)].push(hit);
        }
    }

    /// Add rows, scoring them alone and merging them in; a row already held
    /// is left out, as a search resumed gives its rows again.
    fn push(&mut self, push: PushCommand) {
        let start = self.candidates.len();
        let ranks = push.ranks.unwrap_or_default();
        for (at, row) in push.rows.into_iter().enumerate() {
            if !self.held.insert(row.key) {
                continue;
            }
            let rank = ranks.get(at).copied().unwrap_or_default();
            self.ranked |= rank > 0;
            self.candidates.push(Candidate {
                prepared: prepare(&row, self.on),
                rank,
                row,
            });
        }
        let fresh: Vec<Hit> = (start..self.candidates.len())
            .filter_map(|at| match self.unscored {
                true => Some(Hit { at, score: 0 }),
                false => self.score(at),
            })
            .collect();
        let fresh = self.order(fresh);
        let old = std::mem::take(&mut self.hits);
        self.hits = self.merge(old, fresh);
    }

    fn clear(&mut self) {
        self.candidates.clear();
        self.held.clear();
        self.hits.clear();
        self.ranked = false;
        self.complete = true;
    }

    fn score(&self, at: usize) -> Option<Hit> {
        let score = self.query.score_prepared(&self.candidates[at].prepared)?;
        Some(Hit { at, score })
    }

    /// The order of matches: score first, then use, then arrival. Rows kept
    /// unscored stay in arrival order.
    fn key(&self, hit: &Hit) -> (Reverse<u8>, Reverse<u64>, usize) {
        match self.unscored {
            true => (Reverse(0), Reverse(0), hit.at),
            false => (
                Reverse(hit.score),
                Reverse(self.candidates[hit.at].rank),
                hit.at,
            ),
        }
    }

    /// `hits`, given in arrival order, in the order of matches: by bucket
    /// when no row has a rank, in one pass that keeps arrival order within a
    /// score, else by sorting.
    fn order(&self, hits: Vec<Hit>) -> Vec<Hit> {
        if self.unscored {
            return hits;
        }
        if self.ranked {
            let mut hits = hits;
            hits.sort_by_key(|hit| self.key(hit));
            return hits;
        }
        let mut buckets: Buckets = Default::default();
        for hit in hits {
            buckets[bucket_of(hit.score)].push(hit);
        }
        join_buckets(std::slice::from_ref(&buckets))
    }

    /// Two runs of matches in order, as one.
    fn merge(&self, old: Vec<Hit>, fresh: Vec<Hit>) -> Vec<Hit> {
        if fresh.is_empty() {
            return old;
        }
        let mut merged = Vec::with_capacity(old.len() + fresh.len());
        let mut old = old.into_iter().peekable();
        let mut fresh = fresh.into_iter().peekable();
        loop {
            let take_old = match (old.peek(), fresh.peek()) {
                (Some(a), Some(b)) => self.key(a) <= self.key(b),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => return merged,
            };
            let next = match take_old {
                true => old.next(),
                false => fresh.next(),
            };
            merged.extend(next);
        }
    }

    /// The matches in `window`, with where each matched, and how many there
    /// are, the pinned row last among them.
    fn slice(&self, window: Window) -> (Vec<Shown>, usize) {
        let matched = self.hits.len() + usize::from(self.pinned.is_some());
        let end = window.first.saturating_add(window.count).min(matched);
        let mut rows: Vec<Shown> = self
            .hits
            .get(window.first.min(end)..end.min(self.hits.len()))
            .unwrap_or_default()
            .iter()
            .map(|hit| {
                let row = &self.candidates[hit.at].row;
                let lit = match self.unscored {
                    true => Highlights::default(),
                    false => self
                        .query
                        .score_row(row, self.on)
                        .map(|(_, lit)| lit)
                        .unwrap_or_default(),
                };
                Shown {
                    lit,
                    row: row.clone(),
                }
            })
            .collect();
        let pinned_in = (window.first..end).contains(&self.hits.len());
        if let Some(row) = self.pinned.as_ref().filter(|_| pinned_in) {
            rows.push(Shown {
                lit: Highlights::default(),
                row: row.clone(),
            });
        }
        (rows, matched)
    }

    /// Where `key`'s row is among the matches.
    fn position(&self, key: RowKey) -> Option<usize> {
        let pinned = self.pinned.as_ref().filter(|row| row.key == key);
        self.hits
            .iter()
            .position(|hit| self.candidates[hit.at].row.key == key)
            .or(pinned.map(|_| self.hits.len()))
    }

    fn keys(&self) -> Vec<RowKey> {
        self.hits
            .iter()
            .map(|hit| self.candidates[hit.at].row.key)
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

impl Pool<'_> {
    /// The index of the `k`th row of the pool.
    fn at(&self, k: usize) -> usize {
        self.ats.map_or(k, |ats| ats[k])
    }
}

/// The matches of `parts`, each a run of rows' buckets in arrival order, as
/// one list: highest score first, each score's parts in order, so arrival
/// order holds within a score. Copied once, into a list of its full size.
fn join_buckets(parts: &[Buckets]) -> Vec<Hit> {
    let total = parts.iter().flatten().map(Vec::len).sum();
    let mut hits = Vec::with_capacity(total);
    for score in (0..SCORES).rev() {
        for part in parts {
            hits.extend_from_slice(&part[score]);
        }
    }
    hits
}

/// The bucket of a match scoring `score`.
fn bucket_of(score: u8) -> usize {
    usize::from(score).min(SCORES - 1)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::time::{Duration, Instant};

    use super::*;

    /// How long a threaded test waits for its answer.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// Rows keyed by their labels, so a label given again is the same row.
    fn rows(labels: &[&str]) -> Vec<Row> {
        labels
            .iter()
            .map(|label| Row::new(key(label), *label))
            .collect()
    }

    fn key(label: &str) -> RowKey {
        let mut hasher = DefaultHasher::new();
        label.hash(&mut hasher);
        RowKey(hasher.finish())
    }

    fn labels(reply: &MatchReply) -> Vec<String> {
        reply
            .rows
            .iter()
            .map(|shown| shown.row.label.clone())
            .collect()
    }

    /// The answer once every command given has been applied.
    fn settle(matcher: &mut Matcher) -> MatchReply {
        let deadline = Instant::now() + PATIENCE;
        let mut last = None;
        loop {
            if let Some(reply) = matcher.poll() {
                last = Some(reply);
            }
            if last.as_ref().is_some_and(|reply| !reply.working) {
                return last.unwrap_or_else(|| unreachable!());
            }
            assert!(Instant::now() < deadline, "the matcher never settled");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn both() -> [Matcher; 2] {
        [
            Matcher::threaded(MatchOn::LABEL),
            Matcher::synchronous(MatchOn::LABEL),
        ]
    }

    #[test]
    fn test_a_stale_generation_never_lands_over_a_newer_one() {
        let many: Vec<String> = (0..20_000).map(|n| format!("file{n}.rs")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).chain(["foo.rs"]).collect();
        for mut matcher in both() {
            matcher.push(rows(&many));
            matcher.request_slice(0, 5);
            matcher.set_query("f");
            let newest = matcher.set_query("foo");
            let reply = settle(&mut matcher);
            assert_eq!(reply.generation, newest);
            assert_eq!(labels(&reply), ["foo.rs"]);
        }
    }

    #[test]
    fn test_narrowing_scores_as_scoring_every_row_afresh() {
        let all = [
            "pick.rs",
            "picker/mod.rs",
            "src/pi.rs",
            "kick.rs",
            "a pick b",
        ];
        for mut stepped in both() {
            let mut direct = Matcher::synchronous(MatchOn::LABEL);
            for matcher in [&mut stepped, &mut direct] {
                matcher.push(rows(&all));
                matcher.request_slice(0, 10);
            }
            stepped.set_query("pi");
            stepped.set_query("pick");
            stepped.set_query("pick !mod");
            direct.set_query("pick !mod");
            assert_eq!(settle(&mut stepped).rows, settle(&mut direct).rows);
        }
    }

    #[test]
    fn test_lengthening_a_regex_scores_every_row_again() {
        for mut matcher in both() {
            matcher.push(rows(&["a", "b"]));
            matcher.request_slice(0, 10);
            matcher.set_query("/a|/");
            matcher.set_query("/a|b/");
            assert_eq!(labels(&settle(&mut matcher)), ["a", "b"]);
        }
    }

    #[test]
    fn test_merging_a_batch_scores_as_scoring_every_row_at_once() {
        let all = ["mod.rs", "x/mod.rs", "model.rs", "a", "mod", "z/mod/y"];
        for mut batched in both() {
            let mut whole = Matcher::synchronous(MatchOn::LABEL);
            batched.set_query("mod");
            whole.set_query("mod");
            for part in all.chunks(2) {
                batched.push(rows(part));
            }
            whole.push(rows(&all));
            batched.request_slice(0, 10);
            whole.request_slice(0, 10);
            let (batched, whole) = (settle(&mut batched), settle(&mut whole));
            assert_eq!(labels(&batched), labels(&whole));
        }
    }

    #[test]
    fn test_rows_that_score_the_same_keep_the_order_they_came_in() {
        for mut matcher in both() {
            matcher.set_query("a");
            matcher.push(rows(&["xa1"]));
            matcher.push(rows(&["xa2", "xa3"]));
            matcher.request_slice(0, 10);
            assert_eq!(labels(&settle(&mut matcher)), ["xa1", "xa2", "xa3"]);
        }
    }

    #[test]
    fn test_use_ranks_equal_scores_and_leads_an_empty_query() {
        for mut matcher in both() {
            matcher.push_ranked(rows(&["xa1", "xa2", "a"]), vec![0, 1, 0]);
            matcher.request_slice(0, 10);
            assert_eq!(labels(&settle(&mut matcher)), ["xa2", "xa1", "a"]);
            matcher.set_query("a");
            assert_eq!(
                labels(&settle(&mut matcher)),
                ["a", "xa2", "xa1"],
                "score first"
            );
        }
    }

    #[test]
    fn test_a_slice_is_the_rows_asked_for_with_the_counts() {
        let all: Vec<String> = (0..100).map(|n| format!("row{n:03}")).collect();
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        for mut matcher in both() {
            matcher.push(rows(&all));
            matcher.set_query("row");
            matcher.request_slice(40, 10);
            let reply = settle(&mut matcher);
            assert_eq!(reply.first, 40);
            assert_eq!(labels(&reply)[0], "row040");
            assert_eq!(reply.rows.len(), 10);
            assert_eq!((reply.matched, reply.total), (100, 100));
        }
    }

    #[test]
    fn test_unscored_rows_stay_as_they_came_whatever_the_query() {
        for mut matcher in both() {
            matcher.unscored();
            matcher.set_query("zzz");
            matcher.push(rows(&["b", "a"]));
            matcher.request_slice(0, 10);
            assert_eq!(labels(&settle(&mut matcher)), ["b", "a"]);
        }
    }

    #[test]
    fn test_rows_after_a_clear_that_cut_scoring_short_meet_the_current_query() {
        let mut engine = Engine::new(MatchOn::LABEL);
        engine.push(PushCommand {
            ranks: None,
            rows: rows(&["foo", "bar"]),
        });
        engine.set_query(Query::parse("foo"), &mut || true);
        engine.clear();
        engine.push(PushCommand {
            ranks: None,
            rows: rows(&["foo2", "baz"]),
        });
        let (shown, matched) = engine.slice(Window {
            count: 10,
            first: 0,
        });
        assert_eq!(matched, 1);
        assert_eq!(shown[0].row.label, "foo2");
    }

    #[test]
    fn test_a_row_given_again_is_held_once() {
        for mut matcher in both() {
            matcher.push(rows(&["a", "b"]));
            matcher.push(rows(&["a", "b", "c"]));
            matcher.request_slice(0, 10);
            let reply = settle(&mut matcher);
            assert_eq!(reply.total, 3);
        }
    }

    #[test]
    fn test_a_tracked_row_is_found_where_new_rows_moved_it() {
        for mut matcher in both() {
            matcher.set_query("a");
            matcher.push(rows(&["xa", "ya"]));
            matcher.track(Some(Tracked {
                key: key("ya"),
                offset: 1,
            }));
            matcher.push(rows(&["a"]));
            matcher.request_slice(0, 10);
            let reply = settle(&mut matcher);
            assert_eq!(reply.tracked, Some(2), "the whole match went above it");
            assert_eq!(reply.first, 1, "the slice moved to keep it one row down");
        }
    }

    #[test]
    fn test_a_pinned_row_comes_after_every_match_and_is_counted() {
        for mut matcher in both() {
            matcher.push(rows(&["main.rs", "mod.rs", "lib.rs"]));
            matcher.set_query("ma");
            matcher.pin(Some(Row::new(key("ma"), "ma")));
            matcher.request_slice(0, 10);
            let reply = settle(&mut matcher);
            assert_eq!(labels(&reply), ["main.rs", "ma"], "matches first");
            assert_eq!((reply.matched, reply.total), (2, 4));
            matcher.request_slice(1, 10);
            assert_eq!(labels(&settle(&mut matcher)), ["ma"], "sliced as a match");
            matcher.request_slice(0, 10);
            matcher.track(Some(Tracked {
                key: key("ma"),
                offset: 0,
            }));
            assert_eq!(settle(&mut matcher).tracked, Some(1));
            matcher.pin(None);
            assert_eq!(labels(&settle(&mut matcher)), ["main.rs"]);
        }
    }

    #[test]
    fn test_the_keys_of_every_match_come_in_order() {
        let mut matcher = Matcher::synchronous(MatchOn::LABEL);
        matcher.push(rows(&["xb", "b", "c"]));
        matcher.set_query("b");
        matcher.request_keys();
        assert_eq!(matcher.take_keys(), Some(vec![key("b"), key("xb")]));
    }
}
