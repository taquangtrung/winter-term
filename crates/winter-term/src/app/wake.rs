//! Waking the event loop from the threads that feed it, so an answer that
//! arrives while the loop sleeps is seen at once rather than on the next poll.

use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;

use winit::event_loop::EventLoopProxy;

// ========================================================================
// Statics
// ========================================================================

/// The running window's handle, for a thread a tool started (a file watcher)
/// to wake the window with, since a tool holds no handle of its own.
static SHARED: OnceLock<Waker> = OnceLock::new();

// ========================================================================
// Data Structures
// ========================================================================

/// A handle any thread may hold to wake the event loop. Made before the loop
/// exists and connected to it once it does; until then, and in tests, waking
/// does nothing and the loop's own poll picks the work up.
#[derive(Clone, Default)]
pub(crate) struct Waker {
    proxy: Arc<Mutex<Option<EventLoopProxy<()>>>>,
}

// ========================================================================
// Waker Implementations
// ========================================================================

impl Waker {
    /// Connect every clone of this handle to the running event loop.
    pub(crate) fn connect(&self, proxy: EventLoopProxy<()>) {
        if let Ok(mut slot) = self.proxy.lock() {
            *slot = Some(proxy);
        }
    }

    /// Share this handle with the threads tools start, through
    /// [`wake_window`]. The first window to share is the one woken.
    pub(crate) fn share(&self) {
        let _ = SHARED.set(self.clone());
    }

    /// Wake the event loop, if it is connected and still running.
    pub(crate) fn wake(&self) {
        if let Ok(slot) = self.proxy.lock() {
            if let Some(proxy) = slot.as_ref() {
                // A loop that has closed has nothing left to wake.
                let _ = proxy.send_event(());
            }
        }
    }

    /// A receiver that hands on what `source` delivers, waking the event loop
    /// for each message. For a channel fed by a thread this module does not
    /// own, which cannot be asked to wake the loop itself. The relay ends when
    /// either side is dropped.
    pub(crate) fn relay<T: Send + 'static>(&self, source: mpsc::Receiver<T>) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        let waker = self.clone();
        // Shared rather than moved, so a relay that cannot start hands the
        // source back instead of dropping every message it would have carried.
        let shared = Arc::new(Mutex::new(Some(source)));
        let taken = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("winter wake relay".into())
            .spawn(move || {
                let Some(source) = taken.lock().ok().and_then(|mut slot| slot.take()) else {
                    return;
                };
                while let Ok(message) = source.recv() {
                    if tx.send(message).is_err() {
                        return;
                    }
                    waker.wake();
                }
            });
        if spawned.is_ok() {
            return rx;
        }
        // No thread to relay on: the messages still arrive, only on the
        // loop's next poll rather than at once.
        match shared.lock().ok().and_then(|mut slot| slot.take()) {
            Some(source) => source,
            None => rx,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Wake the running window from any thread: what a tool's background thread
/// calls when it has news. Does nothing before a window has shared its handle.
pub(crate) fn wake_window() {
    if let Some(waker) = SHARED.get() {
        waker.wake();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Long enough for a relay thread on a loaded machine.
    const ARRIVES: Duration = Duration::from_secs(5);

    #[test]
    fn test_a_relay_hands_on_every_message_in_order() {
        let (tx, source) = mpsc::channel();
        let relayed = Waker::default().relay(source);
        tx.send(1).expect("send");
        tx.send(2).expect("send");
        assert_eq!(relayed.recv_timeout(ARRIVES), Ok(1));
        assert_eq!(relayed.recv_timeout(ARRIVES), Ok(2));
        drop(tx);
        assert_eq!(
            relayed.recv_timeout(ARRIVES),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "the relay ends with its source"
        );
    }
}
