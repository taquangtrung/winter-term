//! Keymap: resolve a key event into an [`Action`], given the pane's [`Mode`].
//!
//! In Insert, keys encode to terminal bytes for the PTY (the entry chord is the
//! one exception). In Normal, Winter intercepts keys as navigation/layout actions.
//! In Block-focus, keys forward to the block until `Esc`. Most bindings are
//! built in, but the window-management chords (split, close, focus) are
//! configurable through a [`WindowKeymap`]; see [`resolve_with`].

use std::collections::{HashMap, HashSet};

use super::layout::{Direction, FocusDir};
use super::mode::{Mode, ModeEvent};

// ========================================================================
// Data Structures
// ========================================================================

/// A decoded key event from the windowing layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    pub alt: bool,
    pub code: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
}

/// A physical key, independent of modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCode {
    Backspace,
    Char(char),
    Delete,
    Down,
    End,
    Enter,
    Escape,
    F(u8),
    Home,
    Insert,
    Left,
    PageDown,
    PageUp,
    Right,
    Space,
    Tab,
    Up,
}

/// What a key resolves to in the current mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Close the focused pane (current pane).
    ClosePane,
    CloseOtherPanes,
    /// Close a tab: the active one (`None`) or the Nth tab, 1-based (`Some(n)`).
    CloseTab(Option<usize>),
    /// Change surrounding delimiter pair `target` to `replacement`.
    ChangeSurround {
        target: char,
        replacement: char,
    },
    /// Delete surrounding delimiter pair `target`.
    DeleteSurround(char),
    ChangeLine,
    ChangeToLineEnd,
    ChangeToLineStart,
    ChangeWordBack,
    ChangeWordForward,
    ChangeTextObject(TextObjectSpec),
    SubstituteChar,
    ReplaceChar(char),
    ToggleCaseChar,
    DeleteCharForward,
    DeleteLine,
    DeleteSelection,
    DeleteTextObject(TextObjectSpec),
    DeleteToLineEnd,
    DeleteToLineStart,
    DeleteWordBack,
    DeleteWordForward,
    /// A configurable Insert-mode line edit (e.g. `Ctrl-Backspace` to delete the
    /// word before the cursor). Realized as readline bytes plus a shadow update.
    Edit(EditAction),
    EnterVisual(VisualKind),
    /// A char-search within the current line (`f`/`F`/`t`/`T`).
    FindChar(FindChar),
    /// Pick a labelled landing spot from the `f`/`t` jump overlay.
    FindJump(char),
    /// Dismiss the `f`/`t` jump overlay without moving.
    FindCancel,
    /// Repeat the last char-search (`;`); `reverse` flips its direction (`,`).
    FindRepeat {
        reverse: bool,
    },
    FocusBlock(BlockNav),
    FocusPane(FocusDir),
    FocusPaneByIndex(usize),
    ClosePaneByIndex(usize),
    ForwardToBlock(Vec<u8>),
    /// Jump to a named mark (`{a-z}` or `'{a-z}`).
    GotoMark(GotoMark),
    /// Switch to tab number `n` (1-based).
    GotoTab(usize),
    Ignore,
    MoveCursor(CursorMove),
    /// A motion with a count prefix (`5j`, `3w`): repeat `mv` `count` times.
    /// Only motions where repetition is meaningful resolve to this (see
    /// `count_repeats`); the rest drop the count.
    MoveCursorN {
        count: usize,
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
        forward: bool,
    },
    /// `cgn` / `cgN`: change the next / previous search match.
    ChangeSearchMatch {
        forward: bool,
    },
    /// `dgn` / `dgN`: delete the next / previous search match.
    DeleteSearchMatch {
        forward: bool,
    },
    /// Visual `o`: move the cursor to the selection's other end.
    SwapVisualEnds,
    /// `gv`: re-enter Visual mode with the last selection.
    RestoreVisual,
    MoveTabLeft,
    MoveTabRight,
    NewTab,
    NextTab,
    Paste,
    /// Paste from named register `register` (`after` is true for `p`, false for `P`).
    PasteRegister {
        register: char,
        after: bool,
    },
    PrevTab,
    /// Re-apply an undone prompt edit (`Ctrl-\`).
    PromptRedo,
    /// Undo the last prompt edit (`Ctrl-/`).
    PromptUndo,
    /// `.`: repeat the pane's most recent change at the cursor.
    RepeatLastChange,
    /// `gx`: open the URL or file reference under the Normal-mode cursor.
    OpenUnderCursor,
    QuickCancel,
    QuickJump(char),
    QuickSelect,
    SearchBackspace,
    SearchCancel,
    SearchChar(char),
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
        forward: bool,
    },
    /// `Alt-i` in Normal mode: select the paragraph around the cursor, linewise
    /// (vim's `vip` in one chord).
    SelectParagraph,
    /// Select a text object (e.g. `iw`, `a"`, `i(`, `a{`).
    SelectTextObject(TextObjectSpec),
    /// Surround text object with delimiter.
    SurroundTextObject {
        spec: TextObjectSpec,
        delimiter: char,
    },
    SendBytes(Vec<u8>),
    /// Set a named mark at the current cursor position (`m{a-z}`).
    SetMark(char),
    SplitPane(Direction),
    SwitchMode(Mode),
    /// `i`/`a`/`o`: leave Normal (or Visual) for Insert, placing the cursor per
    /// [`InsertAt`] first.
    EnterInsert(InsertAt),
    /// Toggle the focused pane between full-viewport zoom and normal split layout.
    ZoomPane,
    ToggleFold,
    YankBlock,
    YankSelection,
    /// Yank the visual selection into named register (`"{reg}y`).
    YankSelectionRegister(char),
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollToTop,
    ScrollToBottom,
    Copy,
    OpenSettings,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    TogglePalette,
    ToggleHistoryPalette,
    TogglePaneSwitcher,
    ToggleSwoop,
    /// A chord bound to a command-palette action name with no `WindowAction`
    /// variant (`mux_new_session`, `export_block_svg`, …); dispatched the
    /// same way selecting that palette entry would be.
    RunCommand(String),
}

/// A vim char-search within the current line. `forward` is `f`/`t`; `till`
/// (`t`/`T`) stops one cell short of the target instead of on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindChar {
    pub ch: char,
    pub forward: bool,
    pub till: bool,
}

impl FindChar {
    /// The same search with its direction flipped, used to repeat `f`/`t` the
    /// opposite way on `,`.
    pub fn reversed(self) -> Self {
        Self {
            forward: !self.forward,
            ..self
        }
    }
}

/// Jump to a named mark (`` `{a-z} `` or `'{a-z}`). `exact` lands on the exact column
/// (`` ` ``); otherwise lands on the row's first non-blank (`'`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GotoMark {
    pub exact: bool,
    pub mark: char,
}

impl GotoMark {
    pub fn new(mark: char, exact: bool) -> Self {
        Self { exact, mark }
    }
}

/// A vim text object target (word, delimited quotes, or bracket pairs).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextObject {
    Brackets(char, char),
    Quotes(char),
    Word,
    WordBig,
}

/// Specification for a text object selection or operation (`around` vs `inner`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextObjectSpec {
    pub around: bool,
    pub object: TextObject,
}

impl TextObjectSpec {
    pub fn new(around: bool, object: TextObject) -> Self {
        Self { around, object }
    }
}

/// Whether a Visual selection spans blocks, characters, or whole lines (`Ctrl-V`, `v`, `V`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualKind {
    Block,
    Char,
    Line,
}

/// Where the cursor goes when Normal mode hands control back to the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertAt {
    /// `i`: insert at the cursor, exactly where Normal mode left it.
    Cursor,
    /// `a`: append — one column right of the cursor.
    After,
    /// `o`: at the end of the line. A shell prompt has no line below to open, so
    /// `o` lands where fresh typing continues the command instead.
    LineEnd,
}

/// Direction of a block-selection move in Normal mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockNav {
    Next,
    Previous,
}

impl BlockNav {
    /// The opposite direction, used to walk a sticky search direction the
    /// other way (`N` after `?`/`#` walks forward, `n` after them walks back).
    pub fn reversed(self) -> Self {
        match self {
            BlockNav::Next => BlockNav::Previous,
            BlockNav::Previous => BlockNav::Next,
        }
    }
}

/// A Normal-mode cursor traversal over the pane's whole buffer, scrollback
/// included. Moves past a viewport edge scroll the view to follow the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorMove {
    Bottom,
    Down,
    FirstNonBlank,
    HalfPageDown,
    HalfPageUp,
    /// `g_`: the last non-blank character on the line.
    LastNonBlank,
    Left,
    /// `zb`: scroll so the cursor's line sits on the viewport's last row.
    LineToBottom,
    /// `zz`: scroll so the cursor's line sits in the middle of the viewport.
    LineToCenter,
    /// `zt`: scroll so the cursor's line sits on the viewport's first row.
    LineToTop,
    LineEnd,
    LineStart,
    /// `%`: the bracket matching the one at or right of the cursor.
    MatchingBracket,
    PageDown,
    PageUp,
    /// `{`: the previous paragraph boundary (blank line).
    ParagraphBack,
    /// `}`: the next paragraph boundary (blank line).
    ParagraphForward,
    Right,
    /// `H`: the viewport's first row.
    ScreenTop,
    /// `M`: the middle row of the viewport.
    ScreenMiddle,
    /// `L`: the viewport's last row holding content.
    ScreenBottom,
    Top,
    Up,
    WordBack,
    WordBackBig,
    WordEnd,
    /// `ge`: the end of the previous word.
    WordEndBack,
    /// `gE`: the end of the previous WORD.
    WordEndBackBig,
    WordEndBig,
    WordForward,
    WordForwardBig,
}

/// Tracks a pending prefix awaiting the second key in a vim-style multi-key
/// sequence (e.g. `]b`, `[b`, `za`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingPrefix {
    BracketClose,
    BracketOpen,
    /// A count typed before a motion (`5` before `j`), accumulating further
    /// digits until the motion that spends it resolves.
    Count(usize),
    CtrlW,
    Change,
    /// Awaiting the search match direction after `cg` (`cgn`/`cgN`).
    ChangeG,
    /// Awaiting the object key of a change operator (`ci`/`ca`).
    ChangeObject {
        around: bool,
    },
    Delete,
    /// Awaiting the search match direction after `dg` (`dgn`/`dgN`).
    DeleteG,
    /// Awaiting the object key of a delete operator (`di`/`da`).
    DeleteObject {
        around: bool,
    },
    /// Awaiting the target char of a backward find (`F`).
    FindBackward,
    /// Awaiting the label key of the `f`/`t` jump overlay.
    FindLabel,
    /// Awaiting the target char of a forward find (`f`).
    FindForward,
    G,
    /// Awaiting the letter after `` ` `` (`exact: true`) or `'` (`exact: false`).
    GotoMark {
        exact: bool,
    },
    None,
    QuickSelect,
    /// Awaiting a register name after `"` (`"a`..`"z`, `"0`..`"9`, `"+`, `"*`, `""`).
    Register,
    /// A register has been selected, awaiting operator or motion.
    WithRegister(char),
    /// Awaiting target delimiter after `cs` (change surround).
    ChangeSurroundTarget,
    /// Awaiting replacement delimiter after `cs<target>`.
    ChangeSurroundReplacement {
        target: char,
    },
    /// Awaiting target delimiter after `ds` (delete surround).
    DeleteSurround,
    /// Awaiting target text object or motion after `ys`.
    YieldSurround {
        around: bool,
    },
    /// Awaiting delimiter after `ys<object>`.
    YieldSurroundDelimiter {
        spec: TextObjectSpec,
    },
    SearchInput,
    /// Awaiting the replacement char after `r`.
    ReplaceChar,
    /// Awaiting the mark letter after `m`.
    SetMark,
    /// Awaiting the object key of a visual text object (`i`/`a` in Visual mode).
    TextObject {
        around: bool,
    },
    /// Awaiting the target char of a backward till (`T`).
    TillBackward,
    /// Awaiting the target char of a forward till (`t`).
    TillForward,
    Z,
}

impl PendingPrefix {
    /// Human-readable prefix title and valid continuation pairs `(key, label)`
    /// for which-key discoverability overlays, or `None` if this prefix opts out.
    pub fn hint(&self) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
        match self {
            PendingPrefix::BracketClose => Some(("]", &[("b", "next block")])),
            PendingPrefix::BracketOpen => Some(("[", &[("b", "previous block")])),
            PendingPrefix::CtrlW => Some((
                "Ctrl-W",
                &[
                    ("v", "split vertical"),
                    ("s", "split horizontal"),
                    ("q / x", "close pane"),
                    ("o", "close other panes"),
                    ("z", "zoom pane"),
                    ("h / j / k / l", "focus left / down / up / right"),
                ],
            )),
            PendingPrefix::Change => Some((
                "c",
                &[
                    ("c", "change line"),
                    ("w / e", "change word forward"),
                    ("b", "change word back"),
                    ("$", "change to end of line"),
                    ("0", "change to start of line"),
                    ("i", "inner text object"),
                    ("a", "a text object"),
                    ("g", "search match gn/gN"),
                    ("s", "change surrounding"),
                ],
            )),
            PendingPrefix::ChangeG => Some((
                "cg",
                &[
                    ("n", "change next search match"),
                    ("N", "change prev search match"),
                ],
            )),
            PendingPrefix::ChangeObject { around: false } => Some((
                "ci",
                &[
                    ("w", "inner word"),
                    ("W", "inner WORD"),
                    ("\" / ' / `", "inner quotes"),
                    ("( / ) / b", "inner ()"),
                    ("[ / ]", "inner []"),
                    ("{ / } / B", "inner {}"),
                    ("< / >", "inner <>"),
                ],
            )),
            PendingPrefix::ChangeObject { around: true } => Some((
                "ca",
                &[
                    ("w", "a word"),
                    ("W", "a WORD"),
                    ("\" / ' / `", "a quotes"),
                    ("( / ) / b", "a ()"),
                    ("[ / ]", "a []"),
                    ("{ / } / B", "a {}"),
                    ("< / >", "a <>"),
                ],
            )),
            PendingPrefix::Delete => Some((
                "d",
                &[
                    ("d", "delete line"),
                    ("w", "delete word forward"),
                    ("b", "delete word back"),
                    ("$", "delete to end of line"),
                    ("0", "delete to start of line"),
                    ("i", "inner text object"),
                    ("a", "a text object"),
                    ("g", "search match gn/gN"),
                    ("s", "delete surrounding"),
                ],
            )),
            PendingPrefix::DeleteG => Some((
                "dg",
                &[
                    ("n", "delete next search match"),
                    ("N", "delete prev search match"),
                ],
            )),
            PendingPrefix::DeleteObject { around: false } => Some((
                "di",
                &[
                    ("w", "inner word"),
                    ("W", "inner WORD"),
                    ("\" / ' / `", "inner quotes"),
                    ("( / ) / b", "inner ()"),
                    ("[ / ]", "inner []"),
                    ("{ / } / B", "inner {}"),
                    ("< / >", "inner <>"),
                ],
            )),
            PendingPrefix::DeleteObject { around: true } => Some((
                "da",
                &[
                    ("w", "a word"),
                    ("W", "a WORD"),
                    ("\" / ' / `", "a quotes"),
                    ("( / ) / b", "a ()"),
                    ("[ / ]", "a []"),
                    ("{ / } / B", "a {}"),
                    ("< / >", "a <>"),
                ],
            )),
            PendingPrefix::FindForward => Some(("f", &[("<char>", "find character forward")])),
            PendingPrefix::FindBackward => Some(("F", &[("<char>", "find character backward")])),
            PendingPrefix::TillForward => Some(("t", &[("<char>", "till character forward")])),
            PendingPrefix::TillBackward => Some(("T", &[("<char>", "till character backward")])),
            PendingPrefix::G => Some((
                "g",
                &[
                    ("g", "top of buffer"),
                    ("v", "restore visual"),
                    ("_", "last non-blank"),
                    ("e", "previous word end"),
                    ("E", "previous WORD end"),
                    ("t", "next tab"),
                    ("T", "previous tab"),
                    ("<", "move tab left"),
                    (">", "move tab right"),
                    (";", "previous change"),
                    (",", "next change"),
                    ("x", "open under cursor"),
                    ("s", "buffer swoop"),
                    ("p", "jump to prompt"),
                    ("P", "jump to prev prompt"),
                    ("n", "select next search match"),
                    ("N", "select prev search match"),
                ],
            )),
            PendingPrefix::GotoMark { exact: true } => {
                Some(("`", &[("{a-z}", "jump to mark exact column")]))
            }
            PendingPrefix::GotoMark { exact: false } => {
                Some(("'", &[("{a-z}", "jump to mark first non-blank")]))
            }
            PendingPrefix::SetMark => Some(("m", &[("{a-z}", "set mark a-z")])),
            PendingPrefix::Register => Some((
                "\"",
                &[
                    ("{a-z}", "named register"),
                    ("+ / *", "clipboard register"),
                    ("\"", "unnamed register"),
                    ("0-9", "numbered register"),
                ],
            )),
            PendingPrefix::WithRegister(reg) => {
                let _ = reg;
                Some((
                    "\"<reg>",
                    &[
                        ("y", "yank to register"),
                        ("p / P", "paste from register"),
                        ("d", "delete to register"),
                    ],
                ))
            }
            PendingPrefix::ChangeSurroundTarget => Some((
                "cs",
                &[
                    ("\" / ' / `", "quotes"),
                    ("( / ) / b", "parentheses"),
                    ("[ / ]", "brackets"),
                    ("{ / } / B", "braces"),
                    ("< / >", "angle brackets"),
                ],
            )),
            PendingPrefix::ChangeSurroundReplacement { .. } => Some((
                "cs<target>",
                &[
                    ("\" / ' / `", "replacement quotes"),
                    ("( / [ / { / <", "replacement brackets"),
                ],
            )),
            PendingPrefix::DeleteSurround => Some((
                "ds",
                &[
                    ("\" / ' / `", "quotes"),
                    ("( / ) / b", "parentheses"),
                    ("[ / ]", "brackets"),
                    ("{ / } / B", "braces"),
                    ("< / >", "angle brackets"),
                ],
            )),
            PendingPrefix::YieldSurround { .. } => Some((
                "ys",
                &[
                    ("iw / aw", "word"),
                    ("iW / aW", "WORD"),
                    ("i\" / a\"", "quotes"),
                    ("i( / a(", "parentheses"),
                ],
            )),
            PendingPrefix::YieldSurroundDelimiter { .. } => Some((
                "ys<obj>",
                &[
                    ("\" / ' / `", "wrap with quotes"),
                    ("( / [ / { / <", "wrap with brackets"),
                ],
            )),
            PendingPrefix::TextObject { around: false } => Some((
                "i",
                &[
                    ("w", "inner word"),
                    ("W", "inner WORD"),
                    ("\" / ' / `", "inner quotes"),
                    ("( / ) / b", "inner ()"),
                    ("[ / ]", "inner []"),
                    ("{ / } / B", "inner {}"),
                    ("< / >", "inner <>"),
                ],
            )),
            PendingPrefix::TextObject { around: true } => Some((
                "a",
                &[
                    ("w", "a word"),
                    ("W", "a WORD"),
                    ("\" / ' / `", "a quotes"),
                    ("( / ) / b", "a ()"),
                    ("[ / ]", "a []"),
                    ("{ / } / B", "a {}"),
                    ("< / >", "a <>"),
                ],
            )),
            PendingPrefix::ReplaceChar => Some(("r", &[("<char>", "replace character")])),
            PendingPrefix::Z => Some((
                "z",
                &[
                    ("z", "center line on screen"),
                    ("t", "scroll line to top"),
                    ("b", "scroll line to bottom"),
                    ("a", "toggle fold"),
                ],
            )),
            PendingPrefix::Count(_)
            | PendingPrefix::FindLabel
            | PendingPrefix::None
            | PendingPrefix::QuickSelect
            | PendingPrefix::SearchInput => None,
        }
    }
}

