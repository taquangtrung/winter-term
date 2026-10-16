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

pub use insert::{apply_cursor_key_mode, encode_release};
pub use keymap::{format_key, EditAction, EditBinding, WindowAction, WindowKeymap};
pub(crate) use keymap::{window_action_by_name, DEFAULT_LEADER};
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    /// Whether Alt was held.
    pub alt: bool,
    /// The physical key, independent of modifiers.
    pub code: KeyCode,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
    /// Whether Super (the logo or Windows key) was held.
    pub sup: bool,
}

/// The modifiers held with a key, compared as a whole.
///
/// A shortcut is the exact set of modifiers it names: `C-d` is not
/// `C-S-d`, and neither is `C-M-d`. Handlers match
/// `(key.mods(), key.glyph())` against these constants so a chord with an extra
/// modifier is declined instead of read as the plainer one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mods {
    /// Whether Alt was held.
    pub alt: bool,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
    /// Whether Super was held.
    pub sup: bool,
}

impl Mods {
    /// Alt alone.
    pub const ALT: Mods = Mods {
        alt: true,
        ctrl: false,
        shift: false,
        sup: false,
    };
    /// Alt and Shift together.
    pub const ALT_SHIFT: Mods = Mods {
        alt: true,
        ctrl: false,
        shift: true,
        sup: false,
    };
    /// Ctrl alone.
    pub const CTRL: Mods = Mods {
        alt: false,
        ctrl: true,
        shift: false,
        sup: false,
    };
    /// Ctrl and Alt together.
    pub const CTRL_ALT: Mods = Mods {
        alt: true,
        ctrl: true,
        shift: false,
        sup: false,
    };
    /// Ctrl and Shift together.
    pub const CTRL_SHIFT: Mods = Mods {
        alt: false,
        ctrl: true,
        shift: true,
        sup: false,
    };
    /// No modifier.
    pub const NONE: Mods = Mods {
        alt: false,
        ctrl: false,
        shift: false,
        sup: false,
    };
    /// Shift alone.
    pub const SHIFT: Mods = Mods {
        alt: false,
        ctrl: false,
        shift: true,
        sup: false,
    };
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
    /// Close every tab in the focused pane except the active one.
    CloseOtherTabs,
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
    /// A configurable Insert-mode line edit (e.g. `C-DEL` to delete the
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
    /// `C-o`: jump to the previous recorded position (vim's jumplist).
    JumpOlder,
    /// `C-i`/`Tab`: jump to the next recorded position.
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
    /// Move the current tab to the neighbouring pane in the given direction.
    MoveTabToPane(FocusDir),
    /// Open a new tab.
    NewTab,
    /// Open a new window.
    NewWindow,
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
    /// Re-apply an undone prompt edit (`C-\`).
    PromptRedo,
    /// Undo the last prompt edit (`C-/`).
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
    /// (no input step) in the given direction: forward for `*`, backward for `#`.
    SearchWord {
        /// True to search forward, false to search backward.
        forward: bool,
    },
    /// `M-i` in Normal mode: select the paragraph around the cursor, linewise
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
    /// `yy`: copy the whole line the cursor is on.
    YankLine {
        /// The register it lands in, or `None` for the clipboard.
        register: Option<char>,
    },
    /// `y{motion}`: copy the text between the cursor and where the motion
    /// lands.
    YankMotion {
        /// Where the yank reaches to.
        motion: CursorMove,
        /// The register it lands in, or `None` for the clipboard.
        register: Option<char>,
    },
    /// `yi{object}`/`ya{object}`: copy the text object under the cursor.
    YankTextObject {
        /// Which object, and whether the `a` form.
        spec: TextObjectSpec,
        /// The register it lands in, or `None` for the clipboard.
        register: Option<char>,
    },
    /// Copy the current selection to the clipboard.
    YankSelection,
    /// Visual `Y`: copy the lines the selection touches, whole, whatever shape
    /// it was.
    YankSelectionLines,
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
    /// Copy the current working directory or file reference to the clipboard.
    CopyCwd,
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
    /// Jump to target in current buffer view (fuzzy completion dialog).
    Jump,
    /// A chord bound to a command-palette action name with no `WindowAction`
    /// variant (`mux_new_session`, `export_block_svg`, …); dispatched the
    /// same way selecting that palette entry would be.
    RunCommand(String),
}

// ========================================================================
// Key
// ========================================================================

impl Key {
    /// The modifiers held, as the one set a shortcut is matched against.
    pub fn mods(&self) -> Mods {
        Mods {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            sup: self.sup,
        }
    }

