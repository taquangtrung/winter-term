//! Keymap: resolve a key event into an [`Action`], given the pane's [`Mode`].
//!
//! In Insert, keys encode to terminal bytes for the PTY (the entry chord is the
//! one exception). In Normal, Winter intercepts keys as navigation/layout actions.
//! In Block-focus, keys forward to the block until `Esc`. Most bindings are
//! built in, but the window-management chords (split, close, focus) are
//! configurable through a [`Keymap`]; see [`resolve_with`].

mod actions;
mod insert;
mod keyfile;
mod keymap;
mod motion;
mod normal;
mod pending;
mod visual;
mod when;

pub use crate::vim::grammar::MacroStep;
pub use actions::window_action_by_name;
pub use actions::{EditAction, EditBinding, WindowAction};
pub use chord::{format_key, format_sequence};
pub use insert::{apply_cursor_key_mode, encode_release};
pub use keyfile::{
    block_of, blocks_for_platform, blocks_from_document, platform_of, BLOCK_PROPERTY,
    COMMANDS_NODE, PLATFORM_PROPERTY,
};
pub use keymap::{
    command_action, default_bindings_for, default_bindings_maps, Blocks, Catalog, Keymap,
    SequenceStep,
};
pub use motion::{
    BlockNav, CursorMove, FindChar, GotoMark, InsertAt, TextObject, TextObjectSpec, VisualKind,
};
pub use pending::PendingPrefix;
pub use sequence::MAX_SEQUENCE_LEN;
pub use when::{
    default_when_bindings, when_bindings, when_of, Condition, KeyContext, WhenBinding, WhenMap,
    WHEN_PROPERTY,
};
pub use winter_keys::{table, Key, KeyCode, Mods};

use winter_keys::{chord, sequence};

use insert::resolve_insert;
use normal::resolve_normal;
use visual::resolve_visual;

use super::layout::{Direction, FocusDir};
use super::mode::Mode;

// ========================================================================
// Data Structures
// ========================================================================

/// What a key does in a transient window, the same in every one, as
/// magit's transients have it: past these, a key is the window's own letter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransientKey {
    /// Run, or toggle, the item selected: `RET` or `SPC`.
    Choose,
    /// Close the window: `ESC` or `C-g`.
    Close,
    /// Select the next item: `<down>` or `C-n`.
    Next,
    /// Select the item before: `<up>` or `C-p`.
    Previous,
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
    /// Close all tabs to the left of the active tab in the focused pane.
    CloseTabsToLeft,
    /// Close all tabs to the right of the active tab in the focused pane.
    CloseTabsToRight,
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
    /// Abandon the search and restore the prior position.
    SearchCancel,
    /// Edit the search query being typed with this key, as a one-line field
    /// takes it: typing, and the Emacs keys of editing a line.
    SearchEdit(Key),
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
    /// Record, stop recording, or replay a keyboard macro.
    Macro(MacroStep),
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
// Transient windows
// ========================================================================

