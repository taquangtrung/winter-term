//! Normal mode: resolving motions, counts, and operator sequences.

use super::{Action, Key, KeyCode};
use super::{
    BlockNav, CursorMove, FindChar, GotoMark, InsertAt, PendingPrefix, TextObject, TextObjectSpec,
    VisualKind, WindowAction, WindowKeymap,
};
use crate::model::mode::{Mode, ModeEvent};

// ========================================================================
// Normal mode: motion and operator resolution
// ========================================================================

/// The largest count a `5j`-style prefix accumulates; further digits clamp
/// rather than overflow. Vim's own cap is far larger, but every repeatable
/// motion re-renders nothing between steps, so a runaway `99999j` costs a full
/// buffer walk for no navigation value.
pub(super) const MAX_COUNT: usize = 9999;
/// Accumulate a count digit into `pending` (`5` before `j`). A consumed
/// digit returns `Some(Action::Ignore)` — the count rides in the prefix until
/// the motion that spends it resolves. A leading `0` is not a count (vim's
/// `0` motion); a `0` after other digits extends them. Digits typed while any
/// other sequence is open (a `/` query, a quick-select label, an operator)
/// pass through untouched, as do modified keys (window chords).
pub(super) fn accumulate_count(key: &Key, pending: &mut PendingPrefix) -> Option<Action> {
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
pub(super) fn count_repeats(mv: CursorMove) -> bool {
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
pub(super) fn find_char_action(key: &Key, forward: bool, till: bool) -> Action {
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
pub(super) fn motion_action(
    key: &Key,
    prev: PendingPrefix,
    pending: &mut PendingPrefix,
) -> Option<Action> {
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
pub(super) fn resolve_normal(
    key: &Key,
    pending: &mut PendingPrefix,
    window: &WindowKeymap,
) -> Action {
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{resolve, Direction};
    use super::*;
    use crate::model::input::test_support::*;

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
}
