//! The pending-prefix state machine backing multi-key sequences.

use super::TextObjectSpec;

// ========================================================================
// PendingPrefix
// ========================================================================

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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
