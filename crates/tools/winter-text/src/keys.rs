//! The text editor's key tables the Keys page lists for it: Winter's own
//! keys over the text (the inline suggestions'), as Cherry owns every
//! editing key.

use winter_model::input::table::{Binding, KeyTable, Listed};

// ========================================================================
// Constants
// ========================================================================

/// Every key table of the text editor, for the Keys page to list; Cherry
/// binds every editing key.
pub const KEY_TABLES: &[&dyn Listed] = &[&INLINE_KEYS];

/// The keys over an inline suggestion, ahead of Cherry's: ghost text's
/// while typing, a next edit's while its card shows.
pub(crate) const INLINE_KEYS: KeyTable<InlineAct> = KeyTable {
    block: "inline",
    bindings: &[
        Binding {
            action: InlineAct::Ask,
            id: "ask",
            keys: &["C-\\"],
            label: "Ask for Ghost Text",
        },
        Binding {
            action: InlineAct::TakeAll,
            id: "take_all",
            keys: &["TAB", "C-RET"],
            label: "Take the Suggestion",
        },
        Binding {
            action: InlineAct::TakeWord,
            id: "take_word",
            keys: &["M-f", "M-<right>", "C-,"],
            label: "Take the Ghost Text's Next Word",
        },
        Binding {
            action: InlineAct::TakeLine,
            id: "take_line",
            keys: &["C-."],
            label: "Take the Ghost Text's Line",
        },
        Binding {
            action: InlineAct::Next,
            id: "next",
            keys: &["M-n"],
            label: "Next Ghost Text",
        },
        Binding {
            action: InlineAct::Previous,
            id: "previous",
            keys: &["M-p"],
            label: "Previous Ghost Text",
        },
        Binding {
            action: InlineAct::PutAway,
            id: "put_away",
            keys: &["C-g"],
            label: "Put the Suggestion Away",
        },
        Binding {
            action: InlineAct::Leave,
            id: "leave",
            keys: &["ESC"],
            label: "Put the Suggestion Away, then Escape",
        },
    ],
    configurable: true,
    title: "Text: Inline Suggestions",
};

// ========================================================================
// Data Structures
// ========================================================================

/// What a key does to the inline suggestion on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineAct {
    /// Ask for ghost text where none came by itself.
    Ask,
    /// Put the suggestion away, the key going on to Cherry.
    Leave,
    /// Show the next ghost text suggestion.
    Next,
    /// Show the ghost text suggestion before.
    Previous,
    /// Put the suggestion away.
    PutAway,
    /// Take all of the suggestion.
    TakeAll,
    /// Take the rest of the ghost text's line.
    TakeLine,
    /// Take the ghost text's next word.
    TakeWord,
}
