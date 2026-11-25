//! Rows streamed to a picker in batches as they come, the window woken for
//! each: a program's output, each line made a row, or rows made on a thread
//! of their own. Either stops when it has no more to give, or when the
//! picker goes.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use misty::picker::{Batch, Feed, Row, RowKey};
use winter_base::wake::wake_window;

// ========================================================================
// Constants
// ========================================================================

/// How many rows a batch holds before it is sent, and how long a slow
/// program may go before what it has is sent anyway.
const BATCH_ROWS: usize = 200;
const BATCH_EVERY: Duration = Duration::from_millis(50);

/// The streaming thread's name.
const THREAD_NAME: &str = "picker-stream";

// ========================================================================
// Data Structures
// ========================================================================

/// Rows kept until there are enough of them, or enough time has gone, to
/// send as a batch.
struct Batcher<'a> {
    rows: Vec<Row>,
    send: &'a Sender<Batch>,
    /// When the last batch went.
    since: Instant,
}

// ========================================================================
// Batcher
// ========================================================================

impl<'a> Batcher<'a> {
    fn new(send: &'a Sender<Batch>) -> Self {
        Self {
            rows: Vec::new(),
            send,
            since: Instant::now(),
        }
    }

    /// Keep `row`, sending what is kept once there is a batch of it or the
    /// last was sent a while ago; whether anyone is still listening.
    fn push(&mut self, row: Row) -> bool {
        self.rows.push(row);
        if self.rows.len() < BATCH_ROWS && self.since.elapsed() < BATCH_EVERY {
            return true;
        }
        self.since = Instant::now();
        let batch = Batch {
            rows: std::mem::take(&mut self.rows),
            ..Batch::default()
        };
        deliver(self.send, batch)
    }

    /// Send what is left, saying it is the last.
    fn finish(self) {
        let batch = Batch {
            done: true,
            rows: self.rows,
            ..Batch::default()
        };
        deliver(self.send, batch);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Run `command`, making each line of its output a row with `parse`, and
/// stream the rows. `missing` is said in place of rows when the program
/// cannot be run.
pub(in crate::window) fn stream_lines(
    mut command: Command,
    missing: &str,
    mut parse: impl FnMut(&[u8]) -> Option<Row> + Send + 'static,
) -> Feed {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Feed::Failed(missing.to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        return Feed::Failed(missing.to_string());
    };
    let (send, batches) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || {
            stream(BufReader::new(stdout), &send, &mut parse);
            let _ = child.kill();
            let _ = child.wait();
        });
    if spawned.is_err() {
        return Feed::Failed(missing.to_string());
    }
    Feed::Stream(batches)
}

/// Run `produce` on a thread of its own and stream the rows it makes: it
/// hands each to the function it is given, which says `false` once the
/// picker has gone, for it to stop. `missing` is said in place of rows when
/// the thread cannot be started.
pub(in crate::window) fn stream_rows(
    missing: &str,
    produce: impl FnOnce(&mut dyn FnMut(Row) -> bool) + Send + 'static,
) -> Feed {
    let (send, batches) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || {
            let mut batcher = Batcher::new(&send);
            produce(&mut |row| batcher.push(row));
            batcher.finish();
        });
    if spawned.is_err() {
        return Feed::Failed(missing.to_string());
    }
    Feed::Stream(batches)
}

/// A row key that names the same place the same way every run, so a search
/// run again on resume gives no row twice.
pub(in crate::window) fn place_key(path: &str, line: usize, column: usize) -> RowKey {
    let mut hasher = DefaultHasher::new();
    (path, line, column).hash(&mut hasher);
    RowKey(hasher.finish())
}

/// Read `lines` into batches of rows down `send` until they end or no one
/// is listening.
fn stream(lines: impl BufRead, send: &Sender<Batch>, parse: &mut impl FnMut(&[u8]) -> Option<Row>) {
    let mut batcher = Batcher::new(send);
    for line in lines.split(b'\n').map_while(Result::ok) {
        let Some(row) = parse(&line) else {
            continue;
        };
        if !batcher.push(row) {
            return;
        }
    }
    batcher.finish();
}

/// Send `batch` and wake the window for it; whether anyone took it.
fn deliver(send: &Sender<Batch>, batch: Batch) -> bool {
    let taken = send.send(batch).is_ok();
    wake_window();
    taken
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(text: &str) -> Vec<Batch> {
        let (send, receive) = mpsc::channel();
        let mut parse = |line: &[u8]| {
            let label = String::from_utf8_lossy(line).to_string();
            (!label.is_empty()).then(|| Row::new(RowKey(0), label))
        };
        stream(text.as_bytes(), &send, &mut parse);
        drop(send);
        receive.iter().collect()
    }

    #[test]
    fn test_every_line_comes_as_a_row_and_the_last_batch_is_done() {
        let batches = rows_of("a\nb\n\nc\n");
        let labels: Vec<String> = batches
            .iter()
            .flat_map(|b| &b.rows)
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(labels, ["a", "b", "c"]);
        let last = batches.last().expect("a last batch");
        assert!(last.done && !last.capped);
    }

    #[test]
    fn test_a_place_is_keyed_the_same_every_run() {
        assert_eq!(place_key("a.rs", 3, 5), place_key("a.rs", 3, 5));
        assert_ne!(place_key("a.rs", 3, 5), place_key("a.rs", 4, 5));
    }
}