/// A configurable Insert-mode line edit at the shell prompt. Each is realized by
/// sending the equivalent readline keystrokes (the app layer owns that mapping)
/// and folding the same change into the prompt undo shadow.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditAction {
    DeleteToLineEnd,
    DeleteToLineStart,
    DeleteWordBackward,
    DeleteWordForward,
}

impl EditAction {
    /// Parse a config action name (shared with the binding docs).
    fn from_name(name: &str) -> Option<EditAction> {
        Some(match name {
            "delete_to_line_end" => EditAction::DeleteToLineEnd,
            "delete_to_line_start" => EditAction::DeleteToLineStart,
            "delete_word_backward" => EditAction::DeleteWordBackward,
            "delete_word_forward" => EditAction::DeleteWordForward,
            _ => return None,
        })
    }
}

/// A configurable binding in the `editing` block: either a line edit, or a
/// prompt-history command (undo/redo). Line edits apply only in Insert mode;
/// undo/redo apply in Insert, Normal, and the palette.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditBinding {
    Edit(EditAction),
    Redo,
    Undo,
}

impl EditBinding {
    /// Parse a config action name. Recognizes the line-edit names plus
    /// `prompt_undo`/`undo` and `prompt_redo`/`redo`.
    fn from_name(name: &str) -> Option<EditBinding> {
        Some(match name {
            "prompt_redo" | "redo" => EditBinding::Redo,
            "prompt_undo" | "undo" => EditBinding::Undo,
            other => EditBinding::Edit(EditAction::from_name(other)?),
        })
    }

    /// The dispatched [`Action`] this binding produces.
    fn to_action(self) -> Action {
        match self {
            EditBinding::Edit(edit) => Action::Edit(edit),
            EditBinding::Redo => Action::PromptRedo,
            EditBinding::Undo => Action::PromptUndo,
        }
    }

    /// Whether this binding is a prompt-history command, which (unlike line
    /// edits) is also active in Normal mode and the palette.
    fn is_history(self) -> bool {
        matches!(self, EditBinding::Redo | EditBinding::Undo)
    }
}

/// A window-management command: the configurable subset of Normal-mode bindings.
/// Each maps to a layout-affecting [`Action`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowAction {
    Close,
    CloseOthers,
    FocusDown,
    FocusLeft,
    FocusRight,
    FocusUp,
    SplitHorizontal,
    SplitVertical,
    Zoom,
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollToTop,
    ScrollToBottom,
    PrevTab,
    NextTab,
    Copy,
    Paste,
    NewTab,
    CloseTab,
    OpenSettings,
    FontIncrease,
    FontDecrease,
    FontReset,
    TogglePalette,
    ToggleHistoryPalette,
    TogglePaneSwitcher,
    ToggleSwoop,
    NextBlock,
    PrevBlock,
}

impl WindowAction {
    /// Whether this action scrolls Winter's own scrollback. These bindings are
    /// bypassed when a full-screen (alt-screen) app is running so the key
    /// reaches the app instead — scrolling a TUI app's non-existent scrollback
    /// is never useful, and intercepting the key would swallow app shortcuts
    /// like Claude Code's `Shift+Alt+E/H/L`.
    fn is_scroll(self) -> bool {
        matches!(
            self,
            WindowAction::ScrollPageUp
                | WindowAction::ScrollPageDown
                | WindowAction::ScrollLineUp
                | WindowAction::ScrollLineDown
                | WindowAction::ScrollToTop
                | WindowAction::ScrollToBottom
        )
    }

    /// Whether this action must intercept the key before overlays (the command
    /// palette, tab rename, settings page) get a chance to consume it — e.g.
    /// `Ctrl-,` must open Settings even while the palette is open. Window
    /// layout actions (split/close/focus/scroll/zoom/tab-cycle) are
    /// deliberately excluded: they only apply once no overlay owns the key.
    fn is_global(self) -> bool {
        matches!(
            self,
            WindowAction::Copy
                | WindowAction::Paste
                | WindowAction::NewTab
                | WindowAction::CloseTab
                | WindowAction::OpenSettings
                | WindowAction::FontIncrease
                | WindowAction::FontDecrease
                | WindowAction::FontReset
                | WindowAction::TogglePalette
                | WindowAction::ToggleHistoryPalette
                | WindowAction::TogglePaneSwitcher
                | WindowAction::ToggleSwoop
                | WindowAction::NextBlock
                | WindowAction::PrevBlock
        )
    }
}

/// Which digit-indexed pane/tab operation an [`IndexAction`] performs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum IndexKind {
    ClosePane,
    FocusPane,
    GotoTab,
}

/// A chord bound to one *specific* pane or tab index (e.g. config action
/// `focus_pane_3` always focuses pane 3, whichever key triggers it).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct IndexAction {
    kind: IndexKind,
    n: usize,
}

impl IndexAction {
    fn to_action(self) -> Action {
        match self.kind {
            IndexKind::ClosePane => Action::ClosePaneByIndex(self.n),
            IndexKind::FocusPane => Action::FocusPaneByIndex(self.n),
            IndexKind::GotoTab => Action::GotoTab(self.n),
        }
    }

    /// Parse a config action name like `close_pane_3`, `focus_pane_3`, or
    /// `goto_tab_3` (`n` must be `1..=9`).
    fn from_name(name: &str) -> Option<Self> {
        let (kind, rest) = if let Some(rest) = name.strip_prefix("close_pane_") {
            (IndexKind::ClosePane, rest)
        } else if let Some(rest) = name.strip_prefix("focus_pane_") {
            (IndexKind::FocusPane, rest)
        } else if let Some(rest) = name.strip_prefix("goto_tab_") {
            (IndexKind::GotoTab, rest)
        } else {
            return None;
        };
        let n: usize = rest.parse().ok()?;
        (1..=9).contains(&n).then_some(IndexAction { kind, n })
    }
}

/// User-configurable window-management key bindings. A binding is either a
/// direct chord (e.g. `Ctrl-h` to focus left) or a two-key sequence opened by
/// the `leader` (e.g. `Ctrl-w` then `v` to split). Built from defaults and
/// overlaid with config via [`WindowKeymap::from_config`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowKeymap {
    /// Direct chords: the full key (with modifiers) triggers the action.
    direct: Vec<(Key, WindowAction)>,
    /// Line-edit and prompt-history chords (e.g. `Ctrl-Backspace` → delete word
    /// back, `Ctrl-/` → undo, `Ctrl-\` → redo).
    editing: Vec<(Key, EditBinding)>,
    /// Chords for globally-intercepted actions (settings, font size, tab/pane
    /// management, palette toggles): looked up before the command palette, tab
    /// rename, and settings overlays get a chance to consume the key.
    global: Vec<(Key, WindowAction)>,
    /// Chords bound to one specific pane/tab index (config actions
    /// `close_pane_N`/`focus_pane_N`/`goto_tab_N`, `N` = `1..9`).
    index_specific: Vec<(Key, IndexAction)>,
    /// The chord that opens a two-key window-command sequence.
    leader: Key,
    /// Direct chords bound to a command-palette action name with no
    /// `WindowAction` variant (`mux_new_session`, `export_block_svg`, …),
    /// dispatched through `run_command` instead of `to_action()`.
    named: Vec<(Key, String)>,
    /// Keys that select an action when pressed after the `leader`, matched by
    /// key code alone (modifiers on the follow key are ignored, as in Vim).
    sequence: Vec<(KeyCode, WindowAction)>,
}

// ========================================================================
// Constants
// ========================================================================

const CONTROL_MASK: u8 = 0x1f;
const DELETE: u8 = 0x7f;
const ESCAPE: u8 = 0x1b;
const CARRIAGE_RETURN: u8 = b'\r';
/// The largest count a `5j`-style prefix accumulates; further digits clamp
/// rather than overflow. Vim's own cap is far larger, but every repeatable
/// motion re-renders nothing between steps, so a runaway `99999j` costs a full
/// buffer walk for no navigation value.
const MAX_COUNT: usize = 9999;

/// The built-in default keybindings, embedded from the canonical file (also
/// installed as a reference copy at `~/.config/winter-term/keybindings.kdl`).
/// [`WindowKeymap::default`] parses this through the same pipeline used for
/// the user's own config, so there is exactly one place defining what the
/// defaults are.
const DEFAULT_KEYBINDINGS_KDL: &str = include_str!("../../samples/keybindings.kdl");

/// The leader key when no two-chord `window` binding (default or user) has
/// set one explicitly. There is no built-in two-chord default to parse this
/// from, so it stays a plain constant.
const DEFAULT_LEADER: Key = Key {
    alt: false,
    code: KeyCode::Char('w'),
    ctrl: true,
    shift: false,
};

// Kitty keyboard protocol: functional-key codepoints.
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
const KP_INSERT: u32 = 57348;
const KP_DELETE: u32 = 57349;
const KP_LEFT: u32 = 57350;
const KP_RIGHT: u32 = 57351;
const KP_UP: u32 = 57352;
const KP_DOWN: u32 = 57353;
const KP_PAGE_UP: u32 = 57354;
const KP_PAGE_DOWN: u32 = 57355;
const KP_HOME: u32 = 57356;
const KP_END: u32 = 57357;
const KP_F1: u32 = 57364;

// ========================================================================
// Window keymap
// ========================================================================

impl WindowAction {
    /// The dispatched [`Action`] this window command produces.
    fn to_action(self) -> Action {
        match self {
            WindowAction::Close => Action::ClosePane,
            WindowAction::CloseOthers => Action::CloseOtherPanes,
            WindowAction::FocusDown => Action::FocusPane(FocusDir::Down),
            WindowAction::FocusLeft => Action::FocusPane(FocusDir::Left),
            WindowAction::FocusRight => Action::FocusPane(FocusDir::Right),
            WindowAction::FocusUp => Action::FocusPane(FocusDir::Up),
            WindowAction::SplitHorizontal => Action::SplitPane(Direction::Horizontal),
            WindowAction::SplitVertical => Action::SplitPane(Direction::Vertical),
            WindowAction::Zoom => Action::ZoomPane,
            WindowAction::ScrollPageUp => Action::ScrollPageUp,
            WindowAction::ScrollPageDown => Action::ScrollPageDown,
            WindowAction::ScrollLineUp => Action::ScrollLineUp,
            WindowAction::ScrollLineDown => Action::ScrollLineDown,
            WindowAction::ScrollToTop => Action::ScrollToTop,
            WindowAction::ScrollToBottom => Action::ScrollToBottom,
            WindowAction::PrevTab => Action::PrevTab,
            WindowAction::NextTab => Action::NextTab,
            WindowAction::Copy => Action::Copy,
            WindowAction::Paste => Action::Paste,
            WindowAction::NewTab => Action::NewTab,
            WindowAction::CloseTab => Action::CloseTab(None),
            WindowAction::OpenSettings => Action::OpenSettings,
            WindowAction::FontIncrease => Action::IncreaseFontSize,
            WindowAction::FontDecrease => Action::DecreaseFontSize,
            WindowAction::FontReset => Action::ResetFontSize,
            WindowAction::TogglePalette => Action::TogglePalette,
            WindowAction::ToggleHistoryPalette => Action::ToggleHistoryPalette,
            WindowAction::TogglePaneSwitcher => Action::TogglePaneSwitcher,
            WindowAction::ToggleSwoop => Action::ToggleSwoop,
            WindowAction::NextBlock => Action::FocusBlock(BlockNav::Next),
            WindowAction::PrevBlock => Action::FocusBlock(BlockNav::Previous),
        }
    }

    /// Parse a config action name (matching the command-palette names).
    fn from_name(name: &str) -> Option<WindowAction> {
        Some(match name {
            "close_pane" => WindowAction::Close,
            "close_other_panes" => WindowAction::CloseOthers,
            "focus_down" => WindowAction::FocusDown,
            "focus_left" => WindowAction::FocusLeft,
            "focus_right" => WindowAction::FocusRight,
            "focus_up" => WindowAction::FocusUp,
            "split_horizontal" => WindowAction::SplitHorizontal,
            "split_vertical" => WindowAction::SplitVertical,
            "toggle_pane_zoom" | "zoom_pane" => WindowAction::Zoom,
            "scroll_page_up" => WindowAction::ScrollPageUp,
            "scroll_page_down" => WindowAction::ScrollPageDown,
            "scroll_line_up" => WindowAction::ScrollLineUp,
            "scroll_line_down" => WindowAction::ScrollLineDown,
            "scroll_to_top" => WindowAction::ScrollToTop,
            "scroll_to_bottom" => WindowAction::ScrollToBottom,
            "prev_tab" => WindowAction::PrevTab,
            "next_tab" => WindowAction::NextTab,
            "copy_selection" => WindowAction::Copy,
            "paste_from_clipboard" => WindowAction::Paste,
            "new_tab" => WindowAction::NewTab,
            "close_tab" => WindowAction::CloseTab,
            "open_settings" => WindowAction::OpenSettings,
            "font_increase" => WindowAction::FontIncrease,
            "font_decrease" => WindowAction::FontDecrease,
            "font_reset" => WindowAction::FontReset,
            "toggle_command_palette" => WindowAction::TogglePalette,
            "toggle_history_palette" => WindowAction::ToggleHistoryPalette,
            "select_pane" => WindowAction::TogglePaneSwitcher,
            "swoop" | "toggle_swoop" => WindowAction::ToggleSwoop,
            "next_block" => WindowAction::NextBlock,
            "prev_block" => WindowAction::PrevBlock,
            _ => return None,
        })
    }
}

/// The dispatched action for a config action name (`copy_selection`, …) that
/// mirrors a window command, or `None` when the name names none. Shared by
/// the keymap parser and the command palette so both dispatch identically.
pub(crate) fn window_action_by_name(name: &str) -> Option<Action> {
    WindowAction::from_name(name).map(WindowAction::to_action)
}

