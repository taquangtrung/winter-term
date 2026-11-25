//! What the modal keys of Winter's own surfaces (a field, a listing renamed
//! in place, the terminal's Normal mode) report: the mode that has the
//! keyboard, what a key asked of the host beyond its edit, and what an
//! editor shows of its keys in the status bar. Text editing itself is
//! Cherry's.

use crate::place::Place;
use crate::vim::grammar::MacroStep;

// ========================================================================
// Data Structures
// ========================================================================

/// What an editor puts in the window's status bar while it has the keyboard:
/// where the keys stand, rather than what the editor shows. What the editor
/// shows stays in its own rows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorStatus {
    /// The editor's own mode, drawn where a terminal's is. `None` for an editor
    /// with no mode of its own, which the bar marks as an editor.
    pub mode: Option<EditMode>,
    /// Short facts about where the cursor is and what is part-typed, drawn
    /// at the bar's right edge in order: `12:5`, `340 lines`, `"a`.
    pub segments: Vec<String>,
}

/// Which of the modes has the keyboard.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EditMode {
    /// Typing goes into the text.
    Insert,
    /// Keys are commands.
    #[default]
    Normal,
    /// Typing goes into the text, over what is already there.
    Replace,
    /// Keys are commands, and they act on what is selected.
    Visual,
}

/// What a key asked of the host, beyond the edit it made to the text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditOutcome {
    /// `Esc` in a field that is only ever typed into: the answer is withdrawn.
    Cancel,
    /// The key was spent on the text, the cursor, or a command part-typed.
    Consumed,
    /// `C-o`: hand the text to a fuller editor.
    Escalate,
    /// No binding here, so the host may have one.
    Ignored,
    /// `gb`: show the open files to choose from.
    ListBuffers,
    /// `"+p`: the host reads the clipboard and hands it on.
    Paste,
    /// `ZZ` or `ZQ`: finish, writing the text first or throwing it away.
    Quit {
        /// Whether to write the text first (`ZZ`) rather than discard it.
        save: bool,
    },
    /// `C-s`: write the text.
    Save,
    /// `/` or `?`: ask what to look for.
    Search {
        /// True for `/`, which searches down the text.
        forward: bool,
    },
    /// `]b` or `[b`: on to the next open file, or back to the one before.
    StepBuffer {
        /// True for `]b`.
        forward: bool,
    },
    /// `Enter` while typing into text whose lines are fixed: the edit is done.
    Submit,
    /// `"+y` and the like: put this on the clipboard, and say so.
    Yank(String),
    /// A bare delete or change: put what it took on the clipboard, quietly.
    Cut(String),
    /// Record or replay a keyboard macro, which the host keeps.
    Macro(MacroStep),
    /// `zt`, `zz`, `zb`, `C-l`: start the view so the cursor's screen row is
    /// there, which only the host, knowing how its lines fold, can.
    Place(Place),
}
