//! Keymap: resolve a key event into an [`Action`], given the pane's [`Mode`].
//!
//! In Insert, keys encode to terminal bytes for the PTY (the entry chord is the
//! one exception). In Normal, Winter intercepts keys as navigation/layout actions.
//! In Block-focus, keys forward to the block until `Esc`. Most bindings are
//! built in, but the window-management chords (split, close, focus) are
//! configurable through a [`WindowKeymap`]; see [`resolve_with`].

mod insert;
mod keymap;
mod motion;
mod normal;
mod pending;
mod visual;

pub use insert::encode_release;
pub(crate) use keymap::window_action_by_name;
pub use keymap::{format_key, EditAction, EditBinding, WindowAction, WindowKeymap};
pub use motion::{
    BlockNav, CursorMove, FindChar, GotoMark, InsertAt, TextObject, TextObjectSpec, VisualKind,
};
pub use pending::PendingPrefix;

use insert::{resolve_block_focus, resolve_insert};
use normal::resolve_normal;
use visual::resolve_visual;

use super::layout::{Direction, FocusDir};
use super::mode::Mode;

// ========================================================================
// Data Structures
// ========================================================================

/// A decoded key event from the windowing layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    /// Whether Alt was held.
    pub alt: bool,
    /// The physical key, independent of modifiers.
    pub code: KeyCode,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
}

/// A physical key, independent of modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCode {
    /// The Backspace key.
    Backspace,
    /// A printable character.
    Char(char),
    /// The Delete key.
    Delete,
    /// The Down arrow.
    Down,
    /// The End key.
    End,
    /// The Enter key.
    Enter,
    /// The Escape key.
    Escape,
    /// A function key, numbered from 1.
    F(u8),
    /// The Home key.
    Home,
    /// The Insert key.
    Insert,
    /// The Left arrow.
    Left,
    /// The Page Down key.
    PageDown,
    /// The Page Up key.
    PageUp,
    /// The Right arrow.
    Right,
    /// The space bar.
    Space,
    /// The Tab key.
    Tab,
    /// The Up arrow.
    Up,
}

