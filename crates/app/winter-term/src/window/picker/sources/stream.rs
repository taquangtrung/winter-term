//! Rows streamed from a program's output: each line made a row, sent to the
//! picker in batches as they come, the window woken for each. The program
//! stops when it has given as many rows as asked, or when the picker goes.

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
// Functions
// ========================================================================

/// Run `command`, making each line of its output a row with `parse`, and
/// stream the rows; at most `cap` of them, the last batch saying the cap
/// stopped it. `missing` is said in place of rows when the program cannot be
/// run.
pub(in crate::window) fn stream_lines(
    mut command: Command,
    cap: usize,
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
            stream(BufReader::new(stdout), cap, &send, &mut parse);
            let _ = child.kill();
            let _ = child.wait();
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

/// Read `lines` into batches of rows down `send` until they end, `cap` rows
/// are sent, or no one is listening.
fn stream(
    lines: impl BufRead,
    cap: usize,
    send: &Sender<Batch>,
    parse: &mut impl FnMut(&[u8]) -> Option<Row>,
) {
    let mut rows = Vec::new();
    let mut sent = 0;
    let mut since = Instant::now();
    let mut lines = lines.split(b'\n');
    loop {
        let Some(Ok(line)) = lines.next() else {
            deliver(
                send,
                Batch {
                    done: true,
                    rows,
                    ..Batch::default()
                },
            );
            return;
        };
        let Some(row) = parse(&line) else {
            continue;
        };
        rows.push(row);
        if sent + rows.len() >= cap {
            deliver(
                send,
                Batch {
                    capped: true,
                    done: true,
                    rows,
                    ..Batch::default()
                },
            );
            return;
        }
        if rows.len() >= BATCH_ROWS || since.elapsed() >= BATCH_EVERY {
            sent += rows.len();
            let batch = Batch {
                rows: std::mem::take(&mut rows),
                ..Batch::default()
            };
            if !deliver(send, batch) {
                return;
            }
            since = Instant::now();
        }
    }
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

    fn rows_of(text: &str, cap: usize) -> Vec<Batch> {
        let (send, receive) = mpsc::channel();
        let mut parse = |line: &[u8]| {
            let label = String::from_utf8_lossy(line).to_string();
            (!label.is_empty()).then(|| Row::new(RowKey(0), label))
        };
        stream(text.as_bytes(), cap, &send, &mut parse);
        drop(send);
        receive.iter().collect()
    }

    #[test]
    fn test_every_line_comes_as_a_row_and_the_last_batch_is_done() {
        let batches = rows_of("a\nb\n\nc\n", 100);
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
    fn test_the_cap_stops_the_stream_and_says_so() {
        let batches = rows_of("a\nb\nc\nd\n", 2);
        let count: usize = batches.iter().map(|b| b.rows.len()).sum();
        assert_eq!(count, 2);
        assert!(batches.last().is_some_and(|b| b.capped && b.done));
    }

    #[test]
    fn test_a_place_is_keyed_the_same_every_run() {
        assert_eq!(place_key("a.rs", 3, 5), place_key("a.rs", 3, 5));
        assert_ne!(place_key("a.rs", 3, 5), place_key("a.rs", 4, 5));
    }
}
