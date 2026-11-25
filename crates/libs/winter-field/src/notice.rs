//! What a field tells the user, kept typed until its holder takes it.
//!
//! A field stores a [`FieldNotice`], and its holder wraps it in its own
//! notice enum, so the words live here once and tests compare variants
//! rather than sentences.

use cherry::{NoticeLevel, NoticeRequest};
use winter_model::editor::Notice;

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldNotice {
    /// A message Cherry gave, worded by Cherry, at its level.
    Cherry(NoticeRequest),
    /// A key would have added or taken away a line of a field with one line
    /// per entry.
    FixedLines,
}

// ========================================================================
// FieldNotice
// ========================================================================

impl FieldNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub fn into_notice(self) -> Notice {
        match self {
            Self::Cherry(NoticeRequest { level, message }) => match level {
                NoticeLevel::Error => Notice::error(message),
                NoticeLevel::Warning => Notice::warning(message),
                _ => Notice::info(message),
            },
            Self::FixedLines => {
                Notice::warning("one line per entry: lines are not added or taken away here")
            }
        }
    }
}
