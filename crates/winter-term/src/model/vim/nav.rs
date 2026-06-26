//! The shared key layer of the Vim foundation: one key-to-motion mapping
//! that every surface resolves through, so a key means the same motion
//! wherever it is not otherwise claimed. See [`super`] for the layering and
//! the override contract.

use super::motion::CursorMove;
use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::{Key, KeyCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// The bare motion keys, matched on the key's code alone: see
/// [`bare_motion`].
const MOTIONS: &[Binding<CursorMove>] = &[
    Binding {
        action: CursorMove::Left,
        keys: &["h", "<left>"],
        label: "Left",
    },
    Binding {
        action: CursorMove::Down,
        keys: &["j", "<down>"],
        label: "Down",
    },
    Binding {
        action: CursorMove::Up,
        keys: &["k", "<up>"],
        label: "Up",
    },
    Binding {
        action: CursorMove::Right,
        keys: &["l", "<right>"],
        label: "Right",
    },
    // `|` with no count is column one, same as `0`.
    Binding {
        action: CursorMove::LineStart,
        keys: &["0", "|", "<home>"],
        label: "Start of the line",
    },
    Binding {
        action: CursorMove::LineEnd,
        keys: &["$", "<end>"],
        label: "End of the line",
    },
    Binding {
        action: CursorMove::FirstNonBlank,
        keys: &["^", "_"],
        label: "First non-blank of the line",
    },
    Binding {
        action: CursorMove::WordForward,
        keys: &["w"],
        label: "Next word",
    },
    Binding {
        action: CursorMove::WordForwardBig,
        keys: &["W"],
        label: "Next WORD",
    },
    Binding {
        action: CursorMove::WordBack,
        keys: &["b"],
        label: "Previous word",
    },
    Binding {
        action: CursorMove::WordBackBig,
        keys: &["B"],
        label: "Previous WORD",
    },
    Binding {
        action: CursorMove::WordEnd,
        keys: &["e"],
        label: "End of the word",
    },
    Binding {
        action: CursorMove::WordEndBig,
        keys: &["E"],
        label: "End of the WORD",
    },
    Binding {
        action: CursorMove::ParagraphBack,
        keys: &["{"],
        label: "Previous paragraph",
    },
    Binding {
        action: CursorMove::ParagraphForward,
        keys: &["}"],
        label: "Next paragraph",
    },
    Binding {
        action: CursorMove::MatchingBracket,
        keys: &["%"],
        label: "Matching bracket",
    },
    Binding {
        action: CursorMove::ScreenTop,
        keys: &["H"],
        label: "Top of the screen",
    },
    Binding {
        action: CursorMove::ScreenMiddle,
        keys: &["M"],
        label: "Middle of the screen",
    },
    Binding {
        action: CursorMove::ScreenBottom,
        keys: &["L"],
        label: "Bottom of the screen",
    },
    Binding {
        action: CursorMove::Bottom,
        keys: &["G"],
        label: "Bottom",
    },
    Binding {
        action: CursorMove::PageDown,
        keys: &["<next>"],
        label: "Page down",
    },
    Binding {
        action: CursorMove::PageUp,
        keys: &["<prior>"],
        label: "Page up",
    },
];

/// The motions the layer answers beyond the bare ones: the paging chords and
/// the `g` sequence.
const CHORDS: &[Binding<CursorMove>] = &[
    Binding {
        action: CursorMove::Top,
        keys: &["g g"],
        label: "Top",
    },
    Binding {
        action: CursorMove::HalfPageDown,
        keys: &["C-d"],
        label: "Half a page down",
    },
    Binding {
        action: CursorMove::HalfPageUp,
        keys: &["C-u"],
        label: "Half a page up",
    },
    Binding {
        action: CursorMove::PageDown,
        keys: &["C-f"],
        label: "Page down",
    },
    Binding {
        action: CursorMove::PageUp,
        keys: &["C-b"],
        label: "Page up",
    },
];

/// Emacs' `M-<` and `M->`, as their unshifted keys with Shift held: see
/// [`buffer_end`].
const BUFFER_ENDS: &[Binding<CursorMove>] = &[
    Binding {
        action: CursorMove::Top,
        keys: &["M-S-,"],
        label: "Top",
    },
    Binding {
        action: CursorMove::Bottom,
        keys: &["M-S-."],
        label: "Bottom",
    },
];

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
    /// The first key of a sequence waiting for its follow key.
    pending: Option<Key>,
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
        self.pending.is_some()
    }

    /// Offer one key to the shared layer. Window chords (Alt, and the Ctrl
    /// keys the layer does not bind) fall through unhandled so the host keeps
    /// them, save for the two Alt chords [`buffer_end`] claims.
    pub fn key(&mut self, key: &Key) -> VimKey {
        if key.alt {
            return match buffer_end(key) {
                Some(motion) => VimKey::Motion(motion),
                None => VimKey::Unhandled,
            };
        }
        if let Some(first) = self.pending.take() {
            return match table::lookup(CHORDS, &[first, *key]) {
                TableMatch::Exact(motion) => VimKey::Motion(motion),
                // The sequence is abandoned and the key spent, matching how a
                // stray follow key abandons dir's own two-key leaders.
                TableMatch::Miss | TableMatch::Prefix => VimKey::Pending,
            };
        }
        match table::lookup(CHORDS, std::slice::from_ref(key)) {
            TableMatch::Exact(motion) => return VimKey::Motion(motion),
            TableMatch::Prefix => {
                self.pending = Some(*key);
                return VimKey::Pending;
            }
            TableMatch::Miss if key.ctrl => return VimKey::Unhandled,
            TableMatch::Miss => {}
        }
        match bare_motion(key) {
            Some(motion) => VimKey::Motion(motion),
            None => VimKey::Unhandled,
        }
    }

    /// Every key the layer answers, for a surface's help list.
    pub fn help() -> Vec<KeyHelp> {
        let mut rows = table::help(MOTIONS);
        rows.extend(table::help(CHORDS));
        rows.extend(table::help(BUFFER_ENDS));
        rows
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Emacs' `M-<` and `M->` — beginning and end of the buffer — as the two Alt
/// chords every surface answers to beside its Vim motions, since the tools
/// are read with both sets of habits and `gg`/`G` are the only way there
/// otherwise.
///
/// Both spellings of each land here: a layout that reports the shifted
/// punctuation (`<`, `>`) and one that reports the unshifted key with a shift
/// flag (`,`, `.`) are the same chord under the fingers.
pub fn buffer_end(key: &Key) -> Option<CursorMove> {
    // `M-<` is Alt with Shift held: `M-,` is a different chord.
    if key.mods() != Mods::ALT_SHIFT {
        return None;
    }
    let code = match key.glyph() {
        KeyCode::Char('<') => KeyCode::Char(','),
        KeyCode::Char('>') => KeyCode::Char('.'),
        code => code,
    };
    table::find(BUFFER_ENDS, &Key { code, ..*key })
}

/// The default key-to-motion mapping every surface shares, factored out of
/// the terminal's Normal mode so a motion key resolves identically over the
/// grid, the listing, and the git view. Matches on the key's code alone, as
/// the terminal's own resolver does; the caller owns any modifier policy.
pub fn bare_motion(key: &Key) -> Option<CursorMove> {
    let code_alone = Key {
        alt: false,
        code: key.glyph(),
        ctrl: false,
        shift: false,
        sup: false,
    };
    table::find(MOTIONS, &code_alone)
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
            sup: false,
        }
    }

    fn ctrl(c: char) -> Key {
        Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: false,
            sup: false,
        }
    }

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
            sup: false,
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
    fn test_the_tool_chords_and_the_window_chords_fall_through() {
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

    #[test]
    fn test_a_paging_chord_with_an_extra_modifier_is_declined() {
        // `C-S-d` is the window's `dir_page`, not a half page down.
        let mut nav = VimNav::new();
        for code in ['d', 'u', 'f', 'b'] {
            let shifted = Key {
                shift: true,
                sup: false,
                ..ctrl(code)
            };
            let alted = Key {
                alt: true,
                ..ctrl(code)
            };
            assert_eq!(nav.key(&shifted), VimKey::Unhandled, "ctrl-shift-{code}");
            assert_eq!(nav.key(&alted), VimKey::Unhandled, "ctrl-alt-{code}");
        }
        let ctrl_alt_end = Key {
            ctrl: true,
            ..alt('>')
        };
        assert_eq!(buffer_end(&ctrl_alt_end), None);
    }

    #[test]
    fn test_every_motion_key_is_spelled_right() {
        for keys in [MOTIONS, CHORDS, BUFFER_ENDS] {
            assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
        }
    }

    #[test]
    fn test_the_emacs_buffer_ends_resolve_however_the_layout_spells_them() {
        // `M-<` and `M->` reach the top and the bottom whether the layout
        // reports the shifted punctuation or the unshifted key with a shift
        // flag, and the two chords are claimed from the Alt keys the layer
        // otherwise leaves to the window.
        let shifted = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
            sup: false,
        };
        assert_eq!(
            buffer_end(&Key {
                shift: true,
                ..alt('<')
            }),
            Some(CursorMove::Top)
        );
        assert_eq!(
            buffer_end(&Key {
                shift: true,
                ..alt('>')
            }),
            Some(CursorMove::Bottom)
        );
        assert_eq!(buffer_end(&shifted(',')), Some(CursorMove::Top));
        assert_eq!(buffer_end(&shifted('.')), Some(CursorMove::Bottom));

        // Without Alt they are ordinary text, and an unshifted comma or a bare
        // `M-<` without its Shift are keys of their own.
        assert_eq!(buffer_end(&alt('<')), None);
        assert_eq!(buffer_end(&plain(KeyCode::Char('<'))), None);
        assert_eq!(buffer_end(&alt(',')), None);

        let mut nav = VimNav::new();
        assert_eq!(
            nav.key(&Key {
                shift: true,
                ..alt('>')
            }),
            VimKey::Motion(CursorMove::Bottom)
        );
    }
}
