//! Cursor motions and the text objects operators act on.

// ========================================================================
// The foundation
// ========================================================================

/// The motion vocabulary and the char-search live one layer down, in the
/// foundation ([`crate::model::vim::motion`]); re-exported here so the
/// terminal's resolver and its callers keep their paths.
pub use crate::model::vim::motion::{CursorMove, FindChar};

// ========================================================================
// Data Structures
// ========================================================================

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
    /// A paragraph: the run of lines around the cursor that are all blank or
    /// all not, which over a terminal's output is one block of it.
    Paragraph,
    /// A quoted run delimited by this character.
    Quotes(char),
    /// A sentence: up to a `.`, `!` or `?` and the quotes or brackets that
    /// close after it.
    Sentence,
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
    /// `a`: append: one column right of the cursor.
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