/// What `key` does in a transient window, if it is one of the keys every
/// transient answers the same; `None` for one of the window's own.
pub fn transient_key(key: &Key) -> Option<TransientKey> {
    match key.chord() {
        (Mods::NONE, KeyCode::Enter | KeyCode::Space) => Some(TransientKey::Choose),
        (Mods::NONE, KeyCode::Escape) | (Mods::CTRL, KeyCode::Char('g')) => {
            Some(TransientKey::Close)
        }
        (Mods::NONE, KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n')) => Some(TransientKey::Next),
        (Mods::NONE, KeyCode::Up) | (Mods::CTRL, KeyCode::Char('p')) => {
            Some(TransientKey::Previous)
        }
        _ => None,
    }
}

// ========================================================================
// Resolution
// ========================================================================

/// Resolve a key in the given mode using the default window keymap.
/// `pending` tracks multi-key sequences (e.g. `]b`); it is updated in place.
/// `flags` is the active Kitty keyboard protocol flags for the focused pane
/// (0 = legacy xterm encoding). `modify_other_keys` is the xterm
/// modifyOtherKeys mode (`None` = off, `Some(1)` or `Some(2)`).
pub fn resolve(mode: Mode, key: &Key, pending: &mut PendingPrefix, flags: u32) -> Action {
    resolve_with(mode, key, pending, &Keymap::default(), flags, None, false)
}

/// Like [`resolve`], but using the supplied window keymap for the configurable
/// split/close/focus bindings. `is_alt_screen` is true when a full-screen TUI
/// app owns the pane: scroll-type direct bindings are bypassed in that case
/// so the key reaches the app.
pub fn resolve_with(
    mode: Mode,
    key: &Key,
    pending: &mut PendingPrefix,
    window: &Keymap,
    flags: u32,
    modify_other_keys: Option<i64>,
    is_alt_screen: bool,
) -> Action {
    match mode {
        Mode::Insert => resolve_insert(key, flags, modify_other_keys, window, is_alt_screen),
        Mode::Normal => resolve_normal(key, pending, window),
        Mode::Visual => resolve_visual(key, pending, window),
    }
}

/// Resolve a key an editor tab was offered and declined. The window-layout
/// chords are honored: splitting, closing, zooming, and moving focus all
/// still have to work from an editor tab. So is a bare `Esc`, which puts a lit
/// search away as it does in a terminal's Normal mode, since the search is
/// the window's and not the editor's. Anything else would act on a terminal
/// the tab does not have.
pub fn resolve_editor(key: &Key, window: &Keymap) -> Action {
    if let Some(action) = window.direct_action(key, false) {
        return action;
    }
    // A pane or tab named by its number (`C-M-3` closes pane 3, `M-2` goes
    // to pane 2) is the window's from an editor's pane as from a terminal's.
    if let Some(action) = window.specific_index_action(key) {
        return action;
    }
    match key.glyph() == KeyCode::Escape && key.mods() == Mods::NONE {
        true => Action::SearchCancel,
        false => Action::Ignore,
    }
}

// ---- xterm baseline encoding ------------------------------------------------

// ---- Kitty keyboard protocol encoding ---------------------------------------

// ---- Low-level sequence builders --------------------------------------------

// ========================================================================
// Tests
// ========================================================================

#[cfg(any(test, feature = "test-support"))]
/// Building keys and resolving them in tests, here and in the app's.
pub mod test_support {
    use std::collections::HashMap;

    use super::*;

    /// What opens the reason a key is read by its parts, which
    /// [`raw_key_reads`] then lets pass.
    const KEY_EXACT: &str = "// key-exact:";

    /// What opens the reason a key is matched as written rather than looked
    /// up in a key table, which [`literal_key_matches`] then lets pass.
    const KEY_TABLE: &str = "// key-table:";

    /// `code` pressed alone, with no modifier held.
    pub fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }
    /// The built-in keymap with sequences a user's file might add, which the
    /// defaults hold none of: tabs moved on `M-g`, macros on `C-x`, help on
    /// `F1`, and Git's `M-g r` sharing the window's leader.
    pub fn sequence_keymap() -> Keymap {
        let block = |bindings: &[(&str, &str)]| -> HashMap<String, String> {
            bindings
                .iter()
                .map(|(spec, name)| (spec.to_string(), name.to_string()))
                .collect()
        };
        let window = block(&[
            ("M-g M-h", "move_tab_left"),
            ("M-g M-j", "move_tab_down"),
            ("M-g M-k", "move_tab_up"),
            ("M-g M-l", "move_tab_right"),
            ("C-x (", "macro_start_recording"),
            ("C-x )", "macro_stop_recording"),
            ("C-x e", "macro_replay"),
            ("F1 h", "tab_keys"),
            ("F1 k", "describe_key"),
        ]);
        let blocks = HashMap::from([
            ("window".to_string(), window),
            ("git".to_string(), block(&[("M-g r", "open_remote")])),
        ]);
        Keymap::from_blocks(&blocks)
    }

    /// What `key` does in `mode`, with nothing pending before it.
    pub fn resolve_simple(mode: Mode, key: &Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(mode, key, &mut pending, 0)
    }
    /// What `code` sends in Insert mode under the kitty keyboard protocol.
    pub fn kitty(code: KeyCode) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &key(code), &mut pending, 1)
    }
    /// What `k` sends in Insert mode under the kitty keyboard protocol.
    pub fn kitty_key(k: Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &k, &mut pending, 1)
    }

    /// Every line of the source under `root` that reads a key's parts rather
    /// than its whole [`Key::chord`], as `path:line: text`. Tests, lines
    /// marked `// key-exact:`, and the `exempt` files (paths under `root`,
    /// the key model's own) are left out.
    pub fn raw_key_reads(root: &std::path::Path, exempt: &[&str]) -> Vec<String> {
        let parts = regex::Regex::new(r"\.(ctrl|alt|shift|sup)\b|\.glyph\(\)|\bkey\.code\b")
            .expect("a pattern");
        unmarked_lines(root, exempt, KEY_EXACT, |code| parts.is_match(code))
    }

    /// Every line of the source under `root` that matches a key written out
    /// (`KeyCode::Char('q') =>`, `(Mods::CTRL, KeyCode::Char('d'))`,
    /// `== KeyCode::Enter`) rather than looking it up in a key table, as
    /// `path:line: text`: a key no user could rebind, and the Keys editor
    /// could not list. Tests, lines marked `// key-table:` with why the key
    /// is no table's (text being typed, a capture's own keys), and the
    /// `exempt` files (paths under `root`) are left out.
    pub fn literal_key_matches(root: &std::path::Path, exempt: &[&str]) -> Vec<String> {
        let literal = regex::Regex::new(
            r"Mods::\w+(?:\s*\|\s*Mods::\w+)*\s*,\s*KeyCode::|KeyCode::\w+(?:\([^)]*\))?\s*(?:=>|\|[^|])|==\s*KeyCode::|==\s*\(Mods::",
        )
        .expect("a pattern");
        unmarked_lines(root, exempt, KEY_TABLE, |code| literal.is_match(code))
    }

    /// Every line of the source under `root` whose code `flags`, as
    /// `path:line: text`, unless it or the line above carries `marker` and
    /// the reason after it. Tests, a file's own `#[cfg(test)]` part, and the
    /// `exempt` files (paths under `root`) are left out.
    fn unmarked_lines(
        root: &std::path::Path,
        exempt: &[&str],
        marker: &str,
        flags: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let root = root.to_path_buf();
        let mut found = Vec::new();
        let mut dirs = vec![root.clone()];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for path in entries.flatten().map(|entry| entry.path()) {
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                let is_exempt = path
                    .strip_prefix(&root)
                    .is_ok_and(|rel| exempt.iter().any(|file| rel == std::path::Path::new(file)));
                // A module's tests, in a file of their own or a folder of them.
                let tests = name == "tests.rs"
                    || name.ends_with("_tests.rs")
                    || path
                        .strip_prefix(&root)
                        .is_ok_and(|rel| rel.components().any(|part| part.as_os_str() == "tests"));
                if !name.ends_with(".rs") || tests || is_exempt {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                // A file's own tests build keys by their parts.
                let source = text.split("#[cfg(test)]").next().unwrap_or_default();
                // The reason may close the line or open the one above it,
                // where rustfmt keeps a comment it would move off a line
                // ending in `{`.
                let mut above = "";
                for (at, line) in source.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    let marked = line.contains(marker) || above.trim_start().starts_with(marker);
                    if flags(code) && !marked {
                        let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                        found.push(format!("{rel}:{}: {}", at + 1, line.trim()));
                    }
                    above = line;
                }
            }
        }
        found.sort();
        found
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
    fn test_an_editor_tab_still_honors_the_window_layout_chords() {
        // An editor tab has to stay manageable: `M-h` moves focus off it and
        // `M-S-\\` splits beside it, exactly as from a terminal tab.
        let focus_left = Key {
            alt: true,
            code: KeyCode::Char('h'),
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert_eq!(
            resolve_editor(&focus_left, &Keymap::default()),
            Action::FocusPane(FocusDir::Left)
        );
    }

    #[test]
    fn test_ctrl_shift_d_opens_the_dir_editor_from_any_mode() {
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
        assert_eq!(resolve_editor(&chord, &Keymap::default()), expected);
    }

    #[test]
    fn test_the_search_and_browser_chords_answer_from_any_mode() {
        // Named-command chords: each must parse from the sample keymap and
        // answer in Insert too, where there is no Normal-mode cursor, and
        // from inside an editor.
        for (spec, command) in [("C-S-f", "live_search"), ("C-S-e", "file_browser")] {
            let chord = chord::parse_chord(spec).expect("a chord");
            let expected = Action::RunCommand(command.to_string());
            for mode in [Mode::Insert, Mode::Normal, Mode::Visual] {
                assert_eq!(resolve_simple(mode, &chord), expected, "{spec} in {mode:?}");
            }
            assert_eq!(
                resolve_editor(&chord, &Keymap::default()),
                expected,
                "{spec} in an editor"
            );
        }
    }

    #[test]
    fn test_the_buffer_chords_answer_from_any_mode() {
        // The keys that reach a buried buffer have to answer from inside
        // whatever is showing instead of it, which after a pane was merged
        // away is a terminal in Insert mode.
        for (key, command) in [('k', "kill_buffer"), ('z', "recent_list")] {
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
                resolve_editor(&chord, &Keymap::default()),
                expected,
                "{key} in a page"
            );
        }
    }

    #[test]
    fn test_an_editor_tab_swallows_keys_the_editor_declined() {
        // `i` in an editor tab must not reach the Normal-mode resolver: switching
        // an editor tab to Insert would leave its keystrokes with nowhere to go.
        let insert = Key {
            alt: false,
            code: KeyCode::Char('i'),
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert_eq!(resolve_editor(&insert, &Keymap::default()), Action::Ignore);
    }

    // ---- Kitty keyboard protocol encoding -----------------------------------

    #[test]
    fn test_a_chord_keeps_every_modifier_but_the_shift_a_glyph_already_carries() {
        let shifted = |code| Key {
            shift: true,
            ..Key::plain(code)
        };
        // Shift typed the capital or the symbol: it is the glyph's.
        assert_eq!(
            shifted(KeyCode::Char('g')).chord(),
            (Mods::NONE, KeyCode::Char('G'))
        );
        assert_eq!(
            Key::plain(KeyCode::Char('G')).chord(),
            (Mods::NONE, KeyCode::Char('G'))
        );
        assert_eq!(
            Key::plain(KeyCode::Char('?')).chord(),
            (Mods::NONE, KeyCode::Char('?'))
        );
        // On any layout: a capital beyond ASCII, a digit typed with Shift.
        assert_eq!(
            shifted(KeyCode::Char('Ä')).chord(),
            (Mods::NONE, KeyCode::Char('Ä'))
        );
        assert_eq!(
            shifted(KeyCode::Char('1')).chord(),
            (Mods::NONE, KeyCode::Char('1'))
        );
        // Anywhere else it is a modifier of its own.
        assert_eq!(
            Key::with_ctrl_shift(KeyCode::Char('F')).chord(),
            (Mods::CTRL_SHIFT, KeyCode::Char('f'))
        );
        assert_eq!(shifted(KeyCode::Down).chord(), (Mods::SHIFT, KeyCode::Down));
        assert_ne!(
            Key::with_ctrl_shift(KeyCode::Char('f')).chord(),
            Key::with_ctrl(KeyCode::Char('f')).chord(),
            "C-S-f is not C-f"
        );
        let super_x = Key {
            sup: true,
            ..Key::plain(KeyCode::Char('x'))
        };
        assert_ne!(
            super_x.chord(),
            Key::plain(KeyCode::Char('x')).chord(),
            "s-x is not x"
        );
    }

    #[test]
    fn test_every_key_is_matched_by_its_whole_chord() {
        // A handler that reads `key.ctrl` alone reads `C-S-f` as `C-f`:
        // every key is matched by [`Key::chord`], or says why not.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = test_support::raw_key_reads(&root, &["input/mod.rs"]);
        assert!(
            found.is_empty(),
            "keys read by their parts:\n{}",
            found.join("\n")
        );
    }
}