impl WindowKeymap {
    /// An unbound keymap: no chords at all. Only ever used as the starting
    /// point for [`from_config`](Self::from_config), which immediately layers
    /// the built-in defaults onto it before anything else.
    fn empty() -> Self {
        Self {
            direct: vec![],
            editing: vec![],
            global: vec![],
            index_specific: vec![],
            leader: DEFAULT_LEADER,
            named: vec![],
            sequence: vec![],
        }
    }

    /// Build a keymap from the `window` keybindings block, layered onto the
    /// built-in defaults (parsed from [`DEFAULT_KEYBINDINGS_KDL`]). A
    /// configured binding replaces every default chord for that same action,
    /// so rebinding `split_vertical` drops the default `Ctrl-w v`; actions
    /// left unmentioned keep their defaults.
    pub fn from_config(
        window: Option<&HashMap<String, String>>,
        editing: Option<&HashMap<String, String>>,
    ) -> Self {
        let defaults = default_bindings_maps();
        let mut keymap = Self::empty();
        keymap.apply_window_bindings(defaults.get("window"));
        keymap.apply_editing_bindings(defaults.get("editing"));
        keymap.apply_window_bindings(window);
        keymap.apply_editing_bindings(editing);
        keymap
    }

    /// Overlay the `window` block onto the default window chords. A configured
    /// binding replaces every default chord for that same action; a
    /// single-key chord bound to an unrecognized name becomes a named
    /// binding instead, displacing any `WindowAction` on that same chord.
    fn apply_window_bindings(&mut self, bindings: Option<&HashMap<String, String>>) {
        let Some(bindings) = bindings else {
            return;
        };
        self.apply_index_bindings(bindings);

        let mut parsed: Vec<(Vec<Key>, WindowAction)> = Vec::new();
        let mut named: Vec<(Key, String)> = Vec::new();
        for (spec, name) in bindings {
            let Some(keys) = parse_chord_sequence(spec) else {
                continue;
            };
            if let Some(action) = WindowAction::from_name(name) {
                parsed.push((keys, action));
            } else if IndexAction::from_name(name).is_none() {
                // `close_pane_N`/`focus_pane_N`/`goto_tab_N` already have a
                // home in `index_specific` via `apply_index_bindings` above;
                // only a name neither table recognizes becomes a named
                // command, or those chords would double-resolve.
                if let [single] = keys.as_slice() {
                    named.push((single.clone(), name.clone()));
                }
            }
        }

        if !named.is_empty() {
            self.direct
                .retain(|(key, _)| !named.iter().any(|(k, _)| k == key));
            self.global
                .retain(|(key, _)| !named.iter().any(|(k, _)| k == key));
            self.named.extend(named);
        }

        if parsed.is_empty() {
            return;
        }

        let rebound: HashSet<WindowAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.direct.retain(|(_, action)| !rebound.contains(action));
        self.global.retain(|(_, action)| !rebound.contains(action));
        self.sequence
            .retain(|(_, action)| !rebound.contains(action));

        for (keys, action) in parsed {
            match keys.as_slice() {
                [single] if action.is_global() => self.global.push((single.clone(), action)),
                [single] => self.direct.push((single.clone(), action)),
                [leader, follow] => {
                    self.leader = leader.clone();
                    self.sequence.push((follow.code, action));
                }
                _ => {}
            }
        }
    }

    /// Overlay the `close_pane_N`/`focus_pane_N`/`goto_tab_N` bindings from
    /// the `window` block. A configured chord replaces every prior chord for
    /// that same specific index, including the built-in default.
    fn apply_index_bindings(&mut self, bindings: &HashMap<String, String>) {
        let parsed: Vec<(Key, IndexAction)> = bindings
            .iter()
            .filter_map(|(spec, name)| Some((parse_chord(spec)?, IndexAction::from_name(name)?)))
            .collect();
        if parsed.is_empty() {
            return;
        }
        let rebound: HashSet<IndexAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.index_specific
            .retain(|(_, action)| !rebound.contains(action));
        self.index_specific.extend(parsed);
    }

    /// Overlay the `editing` block onto the default line-edit chords. A configured
    /// binding replaces every default chord for that same action.
    fn apply_editing_bindings(&mut self, bindings: Option<&HashMap<String, String>>) {
        let Some(bindings) = bindings else {
            return;
        };
        let parsed: Vec<(Key, EditBinding)> = bindings
            .iter()
            .filter_map(|(spec, name)| Some((parse_chord(spec)?, EditBinding::from_name(name)?)))
            .collect();
        if parsed.is_empty() {
            return;
        }
        let rebound: HashSet<EditBinding> = parsed.iter().map(|(_, action)| *action).collect();
        self.editing.retain(|(_, action)| !rebound.contains(action));
        self.editing.extend(parsed);
    }

    /// The [`Action`] a direct chord triggers: a `WindowAction` chord first
    /// (skipped without falling through when it's a scroll binding and
    /// `is_alt_screen` is true), else a palette-only named command.
    fn direct_action(&self, key: &Key, is_alt_screen: bool) -> Option<Action> {
        if let Some(action) = find_chord(&self.direct, key) {
            return (!(is_alt_screen && action.is_scroll())).then(|| action.to_action());
        }
        find_chord(&self.named, key).map(Action::RunCommand)
    }

    /// The globally-intercepted [`Action`] a chord triggers, if any (see
    /// [`WindowAction::is_global`]). Checked before the command palette, tab
    /// rename, and settings overlays get a chance to consume the key.
    pub(crate) fn global_action(&self, key: &Key) -> Option<Action> {
        find_chord(&self.global, key).map(WindowAction::to_action)
    }

    /// The pane/tab action a chord triggers via an index binding
    /// (`close_pane_N`/`focus_pane_N`/`goto_tab_N`), if any — including the
    /// built-in defaults.
    fn specific_index_action(&self, key: &Key) -> Option<Action> {
        find_chord(&self.index_specific, key).map(IndexAction::to_action)
    }

    /// The line-edit or prompt-history binding a chord triggers, if any. Like
    /// [`direct_action`](Self::direct_action) it retries with the unshifted glyph
    /// so specs using the physical key label still match.
    pub(crate) fn edit_binding(&self, key: &Key) -> Option<EditBinding> {
        find_chord(&self.editing, key)
    }

    /// The action the follow key selects after the leader, if any.
    fn sequence_action(&self, code: KeyCode) -> Option<WindowAction> {
        self.sequence
            .iter()
            .find(|(follow, _)| *follow == code)
            .map(|(_, action)| *action)
    }
}

impl Default for WindowKeymap {
    /// Built from the embedded default keybindings ([`DEFAULT_KEYBINDINGS_KDL`])
    /// through the same parsing pipeline as user config — see
    /// [`from_config`](Self::from_config).
    fn default() -> Self {
        Self::from_config(None, None)
    }
}

/// Parse [`DEFAULT_KEYBINDINGS_KDL`] into `window`/`editing` action maps, the
/// same shape a user's `keybindings.kdl` parses into.
fn default_bindings_maps() -> HashMap<String, HashMap<String, String>> {
    kdl::de::from_str(DEFAULT_KEYBINDINGS_KDL).unwrap_or_default()
}

impl WindowKeymap {
    /// Return the formatted shortcut hint for a palette command name
    /// (e.g. `"Ctrl-H"` for `"focus_left"`), or an empty string when unbound.
    pub fn chord_hint(&self, command: &str) -> String {
        let Some(action) = WindowAction::from_name(command) else {
            return self
                .named
                .iter()
                .find(|(_, name)| name == command)
                .map(|(key, _)| format_key(key))
                .unwrap_or_default();
        };
        self.direct
            .iter()
            .chain(self.global.iter())
            .find(|(_, a)| *a == action)
            .map(|(key, _)| format_key(key))
            .unwrap_or_default()
    }
}

/// Format a [`Key`] as a human-readable shortcut string (e.g. `"Ctrl-Shift-T"`).
pub fn format_key(key: &Key) -> String {
    let mut s = String::new();
    if key.ctrl {
        s.push_str("Ctrl-");
    }
    if key.shift {
        s.push_str("Shift-");
    }
    if key.alt {
        s.push_str("Alt-");
    }
    match key.code {
        KeyCode::Char(c) => s.push(c),
        KeyCode::Backspace => s.push_str("Backspace"),
        KeyCode::Delete => s.push_str("Del"),
        KeyCode::Down => s.push_str("Down"),
        KeyCode::End => s.push_str("End"),
        KeyCode::Enter => s.push_str("Enter"),
        KeyCode::Escape => s.push_str("Esc"),
        KeyCode::F(n) => {
            s.push('F');
            s.push_str(&n.to_string());
        }
        KeyCode::Home => s.push_str("Home"),
        KeyCode::Insert => s.push_str("Ins"),
        KeyCode::Left => s.push_str("Left"),
        KeyCode::PageDown => s.push_str("PgDn"),
        KeyCode::PageUp => s.push_str("PgUp"),
        KeyCode::Right => s.push_str("Right"),
        KeyCode::Space => s.push_str("Space"),
        KeyCode::Tab => s.push_str("Tab"),
        KeyCode::Up => s.push_str("Up"),
    }
    s
}

/// Parse a key-binding spec of one or two chords (e.g. `"C+h"` or `"C+w v"`)
/// into its keys. Returns `None` on any unrecognized chord or a length outside
/// 1..=2.
fn parse_chord_sequence(spec: &str) -> Option<Vec<Key>> {
    let keys = spec
        .split_whitespace()
        .map(parse_chord)
        .collect::<Option<Vec<Key>>>()?;
    if (1..=2).contains(&keys.len()) {
        Some(keys)
    } else {
        None
    }
}

/// Look up `key` in a chord table, retrying with the unshifted glyph if the
/// first lookup misses. winit reports `logical_key`, which applies the Shift
/// character transformation (`Shift+\` -> `'|'`, `Shift+-` -> `'_'`,
/// `Shift+o` -> `'O'`, ...); binding specs use the physical key label, so a
/// held-Shift chord that misses on the transformed glyph retries on the
/// untransformed one.
fn find_chord<T: Clone>(entries: &[(Key, T)], key: &Key) -> Option<T> {
    if let Some((_, action)) = entries.iter().find(|(chord, _)| chord == key) {
        return Some(action.clone());
    }
    if key.shift {
        if let KeyCode::Char(c) = key.code {
            if let Some(base) = unshift_char(c) {
                let base_key = Key {
                    code: KeyCode::Char(base),
                    ..*key
                };
                return entries
                    .iter()
                    .find(|(chord, _)| chord == &base_key)
                    .map(|(_, a)| a.clone());
            }
        }
    }
    None
}

/// Map a Shift-modified character back to its physical key for US QWERTY,
/// so binding specs like `"S+M+\\"` match the winit `logical_key` `'|'`.
fn unshift_char(c: char) -> Option<char> {
    Some(match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

/// Parse a single chord like `"C+S+Space"` or `"C+w"` into a [`Key`].
/// Modifiers are `+`-separated and precede the key name. Accepted modifier
/// tokens (case-sensitive abbreviations take priority):
///   `C` or `ctrl`/`control` = Ctrl,
///   `S` or `shift` = Shift,
///   `M` or `alt`/`meta`/`option` = Alt.
///
/// A literal `+` key is written as a trailing `++` (e.g. `"C+S++"`), which
/// produces two consecutive empty segments when split on `+`.
fn parse_chord(token: &str) -> Option<Key> {
    let parts: Vec<&str> = token.split('+').collect();
    let n = parts.len();

    // Two consecutive trailing empty segments mean the key literal is `+`
    // (e.g. "C+S++" splits into ["C", "S", "", ""]).
    let (mods, code) = if n >= 2 && parts[n - 1].is_empty() && parts[n - 2].is_empty() {
        (&parts[..n - 2], KeyCode::Char('+'))
    } else {
        let (m, name) = parts.split_at(n.checked_sub(1)?);
        (m, parse_key_code(name[0])?)
    };

    let mut key = Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
    };
    for modifier in mods {
        match *modifier {
            "C" => key.ctrl = true,
            "S" => key.shift = true,
            "M" => key.alt = true,
            _ => match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => key.ctrl = true,
                "shift" => key.shift = true,
                "alt" | "meta" | "option" => key.alt = true,
                _ => return None,
            },
        }
    }
    Some(key)
}

/// Parse a key name into a [`KeyCode`]: a single character is a `Char`, and
/// named keys (`Space`, `Enter`, `F1`, ...) map case-insensitively.
fn parse_key_code(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(KeyCode::Char(first));
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "down" => KeyCode::Down,
        "end" => KeyCode::End,
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "home" => KeyCode::Home,
        "insert" => KeyCode::Insert,
        "left" => KeyCode::Left,
        "pagedown" => KeyCode::PageDown,
        "pageup" => KeyCode::PageUp,
        "right" => KeyCode::Right,
        "space" => KeyCode::Space,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        other => return parse_function_key(other),
    })
}

/// Parse an `f1`..`f12` function-key name into [`KeyCode::F`].
fn parse_function_key(name: &str) -> Option<KeyCode> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&number).then_some(KeyCode::F(number))
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

fn resolve_insert(
    key: &Key,
    flags: u32,
    modify_other_keys: Option<i64>,
    window: &WindowKeymap,
    is_alt_screen: bool,
) -> Action {
    // Window direct chords intercept in every mode so pane management works
    // regardless of what the terminal is doing. Scroll-type chords are bypassed
    // when a full-screen TUI app owns the pane.
    if let Some(action) = window.direct_action(key, is_alt_screen) {
        return action;
    }
    if let Some(a) = window.specific_index_action(key) {
        return a;
    }
    if is_entry_chord(key) {
        return Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
    }
    // Configurable line edits and prompt undo/redo (defaults: `Ctrl-Backspace` →
    // delete word back, `Ctrl-/` → undo, `Ctrl-\` → redo). Claiming the undo/redo
    // chords also stops them reaching the PTY as raw control bytes 0x1f / 0x1c.
    if let Some(binding) = window.edit_binding(key) {
        return binding.to_action();
    }
    // Escape always goes to the PTY. Context-aware mode switching (e.g. at the
    // shell prompt) is handled one layer up in the application event loop.
    Action::SendBytes(encode(key, flags, modify_other_keys))
}

/// Accumulate a count digit into `pending` (`5` before `j`). A consumed
/// digit returns `Some(Action::Ignore)` — the count rides in the prefix until
/// the motion that spends it resolves. A leading `0` is not a count (vim's
/// `0` motion); a `0` after other digits extends them. Digits typed while any
/// other sequence is open (a `/` query, a quick-select label, an operator)
/// pass through untouched, as do modified keys (window chords).
fn accumulate_count(key: &Key, pending: &mut PendingPrefix) -> Option<Action> {
    if key.ctrl || key.alt {
        return None;
    }
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    let digit = ch.to_digit(10)? as usize;
    match *pending {
        PendingPrefix::None if digit > 0 => *pending = PendingPrefix::Count(digit),
        PendingPrefix::Count(n) => *pending = PendingPrefix::Count((n * 10 + digit).min(MAX_COUNT)),
        _ => return None,
    }
    Some(Action::Ignore)
}

/// The motions a count repeats (`5j`, `3w`). Whole-viewport jumps (`gg`, `G`,
/// `H`/`M`/`L`), line positioning (`0`/`$`/`^`), and scroll re-centering
/// (`z`-family) have no meaningful repetition, so they drop the count instead.
fn count_repeats(mv: CursorMove) -> bool {
    matches!(
        mv,
        CursorMove::Left
            | CursorMove::Right
            | CursorMove::Up
            | CursorMove::Down
            | CursorMove::WordForward
            | CursorMove::WordBack
            | CursorMove::WordEnd
            | CursorMove::WordForwardBig
            | CursorMove::WordBackBig
            | CursorMove::WordEndBig
            | CursorMove::WordEndBack
            | CursorMove::WordEndBackBig
            | CursorMove::ParagraphBack
            | CursorMove::ParagraphForward
    )
}

/// Build the char-search action for the key that follows `f`/`F`/`t`/`T`. A
/// printable character is the search target; anything else cancels the search.
fn find_char_action(key: &Key, forward: bool, till: bool) -> Action {
    match key.code {
        KeyCode::Char(ch) => Action::FindChar(FindChar { ch, forward, till }),
        _ => Action::Ignore,
    }
}

