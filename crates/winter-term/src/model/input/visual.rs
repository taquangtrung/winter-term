//! Visual mode: Normal-mode resolution with motions extending a selection.

use super::normal::{accumulate_count, motion_action, text_object};
use super::{Action, Key, KeyCode, Mods};
use super::{InsertAt, Keymap, PendingPrefix, TextObjectSpec, VisualKind};
use crate::model::mode::{Mode, ModeEvent};
use crate::model::vim::nav::chord_motion;

// ========================================================================
// Visual mode: selection-extending resolution
// ========================================================================

/// Resolve a key in Visual mode: the same motions as Normal extend the
/// selection, `y` yanks it, and `v`/`V`/`Esc` leave Visual.
pub(super) fn resolve_visual(key: &Key, pending: &mut PendingPrefix, window: &Keymap) -> Action {
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

    // A key with no modifier, or with Shift alone, carries on below; every
    // other chord resolves here and clears any prefix.
    let (mods, code) = key.chord();
    if !matches!(mods, Mods::NONE | Mods::SHIFT) {
        *pending = PendingPrefix::None;
        if let Some(mv) = chord_motion(key).filter(|_| mods == Mods::CTRL) {
            return Action::MoveCursor(mv);
        }
        return match (mods, code) {
            (Mods::CTRL, KeyCode::Char('v')) => Action::EnterVisual(VisualKind::Block),
            (Mods::CTRL, KeyCode::Char('e')) => Action::ScrollLineDown,
            (Mods::CTRL, KeyCode::Char('y')) => Action::ScrollLineUp,
            // Jumplist walking works from Visual too (extends the selection,
            // like any other cursor move).
            (Mods::CTRL, KeyCode::Char('o')) => Action::JumpOlder,
            (Mods::CTRL, KeyCode::Char('i')) => Action::JumpNewer,
            _ => Action::Ignore,
        };
    }

    // Count digits accumulate here as in Normal (`v3w` extends three words).
    if let Some(action) = accumulate_count(key, pending) {
        return action;
    }

    let prev = *pending;
    *pending = PendingPrefix::None;

    // Every motion Normal has, from the shared table: each one extends the
    // selection instead of just moving the cursor (see `App::handle_action`).
    if let Some(action) = motion_action(key, prev, pending) {
        return action;
    }

    match prev {
        PendingPrefix::TextObject { around } => match text_object(key) {
            Some(object) => Action::SelectTextObject(TextObjectSpec::new(around, object)),
            None => {
                if !around {
                    Action::EnterInsert(InsertAt::Cursor)
                } else {
                    Action::Ignore
                }
            }
        },
        PendingPrefix::Register => match key.typed_char() {
            Some(c) if c.is_ascii_alphanumeric() || c == '+' || c == '*' || c == '"' => {
                *pending = PendingPrefix::WithRegister(c);
                Action::Ignore
            }
            _ => Action::Ignore,
        },
        PendingPrefix::WithRegister(reg) => match key.chord() {
            (Mods::NONE, KeyCode::Char('y')) => Action::YankSelectionRegister(reg),
            (Mods::NONE, KeyCode::Char('d') | KeyCode::Char('x')) => Action::DeleteSelection,
            (Mods::NONE, KeyCode::Char('p')) => Action::PasteRegister {
                register: reg,
                after: true,
            },
            (Mods::NONE, KeyCode::Char('P')) => Action::PasteRegister {
                register: reg,
                after: false,
            },
            _ => Action::Ignore,
        },
        PendingPrefix::G => match key.chord() {
            (Mods::NONE, KeyCode::Char('n')) => Action::SelectSearchMatch { forward: true },
            (Mods::NONE, KeyCode::Char('N')) => Action::SelectSearchMatch { forward: false },
            (Mods::NONE, KeyCode::Char('p')) => Action::JumpToPrompt,
            (Mods::NONE, KeyCode::Char('P')) => Action::JumpToPreviousPrompt,
            (Mods::NONE, KeyCode::Char('s')) => Action::ToggleSwoop,
            (Mods::NONE, KeyCode::Char('x')) => Action::OpenUnderCursor,
            _ => Action::Ignore,
        },
        _ => match key.chord() {
            (Mods::NONE, KeyCode::Char('y')) => Action::YankSelection,
            (Mods::NONE, KeyCode::Char('Y')) => Action::YankSelectionLines,
            (Mods::NONE, KeyCode::Char('d') | KeyCode::Char('x')) => Action::DeleteSelection,
            (Mods::NONE, KeyCode::Char('v')) => Action::EnterVisual(VisualKind::Char),
            (Mods::NONE, KeyCode::Char('V')) => Action::EnterVisual(VisualKind::Line),
            (Mods::NONE, KeyCode::Char('"')) => {
                *pending = PendingPrefix::Register;
                Action::Ignore
            }
            (Mods::NONE, KeyCode::Char('i')) => {
                *pending = PendingPrefix::TextObject { around: false };
                Action::Ignore
            }
            (Mods::NONE, KeyCode::Char('a')) => {
                *pending = PendingPrefix::TextObject { around: true };
                Action::Ignore
            }
            // `o`: extend from the selection's other end instead (the span itself
            // is unchanged: it always runs anchor..cursor in either order).
            (Mods::NONE, KeyCode::Char('o')) => Action::SwapVisualEnds,
            (Mods::NONE, KeyCode::Escape) => {
                Action::SwitchMode(Mode::Visual.apply(ModeEvent::Escape))
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
    use super::super::{resolve, CursorMove, GotoMark, TextObject};
    use super::*;
    use crate::model::input::test_support::*;

    #[test]
    fn test_new_motions_resolve_in_both_normal_and_visual() {
        // The motion table is shared, so every one of these works in Visual too,
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
        // resolve `j` as a plain MoveCursor, or worse, as a count-less drop.
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
    fn test_visual_o_swaps_selection_ends() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('o'))),
            Action::SwapVisualEnds
        );
    }
    #[test]
    fn test_ctrl_o_and_ctrl_i_and_tab_walk_the_jumplist() {
        // `C-o`/`C-i` are vim's own jumplist bindings; Tab doubles for
        // `C-i` because most terminals deliver the two identically.
        let ctrl = |code: KeyCode| Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
            sup: false,
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
    fn test_text_object_and_block_visual_key_resolution() {
        let ctrl = |code: KeyCode| Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
            sup: false,
        };

        // C-v in Normal enters VisualKind::Block
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Block)
        );

        // C-v in Visual toggles / enters VisualKind::Block
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
    fn test_visual_shift_y_yanks_whole_lines() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('Y'))),
            Action::YankSelectionLines
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
    fn test_an_alt_chord_is_not_the_bare_key_it_holds() {
        // Alt chords used to fall through to the bare keys, so `M-y` yanked
        // the selection and `M-d` deleted it.
        for (code, bare) in [
            (KeyCode::Char('y'), Action::YankSelection),
            (KeyCode::Char('d'), Action::DeleteSelection),
        ] {
            assert_eq!(resolve_simple(Mode::Visual, &key(code)), bare);
            assert_ne!(resolve_simple(Mode::Visual, &Key::with_alt(code)), bare);
        }
    }
}
