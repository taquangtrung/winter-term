//! The shared key layer of the Vim foundation: one key-to-motion mapping
//! that every surface resolves through, so a key means the same motion
//! wherever it is not otherwise claimed. See [`super`] for the layering and
//! the override contract.

use super::motion::CursorMove;
use crate::model::input::{Key, KeyCode};

// ========================================================================
// Data Structures
// ========================================================================

/// What offering a key to the shared layer produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VimKey {
    /// The key named a motion, which the surface should now interpret.
    Motion(CursorMove),
    /// The key opened or continued a sequence (`g` awaiting its `g`); it is
    /// spent, and nothing else should see it.
    Pending,
    /// Not a shared key: the surface keeps it.
    Unhandled,
}

/// The shared motion key state a surface keeps between keys: the `g` prefix.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VimNav {
    /// Whether a `g` is waiting for its follow key.
    goto: bool,
}

// ========================================================================
// VimNav
// ========================================================================

impl VimNav {
    /// A nav with no sequence open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a `g` sequence is open, so the next key resolves here before
    /// the surface's own bindings see it.
    pub fn in_sequence(&self) -> bool {
        self.goto
    }

    /// Offer one key to the shared layer. Window chords (Alt, and the Ctrl
    /// keys the layer does not bind) fall through unhandled so the host keeps
    /// them.
    pub fn key(&mut self, key: &Key) -> VimKey {
        if key.alt {
            return VimKey::Unhandled;
        }
        if self.goto {
            self.goto = false;
            return match key.code {
                KeyCode::Char('g') => VimKey::Motion(CursorMove::Top),
                // The sequence is abandoned and the key spent, matching how a
                // stray follow key abandons dir's own two-key leaders.
                _ => VimKey::Pending,
            };
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('d') => VimKey::Motion(CursorMove::HalfPageDown),
                KeyCode::Char('u') => VimKey::Motion(CursorMove::HalfPageUp),
                KeyCode::Char('f') => VimKey::Motion(CursorMove::PageDown),
                KeyCode::Char('b') => VimKey::Motion(CursorMove::PageUp),
                _ => VimKey::Unhandled,
            };
        }
        match bare_motion(key) {
            Some(motion) => VimKey::Motion(motion),
            None if key.code == KeyCode::Char('g') => {
                self.goto = true;
                VimKey::Pending
            }
            None => VimKey::Unhandled,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The default key-to-motion mapping every surface shares, factored out of
/// the terminal's Normal mode so a motion key resolves identically over the
/// grid, the listing, and the git view. Matches on the key's code alone, as
/// the terminal's own resolver does; the caller owns any modifier policy.
pub fn bare_motion(key: &Key) -> Option<CursorMove> {
    use CursorMove as M;
    Some(match key.code {
        KeyCode::Char('h') | KeyCode::Left => M::Left,
        KeyCode::Char('j') | KeyCode::Down => M::Down,
        KeyCode::Char('k') | KeyCode::Up => M::Up,
        KeyCode::Char('l') | KeyCode::Right => M::Right,
        // `|` with no count is column one, same as `0`.
        KeyCode::Char('0') | KeyCode::Char('|') | KeyCode::Home => M::LineStart,
        KeyCode::Char('$') | KeyCode::End => M::LineEnd,
        KeyCode::Char('^') | KeyCode::Char('_') => M::FirstNonBlank,
        KeyCode::Char('w') => M::WordForward,
        KeyCode::Char('W') => M::WordForwardBig,
        KeyCode::Char('b') => M::WordBack,
        KeyCode::Char('B') => M::WordBackBig,
        KeyCode::Char('e') => M::WordEnd,
        KeyCode::Char('E') => M::WordEndBig,
        KeyCode::Char('{') => M::ParagraphBack,
        KeyCode::Char('}') => M::ParagraphForward,
        KeyCode::Char('%') => M::MatchingBracket,
        KeyCode::Char('H') => M::ScreenTop,
        KeyCode::Char('M') => M::ScreenMiddle,
        KeyCode::Char('L') => M::ScreenBottom,
        KeyCode::Char('G') => M::Bottom,
        KeyCode::PageDown => M::PageDown,
        KeyCode::PageUp => M::PageUp,
        _ => return None,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn ctrl(c: char) -> Key {
        Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: false,
        }
    }

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        }
    }

    #[test]
    fn test_the_bare_motions_match_the_terminal_normals_vocabulary() {
        // The same key resolving to the same motion everywhere is the whole
        // point of the layer: spot-check the mapping against the keys the
        // terminal's Normal mode has always used.
        let cases = [
            (KeyCode::Char('w'), CursorMove::WordForward),
            (KeyCode::Char('B'), CursorMove::WordBackBig),
            (KeyCode::Char('}'), CursorMove::ParagraphForward),
            (KeyCode::Char('$'), CursorMove::LineEnd),
            (KeyCode::Char('|'), CursorMove::LineStart),
            (KeyCode::Char('_'), CursorMove::FirstNonBlank),
            (KeyCode::Char('G'), CursorMove::Bottom),
            (KeyCode::Char('%'), CursorMove::MatchingBracket),
            (KeyCode::Home, CursorMove::LineStart),
            (KeyCode::End, CursorMove::LineEnd),
            (KeyCode::PageDown, CursorMove::PageDown),
        ];
        for (code, want) in cases {
            assert_eq!(bare_motion(&plain(code)), Some(want), "{code:?}");
        }
        assert_eq!(bare_motion(&plain(KeyCode::Char('x'))), None);
        // Prefix openers are the caller's to arm, not bare motions.
        assert_eq!(bare_motion(&plain(KeyCode::Char('f'))), None);
        assert_eq!(bare_motion(&plain(KeyCode::Char('z'))), None);
    }

    #[test]
    fn test_gg_completes_and_a_stray_follow_key_abandons() {
        let mut nav = VimNav::new();
        assert_eq!(nav.key(&plain(KeyCode::Char('g'))), VimKey::Pending);
        assert!(nav.in_sequence());
        assert_eq!(
            nav.key(&plain(KeyCode::Char('g'))),
            VimKey::Motion(CursorMove::Top)
        );
        assert!(!nav.in_sequence(), "the sequence resolved");

        nav.key(&plain(KeyCode::Char('g')));
        assert_eq!(
            nav.key(&plain(KeyCode::Char('!'))),
            VimKey::Pending,
            "a stray follow key is spent, abandoning the sequence"
        );
        assert!(!nav.in_sequence());
        assert_eq!(
            nav.key(&plain(KeyCode::Char('w'))),
            VimKey::Motion(CursorMove::WordForward),
            "the next key starts fresh"
        );
    }

    #[test]
    fn test_the_page_chords_and_the_window_chords_fall_through() {
        let mut nav = VimNav::new();
        assert_eq!(
            nav.key(&ctrl('d')),
            VimKey::Motion(CursorMove::HalfPageDown)
        );
        assert_eq!(nav.key(&ctrl('f')), VimKey::Motion(CursorMove::PageDown));
        // Window chords the layer does not bind stay with the host.
        assert_eq!(nav.key(&alt('h')), VimKey::Unhandled);
        assert_eq!(nav.key(&ctrl('x')), VimKey::Unhandled);
    }
}
