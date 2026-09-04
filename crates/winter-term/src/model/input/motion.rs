//! Cursor motions and the text objects operators act on.

// ========================================================================
// Data Structures
// ========================================================================

/// A vim char-search within the current line. `forward` is `f`/`t`; `till`
/// (`t`/`T`) stops one cell short of the target instead of on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindChar {
    /// The character being searched for.
    pub ch: char,
    /// True for `f`/`t`, false for `F`/`T`.
    pub forward: bool,
    /// True for `t`/`T`, which stop one short of the target.
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
    /// True after `` ` `` (restore line and column), false after `'` (line only).
    pub exact: bool,
    /// The mark letter.
    pub mark: char,
}
impl GotoMark {
    /// A jump to a mark; `exact` restores the column as well as the line.
    pub fn new(mark: char, exact: bool) -> Self {
        Self { exact, mark }
    }
}
/// A vim text object target (word, delimited quotes, or bracket pairs).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextObject {
    /// A bracket pair, given as its opening and closing characters.
    Brackets(char, char),
    /// A quoted run delimited by this character.
    Quotes(char),
    /// A word, where punctuation breaks the run.
    Word,
    /// A WORD, where only whitespace breaks the run.
    WordBig,
}
/// Specification for a text object selection or operation (`around` vs `inner`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextObjectSpec {
    /// True for the `a` (around) form, false for `i` (inner).
    pub around: bool,
    /// Which object the operator applies to.
    pub object: TextObject,
}
impl TextObjectSpec {
    /// A text object; `around` selects the `a` form rather than the `i` form.
    pub fn new(around: bool, object: TextObject) -> Self {
        Self { around, object }
    }
}
/// Whether a Visual selection spans blocks, characters, or whole lines (`Ctrl-V`, `v`, `V`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualKind {
    /// A rectangular selection (`Ctrl-V`).
    Block,
    /// A character-wise selection (`v`).
    Char,
    /// A whole-line selection (`V`).
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
    /// Toward the end of the scrollback.
    Next,
    /// Toward the start of the scrollback.
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
    /// To the last line (`G`).
    Bottom,
    /// Down one line (`j`).
    Down,
    /// To the first non-blank column of the line (`^`).
    FirstNonBlank,
    /// Down half a screen (`Ctrl-D`).
    HalfPageDown,
    /// Up half a screen (`Ctrl-U`).
    HalfPageUp,
    /// `g_`: the last non-blank character on the line.
    LastNonBlank,
    /// Left one column (`h`).
    Left,
    /// `zb`: scroll so the cursor's line sits on the viewport's last row.
    LineToBottom,
    /// `zz`: scroll so the cursor's line sits in the middle of the viewport.
    LineToCenter,
    /// `zt`: scroll so the cursor's line sits on the viewport's first row.
    LineToTop,
    /// To the end of the line (`$`).
    LineEnd,
    /// To column zero (`0`).
    LineStart,
    /// `%`: the bracket matching the one at or right of the cursor.
    MatchingBracket,
    /// Down one screen (`Ctrl-F`).
    PageDown,
    /// Up one screen (`Ctrl-B`).
    PageUp,
    /// `{`: the previous paragraph boundary (blank line).
    ParagraphBack,
    /// `}`: the next paragraph boundary (blank line).
    ParagraphForward,
    /// Right one column (`l`).
    Right,
    /// `H`: the viewport's first row.
    ScreenTop,
    /// `M`: the middle row of the viewport.
    ScreenMiddle,
    /// `L`: the viewport's last row holding content.
    ScreenBottom,
    /// To the first line (`gg`).
    Top,
    /// Up one line (`k`).
    Up,
    /// Back to the previous word start (`b`).
    WordBack,
    /// Back to the previous WORD start (`B`).
    WordBackBig,
    /// Forward to the end of a word (`e`).
    WordEnd,
    /// `ge`: the end of the previous word.
    WordEndBack,
    /// `gE`: the end of the previous WORD.
    WordEndBackBig,
    /// Forward to the end of a WORD (`E`).
    WordEndBig,
    /// Forward to the next word start (`w`).
    WordForward,
    /// Forward to the next WORD start (`W`).
    WordForwardBig,
}