/// Resolve `key` as a Vim motion, shared by Normal and Visual mode so the two can
/// never drift apart — a motion added here works in both, extending the selection
/// in Visual (see `App::handle_action`'s `MoveCursor` arm).
///
/// `prev` is the prefix in effect for this key (the caller has already taken and
/// cleared it); `pending` lets a motion open its own prefix (`g`, `z`,
/// `f`/`F`/`t`/`T`). `None` means "not a motion": the caller's own mode-specific
/// keys (`i`, `y`, `v`, `za`, `gt`, ...) get their turn.
fn motion_action(key: &Key, prev: PendingPrefix, pending: &mut PendingPrefix) -> Option<Action> {
    use CursorMove as M;

    match prev {
        PendingPrefix::G => {
            return match key.code {
                KeyCode::Char('g') => Some(Action::MoveCursor(M::Top)),
                KeyCode::Char('_') => Some(Action::MoveCursor(M::LastNonBlank)),
                KeyCode::Char('e') => Some(Action::MoveCursor(M::WordEndBack)),
                KeyCode::Char('E') => Some(Action::MoveCursor(M::WordEndBackBig)),
                _ => None,
            };
        }
        PendingPrefix::Z => {
            return match key.code {
                KeyCode::Char('z') => Some(Action::MoveCursor(M::LineToCenter)),
                KeyCode::Char('t') => Some(Action::MoveCursor(M::LineToTop)),
                KeyCode::Char('b') => Some(Action::MoveCursor(M::LineToBottom)),
                _ => None,
            };
        }
        PendingPrefix::FindForward => return Some(find_char_action(key, true, false)),
        PendingPrefix::FindBackward => return Some(find_char_action(key, false, false)),
        PendingPrefix::TillForward => return Some(find_char_action(key, true, true)),
        PendingPrefix::TillBackward => return Some(find_char_action(key, false, true)),
        // The `f`/`t` overlay is showing: a label key jumps, anything else dismisses
        // it (so `Esc` and a mistyped key both get out of the way).
        PendingPrefix::FindLabel => {
            return Some(match key.code {
                KeyCode::Char(c) if c.is_ascii_lowercase() => Action::FindJump(c),
                _ => Action::FindCancel,
            });
        }
        PendingPrefix::GotoMark { exact } => {
            return Some(match key.code {
                KeyCode::Char(c) if c.is_ascii_lowercase() => {
                    Action::GotoMark(GotoMark::new(c, exact))
                }
                _ => Action::Ignore,
            });
        }
        PendingPrefix::None | PendingPrefix::Count(_) => {}
        // Any other prefix belongs to the caller (`]b`, `dw`, quick-select, ...).
        _ => return None,
    }

    let action = match key.code {
        KeyCode::Char('h') | KeyCode::Left => Action::MoveCursor(M::Left),
        KeyCode::Char('j') | KeyCode::Down => Action::MoveCursor(M::Down),
        KeyCode::Char('k') | KeyCode::Up => Action::MoveCursor(M::Up),
        KeyCode::Char('l') | KeyCode::Right => Action::MoveCursor(M::Right),
        // `|` with no count is column one, same as `0`.
        KeyCode::Char('0') | KeyCode::Char('|') | KeyCode::Home => Action::MoveCursor(M::LineStart),
        KeyCode::Char('$') | KeyCode::End => Action::MoveCursor(M::LineEnd),
        KeyCode::Char('^') | KeyCode::Char('_') => Action::MoveCursor(M::FirstNonBlank),
        KeyCode::Char('w') => Action::MoveCursor(M::WordForward),
        KeyCode::Char('b') => Action::MoveCursor(M::WordBack),
        KeyCode::Char('e') => Action::MoveCursor(M::WordEnd),
        KeyCode::Char('W') => Action::MoveCursor(M::WordForwardBig),
        KeyCode::Char('B') => Action::MoveCursor(M::WordBackBig),
        KeyCode::Char('E') => Action::MoveCursor(M::WordEndBig),
        KeyCode::Char('{') => Action::MoveCursor(M::ParagraphBack),
        KeyCode::Char('}') => Action::MoveCursor(M::ParagraphForward),
        KeyCode::Char('%') => Action::MoveCursor(M::MatchingBracket),
        KeyCode::Char('H') => Action::MoveCursor(M::ScreenTop),
        KeyCode::Char('M') => Action::MoveCursor(M::ScreenMiddle),
        KeyCode::Char('L') => Action::MoveCursor(M::ScreenBottom),
        KeyCode::Char('G') => Action::MoveCursor(M::Bottom),
        KeyCode::PageDown => Action::MoveCursor(M::PageDown),
        KeyCode::PageUp => Action::MoveCursor(M::PageUp),
        KeyCode::Char(';') => Action::FindRepeat { reverse: false },
        KeyCode::Char(',') => Action::FindRepeat { reverse: true },
        KeyCode::Char('`') => {
            *pending = PendingPrefix::GotoMark { exact: true };
            Action::Ignore
        }
        KeyCode::Char('\'') => {
            *pending = PendingPrefix::GotoMark { exact: false };
            Action::Ignore
        }
        KeyCode::Char('f') => {
            *pending = PendingPrefix::FindForward;
            Action::Ignore
        }
        KeyCode::Char('F') => {
            *pending = PendingPrefix::FindBackward;
            Action::Ignore
        }
        KeyCode::Char('t') => {
            *pending = PendingPrefix::TillForward;
            Action::Ignore
        }
        KeyCode::Char('T') => {
            *pending = PendingPrefix::TillBackward;
            Action::Ignore
        }
        // `g` and `z` open sequences this function owns the motion half of; the
        // caller's own follow keys (`gt`, `za`, ...) still resolve from the prefix.
        KeyCode::Char('g') => {
            *pending = PendingPrefix::G;
            Action::Ignore
        }
        KeyCode::Char('z') => {
            *pending = PendingPrefix::Z;
            Action::Ignore
        }
        _ => return None,
    };
    // A pending count repeats motions where repetition is meaningful; anything
    // else (`gg`, `0`, a follow-key opener like `f`) drops it.
    if let PendingPrefix::Count(count) = prev {
        if let Action::MoveCursor(mv) = action {
            if count_repeats(mv) {
                return Some(Action::MoveCursorN { count, mv });
            }
        }
    }
    Some(action)
}

fn resolve_normal(key: &Key, pending: &mut PendingPrefix, window: &WindowKeymap) -> Action {
    // A window-command sequence is open: the follow key selects its action
    // (matched by code, so `Ctrl-w v` and `Ctrl-w Ctrl-v` both split).
    if *pending == PendingPrefix::CtrlW {
        *pending = PendingPrefix::None;
        return window
            .sequence_action(key.code)
            .map_or(Action::Ignore, WindowAction::to_action);
    }

    // The configurable window leader opens that sequence.
    if key == &window.leader {
        *pending = PendingPrefix::CtrlW;
        return Action::Ignore;
    }

    // A direct window chord (Ctrl-h/j/k/l focus motions by default). Normal
    // mode is Winter's own navigation mode, so scroll bindings always apply
    // here (pass `false` for is_alt_screen).
    if let Some(action) = window.direct_action(key, false) {
        *pending = PendingPrefix::None;
        return action;
    }

    if let Some(a) = window.specific_index_action(key) {
        *pending = PendingPrefix::None;
        return a;
    }

    // Prompt undo/redo (default `Ctrl-/` / `Ctrl-\`) also work in Normal mode.
    // Line-edit bindings are Insert-only, so they fall through here.
    if let Some(binding) = window.edit_binding(key) {
        if binding.is_history() {
            *pending = PendingPrefix::None;
            return binding.to_action();
        }
    }

    if key.ctrl {
        // Non-window control chords resolve immediately and clear any prefix.
        *pending = PendingPrefix::None;
        return match key.code {
            KeyCode::Char('d') => Action::MoveCursor(CursorMove::HalfPageDown),
            KeyCode::Char('u') => Action::MoveCursor(CursorMove::HalfPageUp),
            KeyCode::Char('v') => Action::EnterVisual(VisualKind::Block),
            // `Ctrl+O`/`Ctrl+I` walk the jumplist (vim's own binding).
            KeyCode::Char('o') => Action::JumpOlder,
            KeyCode::Char('i') => Action::JumpNewer,
            KeyCode::Home => Action::MoveCursor(CursorMove::Top),
            KeyCode::End => Action::MoveCursor(CursorMove::Bottom),
            _ => Action::Ignore,
        };
    }

    if key.alt {
        // Alt chords resolve immediately too, clearing any prefix.
        *pending = PendingPrefix::None;
        return match key.code {
            // `Alt-i`: vim's `vip` in a single chord.
            KeyCode::Char('i') | KeyCode::Char('I') => Action::SelectParagraph,
            _ => Action::Ignore,
        };
    }

    // Count digits accumulate while no other sequence is open (`5j`, `3w`).
    if let Some(action) = accumulate_count(key, pending) {
        return action;
    }

    let prev = *pending;
    *pending = PendingPrefix::None;

    // Motions come from the table Normal shares with Visual, so a motion added
    // once works in both modes. Mode-specific keys and the sequences Normal owns
    // (`gt`, `za`, `dw`, `]b`, the search input, ...) fall through to the match
    // below, which `motion_action` declines by returning `None`.
    if let Some(action) = motion_action(key, prev, pending) {
        return action;
    }

    match prev {
        PendingPrefix::BracketClose => match key.code {
            KeyCode::Char('b') => Action::FocusBlock(BlockNav::Next),
            _ => Action::Ignore,
        },
        PendingPrefix::BracketOpen => match key.code {
            KeyCode::Char('b') => Action::FocusBlock(BlockNav::Previous),
            _ => Action::Ignore,
        },
        // The leader sequence is resolved at the top of this function.
        PendingPrefix::CtrlW => Action::Ignore,
        PendingPrefix::Change => match key.code {
            KeyCode::Char('c') => Action::ChangeLine,
            KeyCode::Char('s') => {
                *pending = PendingPrefix::ChangeSurroundTarget;
                Action::Ignore
            }
            KeyCode::Char('g') => {
                *pending = PendingPrefix::ChangeG;
                Action::Ignore
            }
            KeyCode::Char('w') | KeyCode::Char('e') => Action::ChangeWordForward,
            KeyCode::Char('b') => Action::ChangeWordBack,
            KeyCode::Char('$') => Action::ChangeToLineEnd,
            KeyCode::Char('0') => Action::ChangeToLineStart,
            KeyCode::Char('i') => {
                *pending = PendingPrefix::ChangeObject { around: false };
                Action::Ignore
            }
            KeyCode::Char('a') => {
                *pending = PendingPrefix::ChangeObject { around: true };
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::ChangeG => match key.code {
            KeyCode::Char('n') => Action::ChangeSearchMatch { forward: true },
            KeyCode::Char('N') => Action::ChangeSearchMatch { forward: false },
            _ => Action::Ignore,
        },
        PendingPrefix::ChangeObject { around } => match key.code {
            KeyCode::Char('w') => {
                Action::ChangeTextObject(TextObjectSpec::new(around, TextObject::Word))
            }
            KeyCode::Char('W') => {
                Action::ChangeTextObject(TextObjectSpec::new(around, TextObject::WordBig))
            }
            KeyCode::Char(c @ ('"' | '\'' | '`')) => {
                Action::ChangeTextObject(TextObjectSpec::new(around, TextObject::Quotes(c)))
            }
            KeyCode::Char('(' | ')' | 'b') => Action::ChangeTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('(', ')'),
            )),
            KeyCode::Char('[' | ']') => Action::ChangeTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('[', ']'),
            )),
            KeyCode::Char('{' | '}' | 'B') => Action::ChangeTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('{', '}'),
            )),
            KeyCode::Char('<' | '>') => Action::ChangeTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('<', '>'),
            )),
            _ => Action::Ignore,
        },
        PendingPrefix::Delete => match key.code {
            KeyCode::Char('d') => Action::DeleteLine,
            KeyCode::Char('s') => {
                *pending = PendingPrefix::DeleteSurround;
                Action::Ignore
            }
            KeyCode::Char('g') => {
                *pending = PendingPrefix::DeleteG;
                Action::Ignore
            }
            KeyCode::Char('w') => Action::DeleteWordForward,
            KeyCode::Char('b') => Action::DeleteWordBack,
            KeyCode::Char('$') => Action::DeleteToLineEnd,
            KeyCode::Char('0') => Action::DeleteToLineStart,
            KeyCode::Char('i') => {
                *pending = PendingPrefix::DeleteObject { around: false };
                Action::Ignore
            }
            KeyCode::Char('a') => {
                *pending = PendingPrefix::DeleteObject { around: true };
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::DeleteG => match key.code {
            KeyCode::Char('n') => Action::DeleteSearchMatch { forward: true },
            KeyCode::Char('N') => Action::DeleteSearchMatch { forward: false },
            _ => Action::Ignore,
        },
        PendingPrefix::DeleteObject { around } => match key.code {
            KeyCode::Char('w') => {
                Action::DeleteTextObject(TextObjectSpec::new(around, TextObject::Word))
            }
            KeyCode::Char('W') => {
                Action::DeleteTextObject(TextObjectSpec::new(around, TextObject::WordBig))
            }
            KeyCode::Char(c @ ('"' | '\'' | '`')) => {
                Action::DeleteTextObject(TextObjectSpec::new(around, TextObject::Quotes(c)))
            }
            KeyCode::Char('(' | ')' | 'b') => Action::DeleteTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('(', ')'),
            )),
            KeyCode::Char('[' | ']') => Action::DeleteTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('[', ']'),
            )),
            KeyCode::Char('{' | '}' | 'B') => Action::DeleteTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('{', '}'),
            )),
            KeyCode::Char('<' | '>') => Action::DeleteTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('<', '>'),
            )),
            _ => Action::Ignore,
        },
        // GotoMark and char-search prefixes (including the jump overlay's label key)
        // are resolved by `motion_action` above, for both Normal and Visual.
        PendingPrefix::FindForward
        | PendingPrefix::FindBackward
        | PendingPrefix::TillForward
        | PendingPrefix::TillBackward
        | PendingPrefix::FindLabel
        | PendingPrefix::GotoMark { .. }
        | PendingPrefix::TextObject { .. } => Action::Ignore,
        // `gg`, `g_`, `ge`/`gE` are motions (see `motion_action`); these are the
        // tab commands Normal adds to the same prefix, plus `gv` restoring the
        // last Visual selection.
        PendingPrefix::G => match key.code {
            KeyCode::Char('v') => Action::RestoreVisual,
            KeyCode::Char('<') => Action::MoveTabLeft,
            KeyCode::Char('>') => Action::MoveTabRight,
            KeyCode::Char('t') => Action::NextTab,
            KeyCode::Char('T') => Action::PrevTab,
            KeyCode::Char(';') => Action::ChangeOlder,
            KeyCode::Char(',') => Action::ChangeNewer,
            KeyCode::Char('x') => Action::OpenUnderCursor,
            KeyCode::Char('s') => Action::ToggleSwoop,
            KeyCode::Char('p') => Action::JumpToPrompt,
            KeyCode::Char('P') => Action::JumpToPreviousPrompt,
            KeyCode::Char('n') => Action::SelectSearchMatch { forward: true },
            KeyCode::Char('N') => Action::SelectSearchMatch { forward: false },
            _ => Action::Ignore,
        },
        PendingPrefix::SetMark => match key.code {
            KeyCode::Char(c) if c.is_ascii_lowercase() => Action::SetMark(c),
            _ => Action::Ignore,
        },
        PendingPrefix::Register => match key.code {
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == '+' || c == '*' || c == '"' => {
                *pending = PendingPrefix::WithRegister(c);
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::WithRegister(reg) => match key.code {
            KeyCode::Char('y') => Action::YankSelectionRegister(reg),
            KeyCode::Char('p') => Action::PasteRegister {
                register: reg,
                after: true,
            },
            KeyCode::Char('P') => Action::PasteRegister {
                register: reg,
                after: false,
            },
            KeyCode::Char('d') => {
                *pending = PendingPrefix::Delete;
                Action::Ignore
            }
            KeyCode::Char('c') => {
                *pending = PendingPrefix::Change;
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::ChangeSurroundTarget => match key.code {
            KeyCode::Char(c) => {
                *pending = PendingPrefix::ChangeSurroundReplacement { target: c };
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::ChangeSurroundReplacement { target } => match key.code {
            KeyCode::Char(r) => Action::ChangeSurround {
                target,
                replacement: r,
            },
            _ => Action::Ignore,
        },
        PendingPrefix::DeleteSurround => match key.code {
            KeyCode::Char(c) => Action::DeleteSurround(c),
            _ => Action::Ignore,
        },
        PendingPrefix::YieldSurround { around } => match key.code {
            KeyCode::Char('i') => {
                *pending = PendingPrefix::YieldSurround { around: false };
                Action::Ignore
            }
            KeyCode::Char('a') => {
                *pending = PendingPrefix::YieldSurround { around: true };
                Action::Ignore
            }
            KeyCode::Char('w') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Word),
                };
                Action::Ignore
            }
            KeyCode::Char('W') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::WordBig),
                };
                Action::Ignore
            }
            KeyCode::Char(c @ ('"' | '\'' | '`')) => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Quotes(c)),
                };
                Action::Ignore
            }
            KeyCode::Char('(' | ')' | 'b') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Brackets('(', ')')),
                };
                Action::Ignore
            }
            KeyCode::Char('[' | ']') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Brackets('[', ']')),
                };
                Action::Ignore
            }
            KeyCode::Char('{' | '}' | 'B') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Brackets('{', '}')),
                };
                Action::Ignore
            }
            KeyCode::Char('<' | '>') => {
                *pending = PendingPrefix::YieldSurroundDelimiter {
                    spec: TextObjectSpec::new(around, TextObject::Brackets('<', '>')),
                };
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::YieldSurroundDelimiter { spec } => match key.code {
            KeyCode::Char(d) => Action::SurroundTextObject { spec, delimiter: d },
            _ => Action::Ignore,
        },
        PendingPrefix::ReplaceChar => match key.code {
            KeyCode::Char(c) => Action::ReplaceChar(c),
            _ => Action::Ignore,
        },
        // `zz`/`zt`/`zb` are motions; `za` is Normal's fold toggle.
        PendingPrefix::Z => match key.code {
            KeyCode::Char('a') => Action::ToggleFold,
            _ => Action::Ignore,
        },
        PendingPrefix::QuickSelect => match key.code {
            KeyCode::Escape => Action::QuickCancel,
            KeyCode::Char(c) if c.is_ascii_alphabetic() => Action::QuickJump(c),
            _ => Action::QuickCancel,
        },
        PendingPrefix::SearchInput => match key.code {
            KeyCode::Escape => Action::SearchCancel,
            KeyCode::Enter => Action::SearchExecute,
            KeyCode::Backspace => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchBackspace
            }
            KeyCode::Char(c) => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchChar(c)
            }
            _ => Action::Ignore,
        },
        // A count applies only to motions (already resolved by
        // `motion_action` above); any other key treats it as no prefix, so
        // e.g. `3d` opens the delete operator with the count dropped.
        PendingPrefix::None | PendingPrefix::Count(_) => match key.code {
            KeyCode::Char('i') => Action::EnterInsert(InsertAt::Cursor),
            KeyCode::Char('a') => Action::EnterInsert(InsertAt::After),
            KeyCode::Char('o') => Action::EnterInsert(InsertAt::LineEnd),
            // Tab is `Ctrl+I`'s unmodified twin on most terminals (same byte
            // without the kitty protocol), so it walks the jumplist forward.
            KeyCode::Tab => Action::JumpNewer,
            // `Esc` does not leave Normal (see `ModeEvent::Escape`); it clears the
            // transient search state, vim's `:nohlsearch`.
            KeyCode::Escape => Action::SearchCancel,
            KeyCode::Enter => Action::SwitchMode(Mode::Normal.apply(ModeEvent::FocusBlock)),
            KeyCode::Char('v') => Action::EnterVisual(VisualKind::Char),
            KeyCode::Char('V') => Action::EnterVisual(VisualKind::Line),
            KeyCode::Char('p') => Action::Paste,
            KeyCode::Char('x') => Action::DeleteCharForward,
            KeyCode::Char('.') => Action::RepeatLastChange,
            KeyCode::Char('D') => Action::DeleteToLineEnd,
            KeyCode::Char('d') => {
                *pending = PendingPrefix::Delete;
                Action::Ignore
            }
            KeyCode::Char('C') => Action::ChangeToLineEnd,
            KeyCode::Char('c') => {
                *pending = PendingPrefix::Change;
                Action::Ignore
            }
            KeyCode::Char('s') => Action::SubstituteChar,
            KeyCode::Char('S') => Action::ChangeLine,
            KeyCode::Char('r') => {
                *pending = PendingPrefix::ReplaceChar;
                Action::Ignore
            }
            KeyCode::Char('~') => Action::ToggleCaseChar,
            KeyCode::Char('m') => {
                *pending = PendingPrefix::SetMark;
                Action::Ignore
            }
            KeyCode::Char('"') => {
                *pending = PendingPrefix::Register;
                Action::Ignore
            }
            KeyCode::Char('/') => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchStart
            }
            KeyCode::Char('?') => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchStartBackward
            }
            KeyCode::Char('*') => Action::SearchWord { forward: true },
            KeyCode::Char('#') => Action::SearchWord { forward: false },
            KeyCode::Char('n') => Action::SearchNext,
            KeyCode::Char('N') => Action::SearchPrevious,
            KeyCode::Char('y') => Action::YankBlock,
            KeyCode::Char(']') => {
                *pending = PendingPrefix::BracketClose;
                Action::Ignore
            }
            KeyCode::Char('[') => {
                *pending = PendingPrefix::BracketOpen;
                Action::Ignore
            }
            KeyCode::Char('q') => {
                *pending = PendingPrefix::QuickSelect;
                Action::QuickSelect
            }
            _ => Action::Ignore,
        },
    }
}

