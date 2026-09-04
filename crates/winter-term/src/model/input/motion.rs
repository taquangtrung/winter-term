//! Cursor motions and the text objects operators act on.

// ========================================================================
// Items
// ========================================================================

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