/// What a key resolves to in the current mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Close the focused pane (current pane).
    ClosePane,
    /// Close every pane in the tab except the focused one.
    CloseOtherPanes,
    /// Close a tab: the active one (`None`) or the Nth tab, 1-based (`Some(n)`).
    CloseTab(Option<usize>),
    /// Change surrounding delimiter pair `target` to `replacement`.
    ChangeSurround {
        /// The delimiter being replaced.
        target: char,
        /// The delimiter to put in its place.
        replacement: char,
    },
    /// Delete surrounding delimiter pair `target`.
    DeleteSurround(char),
    /// Change the whole line (`cc`).
    ChangeLine,
    /// Change to the end of the line (`C`).
    ChangeToLineEnd,
    /// Change to the start of the line.
    ChangeToLineStart,
    /// Change back to the previous word (`cb`).
    ChangeWordBack,
    /// Change forward to the next word (`cw`).
    ChangeWordForward,
    /// Change the given text object (`ciw`, `ca"`).
    ChangeTextObject(TextObjectSpec),
    /// Replace the character under the cursor and enter Insert (`s`).
    SubstituteChar,
    /// Replace the character under the cursor, staying in Normal (`r`).
    ReplaceChar(char),
    /// Flip the case of the character under the cursor (`~`).
    ToggleCaseChar,
    /// Delete the character under the cursor (`x`).
    DeleteCharForward,
    /// Delete the whole line (`dd`).
    DeleteLine,
    /// Delete the current selection.
    DeleteSelection,
    /// Delete the given text object (`diw`, `da"`).
    DeleteTextObject(TextObjectSpec),
    /// Delete to the end of the line (`D`).
    DeleteToLineEnd,
    /// Delete to the start of the line.
    DeleteToLineStart,
    /// Delete back to the previous word (`db`).
    DeleteWordBack,
    /// Delete forward to the next word (`dw`).
    DeleteWordForward,
    /// A configurable Insert-mode line edit (e.g. `Ctrl-Backspace` to delete the
    /// word before the cursor). Realized as readline bytes plus a shadow update.
    Edit(EditAction),
    /// Enter Visual mode with the given selection shape.
    EnterVisual(VisualKind),
    /// A char-search within the current line (`f`/`F`/`t`/`T`).
    FindChar(FindChar),
    /// Pick a labelled landing spot from the `f`/`t` jump overlay.
    FindJump(char),
    /// Dismiss the `f`/`t` jump overlay without moving.
    FindCancel,
    /// Repeat the last char-search (`;`); `reverse` flips its direction (`,`).
    FindRepeat {
        /// True to search forward, false to search backward.
        reverse: bool,
    },
    /// Focus a rich block relative to the cursor.
    FocusBlock(BlockNav),
    /// Move focus to the neighbouring pane.
    FocusPane(FocusDir),
    /// Focus the pane at this index in the tab.
    FocusPaneByIndex(usize),
    /// Close the pane at this index in the tab.
    ClosePaneByIndex(usize),
    /// Forward these bytes to the focused block instead of the PTY.
    ForwardToBlock(Vec<u8>),
    /// Jump to a named mark (`{a-z}` or `'{a-z}`).
    GotoMark(GotoMark),
    /// Switch to tab number `n` (1-based).
    GotoTab(usize),
    /// Do nothing; the key is consumed without effect.
    Ignore,
    /// Move the cursor by one motion.
    MoveCursor(CursorMove),
    /// A motion with a count prefix (`5j`, `3w`): repeat `mv` `count` times.
    /// Only motions where repetition is meaningful resolve to this (see
    /// `count_repeats`); the rest drop the count.
    MoveCursorN {
        /// How many times to repeat the motion.
        count: usize,
        /// The motion to repeat.
        mv: CursorMove,
    },
    /// `Ctrl+O`: jump to the previous recorded position (vim's jumplist).
    JumpOlder,
    /// `Ctrl+I`/`Tab`: jump to the next recorded position.
    JumpNewer,
    /// `g;`: jump to where the previous change began (vim's changelist).
    ChangeOlder,
    /// `g,`: jump to where the next change began.
    ChangeNewer,
    /// `gp`: jump cursor to the prompt row.
    JumpToPrompt,
    /// `gP`: jump cursor to previous prompt/command mark.
    JumpToPreviousPrompt,
    /// `gn` / `gN`: select the next / previous search match into visual mode.
    SelectSearchMatch {
        /// True to move forward, false to move backward.
        forward: bool,
    },
    /// `cgn` / `cgN`: change the next / previous search match.
    ChangeSearchMatch {
        /// True to move forward, false to move backward.
        forward: bool,
    },
    /// `dgn` / `dgN`: delete the next / previous search match.
    DeleteSearchMatch {
        /// True to move forward, false to move backward.
        forward: bool,
    },
    /// Visual `o`: move the cursor to the selection's other end.
    SwapVisualEnds,
    /// `gv`: re-enter Visual mode with the last selection.
    RestoreVisual,
    /// Move the active tab one place left.
    MoveTabLeft,
    /// Move the active tab one place right.
    MoveTabRight,
    /// Open a new tab.
    NewTab,
    /// Activate the next tab.
    NextTab,
    /// Paste the clipboard into the PTY.
    Paste,
    /// Paste from named register `register` (`after` is true for `p`, false for `P`).
    PasteRegister {
        /// Register to paste from.
        register: char,
        /// True to paste after the cursor, false for before.
        after: bool,
    },
    /// Activate the previous tab.
    PrevTab,
    /// Re-apply an undone prompt edit (`Ctrl-\`).
    PromptRedo,
    /// Undo the last prompt edit (`Ctrl-/`).
    PromptUndo,
    /// `.`: repeat the pane's most recent change at the cursor.
    RepeatLastChange,
    /// `gx`: open the URL or file reference under the Normal-mode cursor.
    OpenUnderCursor,
    /// Dismiss the quick-select overlay.
    QuickCancel,
    /// Jump to the quick-select label with this key.
    QuickJump(char),
    /// Show the quick-select overlay.
    QuickSelect,
    /// Delete the last character of the search query.
    SearchBackspace,
    /// Abandon the search and restore the prior position.
    SearchCancel,
    /// Append a character to the search query.
    SearchChar(char),
    /// Run the search and jump to the first match.
    SearchExecute,
    /// `n`: repeat the last search in its own direction (sticky after `?`/`#`).
    SearchNext,
    /// `N`: repeat the last search in the opposite direction.
    SearchPrevious,
    /// `/`: open search input, searching forward.
    SearchStart,
    /// `?`: open search input, searching backward.
    SearchStartBackward,
    /// `*`/`#`: search for the word under the Normal-mode cursor, immediately
    /// (no input step) in the given direction — forward for `*`, backward for `#`.
    SearchWord {
        /// True to search forward, false to search backward.
        forward: bool,
    },
    /// `Alt-i` in Normal mode: select the paragraph around the cursor, linewise
    /// (vim's `vip` in one chord).
    SelectParagraph,
    /// Select a text object (e.g. `iw`, `a"`, `i(`, `a{`).
    SelectTextObject(TextObjectSpec),
    /// Surround text object with delimiter.
    SurroundTextObject {
        /// The text object to wrap.
        spec: TextObjectSpec,
        /// The delimiter to wrap it in.
        delimiter: char,
    },
    /// Write these bytes to the PTY.
    SendBytes(Vec<u8>),
    /// Set a named mark at the current cursor position (`m{a-z}`).
    SetMark(char),
    /// Split the focused pane in this direction.
    SplitPane(Direction),
    /// Switch the focused pane to this mode.
    SwitchMode(Mode),
    /// `i`/`a`/`o`: leave Normal (or Visual) for Insert, placing the cursor per
    /// [`InsertAt`] first.
    EnterInsert(InsertAt),
    /// Toggle the focused pane between full-viewport zoom and normal split layout.
    ZoomPane,
    /// Fold or unfold the block under the cursor.
    ToggleFold,
    /// Copy the block under the cursor to the clipboard.
    YankBlock,
    /// Copy the current selection to the clipboard.
    YankSelection,
    /// Yank the visual selection into named register (`"{reg}y`).
    YankSelectionRegister(char),
    /// Scroll up one screen.
    ScrollPageUp,
    /// Scroll down one screen.
    ScrollPageDown,
    /// Scroll up one line.
    ScrollLineUp,
    /// Scroll down one line.
    ScrollLineDown,
    /// Jump to the oldest retained scrollback.
    ScrollToTop,
    /// Jump back to the live output.
    ScrollToBottom,
    /// Copy the current selection to the clipboard.
    Copy,
    /// Open the settings page.
    OpenSettings,
    /// Increase the font size one step.
    IncreaseFontSize,
    /// Decrease the font size one step.
    DecreaseFontSize,
    /// Return the font size to the configured value.
    ResetFontSize,
    /// Show or hide the command palette.
    TogglePalette,
    /// Show or hide the palette over past queries.
    ToggleHistoryPalette,
    /// Show or hide the pane switcher.
    TogglePaneSwitcher,
    /// Show or hide buffer swoop.
    ToggleSwoop,
    /// A chord bound to a command-palette action name with no `WindowAction`
    /// variant (`mux_new_session`, `export_block_svg`, …); dispatched the
    /// same way selecting that palette entry would be.
    RunCommand(String),
}