/// Resolve a key in Visual mode: the same motions as Normal extend the
/// selection, `y` yanks it, and `v`/`V`/`Esc` leave Visual.
fn resolve_visual(key: &Key, pending: &mut PendingPrefix, window: &WindowKeymap) -> Action {
    // Window direct/index chords take priority over Visual's own bindings, so
    // pane management (split/focus/close/zoom) works regardless of mode.
    // Visual is Winter's own selection mode, never owned by a full-screen TUI
    // app, so scroll-type chords always apply here too (matches resolve_normal).
    if let Some(action) = window.direct_action(key, false) {
        *pending = PendingPrefix::None;
        return action;
    }
    if let Some(a) = window.specific_index_action(key) {
        *pending = PendingPrefix::None;
        return a;
    }

    if key.ctrl {
        *pending = PendingPrefix::None;
        return match key.code {
            KeyCode::Char('d') => Action::MoveCursor(CursorMove::HalfPageDown),
            KeyCode::Char('u') => Action::MoveCursor(CursorMove::HalfPageUp),
            KeyCode::Char('v') => Action::EnterVisual(VisualKind::Block),
            // Jumplist walking works from Visual too (extends the selection,
            // like any other cursor move).
            KeyCode::Char('o') => Action::JumpOlder,
            KeyCode::Char('i') => Action::JumpNewer,
            _ => Action::Ignore,
        };
    }

    // Count digits accumulate here as in Normal (`v3w` extends three words).
    if let Some(action) = accumulate_count(key, pending) {
        return action;
    }

    let prev = *pending;
    *pending = PendingPrefix::None;

    // Every motion Normal has, from the shared table — each one extends the
    // selection instead of just moving the cursor (see `App::handle_action`).
    if let Some(action) = motion_action(key, prev, pending) {
        return action;
    }

    match prev {
        PendingPrefix::TextObject { around } => match key.code {
            KeyCode::Char('w') => {
                Action::SelectTextObject(TextObjectSpec::new(around, TextObject::Word))
            }
            KeyCode::Char('W') => {
                Action::SelectTextObject(TextObjectSpec::new(around, TextObject::WordBig))
            }
            KeyCode::Char(c @ ('"' | '\'' | '`')) => {
                Action::SelectTextObject(TextObjectSpec::new(around, TextObject::Quotes(c)))
            }
            KeyCode::Char('(' | ')' | 'b') => Action::SelectTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('(', ')'),
            )),
            KeyCode::Char('[' | ']') => Action::SelectTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('[', ']'),
            )),
            KeyCode::Char('{' | '}' | 'B') => Action::SelectTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('{', '}'),
            )),
            KeyCode::Char('<' | '>') => Action::SelectTextObject(TextObjectSpec::new(
                around,
                TextObject::Brackets('<', '>'),
            )),
            _ => {
                if !around {
                    Action::EnterInsert(InsertAt::Cursor)
                } else {
                    Action::Ignore
                }
            }
        },
        PendingPrefix::Register => match key.code {
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == '+' || c == '*' || c == '"' => {
                *pending = PendingPrefix::WithRegister(c);
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::WithRegister(reg) => match key.code {
            KeyCode::Char('y') => Action::YankSelectionRegister(reg),
            KeyCode::Char('d') | KeyCode::Char('x') => Action::DeleteSelection,
            KeyCode::Char('p') => Action::PasteRegister {
                register: reg,
                after: true,
            },
            KeyCode::Char('P') => Action::PasteRegister {
                register: reg,
                after: false,
            },
            _ => Action::Ignore,
        },
        PendingPrefix::G => match key.code {
            KeyCode::Char('n') => Action::SelectSearchMatch { forward: true },
            KeyCode::Char('N') => Action::SelectSearchMatch { forward: false },
            KeyCode::Char('p') => Action::JumpToPrompt,
            KeyCode::Char('P') => Action::JumpToPreviousPrompt,
            KeyCode::Char('s') => Action::ToggleSwoop,
            KeyCode::Char('x') => Action::OpenUnderCursor,
            _ => Action::Ignore,
        },
        _ => match key.code {
            KeyCode::Char('y') => Action::YankSelection,
            KeyCode::Char('d') | KeyCode::Char('x') => Action::DeleteSelection,
            KeyCode::Char('v') => Action::EnterVisual(VisualKind::Char),
            KeyCode::Char('V') => Action::EnterVisual(VisualKind::Line),
            KeyCode::Char('"') => {
                *pending = PendingPrefix::Register;
                Action::Ignore
            }
            KeyCode::Char('i') => {
                *pending = PendingPrefix::TextObject { around: false };
                Action::Ignore
            }
            KeyCode::Char('a') => {
                *pending = PendingPrefix::TextObject { around: true };
                Action::Ignore
            }
            // `o`: extend from the selection's other end instead (the span itself
            // is unchanged — it always runs anchor..cursor in either order).
            KeyCode::Char('o') => Action::SwapVisualEnds,
            KeyCode::Escape => Action::SwitchMode(Mode::Visual.apply(ModeEvent::Escape)),
            _ => Action::Ignore,
        },
    }
}

fn resolve_block_focus(
    key: &Key,
    flags: u32,
    window: &WindowKeymap,
    is_alt_screen: bool,
) -> Action {
    if key.code == KeyCode::Escape {
        return Action::SwitchMode(Mode::BlockFocus.apply(ModeEvent::Escape));
    }
    if let Some(action) = window.direct_action(key, is_alt_screen) {
        return action;
    }
    Action::ForwardToBlock(encode(key, flags, None))
}

fn is_entry_chord(key: &Key) -> bool {
    key.ctrl && key.shift && key.code == KeyCode::Space
}

/// Encode a key as the bytes a terminal program expects on the PTY.
/// `flags` is the active Kitty keyboard protocol bitmask for the pane
/// (0 = legacy xterm encoding). `modify_other_keys` is the xterm
/// modifyOtherKeys mode (`None` = off, `Some(1)` or `Some(2)`).
fn encode(key: &Key, flags: u32, modify_other_keys: Option<i64>) -> Vec<u8> {
    if flags != 0 {
        encode_kitty(key)
    } else {
        encode_xterm(key, modify_other_keys)
    }
}

/// Encode a key-release event for the Kitty protocol.
/// Returns bytes only when `flags & 2 != 0` (bit 1: report event types).
/// Release sequences use `: 3` as the event-type sub-field:
///   `CSI codepoint :: 3 u`  (no modifier)
///   `CSI codepoint ; modifier : 3 u`  (with modifier)
pub fn encode_release(key: &Key, flags: u32) -> Vec<u8> {
    if flags & 2 == 0 {
        return Vec::new();
    }
    encode_kitty_release(key)
}

// ---- xterm baseline encoding ------------------------------------------------

/// xterm modifier byte: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
fn xterm_modifier(key: &Key) -> u8 {
    1 + (key.shift as u8) + 2 * (key.alt as u8) + 4 * (key.ctrl as u8)
}

/// Prepend an ESC byte (Alt prefix convention).
fn esc_prefix(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.insert(0, ESCAPE);
    bytes
}

/// Navigation key: bare CSI when no modifier, `\e[1;NX` with one.
fn nav_csi_xterm(final_byte: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi(final_byte)
    } else {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    }
}

/// Tilde-form key: `\e[k~` bare, `\e[k;N~` with modifier.
fn tilde_xterm(param: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi_param(param, b'~')
    } else {
        format!("\x1b[{param};{m}~").into_bytes()
    }
}

/// xterm encoding for function keys F1-F12 with optional modifier.
fn encode_f_xterm(n: u8, m: u8) -> Vec<u8> {
    match n {
        1 => {
            if m > 1 {
                format!("\x1b[1;{m}P").into_bytes()
            } else {
                ss3(b'P')
            }
        }
        2 => {
            if m > 1 {
                format!("\x1b[1;{m}Q").into_bytes()
            } else {
                ss3(b'Q')
            }
        }
        3 => {
            if m > 1 {
                format!("\x1b[1;{m}R").into_bytes()
            } else {
                ss3(b'R')
            }
        }
        4 => {
            if m > 1 {
                format!("\x1b[1;{m}S").into_bytes()
            } else {
                ss3(b'S')
            }
        }
        5 => tilde_xterm(15, m),
        6 => tilde_xterm(17, m),
        7 => tilde_xterm(18, m),
        8 => tilde_xterm(19, m),
        9 => tilde_xterm(20, m),
        10 => tilde_xterm(21, m),
        11 => tilde_xterm(23, m),
        12 => tilde_xterm(24, m),
        _ => Vec::new(),
    }
}

fn encode_xterm(key: &Key, modify_other_keys: Option<i64>) -> Vec<u8> {
    let m = xterm_modifier(key);
    let bytes = match key.code {
        KeyCode::Backspace => vec![DELETE],
        KeyCode::Char('\0') => return Vec::new(),
        KeyCode::Char(c) => {
            if key.ctrl && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_uppercase() as u8) & CONTROL_MASK]
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Delete => tilde_xterm(3, m),
        KeyCode::Down => nav_csi_xterm(b'B', m),
        KeyCode::End => {
            if m > 1 {
                format!("\x1b[1;{m}F").into_bytes()
            } else {
                csi(b'F')
            }
        }
        KeyCode::Enter => vec![CARRIAGE_RETURN],
        KeyCode::Escape => vec![ESCAPE],
        KeyCode::F(n) => encode_f_xterm(n, m),
        KeyCode::Home => {
            if m > 1 {
                format!("\x1b[1;{m}H").into_bytes()
            } else {
                csi(b'H')
            }
        }
        KeyCode::Insert => tilde_xterm(2, m),
        KeyCode::Left => nav_csi_xterm(b'D', m),
        KeyCode::PageDown => tilde_xterm(6, m),
        KeyCode::PageUp => tilde_xterm(5, m),
        KeyCode::Right => nav_csi_xterm(b'C', m),
        KeyCode::Space => vec![b' '],
        KeyCode::Tab => {
            if key.shift {
                vec![ESCAPE, b'[', b'Z'] // backtab / reverse-tab
            } else {
                vec![b'\t']
            }
        }
        KeyCode::Up => nav_csi_xterm(b'A', m),
    };

    // xterm modifyOtherKeys: when an app has enabled it (CSI > 4;N m) and the
    // key has at least one modifier, encode character keys as
    // `\x1b[27;<modifier>;<codepoint>~` instead of the legacy `\x1b<char>`.
    // This is unambiguous — there is no ESC prefix that could be parsed as a
    // standalone Escape — so Shift+Alt+E/H/L arrive intact. Mirrors WezTerm.
    //
    // Mode 1 excludes a few well-known keys from the extended encoding (the
    // same set xterm excludes); mode 2 applies it to all modified chars.
    if let Some(mode) = modify_other_keys {
        if let (KeyCode::Char(c), true) = (key.code, (key.shift || key.ctrl || key.alt)) {
            let cp = if key.ctrl && c.is_ascii_alphabetic() {
                (c.to_ascii_uppercase() as u32) & 0x1f
            } else {
                c as u32
            };
            let mode1_excluded = mode == 1 && matches!(c, 'c' | 'd' | '\x1b' | '\x7f' | '\x08');
            if !mode1_excluded {
                return format!("\x1b[27;{m};{cp}~").into_bytes();
            }
        }
    }

    // Alt prefix: prepend ESC (never on bare Escape to avoid double-ESC).
    if key.alt && !matches!(key.code, KeyCode::Escape | KeyCode::Char('\0')) {
        esc_prefix(bytes)
    } else {
        bytes
    }
}

// ---- Kitty keyboard protocol encoding ---------------------------------------

/// Kitty modifier value: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
fn kitty_modifier(key: &Key) -> u32 {
    1 + (key.shift as u32) + 2 * (key.alt as u32) + 4 * (key.ctrl as u32)
}

/// `CSI codepoint u` or `CSI codepoint ; modifier u` (omit `;1`).
fn kitty_csi(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}u").into_bytes()
    }
}

/// Release variant: `CSI codepoint :: 3 u` (no modifier) or
/// `CSI codepoint ; modifier : 3 u` (with modifier).
fn kitty_csi_release(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}::3u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}:3u").into_bytes()
    }
}

