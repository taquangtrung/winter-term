//! The Vim motion vocabulary: the intents every surface resolves keys into,
//! and the char-search they share. The foundation's naming layer — the
//! terminal grid and the tool pages both speak in these terms.

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

/// A Normal-mode cursor traversal, in the terms every surface shares. Over
/// the terminal grid these are lines and columns; over a tool's rows, the
/// ends and the halves are what the words and lines become there.
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
