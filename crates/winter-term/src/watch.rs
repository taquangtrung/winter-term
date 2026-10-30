//! What the file watchers share: telling a change from a look, and waiting
//! a burst of changes out so it is read twice at most, once just after it
//! starts and once after it has gone quiet.

use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode};
use notify::EventKind;

// ========================================================================
// Data Structures
// ========================================================================

/// Changes waited out at a pace of their own: each run of them is read just
/// after it starts, once it has been quiet long enough, and every so often
/// while it never goes quiet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Debounce {
    burst: Option<Burst>,
    pace: Pace,
}

/// How long a watcher waits a burst out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Pace {
    /// How long after a burst's first change it is read: long enough for the
    /// write the change is part of to finish.
    pub(crate) lead: Duration,
    /// The longest a burst that never goes quiet waits between two reads.
    pub(crate) max_wait: Duration,
    /// How long the tree must be quiet after a burst's last change before
    /// it is read again.
    pub(crate) quiet: Duration,
}

/// One run of changes, from its first until it has gone quiet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Burst {
    first: Instant,
    last: Instant,
    /// Whether changes have arrived since the last read.
    pending: bool,
    /// When the burst was last read, if it has been.
    read_at: Option<Instant>,
}

// ========================================================================
// Debounce
// ========================================================================

impl Debounce {
    /// Nothing waited on yet, at `pace`.
    pub(crate) fn new(pace: Pace) -> Self {
        Self { burst: None, pace }
    }

    /// Take in a change that arrived at `now`: it opens a burst, or extends
    /// the one under way. A burst read and quiet since is over, so the
    /// change starts the next.
    pub(crate) fn take_change(&mut self, now: Instant) {
        match self.burst.as_mut() {
            Some(open) if open.pending || now.duration_since(open.last) < self.pace.quiet => {
                open.last = now;
                open.pending = true;
            }
            _ => {
                self.burst = Some(Burst {
                    first: now,
                    last: now,
                    pending: true,
                    read_at: None,
                });
            }
        }
    }

    /// When the changes waiting to be read fall due: just after the burst
    /// starts if it has not been read yet, once it has been quiet long
    /// enough, or once it has waited as long as it may, whichever comes
    /// first. `None` with nothing waiting.
    pub(crate) fn due_at(&self) -> Option<Instant> {
        let burst = self.burst.filter(|burst| burst.pending)?;
        let quiet = burst.last + self.pace.quiet;
        let overdue = burst.read_at.unwrap_or(burst.first) + self.pace.max_wait;
        let lead = match burst.read_at {
            None => burst.first + self.pace.lead,
            Some(_) => quiet,
        };
        Some(lead.min(quiet).min(overdue))
    }

    /// Whether the burst has waited as long as one may between two reads.
    pub(crate) fn is_overdue(&self, now: Instant) -> bool {
        self.burst.is_some_and(|burst| {
            now.duration_since(burst.read_at.unwrap_or(burst.first)) >= self.pace.max_wait
        })
    }

    /// Whether changes are waiting that are due to be read at `now`.
    pub(crate) fn is_due(&self, now: Instant) -> bool {
        self.due_at().is_some_and(|due| now >= due)
    }

    /// Note the changes waiting as read at `now`.
    pub(crate) fn mark_read(&mut self, now: Instant) {
        if let Some(open) = self.burst.as_mut() {
            open.pending = false;
            open.read_at = Some(now);
        }
    }

    /// Whether the changes waiting are due at `now`, noting them read when
    /// they are.
    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        let due = self.is_due(now);
        if due {
            self.mark_read(now);
        }
        due
    }

    /// Keep the changes last read waiting, for a reader that was not ready
    /// to take them yet.
    pub(crate) fn defer(&mut self) {
        if let Some(open) = self.burst.as_mut() {
            open.pending = true;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `kind` is a file being changed rather than only looked at.
/// Reading a watched file raises access events (git answering a status, the
/// app reloading its own settings), which would otherwise set off another
/// read: reloading the settings opens them, and taken as a change that
/// would reload them again, for ever. A close after a write is a real save,
/// and every other save still reports through a modify, create, or remove.
pub(crate) fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Other | EventKind::Remove(_) => true,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};

    use super::*;

    const PACE: Pace = Pace {
        lead: Duration::from_millis(150),
        max_wait: Duration::from_secs(2),
        quiet: Duration::from_millis(300),
    };

    #[test]
    fn test_a_burst_is_read_after_its_lead_and_again_once_quiet() {
        let t0 = Instant::now();
        let mut debounce = Debounce::new(PACE);
        debounce.take_change(t0);
        assert!(!debounce.take_due(t0), "the write may not be done");
        assert!(!debounce.take_due(t0 + PACE.lead - Duration::from_millis(10)));
        assert!(debounce.take_due(t0 + PACE.lead));
        assert!(!debounce.take_due(t0 + PACE.lead), "read once");

        let later = t0 + PACE.lead + Duration::from_millis(50);
        debounce.take_change(later);
        assert!(!debounce.take_due(later + PACE.quiet / 2), "still busy");
        assert!(debounce.take_due(later + PACE.quiet));
    }

    #[test]
    fn test_a_burst_that_never_goes_quiet_is_still_read_every_so_often() {
        let t0 = Instant::now();
        let mut debounce = Debounce::new(PACE);
        debounce.take_change(t0);
        assert!(debounce.take_due(t0 + PACE.lead));
        let mut now = t0 + PACE.lead;
        let mut reads = 0;
        while now < t0 + PACE.lead + PACE.max_wait * 2 {
            now += PACE.quiet / 3;
            debounce.take_change(now);
            if debounce.take_due(now) {
                reads += 1;
            }
        }
        assert_eq!(reads, 2, "once per max wait, however busy");
    }

    #[test]
    fn test_a_change_long_after_the_last_burst_is_read_at_once_again() {
        let t0 = Instant::now();
        let mut debounce = Debounce::new(PACE);
        debounce.take_change(t0);
        assert!(debounce.take_due(t0 + PACE.lead));
        let much_later = t0 + Duration::from_secs(60);
        debounce.take_change(much_later);
        assert!(
            debounce.take_due(much_later + PACE.lead),
            "after the lead, not the quiet time"
        );
    }

    #[test]
    fn test_a_deferred_read_falls_due_again() {
        let t0 = Instant::now();
        let mut debounce = Debounce::new(PACE);
        debounce.take_change(t0);
        assert!(debounce.take_due(t0 + PACE.lead));
        assert_eq!(debounce.due_at(), None, "nothing left waiting");
        debounce.defer();
        assert!(debounce.due_at().is_some(), "the read is owed again");
    }

    #[test]
    fn test_reading_a_file_is_not_a_change_but_saving_one_is() {
        assert!(!is_content_change(&EventKind::Access(AccessKind::Open(AccessMode::Read))));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(is_content_change(&EventKind::Access(AccessKind::Close(AccessMode::Write))));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(DataChange::Any))));
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
    }
}