/// Kitty release encoding: same key mapping as `encode_kitty` but with the
/// `: 3` event-type suffix. Keys that map to raw bytes on press (bare chars,
/// bare Tab, bare Enter, etc.) get full CSI sequences on release so the app
/// can distinguish press from release.
fn encode_kitty_release(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => kitty_csi_release(base_codepoint(c, key.shift), m),
        KeyCode::Space => kitty_csi_release(32, m),
        KeyCode::Tab => kitty_csi_release(9, m),
        KeyCode::Enter => kitty_csi_release(13, m),
        KeyCode::Escape => kitty_csi_release(27, m),
        KeyCode::Backspace => kitty_csi_release(127, m),
        KeyCode::Insert => kitty_csi_release(KP_INSERT, m),
        KeyCode::Delete => kitty_csi_release(KP_DELETE, m),
        KeyCode::Left => kitty_csi_release(KP_LEFT, m),
        KeyCode::Right => kitty_csi_release(KP_RIGHT, m),
        KeyCode::Up => kitty_csi_release(KP_UP, m),
        KeyCode::Down => kitty_csi_release(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi_release(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi_release(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi_release(KP_HOME, m),
        KeyCode::End => kitty_csi_release(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi_release(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}

/// The base Unicode codepoint of a character key (lowercase for ASCII alpha
/// so Ctrl+Shift+a uses codepoint 97, not 65).
fn base_codepoint(c: char, shift: bool) -> u32 {
    if shift && c.is_ascii_uppercase() {
        c.to_ascii_lowercase() as u32
    } else {
        c as u32
    }
}

fn encode_kitty(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => {
            // No modifier or lone Shift: send raw UTF-8. Shift is already
            // encoded in the char winit provides ('A' for Shift+a).
            if m == 1 || m == 2 {
                return c.to_string().into_bytes();
            }
            kitty_csi(base_codepoint(c, key.shift), m)
        }
        KeyCode::Space => {
            if m == 1 {
                vec![b' ']
            } else {
                kitty_csi(32, m)
            }
        }
        KeyCode::Tab => {
            if m == 1 {
                vec![b'\t']
            } else if key.shift && !key.ctrl && !key.alt {
                vec![ESCAPE, b'[', b'Z'] // backtab
            } else {
                kitty_csi(9, m)
            }
        }
        KeyCode::Enter => {
            if m == 1 {
                vec![CARRIAGE_RETURN]
            } else {
                kitty_csi(13, m)
            }
        }
        KeyCode::Escape => {
            if m == 1 {
                vec![ESCAPE]
            } else {
                kitty_csi(27, m)
            }
        }
        KeyCode::Backspace => {
            if m == 1 {
                vec![DELETE]
            } else {
                kitty_csi(127, m)
            }
        }
        KeyCode::Insert => kitty_csi(KP_INSERT, m),
        KeyCode::Delete => kitty_csi(KP_DELETE, m),
        KeyCode::Left => kitty_csi(KP_LEFT, m),
        KeyCode::Right => kitty_csi(KP_RIGHT, m),
        KeyCode::Up => kitty_csi(KP_UP, m),
        KeyCode::Down => kitty_csi(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi(KP_HOME, m),
        KeyCode::End => kitty_csi(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}

// ---- Low-level sequence builders --------------------------------------------

fn csi(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', final_byte]
}

/// `param` is written as its decimal ASCII digits (`3` -> `b"3"`), not the raw
/// byte value: `3u8` alone is the ETX control character, and a raw control
/// byte embedded mid-sequence gets caught by the pty's line discipline as a
/// signal (e.g. `3` is `Ctrl-C`/`VINTR`) instead of reaching the app as part
/// of the escape sequence.
fn csi_param(param: u8, final_byte: u8) -> Vec<u8> {
    let mut bytes = vec![ESCAPE, b'['];
    bytes.extend(param.to_string().into_bytes());
    bytes.push(final_byte);
    bytes
}

fn ss3(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'O', final_byte]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_commands_dispatch_through_the_keymap_path() {
        // Regression: eleven palette entries (copy/paste, font size,
        // scrolling) had no dispatch arm of their own, so selecting them
        // from the palette silently did nothing even though their
        // keybindings worked.
        for name in [
            "copy_selection",
            "paste_from_clipboard",
            "font_decrease",
            "font_increase",
            "font_reset",
            "scroll_line_up",
            "scroll_line_down",
            "scroll_page_up",
            "scroll_page_down",
            "scroll_to_top",
            "scroll_to_bottom",
        ] {
            assert!(
                window_action_by_name(name).is_some(),
                "{name} must dispatch through the keymap path"
            );
        }
        assert!(window_action_by_name("not_a_command").is_none());
    }

    fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn resolve_simple(mode: Mode, key: &Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(mode, key, &mut pending, 0)
    }

    #[test]
    fn test_insert_sends_printable_bytes() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Char('a'))),
            Action::SendBytes(vec![b'a'])
        );
    }

    #[test]
    fn test_insert_encodes_control_chars() {
        let ctrl_c = Key {
            ctrl: true,
            ..key(KeyCode::Char('c'))
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_c),
            Action::SendBytes(vec![0x03])
        );
    }

    #[test]
    fn test_insert_encodes_arrow_keys() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Up)),
            Action::SendBytes(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn test_delete_key_sends_ascii_tilde_sequence() {
        // Regression: `csi_param` used to splice the raw byte value 3 into the
        // sequence instead of the ASCII digit '3', producing
        // `\x1b[\x03~` — embedding a literal Ctrl-C (ETX) that a pty's line
        // discipline intercepts as SIGINT, so a bare Delete keypress could
        // kill the foreground program instead of forward-deleting.
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Delete)),
            Action::SendBytes(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn test_tilde_form_keys_use_ascii_digits_for_their_param() {
        // Every `tilde_xterm` key must carry its CSI parameter as ASCII
        // digits, including the multi-digit function-key params (e.g. F5 is
        // `15`, two bytes `b"15"`, not the single raw byte 15).
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Insert)),
            Action::SendBytes(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::PageUp)),
            Action::SendBytes(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::PageDown)),
            Action::SendBytes(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::F(5))),
            Action::SendBytes(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::F(12))),
            Action::SendBytes(b"\x1b[24~".to_vec())
        );
    }

    #[test]
    fn test_esc_is_always_sent_to_pty_in_insert_mode() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Insert, &key(KeyCode::Escape), &mut pending, 0),
            Action::SendBytes(vec![0x1b])
        );
    }

    #[test]
    fn test_entry_chord_switches_to_normal() {
        let chord = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Space)
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &chord),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_new_motions_resolve_in_both_normal_and_visual() {
        // The motion table is shared, so every one of these works in Visual too —
        // that's the point of `motion_action`.
        let cases = [
            (KeyCode::Char('{'), CursorMove::ParagraphBack),
            (KeyCode::Char('}'), CursorMove::ParagraphForward),
            (KeyCode::Char('%'), CursorMove::MatchingBracket),
            (KeyCode::Char('H'), CursorMove::ScreenTop),
            (KeyCode::Char('M'), CursorMove::ScreenMiddle),
            (KeyCode::Char('L'), CursorMove::ScreenBottom),
            (KeyCode::Char('_'), CursorMove::FirstNonBlank),
            (KeyCode::Char('|'), CursorMove::LineStart),
            (KeyCode::Home, CursorMove::LineStart),
            (KeyCode::End, CursorMove::LineEnd),
        ];
        for (code, mv) in cases {
            for mode in [Mode::Normal, Mode::Visual] {
                assert_eq!(
                    resolve_simple(mode, &key(code)),
                    Action::MoveCursor(mv),
                    "{code:?} in {mode:?}"
                );
            }
        }
    }

    #[test]
    fn test_count_accumulates_and_repeats_a_motion() {
        // `5j` must spend the count on the motion, in Normal and Visual alike
        // (the motion table is shared). A digit that fails to accumulate would
        // resolve `j` as a plain MoveCursor — or worse, as a count-less drop.
        for mode in [Mode::Normal, Mode::Visual] {
            let mut pending = PendingPrefix::None;
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('3')), &mut pending, 0),
                Action::Ignore
            );
            assert_eq!(pending, PendingPrefix::Count(3));
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('j')), &mut pending, 0),
                Action::MoveCursorN {
                    count: 3,
                    mv: CursorMove::Down
                }
            );
            assert_eq!(pending, PendingPrefix::None, "the spent count clears");
        }
    }

    #[test]
    fn test_leading_zero_is_the_zero_motion_not_a_count() {
        // Vim's `0` goes to column zero when no count is open; making it a
        // count would strand the line-start motion behind an unreachable
        // prefix (`00j` is not a thing anyone types on purpose).
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('0')), &mut pending, 0),
            Action::MoveCursor(CursorMove::LineStart)
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_zero_extends_an_accumulated_count() {
        // `10w` is ten words, not "1 then the zero motion then w".
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('1')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('0')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::Count(10));
    }

    #[test]
    fn test_count_is_dropped_for_non_motion_keys() {
        // `3i` enters Insert once — the count has no meaning for mode
        // switches and must not swallow the key (the bug this catches:
        // digits resolving EnterInsert to Ignore and locking the keyboard).
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('3')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('i')), &mut pending, 0),
            Action::EnterInsert(InsertAt::Cursor)
        );
    }

    #[test]
    fn test_visual_o_swaps_selection_ends() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('o'))),
            Action::SwapVisualEnds
        );
    }

    #[test]
    fn test_g_semicolon_and_comma_walk_the_changelist() {
        // `g;`/`g,` share the `g` prefix with the motions and tab commands;
        // the semicolon walks back through recorded change positions, the
        // comma forward.
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char(';')), &mut pending, 0),
            Action::ChangeOlder
        );
        // Each pair spends the prefix, so `g,` opens its own.
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char(',')), &mut pending, 0),
            Action::ChangeNewer
        );
    }

    #[test]
    fn test_dot_resolves_to_repeat_last_change() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('.'))),
            Action::RepeatLastChange
        );
    }

    #[test]
    fn test_gx_resolves_to_open_under_cursor() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0),
            Action::OpenUnderCursor
        );
    }

    #[test]
    fn test_gv_resolves_to_restore_visual() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('v')), &mut pending, 0),
            Action::RestoreVisual
        );
    }

    #[test]
    fn test_ctrl_o_and_ctrl_i_and_tab_walk_the_jumplist() {
        // `Ctrl+O`/`Ctrl+I` are vim's own jumplist bindings; Tab doubles for
        // `Ctrl+I` because most terminals deliver the two identically.
        let ctrl = |code: KeyCode| Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl(KeyCode::Char('o'))),
            Action::JumpOlder
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl(KeyCode::Char('i'))),
            Action::JumpNewer
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Tab)),
            Action::JumpNewer
        );
        assert_eq!(
            resolve_simple(Mode::Visual, &ctrl(KeyCode::Char('o'))),
            Action::JumpOlder
        );
    }

    #[test]
    fn test_set_mark_and_goto_mark_key_resolution() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('m')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::SetMark);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, 0),
            Action::SetMark('a')
        );
        assert_eq!(pending, PendingPrefix::None);

        // Exact mark jump (`)
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('`')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::GotoMark { exact: true });
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('z')), &mut pending, 0),
            Action::GotoMark(GotoMark::new('z', true))
        );
        assert_eq!(pending, PendingPrefix::None);

        // First non-blank mark jump (')
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('\'')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::GotoMark { exact: false });
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('k')), &mut pending, 0),
            Action::GotoMark(GotoMark::new('k', false))
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_invalid_mark_character_cancels_cleanly() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('m')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('1')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(
            pending,
            PendingPrefix::None,
            "invalid mark digit cancels prefix"
        );

        resolve(Mode::Normal, &key(KeyCode::Char('`')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::None, "Esc cancels goto mark prefix");
    }

    #[test]
    fn test_goto_mark_resolves_in_both_normal_and_visual() {
        for mode in [Mode::Normal, Mode::Visual] {
            let mut pending = PendingPrefix::None;
            resolve(mode, &key(KeyCode::Char('`')), &mut pending, 0);
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('b')), &mut pending, 0),
                Action::GotoMark(GotoMark::new('b', true))
            );
            assert_eq!(pending, PendingPrefix::None);

            resolve(mode, &key(KeyCode::Char('\'')), &mut pending, 0);
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('b')), &mut pending, 0),
                Action::GotoMark(GotoMark::new('b', false))
            );
            assert_eq!(pending, PendingPrefix::None);
        }
    }

    #[test]
    fn test_which_key_hint_descriptor_completeness() {
        // Prefixes that expect continuations must return non-empty lists
        let hints = [
            PendingPrefix::G,
            PendingPrefix::Z,
            PendingPrefix::CtrlW,
            PendingPrefix::Delete,
            PendingPrefix::DeleteObject { around: false },
            PendingPrefix::DeleteObject { around: true },
            PendingPrefix::BracketClose,
            PendingPrefix::BracketOpen,
            PendingPrefix::FindForward,
            PendingPrefix::FindBackward,
            PendingPrefix::TillForward,
            PendingPrefix::TillBackward,
            PendingPrefix::SetMark,
            PendingPrefix::GotoMark { exact: true },
            PendingPrefix::GotoMark { exact: false },
            PendingPrefix::TextObject { around: false },
            PendingPrefix::TextObject { around: true },
        ];
        for prefix in hints {
            let hint = prefix.hint();
            assert!(hint.is_some(), "{prefix:?} must yield which-key hint");
            let (title, items) = hint.unwrap();
            assert!(!title.is_empty(), "title must not be empty");
            assert!(!items.is_empty(), "items list must not be empty");
        }

        // Prefixes that opt out must return None
        let opt_outs = [
            PendingPrefix::None,
            PendingPrefix::Count(3),
            PendingPrefix::FindLabel,
            PendingPrefix::QuickSelect,
            PendingPrefix::SearchInput,
        ];
        for prefix in opt_outs {
            assert!(
                prefix.hint().is_none(),
                "{prefix:?} must opt out of which-key hint"
            );
        }
    }

    #[test]
    fn test_text_object_and_block_visual_key_resolution() {
        let ctrl = |code: KeyCode| Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
        };

        // Ctrl+V in Normal enters VisualKind::Block
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Block)
        );

        // Ctrl+V in Visual toggles / enters VisualKind::Block
        assert_eq!(
            resolve_simple(Mode::Visual, &ctrl(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Block)
        );

        // Visual `iw` and `aw`
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Visual, &key(KeyCode::Char('i')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::TextObject { around: false });
        assert_eq!(
            resolve(Mode::Visual, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::SelectTextObject(TextObjectSpec::new(false, TextObject::Word))
        );
        assert_eq!(pending, PendingPrefix::None);

        resolve(Mode::Visual, &key(KeyCode::Char('a')), &mut pending, 0);
        assert_eq!(pending, PendingPrefix::TextObject { around: true });
        assert_eq!(
            resolve(Mode::Visual, &key(KeyCode::Char('"')), &mut pending, 0),
            Action::SelectTextObject(TextObjectSpec::new(true, TextObject::Quotes('"')))
        );

        // Delete operator `diw` and `da(`
        resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(pending, PendingPrefix::Delete);
        resolve(Mode::Normal, &key(KeyCode::Char('i')), &mut pending, 0);
        assert_eq!(pending, PendingPrefix::DeleteObject { around: false });
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::DeleteTextObject(TextObjectSpec::new(false, TextObject::Word))
        );

        resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('(')), &mut pending, 0),
            Action::DeleteTextObject(TextObjectSpec::new(true, TextObject::Brackets('(', ')')))
        );
    }

    #[test]
    fn test_g_and_z_motion_sequences_resolve_in_both_modes() {
        for mode in [Mode::Normal, Mode::Visual] {
            for (lead, follow, mv) in [
                ('g', 'g', CursorMove::Top),
                ('g', '_', CursorMove::LastNonBlank),
                ('g', 'e', CursorMove::WordEndBack),
                ('g', 'E', CursorMove::WordEndBackBig),
                ('z', 'z', CursorMove::LineToCenter),
                ('z', 't', CursorMove::LineToTop),
                ('z', 'b', CursorMove::LineToBottom),
            ] {
                let mut pending = PendingPrefix::None;
                assert_eq!(
                    resolve(mode, &key(KeyCode::Char(lead)), &mut pending, 0),
                    Action::Ignore
                );
                assert_eq!(
                    resolve(mode, &key(KeyCode::Char(follow)), &mut pending, 0),
                    Action::MoveCursor(mv),
                    "{lead}{follow} in {mode:?}"
                );
                assert_eq!(pending, PendingPrefix::None);
            }
        }
    }

    #[test]
    fn test_normal_keeps_its_own_g_and_z_sequences_alongside_the_motions() {
        // Sharing the prefix with the motion table must not swallow `gt`/`gT`/
        // `g<`/`g>` or `za`.
        for (follow, expected) in [
            ('t', Action::NextTab),
            ('T', Action::PrevTab),
            ('<', Action::MoveTabLeft),
            ('>', Action::MoveTabRight),
        ] {
            let mut pending = PendingPrefix::None;
            resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
            assert_eq!(
                resolve(Mode::Normal, &key(KeyCode::Char(follow)), &mut pending, 0),
                expected
            );
        }
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('z')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, 0),
            Action::ToggleFold
        );
    }

    #[test]
    fn test_motions_do_not_hijack_the_search_input_or_delete_sequences() {
        // Characters typed into `/` must reach the query, and `dw`/`d0` stay
        // deletes even though `w`/`0` are motion keys.
        let mut pending = PendingPrefix::SearchInput;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::SearchChar('w')
        );
        let mut pending = PendingPrefix::Delete;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::DeleteWordForward
        );
        let mut pending = PendingPrefix::Delete;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('0')), &mut pending, 0),
            Action::DeleteToLineStart
        );
    }

    #[test]
    fn test_find_label_prefix_resolves_in_both_modes() {
        // With the `f`/`t` overlay up, a lowercase key picks a label; anything else
        // (Esc, an uppercase letter, a digit) dismisses it.
        for mode in [Mode::Normal, Mode::Visual] {
            let mut pending = PendingPrefix::FindLabel;
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('s')), &mut pending, 0),
                Action::FindJump('s')
            );
            assert_eq!(pending, PendingPrefix::None);

            let mut pending = PendingPrefix::FindLabel;
            assert_eq!(
                resolve(mode, &key(KeyCode::Escape), &mut pending, 0),
                Action::FindCancel
            );

            let mut pending = PendingPrefix::FindLabel;
            assert_eq!(
                resolve(mode, &key(KeyCode::Char('S')), &mut pending, 0),
                Action::FindCancel
            );
        }
    }

    #[test]
    fn test_alt_i_selects_the_paragraph_in_normal_mode() {
        let alt_i = Key {
            alt: true,
            ..key(KeyCode::Char('i'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &alt_i),
            Action::SelectParagraph
        );
        // Shift transforms the glyph on some layouts; both reach the same action.
        let alt_shift_i = Key {
            alt: true,
            shift: true,
            ..key(KeyCode::Char('I'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &alt_shift_i),
            Action::SelectParagraph
        );
        // Alt-i in Insert mode still belongs to the shell.
        assert_ne!(
            resolve_simple(Mode::Insert, &alt_i),
            Action::SelectParagraph
        );
    }

    #[test]
    fn test_normal_navigation_and_mode_exits() {
        // `i`/`a`/`o` are the only ways back to Insert, each with its own landing
        // spot; `Esc` clears the search instead of leaving Normal.
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('i'))),
            Action::EnterInsert(InsertAt::Cursor)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('a'))),
            Action::EnterInsert(InsertAt::After)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('o'))),
            Action::EnterInsert(InsertAt::LineEnd)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Escape)),
            Action::SearchCancel
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Enter)),
            Action::SwitchMode(Mode::BlockFocus)
        );
    }

    #[test]
    fn test_normal_hjkl_moves_the_cursor() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('h'))),
            Action::MoveCursor(CursorMove::Left)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('j'))),
            Action::MoveCursor(CursorMove::Down)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('k'))),
            Action::MoveCursor(CursorMove::Up)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('l'))),
            Action::MoveCursor(CursorMove::Right)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('G'))),
            Action::MoveCursor(CursorMove::Bottom)
        );
    }

    #[test]
    fn test_home_end_move_within_the_line() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Home)),
            Action::MoveCursor(CursorMove::LineStart)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::End)),
            Action::MoveCursor(CursorMove::LineEnd)
        );
    }

    #[test]
    fn test_ctrl_home_end_jump_to_buffer_top_and_bottom() {
        let ctrl_home = Key {
            ctrl: true,
            ..key(KeyCode::Home)
        };
        let ctrl_end = Key {
            ctrl: true,
            ..key(KeyCode::End)
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_home),
            Action::MoveCursor(CursorMove::Top)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_end),
            Action::MoveCursor(CursorMove::Bottom)
        );
    }

    #[test]
    fn test_gg_jumps_to_top_of_buffer() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::G);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::MoveCursor(CursorMove::Top));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_char_search_sets_prefix_then_resolves_target() {
        let mut pending = PendingPrefix::None;
        // `t` opens a forward-till search rather than acting immediately.
        let action = resolve(Mode::Normal, &key(KeyCode::Char('t')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::TillForward);
        // The next key is the search target.
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
        assert_eq!(
            action,
            Action::FindChar(FindChar {
                ch: 'x',
                forward: true,
                till: true,
            })
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_find_repeat_keys() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char(';'))),
            Action::FindRepeat { reverse: false }
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char(','))),
            Action::FindRepeat { reverse: true }
        );
    }

    #[test]
    fn test_gt_switches_tabs() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('t')), &mut pending, 0),
            Action::NextTab
        );
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('T')), &mut pending, 0),
            Action::PrevTab
        );
    }

    #[test]
    fn test_ctrl_d_scrolls_half_page() {
        let ctrl_d = Key {
            ctrl: true,
            ..key(KeyCode::Char('d'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_d),
            Action::MoveCursor(CursorMove::HalfPageDown)
        );
    }

    #[test]
    fn test_alt_hjkl_moves_pane_focus() {
        let alt = |c: char| Key {
            alt: true,
            shift: false,
            ..key(KeyCode::Char(c))
        };
        let cases = [
            (alt('h'), FocusDir::Left),
            (alt('j'), FocusDir::Down),
            (alt('k'), FocusDir::Up),
            (alt('l'), FocusDir::Right),
        ];
        for (k, dir) in cases {
            assert_eq!(resolve_simple(Mode::Normal, &k), Action::FocusPane(dir));
            assert_eq!(resolve_simple(Mode::Insert, &k), Action::FocusPane(dir));
            // Regression: `resolve_visual` used to skip the window keymap
            // entirely, so pane-focus chords either did nothing or, worse,
            // were shadowed by Visual's own `h`/`j`/`k`/`l` motions instead
            // of moving focus.
            assert_eq!(resolve_simple(Mode::Visual, &k), Action::FocusPane(dir));
        }
    }

    #[test]
    fn test_ctrl_digit_goes_to_tab() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: true,
                alt: false,
                shift: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(resolve_simple(Mode::Normal, &k), Action::GotoTab(n));
            assert_eq!(resolve_simple(Mode::Insert, &k), Action::GotoTab(n));
        }
    }

    #[test]
    fn test_alt_digit_focuses_pane_by_default() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: false,
                alt: true,
                shift: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(
                resolve_simple(Mode::Normal, &k),
                Action::FocusPaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Insert, &k),
                Action::FocusPaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Visual, &k),
                Action::FocusPaneByIndex(n)
            );
        }
    }

    #[test]
    fn test_ctrl_alt_digit_closes_pane_by_default() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: true,
                alt: true,
                shift: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(
                resolve_simple(Mode::Normal, &k),
                Action::ClosePaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Insert, &k),
                Action::ClosePaneByIndex(n)
            );
        }
    }

    #[test]
    fn test_insert_sends_shell_control_chars_to_pty() {
        // Every Ctrl+letter that the shell binds (readline / vi mode) must reach
        // the PTY as its control byte, never be claimed by window bindings.
        let ctrl = |c: char| Key {
            ctrl: true,
            ..key(KeyCode::Char(c))
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('a')),
            Action::SendBytes(vec![0x01])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('e')),
            Action::SendBytes(vec![0x05])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('h')),
            Action::SendBytes(vec![0x08])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('j')),
            Action::SendBytes(vec![0x0a])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('k')),
            Action::SendBytes(vec![0x0b])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('l')),
            Action::SendBytes(vec![0x0c])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('u')),
            Action::SendBytes(vec![0x15])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('w')),
            Action::SendBytes(vec![0x17])
        );
    }

    #[test]
    fn test_block_focus_escape_returns_to_normal() {
        assert_eq!(
            resolve_simple(Mode::BlockFocus, &key(KeyCode::Escape)),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_unbound_normal_key_is_ignored() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('z'))),
            Action::Ignore
        );
    }

    #[test]
    fn test_bracket_close_b_navigates_next_block() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char(']')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketClose);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, 0);
        assert_eq!(action, Action::FocusBlock(BlockNav::Next));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_open_b_navigates_previous_block() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('[')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketOpen);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, 0);
        assert_eq!(action, Action::FocusBlock(BlockNav::Previous));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_prefix_cancelled_by_ctrl() {
        let mut pending = PendingPrefix::BracketClose;
        let ctrl_h = Key {
            ctrl: true,
            ..key(KeyCode::Char('h'))
        };
        // C-h is not a window chord anymore (it would steal backspace from the
        // shell), so in Normal mode it merely cancels the pending prefix.
        let action = resolve(Mode::Normal, &ctrl_h, &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_prefix_with_unknown_key_is_ignored() {
        let mut pending = PendingPrefix::BracketClose;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_slash_starts_search() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('/'))),
            Action::SearchStart
        );
    }

    #[test]
    fn test_question_mark_starts_backward_search() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('?'))),
            Action::SearchStartBackward
        );
    }

    #[test]
    fn test_question_mark_arms_search_input_like_slash() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('?')), &mut pending, 0);
        assert_eq!(action, Action::SearchStartBackward);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_star_searches_word_under_cursor_forward() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('*'))),
            Action::SearchWord { forward: true }
        );
    }

    #[test]
    fn test_hash_searches_word_under_cursor_backward() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('#'))),
            Action::SearchWord { forward: false }
        );
    }

    #[test]
    fn test_n_goes_to_next_search_match() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('n'))),
            Action::SearchNext
        );
    }

    #[test]
    fn test_y_yanks_block() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('y'))),
            Action::YankBlock
        );
    }

    #[test]
    fn test_za_toggles_fold() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('z')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Z);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, 0);
        assert_eq!(action, Action::ToggleFold);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_z_followed_by_unknown_is_ignored() {
        let mut pending = PendingPrefix::Z;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_q_enters_quick_select() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('q')), &mut pending, 0);
        assert_eq!(action, Action::QuickSelect);
        assert_eq!(pending, PendingPrefix::QuickSelect);
    }

    #[test]
    fn test_quick_select_label_jumps() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('s')), &mut pending, 0);
        assert_eq!(action, Action::QuickJump('s'));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_escape_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, 0);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_non_alpha_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, 0);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_slash_enters_search_input() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('/')), &mut pending, 0);
        assert_eq!(action, Action::SearchStart);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_collects_chars() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('h')), &mut pending, 0);
        assert_eq!(action, Action::SearchChar('h'));
        assert_eq!(pending, PendingPrefix::SearchInput);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('i')), &mut pending, 0);
        assert_eq!(action, Action::SearchChar('i'));
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_enter_executes() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, 0);
        assert_eq!(action, Action::SearchExecute);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_escape_cancels() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, 0);
        assert_eq!(action, Action::SearchCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_backspace() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Backspace), &mut pending, 0);
        assert_eq!(action, Action::SearchBackspace);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_v_enters_charwise_visual() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Char)
        );
    }

    #[test]
    fn test_shift_v_enters_linewise_visual() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('V'))),
            Action::EnterVisual(VisualKind::Line)
        );
    }

    #[test]
    fn test_p_pastes_in_normal() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('p'))),
            Action::Paste
        );
    }

    #[test]
    fn test_ctrl_w_prefix_is_empty_sequence() {
        // C+w still opens the leader prefix, but no follow keys are bound by default.
        let mut pending = PendingPrefix::None;
        let ctrl_w = Key {
            ctrl: true,
            ..key(KeyCode::Char('w'))
        };
        assert_eq!(
            resolve(Mode::Normal, &ctrl_w, &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::CtrlW);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('o')), &mut pending, 0),
            Action::Ignore
        );
    }

    #[test]
    fn test_visual_motion_extends_and_y_yanks() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('j'))),
            Action::MoveCursor(CursorMove::Down)
        );
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('y'))),
            Action::YankSelection
        );
    }

    #[test]
    fn test_visual_escape_returns_to_normal() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Escape)),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_visual_v_toggles_back_to_normal() {
        // `v` in Visual resolves to EnterVisual; the handler toggles it off.
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Char)
        );
    }

    #[test]
    fn test_visual_gg_jumps_to_top() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Visual, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::G);
        let action = resolve(Mode::Visual, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::MoveCursor(CursorMove::Top));
    }

    #[test]
    fn test_ctrl_slash_is_prompt_undo_in_both_modes() {
        let undo = Key {
            ctrl: true,
            ..key(KeyCode::Char('/'))
        };
        assert_eq!(resolve_simple(Mode::Insert, &undo), Action::PromptUndo);
        assert_eq!(resolve_simple(Mode::Normal, &undo), Action::PromptUndo);
    }

    #[test]
    fn test_ctrl_backslash_is_prompt_redo_in_both_modes() {
        let redo = Key {
            ctrl: true,
            ..key(KeyCode::Char('\\'))
        };
        assert_eq!(resolve_simple(Mode::Insert, &redo), Action::PromptRedo);
        assert_eq!(resolve_simple(Mode::Normal, &redo), Action::PromptRedo);
    }

    #[test]
    fn test_plain_slash_is_not_undo() {
        // `/` without Ctrl still starts a block search in Normal mode and is
        // forwarded to the PTY in Insert mode.
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Char('/'))),
            Action::SendBytes(vec![b'/'])
        );
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('/')), &mut pending, 0),
            Action::SearchStart
        );
    }

    #[test]
    fn test_x_deletes_char_on_prompt() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('x'))),
            Action::DeleteCharForward
        );
    }

    #[test]
    fn test_shift_d_deletes_to_line_end() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('D'))),
            Action::DeleteToLineEnd
        );
    }

    #[test]
    fn test_dd_deletes_line() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Delete);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(action, Action::DeleteLine);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_dw_deletes_word() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::DeleteWordForward
        );
    }

    #[test]
    fn test_ctrl_shift_q_closes_pane() {
        let ctrl_shift_q = Key {
            alt: false,
            code: KeyCode::Char('q'),
            ctrl: true,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &ctrl_shift_q, &mut pending, 0),
            Action::ClosePane
        );
    }

    #[test]
    fn test_default_split_bindings() {
        // Shift-Alt-- splits horizontally; Shift-Alt-\ splits vertically.
        let shift_alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &shift_alt('-'), &mut pending, 0),
            Action::SplitPane(Direction::Horizontal)
        );
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &shift_alt('\\'), &mut pending, 0),
            Action::SplitPane(Direction::Vertical)
        );
        // The old Ctrl-w v/s/S no longer trigger splits by default.
        for code in [KeyCode::Char('v'), KeyCode::Char('s'), KeyCode::Char('S')] {
            let mut pending = PendingPrefix::CtrlW;
            assert_eq!(
                resolve(Mode::Normal, &key(code), &mut pending, 0),
                Action::Ignore
            );
        }
    }

    #[test]
    fn test_parse_chord_modifiers_and_named_keys() {
        // Abbreviated single-letter modifiers (case-sensitive).
        assert_eq!(
            parse_chord("C+w"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('w'),
                ctrl: true,
                shift: false,
            })
        );
        assert_eq!(
            parse_chord("C+S+Space"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
            })
        );
        // Full names still accepted.
        assert_eq!(
            parse_chord("ctrl+shift+Space"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
            })
        );
        assert_eq!(parse_chord("F5").map(|k| k.code), Some(KeyCode::F(5)));
        assert_eq!(parse_chord("v").map(|k| k.code), Some(KeyCode::Char('v')));
        assert_eq!(parse_chord("Hyper+x"), None);
        // `-` key is just the last segment; no special escaping needed.
        assert_eq!(
            parse_chord("S+M+-"),
            Some(Key {
                alt: true,
                code: KeyCode::Char('-'),
                ctrl: false,
                shift: true,
            })
        );
        assert_eq!(
            parse_chord("C+-"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('-'),
                ctrl: true,
                shift: false,
            })
        );
        // `+` key is written as a trailing `++`.
        assert_eq!(
            parse_chord("C+S++"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('+'),
                ctrl: true,
                shift: true,
            })
        );
    }

    #[test]
    fn test_parse_chord_sequence_length_bounds() {
        assert_eq!(parse_chord_sequence("C+w v").map(|k| k.len()), Some(2));
        assert_eq!(parse_chord_sequence("C+h").map(|k| k.len()), Some(1));
        assert_eq!(parse_chord_sequence(""), None);
        assert_eq!(parse_chord_sequence("a b c"), None);
    }

    #[test]
    fn test_config_rebinds_window_action_and_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("C+w b".to_string(), "split_horizontal".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        // The rebound sequence key now splits horizontally.
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &key(KeyCode::Char('b')),
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::SplitPane(Direction::Horizontal)
        );
        // The default S+M+- is dropped because split_horizontal was rebound.
        let shift_alt_minus = Key {
            alt: true,
            code: KeyCode::Char('-'),
            ctrl: false,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &shift_alt_minus,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Ignore
        );
        // An unmentioned action keeps its default (Ctrl-Shift-q still closes).
        let ctrl_shift_q = Key {
            alt: false,
            code: KeyCode::Char('q'),
            ctrl: true,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &ctrl_shift_q,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::ClosePane
        );
    }

    #[test]
    fn test_ctrl_backspace_deletes_word_back_by_default() {
        let keymap = WindowKeymap::default();
        let ctrl_bsp = Key {
            ctrl: true,
            ..key(KeyCode::Backspace)
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Insert,
                &ctrl_bsp,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Edit(EditAction::DeleteWordBackward)
        );
    }

    #[test]
    fn test_config_rebinds_undo_redo() {
        let mut editing = HashMap::new();
        editing.insert("C+z".to_string(), "prompt_undo".to_string());
        editing.insert("C+y".to_string(), "prompt_redo".to_string());
        let keymap = WindowKeymap::from_config(None, Some(&editing));

        let ctrl_z = Key {
            ctrl: true,
            ..key(KeyCode::Char('z'))
        };
        let ctrl_y = Key {
            ctrl: true,
            ..key(KeyCode::Char('y'))
        };
        let resolve = |mode, k: &Key| {
            let mut pending = PendingPrefix::None;
            resolve_with(mode, k, &mut pending, &keymap, 0, None, false)
        };
        // The rebound chords drive undo/redo in both Insert and Normal mode.
        assert_eq!(resolve(Mode::Insert, &ctrl_z), Action::PromptUndo);
        assert_eq!(resolve(Mode::Normal, &ctrl_z), Action::PromptUndo);
        assert_eq!(resolve(Mode::Insert, &ctrl_y), Action::PromptRedo);
        assert_eq!(resolve(Mode::Normal, &ctrl_y), Action::PromptRedo);
        // The default Ctrl-/ is dropped because undo was rebound.
        let ctrl_slash = Key {
            ctrl: true,
            ..key(KeyCode::Char('/'))
        };
        assert_eq!(
            resolve(Mode::Insert, &ctrl_slash),
            Action::SendBytes(encode(&ctrl_slash, 0, None))
        );
    }

    #[test]
    fn test_config_rebinds_editing_action() {
        let mut editing = HashMap::new();
        editing.insert("C+u".to_string(), "delete_to_line_start".to_string());
        let keymap = WindowKeymap::from_config(None, Some(&editing));

        let ctrl_u = Key {
            ctrl: true,
            ..key(KeyCode::Char('u'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Insert, &ctrl_u, &mut pending, &keymap, 0, None, false),
            Action::Edit(EditAction::DeleteToLineStart)
        );
        // The default Ctrl-Backspace binding survives (a different action).
        let ctrl_bsp = Key {
            ctrl: true,
            ..key(KeyCode::Backspace)
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Insert,
                &ctrl_bsp,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Edit(EditAction::DeleteWordBackward)
        );
    }

    #[test]
    fn test_config_custom_leader_and_direct_focus() {
        let mut bindings = HashMap::new();
        bindings.insert("M+y".to_string(), "split_horizontal".to_string());
        bindings.insert("C+b o".to_string(), "close_other_panes".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        // A direct, non-default chord splits horizontally.
        let mut pending = PendingPrefix::None;
        let alt_y = Key {
            alt: true,
            ..key(KeyCode::Char('y'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &alt_y, &mut pending, &keymap, 0, None, false),
            Action::SplitPane(Direction::Horizontal)
        );

        // The leader is now C+b; C+b then o closes other panes.
        let mut pending = PendingPrefix::None;
        let ctrl_b = Key {
            ctrl: true,
            ..key(KeyCode::Char('b'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &ctrl_b, &mut pending, &keymap, 0, None, false),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::CtrlW);
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &key(KeyCode::Char('o')),
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::CloseOtherPanes
        );
    }

    #[test]
    fn test_global_action_open_settings_by_default() {
        let keymap = WindowKeymap::default();
        let ctrl_comma = Key {
            ctrl: true,
            ..key(KeyCode::Char(','))
        };
        assert_eq!(
            keymap.global_action(&ctrl_comma),
            Some(Action::OpenSettings)
        );
    }

    #[test]
    fn test_global_action_rebind_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("C+S+o".to_string(), "open_settings".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let ctrl_shift_o = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Char('o'))
        };
        assert_eq!(
            keymap.global_action(&ctrl_shift_o),
            Some(Action::OpenSettings)
        );

        // The default Ctrl-, no longer opens settings once rebound.
        let ctrl_comma = Key {
            ctrl: true,
            ..key(KeyCode::Char(','))
        };
        assert_eq!(keymap.global_action(&ctrl_comma), None);
    }

    #[test]
    fn test_zoom_has_default_and_ctrl_shift_m_chords() {
        let shift_alt_equals = Key {
            alt: true,
            shift: true,
            ..key(KeyCode::Char('='))
        };
        let ctrl_shift_m = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Char('m'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt_equals),
            Action::ZoomPane
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_shift_m),
            Action::ZoomPane
        );
    }

    #[test]
    fn test_config_rebinds_index_action_and_drops_default() {
        let mut bindings = HashMap::new();
        // Move focus_pane_2 off its default Alt+2 chord onto Alt+Q.
        bindings.insert("M+q".to_string(), "focus_pane_2".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(2)
        );

        // The default Alt+2 no longer focuses pane 2 — it was dropped in favor
        // of the rebound chord.
        let alt_2 = Key {
            alt: true,
            ..key(KeyCode::Char('2'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_2, &mut pending, &keymap, 0, None, false),
            Action::Ignore
        );

        // An unmentioned index (Alt+3) keeps its default.
        let alt_3 = Key {
            alt: true,
            ..key(KeyCode::Char('3'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_3, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(3)
        );
    }

    #[test]
    fn test_config_binds_a_chord_to_a_palette_only_command() {
        // `mux_new_session` has no `WindowAction` variant; the chord must
        // resolve to `RunCommand`, not silently drop like it used to.
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let f5 = key(KeyCode::F(5));
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &f5, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }

    #[test]
    fn test_config_named_command_displaces_the_default_window_action_on_that_chord() {
        // Rebinding a chord that already has a default WindowAction (Alt+h /
        // focus_left) to a named command must actually take effect, not be
        // shadowed by the still-active default.
        let mut bindings = HashMap::new();
        bindings.insert("M+h".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_h = key(KeyCode::Char('h'));
        let alt_h = Key { alt: true, ..alt_h };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_h, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }

    #[test]
    fn test_config_ignores_a_two_key_sequence_bound_to_an_unrecognized_name() {
        // Named commands only support single-key chords in v1; a
        // leader-sequence spec paired with a palette-only name must be
        // dropped, not panic or half-bind.
        let mut bindings = HashMap::new();
        bindings.insert("C+w x".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);
        assert!(keymap.named.is_empty());
    }

    #[test]
    fn test_chord_hint_reports_a_bound_named_command() {
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);
        assert_eq!(keymap.chord_hint("mux_new_session"), "F5");
        assert_eq!(keymap.chord_hint("cd_recent"), "");
    }

    #[test]
    fn test_specific_index_binding_targets_a_fixed_pane() {
        let mut bindings = HashMap::new();
        bindings.insert("M+q".to_string(), "focus_pane_1".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(1)
        );

        // An unrelated default (Alt+3) is untouched by the new binding.
        let alt_3 = Key {
            alt: true,
            ..key(KeyCode::Char('3'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_3, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(3)
        );
    }

    #[test]
    fn test_specific_index_binding_rejects_out_of_range_index() {
        assert_eq!(IndexAction::from_name("goto_tab_0"), None);
        assert_eq!(IndexAction::from_name("goto_tab_10"), None);
        assert_eq!(IndexAction::from_name("goto_tab_x"), None);
        assert_eq!(
            IndexAction::from_name("goto_tab_9"),
            Some(IndexAction {
                kind: IndexKind::GotoTab,
                n: 9
            })
        );
    }

    #[test]
    fn test_shift_alt_o_closes_other_panes() {
        let shift_alt_o = Key {
            alt: true,
            code: KeyCode::Char('o'),
            ctrl: false,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &shift_alt_o, &mut pending, 0),
            Action::CloseOtherPanes
        );
    }

    // ---- Kitty keyboard protocol encoding -----------------------------------

    fn kitty(code: KeyCode) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &key(code), &mut pending, 1)
    }

    fn kitty_key(k: Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &k, &mut pending, 1)
    }

    #[test]
    fn test_xterm_ctrl_i_equals_tab_ambiguity() {
        // In legacy xterm mode, Ctrl+I and Tab produce the same bytes.
        let tab = resolve_simple(Mode::Insert, &key(KeyCode::Tab));
        let ctrl_i = resolve_simple(
            Mode::Insert,
            &Key {
                ctrl: true,
                ..key(KeyCode::Char('i'))
            },
        );
        assert_eq!(tab, ctrl_i);
    }

    #[test]
    fn test_kitty_ctrl_i_differs_from_tab() {
        // In Kitty mode, Tab still produces \t but Ctrl+I is disambiguated.
        let tab = kitty(KeyCode::Tab);
        let ctrl_i = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('i'))
        });
        assert_ne!(tab, ctrl_i);
        assert_eq!(tab, Action::SendBytes(vec![b'\t']));
        // Ctrl+I: codepoint 105 ('i'), modifier 5 (ctrl=4, +1 base).
        assert_eq!(ctrl_i, Action::SendBytes(b"\x1b[105;5u".to_vec()));
    }

    #[test]
    fn test_kitty_shift_enter() {
        let shift_enter = kitty_key(Key {
            shift: true,
            ..key(KeyCode::Enter)
        });
        // Codepoint 13 (CR), modifier 2 (shift).
        assert_eq!(shift_enter, Action::SendBytes(b"\x1b[13;2u".to_vec()));
    }

    #[test]
    fn test_kitty_bare_enter_unchanged() {
        assert_eq!(kitty(KeyCode::Enter), Action::SendBytes(vec![b'\r']));
    }

    #[test]
    fn test_kitty_ctrl_escape() {
        let ctrl_esc = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Escape)
        });
        // Codepoint 27 (ESC), modifier 5 (ctrl).
        assert_eq!(ctrl_esc, Action::SendBytes(b"\x1b[27;5u".to_vec()));
    }

    #[test]
    fn test_kitty_bare_escape_unchanged() {
        assert_eq!(kitty(KeyCode::Escape), Action::SendBytes(vec![ESCAPE]));
    }

    #[test]
    fn test_kitty_ctrl_left_arrow() {
        let ctrl_left = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Left)
        });
        // KP_LEFT = 57350, modifier 5 (ctrl).
        assert_eq!(ctrl_left, Action::SendBytes(b"\x1b[57350;5u".to_vec()));
    }

    #[test]
    fn test_kitty_printable_no_modifier_passthrough() {
        assert_eq!(kitty(KeyCode::Char('a')), Action::SendBytes(vec![b'a']));
        assert_eq!(kitty(KeyCode::Char('Z')), Action::SendBytes(vec![b'Z']));
    }

    #[test]
    fn test_kitty_ctrl_printable() {
        // Ctrl+A: codepoint 97 ('a'), modifier 5 (ctrl).
        let ctrl_a = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('a'))
        });
        assert_eq!(ctrl_a, Action::SendBytes(b"\x1b[97;5u".to_vec()));
    }

    #[test]
    fn test_kitty_f1_through_f4() {
        assert_eq!(
            kitty(KeyCode::F(1)),
            Action::SendBytes(b"\x1b[57364u".to_vec())
        );
        assert_eq!(
            kitty(KeyCode::F(4)),
            Action::SendBytes(b"\x1b[57367u".to_vec())
        );
    }

    #[test]
    fn test_kitty_shift_tab_backtab() {
        let shift_tab = kitty_key(Key {
            shift: true,
            ..key(KeyCode::Tab)
        });
        assert_eq!(shift_tab, Action::SendBytes(vec![ESCAPE, b'[', b'Z']));
    }

    #[test]
    fn test_g_angle_bracket_moves_tab() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('<')), &mut pending, 0),
            Action::MoveTabLeft,
        );
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('>')), &mut pending, 0),
            Action::MoveTabRight,
        );
    }

    #[test]
    fn test_scroll_keybindings_resolve() {
        let shift_alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('h')),
            Action::ScrollPageUp
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('l')),
            Action::ScrollPageDown
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('j')),
            Action::ScrollLineDown
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('k')),
            Action::ScrollLineUp
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('a')),
            Action::ScrollToTop
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('e')),
            Action::ScrollToBottom
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt(',')),
            Action::ScrollToTop
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('.')),
            Action::ScrollToBottom
        );

        // Also check that it intercepts in Insert mode
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt('h')),
            Action::ScrollPageUp
        );
    }

    #[test]
    fn test_tab_switching_keybindings_resolve() {
        let ctrl_pageup = Key {
            alt: false,
            code: KeyCode::PageUp,
            ctrl: true,
            shift: false,
        };
        let ctrl_pagedown = Key {
            alt: false,
            code: KeyCode::PageDown,
            ctrl: true,
            shift: false,
        };
        let shift_alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
        };
        assert_eq!(resolve_simple(Mode::Normal, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt(']')),
            Action::NextTab
        );

        // Also check that it intercepts in Insert mode
        assert_eq!(resolve_simple(Mode::Insert, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt(']')),
            Action::NextTab
        );
    }

    #[test]
    fn test_shift_alt_letter_encodes_esc_prefix_in_legacy_mode() {
        // Shift+Alt+E in legacy (flags=0) mode produces \x1bE (ESC + uppercase
        // letter). The shift is encoded as the uppercase character; the alt is
        // the ESC prefix. This is the correct xterm encoding but is timing-
        // ambiguous (can parse as Escape then E).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, None);
        assert_eq!(bytes, b"\x1bE".to_vec());
    }

    #[test]
    fn test_shift_alt_letter_encodes_csi_u_in_kitty_mode() {
        // Shift+Alt+E with Kitty flags active produces \x1b[101;4u — the
        // unambiguous CSI-u encoding (codepoint 101='e', modifier 4=shift+alt).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 1, None); // flag 1 = disambiguate
        assert_eq!(bytes, b"\x1b[101;4u".to_vec());
    }

    #[test]
    fn test_shift_alt_h_l_encode_correctly_in_kitty_mode() {
        // Verify Shift+Alt+H and Shift+Alt+L as well.
        let mk = |c: char| Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char(c),
        };
        assert_eq!(encode(&mk('H'), 1, None), b"\x1b[104;4u".to_vec());
        assert_eq!(encode(&mk('L'), 1, None), b"\x1b[108;4u".to_vec());
    }

    #[test]
    fn test_modify_other_keys_shift_alt_e_uses_27_format() {
        // When modifyOtherKeys is active (mode 2), Shift+Alt+E produces
        // `\x1b[27;4;69~` (modifier 4 = shift+alt, codepoint 69 = 'E').
        // This is unambiguous — no ESC prefix that could parse as standalone Escape.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, Some(2));
        assert_eq!(bytes, b"\x1b[27;4;69~".to_vec());
    }

    #[test]
    fn test_modify_other_keys_disabled_falls_back_to_esc_prefix() {
        // Without modifyOtherKeys, Shift+Alt+E falls back to the legacy \x1bE.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, None);
        assert_eq!(bytes, b"\x1bE".to_vec());
    }

    #[test]
    fn test_modify_other_keys_unmodified_char_not_affected() {
        // A bare character (no modifiers) is never affected by modifyOtherKeys.
        let plain_e = Key {
            alt: false,
            shift: false,
            ctrl: false,
            code: KeyCode::Char('e'),
        };
        let bytes = encode(&plain_e, 0, Some(2));
        assert_eq!(bytes, b"e".to_vec());
    }

    #[test]
    fn test_named_register_yank_and_paste_resolution() {
        let win = WindowKeymap::default();
        let mut pending = PendingPrefix::None;

        // Press `"`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Register);

        // Press `a`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('a'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::Ignore);
        assert_eq!(pending, PendingPrefix::WithRegister('a'));

        // Press `p`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('p'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::PasteRegister {
                register: 'a',
                after: true
            }
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_change_surround_and_delete_surround_resolution() {
        let win = WindowKeymap::default();

        // `ds"`
        let mut pending = PendingPrefix::None;
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('d'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::DeleteSurround('"'));

        // `cs"'`
        let mut pending = PendingPrefix::None;
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('\''),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::ChangeSurround {
                target: '"',
                replacement: '\''
            }
        );
    }

    #[test]
    fn test_change_and_replace_operators_resolution() {
        let win = WindowKeymap::default();

        // `cw`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::Change);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('w'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeWordForward);

        // `ciw`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('i'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ChangeObject { around: false });
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('w'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::ChangeTextObject(TextObjectSpec::new(false, TextObject::Word))
        );

        // `C`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('C'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeToLineEnd);

        // `s`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SubstituteChar);

        // `S`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('S'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeLine);

        // `rx`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('r'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ReplaceChar);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('x'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ReplaceChar('x'));

        // `~`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('~'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ToggleCaseChar);
    }

    #[test]
    fn test_search_match_and_g_shortcuts_resolution() {
        let win = WindowKeymap::default();

        // `gn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::G);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SelectSearchMatch { forward: true });

        // `gN`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('N'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SelectSearchMatch { forward: false });

        // `cgn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ChangeG);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeSearchMatch { forward: true });

        // `dgn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('d'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::DeleteG);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::DeleteSearchMatch { forward: true });

        // `gs` (Swoop)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ToggleSwoop);

        // `gp` (Prompt jump)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('p'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::JumpToPrompt);

        // `gP` (Prev prompt jump)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('P'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::JumpToPreviousPrompt);
    }
}