    /// Whether this is a key press winit could not name, such as a bare Ctrl
    /// or Shift going down, which arrives as the NUL character. It types
    /// nothing, and text entry that inserted it would show a stray glyph.
    pub fn is_unidentified(&self) -> bool {
        self.code == KeyCode::Char('\0')
    }

    /// This key with an unmodified space bar read as the space it types.
    ///
    /// The space bar has a code of its own, so text entry that matches
    /// `KeyCode::Char` alone never sees it; this lets that entry treat it as
    /// the character it is. A chord with Ctrl or Alt held is not typing, and
    /// stays as it arrived.
    pub fn typed(&self) -> Self {
        match (self.glyph(), !self.ctrl && !self.alt && !self.sup) {
            (KeyCode::Space, true) => Self {
                code: KeyCode::Char(' '),
                ..*self
            },
            (code, _) => Self { code, ..*self },
        }
    }

    /// This key in the one form every handler matches: a letter is the
    /// lowercase letter, and whether Shift was held is the `shift` flag alone.
    /// A shifted symbol is shifted however it was reached.
    ///
    /// The window reports the character Shift produces, so `S-s` arrives
    /// as `Char('S')` and `C-S-c` as `Char('C')`, while a layout or a
    /// Caps Lock may report either. Folding them all to `Char('s')` with
    /// `shift` set makes `s` and `S-s` two plainly different keys, and
    /// lets a binding written `S+s` match whichever way the key arrived. Use
    /// [`glyph`](Self::glyph) for the character a key types.
    pub fn canonical(&self) -> Self {
        match self.code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => Self {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                shift: true,
                ..*self
            },
            // A symbol that is itself the shifted glyph (`?`, `+`, `(`) is
            // shifted wherever it came from: the shifted key, a layout with it
            // unshifted, the keypad, or a chord written `M-?`.
            KeyCode::Char(c) if keymap::is_shifted_symbol(c) => Self {
                shift: true,
                ..*self
            },
            _ => *self,
        }
    }

    /// Whether Shift is held on a key that Shift gives no meaning of its own
    /// (`S-RET`, `S-ESC`, `S-<down>`, ...). A handler that binds
    /// the plain key has not bound this one, so it is left for the window to
    /// match rather than read as the unshifted key. Letters, symbols, `Tab`,
    /// `Space`, `Backspace`, and the function keys all take Shift.
    pub fn is_shifted_plain_key(&self) -> bool {
        self.shift
            && !self.ctrl
            && !self.alt
            && !self.sup
            && matches!(
                self.code,
                KeyCode::Delete
                    | KeyCode::Down
                    | KeyCode::End
                    | KeyCode::Enter
                    | KeyCode::Escape
                    | KeyCode::Home
                    | KeyCode::Insert
                    | KeyCode::Left
                    | KeyCode::PageDown
                    | KeyCode::PageUp
                    | KeyCode::Right
                    | KeyCode::Up
            )
    }

    /// The code this key types: a letter pressed with Shift alone is the
    /// capital, every other key is its code as it stands. The inverse of the
    /// letter folding in [`canonical`](Self::canonical) for keys that type.
    pub fn glyph(&self) -> KeyCode {
        match self.code {
            KeyCode::Char(c) if self.shift && !self.ctrl && !self.alt && !self.sup => {
                KeyCode::Char(c.to_ascii_uppercase())
            }
            code => code,
        }
    }

    /// `code` with no modifier held, which for a letter is the letter as
    /// typed: `Key::plain(KeyCode::Char('S'))` is what S-s arrives as.
    pub fn plain(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    /// `code` with Alt held.
    pub fn with_alt(code: KeyCode) -> Self {
        Self {
            alt: true,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    /// `code` with Ctrl held.
    pub fn with_ctrl(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: true,
            shift: false,
            sup: false,
        }
    }

    /// `code` with both Ctrl and Shift held.
    pub fn with_ctrl_shift(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: true,
            shift: true,
            sup: false,
        }
    }
}

