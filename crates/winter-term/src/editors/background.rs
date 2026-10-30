//! Work an editor runs on a thread of its own, for the reads the host's job
//! types have no shape for: a directory tree read to a budget, a batch of
//! edits written back to their files.
//!
//! A [`Background`] is one piece of such work. Dropping it, or starting the
//! next one in its place, asks it to stop, and an answer to work that has
//! been replaced can never arrive: the receiver it would have come down is
//! gone with it. That is what keeps a slow read from overwriting what a newer
//! one already showed. The editor polls it from [`crate::model::editor::Editor::on_tick`],
//! after first giving it a moment to finish, so quick work lands in the same
//! frame as the key that asked for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::wake::wake_window;

// ========================================================================
// Constants
// ========================================================================

/// How often an editor with work running looks for its answer.
pub const POLL: Duration = Duration::from_millis(30);

// ========================================================================
// Data Structures
// ========================================================================

/// One piece of work running on its own thread, answering with a `T`.
#[derive(Debug)]
pub struct Background<T> {
    /// Where the answer arrives.
    answer: mpsc::Receiver<T>,
    /// Set to ask the work to stop; the work checks it as it goes.
    cancel: Arc<AtomicBool>,
    /// Set once the work has answered, or once it is known it never will.
    done: Arc<AtomicBool>,
}

// ========================================================================
// Background
// ========================================================================

impl<T: Send + 'static> Background<T> {
    /// Start `work` on a thread of its own. It is handed the flag that asks
    /// it to stop, which it should check often enough to stop promptly.
    pub fn start(name: &str, work: impl FnOnce(&AtomicBool) -> T + Send + 'static) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (send, answer) = mpsc::channel();
        let (flag, finished) = (Arc::clone(&cancel), Arc::clone(&done));
        // A detached worker: nothing joins it, and one asked to stop is left
        // to notice and end by itself. Its answer wakes the window, so an
        // editor waiting on it need not look again and again.
        let spawned = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _ = send.send(work(&flag));
                finished.store(true, Ordering::Release);
                wake_window();
            });
        if spawned.is_err() {
            // No thread to be had: the answer never comes, which the caller
            // sees as work done with nothing to show.
            cancel.store(true, Ordering::Relaxed);
            done.store(true, Ordering::Release);
        }
        Self {
            answer,
            cancel,
            done,
        }
    }

    /// The answer, waiting up to `patience` for it: what lets quick work land
    /// before the frame that asked for it is drawn.
    pub fn wait(&self, patience: Duration) -> Option<T> {
        self.answer.recv_timeout(patience).ok()
    }

    /// The answer, if it has arrived.
    pub fn poll(&self) -> Option<T> {
        self.answer.try_recv().ok()
    }

    /// Whether the work has answered, or will never: [`Self::poll`] then
    /// has what there is to take, which is nothing for work that could not
    /// start.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_work_lands_inside_the_wait() {
        let work = Background::start("test", |_| 7);
        assert_eq!(work.wait(Duration::from_secs(5)), Some(7));
    }

    #[test]
    fn test_slow_work_is_left_running_and_polled_for_later() {
        let (release, gate) = mpsc::channel::<()>();
        let work = Background::start("test", move |_| {
            let _ = gate.recv();
            "done"
        });
        assert_eq!(work.wait(Duration::from_millis(20)), None, "not yet");
        assert_eq!(work.poll(), None);
        assert!(!work.is_done());
        release.send(()).expect("released");
        // Once it says it is done, its answer is there to take.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !work.is_done() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(work.is_done(), "said done once it answered");
        assert_eq!(work.poll(), Some("done"));
    }

    #[test]
    fn test_dropping_the_work_asks_it_to_stop() {
        let (seen, stopped) = mpsc::channel();
        let work = Background::start("test", move |cancel| {
            while !cancel.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(1));
            }
            let _ = seen.send(());
        });
        drop(work);
        assert!(
            stopped.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the work saw the flag and ended"
        );
    }
}
