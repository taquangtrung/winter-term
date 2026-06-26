//! The kill ring: what the Emacs kills took, newest first, so a run of kills
//! comes back as one and `M-y` reaches the ones before.

use std::collections::VecDeque;

use crate::model::textedit::buffer::Register;

// ========================================================================
// Constants
// ========================================================================

/// How many kills the ring keeps before the oldest falls off: Emacs' own
/// default, and far more than anyone cycles back through.
const KILL_RING_SIZE: usize = 64;

// ========================================================================
// Data Structures
// ========================================================================

/// How a kill joins the ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Merge {
    /// After the newest entry: a forward kill straight after another kill.
    Append,
    /// As an entry of its own.
    New,
    /// Before the newest entry: a backward kill straight after another.
    Prepend,
}

/// The kills, newest first, and which of them a yank last put back.
#[derive(Clone, Debug, Default)]
pub struct KillRing {
    entries: VecDeque<Register>,
    /// The entry the last yank or `M-y` put back.
    pointer: usize,
}

// ========================================================================
// KillRing
// ========================================================================

impl KillRing {
    /// Keep `taken`, as an entry of its own or joined onto the newest.
    pub fn kill(&mut self, taken: Register, merge: Merge) {
        self.pointer = 0;
        let newest = match merge {
            Merge::New => None,
            Merge::Append | Merge::Prepend => self.entries.front_mut(),
        };
        let Some(newest) = newest else {
            self.entries.push_front(taken);
            self.entries.truncate(KILL_RING_SIZE);
            return;
        };
        newest.text = match merge {
            Merge::Prepend => format!("{}{}", taken.text, newest.text),
            Merge::Append | Merge::New => format!("{}{}", newest.text, taken.text),
        };
        newest.linewise = false;
    }

    /// The newest kill, for a yank, which starts any `M-y` cycle from there.
    pub fn yank(&mut self) -> Option<Register> {
        self.pointer = 0;
        self.entries.front().cloned()
    }

    /// The kill before the one last put back, wrapping to the newest after
    /// the oldest, for `M-y`.
    pub fn yank_pop(&mut self) -> Option<Register> {
        if self.entries.is_empty() {
            return None;
        }
        self.pointer = (self.pointer + 1) % self.entries.len();
        self.entries.get(self.pointer).cloned()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Register {
        Register {
            linewise: false,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_a_run_of_kills_is_one_entry_and_yank_pop_walks_back_and_wraps() {
        let mut ring = KillRing::default();
        ring.kill(text("first"), Merge::New);
        ring.kill(text("world"), Merge::New);
        ring.kill(text("!"), Merge::Append);
        ring.kill(text("hello "), Merge::Prepend);

        assert_eq!(ring.yank().map(|r| r.text).as_deref(), Some("hello world!"));
        assert_eq!(ring.yank_pop().map(|r| r.text).as_deref(), Some("first"));
        assert_eq!(
            ring.yank_pop().map(|r| r.text).as_deref(),
            Some("hello world!"),
            "past the oldest it wraps"
        );
    }
}