impl KeyCode {
    /// The key a web engine's `KeyboardEvent.key` names, or `None` for one
    /// Winter has no code for: a bare modifier, or anything exotic enough
    /// that guessing would be worse than dropping it.
    ///
    /// Needed because a child WebView holds the keyboard whenever one is on
    /// screen, so a key typed over it reaches Winter as a DOM name rather
    /// than through the windowing layer.
    pub fn from_web_name(name: &str) -> Option<Self> {
        // The space bar names itself with the character it types, and Winter
        // has a code of its own for it.
        if name == " " {
            return Some(KeyCode::Space);
        }
        let mut chars = name.chars();
        if let (Some(only), None) = (chars.next(), chars.next()) {
            return Some(KeyCode::Char(only));
        }
        if let Some(number) = name.strip_prefix('F') {
            if let Ok(index) = number.parse::<u8>() {
                return Some(KeyCode::F(index));
            }
        }
        match name {
            "ArrowDown" => Some(KeyCode::Down),
            "ArrowLeft" => Some(KeyCode::Left),
            "ArrowRight" => Some(KeyCode::Right),
            "ArrowUp" => Some(KeyCode::Up),
            "Backspace" => Some(KeyCode::Backspace),
            "Delete" => Some(KeyCode::Delete),
            "End" => Some(KeyCode::End),
            "Enter" => Some(KeyCode::Enter),
            "Escape" => Some(KeyCode::Escape),
            "Home" => Some(KeyCode::Home),
            "Insert" => Some(KeyCode::Insert),
            "PageDown" => Some(KeyCode::PageDown),
            "PageUp" => Some(KeyCode::PageUp),
            "Tab" => Some(KeyCode::Tab),
            _ => None,
        }
    }
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
/// app owns the pane: scroll-type direct bindings are bypassed in that case
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
        Mode::Insert => resolve_insert(
            key,
            pending,
            flags,
            modify_other_keys,
            window,
            is_alt_screen,
        ),
        Mode::Normal => resolve_normal(key, pending, window),
        Mode::Visual => resolve_visual(key, pending, window),
        Mode::BlockFocus => resolve_block_focus(key, flags, window, is_alt_screen),
    }
}

