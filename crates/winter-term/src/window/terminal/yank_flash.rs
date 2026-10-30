//! The highlight a yank leaves on the span it took, and putting it out once
//! it has burned out.

use std::time::Instant;

use crate::window::{App, YANK_FLASH_DURATION};

// ========================================================================
// App: yank flash
// ========================================================================

impl App {
    /// Leave the span the last yank took lit, so what landed in the register
    /// is visible rather than only reported. The span is already the
    /// selection: what this adds is the deadline it goes away at.
    pub(in crate::window) fn flash_yank(&mut self) {
        let Some(span) = self.selection.span.clone() else {
            return;
        };
        self.term.yank_flash = Some((span, Instant::now() + YANK_FLASH_DURATION));
        self.dirty = true;
    }

    /// Put out a yank flash that has burned out, and report when the live one
    /// expires so the event loop can wake to clear it.
    ///
    /// Only a selection still showing exactly what the yank took is cleared: a
    /// drag or a Visual selection made since owns the span now, and taking it
    /// away under the user would be worse than a highlight that overstays.
    pub(in crate::window) fn expire_yank_flash(&mut self) -> Option<Instant> {
        let (span, deadline) = self.term.yank_flash.as_ref()?;
        if Instant::now() < *deadline {
            return Some(*deadline);
        }
        if self.selection.span.as_ref() == Some(span) {
            self.selection.span = None;
        }
        self.term.yank_flash = None;
        self.dirty = true;
        None
    }
}
