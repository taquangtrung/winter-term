//! The thread that asks each provider for its quota on the provider's own
//! schedule, and what the window holds of the answers.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

use crate::wake::Waker;

use super::{Fetched, Provider, Window};

// ========================================================================
// Data Structures
// ========================================================================

/// The quotas as last heard, and the line to the thread asking for them.
pub(crate) struct Monitor {
    answers: Receiver<(Provider, Fetched)>,
    /// Asks the thread to ask a provider again now.
    asks: Sender<Provider>,
    /// The providers the thread asks; the others are never asked.
    providers: BTreeSet<Provider>,
    shown: BTreeMap<Provider, Vec<Window>>,
}

// ========================================================================
// Monitor Implementations
// ========================================================================

impl Monitor {
    /// Start asking `providers`, waking the event loop with each answer.
    pub(crate) fn start(waker: &Waker, providers: BTreeSet<Provider>) -> Self {
        let (answer_tx, answer_rx) = mpsc::channel();
        let (ask_tx, ask_rx) = mpsc::channel();
        let asked = providers.clone();
        // A monitor that cannot start shows nothing, which is all a thread
        // that never ran would have shown.
        let _ = thread::Builder::new()
            .name("winter quota".into())
            .spawn(move || ask_forever(&asked, &answer_tx, &ask_rx));
        Self {
            answers: waker.relay(answer_rx),
            asks: ask_tx,
            providers,
            shown: BTreeMap::new(),
        }
    }

    /// The providers this monitor asks.
    pub(crate) fn providers(&self) -> &BTreeSet<Provider> {
        &self.providers
    }

    /// Ask `provider` again now rather than when it is next due.
    pub(crate) fn refresh(&self, provider: Provider) {
        let _ = self.asks.send(provider);
    }

    /// Take in the answers that have come since. Returns whether what is
    /// shown changed.
    pub(crate) fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((provider, fetched)) = self.answers.try_recv() {
            changed |= apply(&mut self.shown, provider, fetched);
        }
        changed
    }

    /// Each provider with something to show, in the order they stand.
    pub(crate) fn shown(&self) -> impl Iterator<Item = (Provider, &[Window])> {
        self.shown
            .iter()
            .map(|(provider, windows)| (*provider, windows.as_slice()))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Ask each of `providers` when it is due, or at once when asked to, until
/// the window that wants the answers is gone. Asked for one it does not ask,
/// it does nothing; with none to ask, it ends.
fn ask_forever(
    providers: &BTreeSet<Provider>,
    answers: &Sender<(Provider, Fetched)>,
    asks: &Receiver<Provider>,
) {
    let start = Instant::now();
    let mut due: HashMap<Provider, Instant> = providers
        .iter()
        .map(|&provider| (provider, start))
        .collect();
    loop {
        for &provider in providers {
            if due[&provider] > Instant::now() {
                continue;
            }
            if answers.send((provider, provider.fetch())).is_err() {
                return;
            }
            due.insert(provider, Instant::now() + provider.refresh_every());
        }
        let Some(next) = due.values().min().copied() else {
            return;
        };
        match asks.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(provider) => {
                if let Some(at) = due.get_mut(&provider) {
                    *at = Instant::now();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Take `fetched` in as what `provider` shows. Returns whether that changed.
fn apply(
    shown: &mut BTreeMap<Provider, Vec<Window>>,
    provider: Provider,
    fetched: Fetched,
) -> bool {
    match fetched {
        Fetched::Failed => false,
        Fetched::Missing => shown.remove(&provider).is_some(),
        Fetched::Usage(windows) => {
            shown.insert(provider, windows.clone()).as_ref() != Some(&windows)
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Far longer than a monitor with nothing to ask takes to end.
    const END_TIMEOUT: Duration = Duration::from_secs(5);

    fn usage(percent: f64) -> Fetched {
        Fetched::Usage(vec![Window {
            resets_at: None,
            used_percent: percent,
            weekly: false,
        }])
    }

    #[test]
    fn test_a_failed_asking_keeps_what_was_shown_and_a_missing_one_drops_it() {
        let mut shown = BTreeMap::new();
        assert!(apply(&mut shown, Provider::Claude, usage(10.0)));
        assert!(
            !apply(&mut shown, Provider::Claude, usage(10.0)),
            "unchanged"
        );
        assert!(!apply(&mut shown, Provider::Claude, Fetched::Failed));
        assert_eq!(shown.len(), 1, "a network blip does not hide it");
        assert!(apply(&mut shown, Provider::Claude, Fetched::Missing));
        assert!(shown.is_empty(), "signed out: nothing to show");
    }

    #[test]
    fn test_a_monitor_with_no_provider_to_ask_ends() {
        // The asking line stays open, so only having nothing to ask can end
        // it, rather than spinning on a timeout of zero.
        let (answer_tx, _answers) = mpsc::channel();
        let (_asks, ask_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            ask_forever(&BTreeSet::new(), &answer_tx, &ask_rx);
            let _ = done_tx.send(());
        });
        assert!(done_rx.recv_timeout(END_TIMEOUT).is_ok());
    }
}
