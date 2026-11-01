//! The threads that ask each provider for its quota on the provider's own
//! schedule, one to a provider so a slow one holds up no other, and what the
//! window holds of the answers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use winter_base::wake::Waker;

use super::{Fetched, Provider, Window};

// ========================================================================
// Data Structures
// ========================================================================

/// The quotas as last heard, and the lines to the threads asking for them.
pub(crate) struct Monitor {
    answers: Receiver<(Provider, Fetched)>,
    /// Asks a provider's thread to ask again now.
    asks: BTreeMap<Provider, Sender<()>>,
    /// The providers the threads ask; the others are never asked.
    providers: BTreeSet<Provider>,
    shown: BTreeMap<Provider, Shown>,
}

/// What a provider last said of its quota, and how long ago.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shown {
    /// Whether asking it again has failed since: what it says may be out of
    /// date.
    pub(crate) failing: bool,
    /// When it said it.
    pub(crate) heard_at: Instant,
    /// Its windows, the short one first.
    pub(crate) windows: Vec<Window>,
}

// ========================================================================
// Monitor Implementations
// ========================================================================

impl Monitor {
    /// Start asking `providers`, waking the event loop with each answer.
    pub(crate) fn start(waker: &Waker, providers: BTreeSet<Provider>) -> Self {
        let (answer_tx, answer_rx) = mpsc::channel();
        let asks = providers
            .iter()
            .map(|&provider| {
                let (ask_tx, ask_rx) = mpsc::channel();
                let answers = answer_tx.clone();
                // A thread that cannot start shows nothing, which is all one
                // that never ran would have shown.
                let _ = thread::Builder::new()
                    .name(format!("winter quota {provider:?}"))
                    .spawn(move || {
                        ask_forever(
                            provider.refresh_every(),
                            || provider.fetch(),
                            |fetched| answers.send((provider, fetched)).is_ok(),
                            &ask_rx,
                        )
                    });
                (provider, ask_tx)
            })
            .collect();
        Self {
            answers: waker.relay(answer_rx),
            asks,
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
        if let Some(ask) = self.asks.get(&provider) {
            let _ = ask.send(());
        }
    }

    /// Take in the answers that have come since. Returns whether what is
    /// shown changed.
    pub(crate) fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((provider, fetched)) = self.answers.try_recv() {
            changed |= apply(&mut self.shown, provider, fetched, Instant::now());
        }
        changed
    }

    /// Each provider with something to show, in the order they stand.
    pub(crate) fn shown(&self) -> impl Iterator<Item = (Provider, &Shown)> {
        self.shown
            .iter()
            .map(|(provider, shown)| (*provider, shown))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Fetch every `every`, or at once when `asks` says so, handing each answer
/// to `answer` until it says the window that wants them is gone or `asks`
/// is dropped with the monitor.
fn ask_forever(
    every: Duration,
    fetch: impl Fn() -> Fetched,
    answer: impl Fn(Fetched) -> bool,
    asks: &Receiver<()>,
) {
    loop {
        let fetched = fetch();
        // Asks that came while fetching are answered by the fetch just done;
        // dropped before it is told, so one made once it is seen is kept.
        while asks.try_recv().is_ok() {}
        if !answer(fetched) {
            return;
        }
        let due = Instant::now() + every;
        match asks.recv_timeout(due.saturating_duration_since(Instant::now())) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Take in `fetched`, heard `now`, as what `provider` shows. Returns whether
/// that changed.
fn apply(
    shown: &mut BTreeMap<Provider, Shown>,
    provider: Provider,
    fetched: Fetched,
    now: Instant,
) -> bool {
    match fetched {
        Fetched::Failed => shown
            .get_mut(&provider)
            .is_some_and(|shown| !std::mem::replace(&mut shown.failing, true)),
        Fetched::Missing => shown.remove(&provider).is_some(),
        Fetched::Usage(windows) => {
            let heard = Shown {
                failing: false,
                heard_at: now,
                windows,
            };
            let before = shown.insert(provider, heard.clone());
            before.is_none_or(|before| before.windows != heard.windows || before.failing)
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::*;

    /// Far longer than a thread with nothing more to do takes to end.
    const END_TIMEOUT: Duration = Duration::from_secs(5);

    fn usage(percent: f64) -> Fetched {
        Fetched::Usage(vec![Window {
            resets_at: None,
            used_percent: percent,
            weekly: false,
        }])
    }

    #[test]
    fn test_a_failed_asking_keeps_what_was_shown_marked_and_a_missing_one_drops_it() {
        let mut shown = BTreeMap::new();
        let start = Instant::now();
        assert!(apply(&mut shown, Provider::Claude, usage(10.0), start));
        assert!(
            !apply(&mut shown, Provider::Claude, usage(10.0), start),
            "unchanged"
        );
        assert!(
            apply(&mut shown, Provider::Claude, Fetched::Failed, start),
            "now marked out of date, which redraws it"
        );
        assert!(
            !apply(&mut shown, Provider::Claude, Fetched::Failed, start),
            "already marked"
        );
        let kept = &shown[&Provider::Claude];
        assert!(kept.failing, "a network blip does not hide it, but says so");
        assert_eq!(kept.heard_at, start, "and how old it is");
        let later = start + Duration::from_secs(60);
        assert!(
            apply(&mut shown, Provider::Claude, usage(10.0), later),
            "the same numbers heard again are no longer out of date"
        );
        assert_eq!(shown[&Provider::Claude].heard_at, later);
        assert!(
            !apply(&mut shown, Provider::Zai, Fetched::Failed, start),
            "nothing to mark"
        );
        assert!(apply(&mut shown, Provider::Claude, Fetched::Missing, start));
        assert!(shown.is_empty(), "signed out: nothing to show");
    }

    #[test]
    fn test_an_ask_fetches_at_once_and_the_thread_ends_with_its_monitor() {
        // Each provider has a thread of its own, so a click is answered as
        // soon as that provider answers, not after every other one has.
        let (ask_tx, ask_rx) = mpsc::channel();
        let (seen_tx, seen_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let count = Cell::new(0);
            ask_forever(
                Duration::from_secs(3600),
                || {
                    count.set(count.get() + 1);
                    Fetched::Missing
                },
                |_| seen_tx.send(count.get()).is_ok(),
                &ask_rx,
            );
            let _ = done_tx.send(());
        });
        assert_eq!(seen_rx.recv_timeout(END_TIMEOUT), Ok(1), "asked at start");
        ask_tx.send(()).unwrap();
        assert_eq!(
            seen_rx.recv_timeout(END_TIMEOUT),
            Ok(2),
            "asked again at once, not in an hour"
        );
        drop(ask_tx);
        assert!(
            done_rx.recv_timeout(END_TIMEOUT).is_ok(),
            "ends with the monitor"
        );
    }
}