// ========================================================================
// Constants
// ========================================================================

// ========================================================================
// Window keymap
// ========================================================================

// ========================================================================
// Resolution
// ========================================================================

/// Resolve a key in the given mode using the default window keymap.
/// `pending` tracks multi-key sequences (e.g. `]b`); it is updated in place.
/// `flags` is the active Kitty keyboard protocol flags for the focused pane
/// (0 = legacy xterm encoding). `modify_other_keys` is the xterm
/// modifyOtherKeys mode (`None` = off, `Some(1)` or `Some(2)`).
pub fn resolve(mode: Mode, key: &Key, pending: &mut PendingPrefix, flags: u32) -> Action {
    resolve_with(
        mode,
        key,
        pending,
        &WindowKeymap::default(),
        flags,
        None,
        false,
    )
}

/// Like [`resolve`], but using the supplied window keymap for the configurable
/// split/close/focus bindings. `is_alt_screen` is true when a full-screen TUI
/// app owns the pane — scroll-type direct bindings are bypassed in that case
/// so the key reaches the app.
pub fn resolve_with(
    mode: Mode,
    key: &Key,
    pending: &mut PendingPrefix,
    window: &WindowKeymap,
    flags: u32,
    modify_other_keys: Option<i64>,
    is_alt_screen: bool,
) -> Action {
    match mode {
        Mode::Insert => resolve_insert(key, flags, modify_other_keys, window, is_alt_screen),
        Mode::Normal => resolve_normal(key, pending, window),
        Mode::Visual => resolve_visual(key, pending, window),
        Mode::BlockFocus => resolve_block_focus(key, flags, window, is_alt_screen),
    }
}

// ---- xterm baseline encoding ------------------------------------------------

// ---- Kitty keyboard protocol encoding ---------------------------------------

// ---- Low-level sequence builders --------------------------------------------

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    pub(crate) fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }
    pub(crate) fn resolve_simple(mode: Mode, key: &Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(mode, key, &mut pending, 0)
    }
    pub(crate) fn kitty(code: KeyCode) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &key(code), &mut pending, 1)
    }
    pub(crate) fn kitty_key(k: Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &k, &mut pending, 1)
    }
}

#[cfg(test)]
mod tests {

    // ---- Kitty keyboard protocol encoding -----------------------------------
}