/// Resolve a key a tool tab was offered and declined. Only the window-layout
/// chords are left to honor: splitting, closing, zooming, and moving focus
/// all still have to work from a tool tab. Anything else would act on a
/// terminal the tab does not have.
pub fn resolve_tool(key: &Key, window: &WindowKeymap) -> Action {
    window.direct_action(key, false).unwrap_or(Action::Ignore)
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
            sup: false,
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
    use super::*;

    #[test]
    fn test_typed_reads_a_bare_space_bar_as_a_space_and_leaves_chords_alone() {
        assert_eq!(
            Key::plain(KeyCode::Space).typed(),
            Key::plain(KeyCode::Char(' '))
        );
        let ctrl_space = Key::with_ctrl(KeyCode::Space);
        assert_eq!(ctrl_space.typed(), ctrl_space);
        let letter = Key::plain(KeyCode::Char('a'));
        assert_eq!(letter.typed(), letter);
    }

    #[test]
    fn test_web_key_names_map_to_the_codes_winter_binds() {
        // A surface hands keys back by their DOM name, so a name that maps to
        // nothing is a key that silently stops working inside a document.
        assert_eq!(KeyCode::from_web_name("j"), Some(KeyCode::Char('j')));
        assert_eq!(KeyCode::from_web_name("J"), Some(KeyCode::Char('J')));
        assert_eq!(KeyCode::from_web_name(" "), Some(KeyCode::Space));
        assert_eq!(KeyCode::from_web_name("ArrowDown"), Some(KeyCode::Down));
        assert_eq!(KeyCode::from_web_name("Escape"), Some(KeyCode::Escape));
        assert_eq!(KeyCode::from_web_name("F5"), Some(KeyCode::F(5)));
    }

    #[test]
    fn test_shift_on_a_named_key_is_not_the_plain_key() {
        let shifted = |code| Key {
            shift: true,
            sup: false,
            ..Key::plain(code)
        };
        assert!(shifted(KeyCode::Enter).is_shifted_plain_key());
        assert!(shifted(KeyCode::Down).is_shifted_plain_key());
        assert!(!Key::plain(KeyCode::Enter).is_shifted_plain_key());
        // Shift means something on these, so they are matched as they come.
        assert!(!shifted(KeyCode::Tab).is_shifted_plain_key());
        assert!(!shifted(KeyCode::Char('s')).is_shifted_plain_key());
        assert!(!shifted(KeyCode::Backspace).is_shifted_plain_key());
    }

    #[test]
    fn test_a_bare_modifier_is_not_a_key() {
        // Holding Shift fires a keydown of its own; routing it as a key would
        // send a stray press to the tool on every chord typed.
        assert_eq!(KeyCode::from_web_name("Shift"), None);
        assert_eq!(KeyCode::from_web_name("Control"), None);
        assert_eq!(KeyCode::from_web_name("Unidentified"), None);
    }
    use super::test_support::resolve_simple;

    #[test]
    fn test_a_chord_reads_as_the_lowercase_letter_but_typing_keeps_its_capital() {
        let ctrl_shift_c = Key {
            code: KeyCode::Char('C'),
            ..Key::with_ctrl_shift(KeyCode::Char('c'))
        };
        assert_eq!(
            ctrl_shift_c.canonical(),
            Key::with_ctrl_shift(KeyCode::Char('c'))
        );
        let alt_shift_q = Key {
            shift: true,
            sup: false,
            ..Key::with_alt(KeyCode::Char('Q'))
        };
        assert_eq!(alt_shift_q.canonical().code, KeyCode::Char('q'));
        let shift_s = Key::plain(KeyCode::Char('S')).canonical();
        assert_eq!(
            shift_s,
            Key {
                shift: true,
                sup: false,
                ..Key::plain(KeyCode::Char('s'))
            },
            "S-s is the lowercase letter with Shift held"
        );
        assert_eq!(
            shift_s.glyph(),
            KeyCode::Char('S'),
            "and it still types a capital"
        );
        assert_ne!(shift_s, Key::plain(KeyCode::Char('s')));
        // Super survives the folding; it is a modifier of its own.
        let super_shift_s = Key {
            sup: true,
            ..Key::plain(KeyCode::Char('S'))
        };
        assert!(super_shift_s.canonical().sup);
        assert_eq!(
            Key::with_ctrl(KeyCode::Char('+')).canonical(),
            Key::with_ctrl_shift(KeyCode::Char('+')),
            "a shifted symbol is shifted"
        );
    }

    #[test]
    fn test_a_tool_tab_still_honors_the_window_layout_chords() {
        // A tool tab has to stay manageable: `M-h` moves focus off it and
        // `M-S-\\` splits beside it, exactly as from a terminal tab.
        let focus_left = Key {
            alt: true,
            code: KeyCode::Char('h'),
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert_eq!(
            resolve_tool(&focus_left, &WindowKeymap::default()),
            Action::FocusPane(FocusDir::Left)
        );
    }

    #[test]
    fn test_ctrl_shift_d_opens_the_dir_tool_from_any_mode() {
        // The chord is a named-command binding, not a built-in window action,
        // so it has to resolve through every mode's window-chord lookup.
        let chord = Key {
            alt: false,
            code: KeyCode::Char('d'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        let expected = Action::RunCommand("dir_page".to_string());
        for mode in [Mode::Insert, Mode::Normal, Mode::Visual] {
            assert_eq!(resolve_simple(mode, &chord), expected, "mode {mode:?}");
        }
        assert_eq!(resolve_tool(&chord, &WindowKeymap::default()), expected);
    }

    #[test]
    fn test_alt_period_opens_what_the_terminal_points_at_from_any_mode() {
        // A named-command chord: it must parse from the sample keymap and
        // answer in Insert too, where there is no Normal-mode cursor.
        let chord = Key {
            alt: true,
            code: KeyCode::Char('.'),
            ctrl: false,
            shift: false,
            sup: false,
        };
        let expected = Action::RunCommand("open_under_cursor".to_string());
        for mode in [Mode::Insert, Mode::Normal, Mode::Visual] {
            assert_eq!(resolve_simple(mode, &chord), expected, "mode {mode:?}");
        }
    }

    #[test]
    fn test_the_reopen_chords_answer_from_any_mode() {
        // The keys that put back a tab closed by accident have to answer from
        // inside whatever is showing instead of it, which after a pane was
        // merged away is a terminal in Insert mode.
        for (key, command) in [('u', "reopen_page"), ('e', "tool_list")] {
            let chord = Key {
                alt: false,
                code: KeyCode::Char(key),
                ctrl: true,
                shift: true,
                sup: false,
            };
            let expected = Action::RunCommand(command.to_string());
            for mode in [Mode::Insert, Mode::Normal, Mode::Visual] {
                assert_eq!(resolve_simple(mode, &chord), expected, "{key} in {mode:?}");
            }
            assert_eq!(
                resolve_tool(&chord, &WindowKeymap::default()),
                expected,
                "{key} in a page"
            );
        }
    }

    #[test]
    fn test_a_tool_tab_swallows_keys_the_tool_declined() {
        // `i` in a tool tab must not reach the Normal-mode resolver: switching
        // a tool tab to Insert would leave its keystrokes with nowhere to go.
        let insert = Key {
            alt: false,
            code: KeyCode::Char('i'),
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert_eq!(
            resolve_tool(&insert, &WindowKeymap::default()),
            Action::Ignore
        );
    }

    // ---- Kitty keyboard protocol encoding -----------------------------------
}
