//! Keys as the window reports them: what was pressed, which modifiers were
//! held, and the canonical form every handler matches.

use super::chord;

// ========================================================================
// Data Structures
// ========================================================================

/// A decoded key event from the windowing layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    /// Whether Alt was held.
    pub alt: bool,
    /// The physical key, independent of modifiers.
    pub code: KeyCode,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
    /// Whether Super (the logo or Windows key) was held.
    pub sup: bool,
}

/// The modifiers held with a key, compared as a whole.
///
/// A shortcut is the exact set of modifiers it names: `C-d` is not
/// `C-S-d`, and neither is `C-M-d`. Handlers match [`Key::chord`] against
/// these constants so a chord with an extra modifier is declined instead of
/// read as the plainer one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mods {
    /// Whether Alt was held.
    pub alt: bool,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
    /// Whether Super was held.
    pub sup: bool,
}

impl Mods {
    /// Alt alone.
    pub const ALT: Mods = Mods {
        alt: true,
        ctrl: false,
        shift: false,
        sup: false,
    };
    /// Alt and Shift together.
    pub const ALT_SHIFT: Mods = Mods {
        alt: true,
        ctrl: false,
        shift: true,
        sup: false,
    };
    /// Ctrl alone.
    pub const CTRL: Mods = Mods {
        alt: false,
        ctrl: true,
        shift: false,
        sup: false,
    };
    /// Ctrl and Alt together.
    pub const CTRL_ALT: Mods = Mods {
        alt: true,
        ctrl: true,
        shift: false,
        sup: false,
    };
    /// Ctrl and Shift together.
    pub const CTRL_SHIFT: Mods = Mods {
        alt: false,
        ctrl: true,
        shift: true,
        sup: false,
    };
    /// Ctrl, Shift and Alt together.
    pub const CTRL_SHIFT_ALT: Mods = Mods {
        alt: true,
        ctrl: true,
        shift: true,
        sup: false,
    };
    /// No modifier.
    pub const NONE: Mods = Mods {
        alt: false,
        ctrl: false,
        shift: false,
        sup: false,
    };
    /// Shift alone.
    pub const SHIFT: Mods = Mods {
        alt: false,
        ctrl: false,
        shift: true,
        sup: false,
    };
}

/// A physical key, independent of modifiers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyCode {
    /// The Backspace key.
    Backspace,
    /// A printable character.
    Char(char),
    /// The Delete key.
    Delete,
    /// The Down arrow.
    Down,
    /// The End key.
    End,
    /// The Enter key.
    Enter,
    /// The Escape key.
    Escape,
    /// A function key, numbered from 1.
    F(u8),
    /// The Home key.
    Home,
    /// The Insert key.
    Insert,
    /// The Left arrow.
    Left,
    /// The Page Down key.
    PageDown,
    /// The Page Up key.
    PageUp,
    /// The Right arrow.
    Right,
    /// The space bar.
    Space,
    /// The Tab key.
    Tab,
    /// The Up arrow.
    Up,
}

// ========================================================================
// Key
// ========================================================================

impl Key {
    /// The modifiers held, as the one set a shortcut is matched against.
    pub fn mods(&self) -> Mods {
        Mods {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            sup: self.sup,
        }
    }

    /// Whether this is a key press winit could not name, such as a bare Ctrl
    /// or Shift going down, which arrives as the NUL character. It types
    /// nothing, and text entry that inserted it would show a stray glyph.
    pub fn is_unidentified(&self) -> bool {
        self.code == KeyCode::Char('\0')
    }

    /// This key with an unmodified space bar read as the space it types.
    ///
    /// The space bar has a code of its own, so text entry that matches
    /// `KeyCode::Char` alone never sees it; this lets that entry treat it as
    /// the character it is. A chord with Ctrl or Alt held is not typing, and
    /// stays as it arrived.
    pub fn typed(&self) -> Self {
        match (self.glyph(), !self.ctrl && !self.alt && !self.sup) {
            (KeyCode::Space, true) => Self {
                code: KeyCode::Char(' '),
                ..*self
            },
            (code, _) => Self { code, ..*self },
        }
    }

    /// This key in the one form every handler matches: a letter is the
    /// lowercase letter, and whether Shift was held is the `shift` flag alone.
    /// A shifted symbol is shifted however it was reached.
    ///
    /// The window reports the character Shift produces, so `S-s` arrives
    /// as `Char('S')` and `C-S-c` as `Char('C')`, while a layout or a
    /// Caps Lock may report either. Folding them all to `Char('s')` with
    /// `shift` set makes `s` and `S-s` two plainly different keys, and
    /// lets a binding written `S+s` match whichever way the key arrived. Use
    /// [`glyph`](Self::glyph) for the character a key types.
    pub fn canonical(&self) -> Self {
        match self.code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => Self {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                shift: true,
                ..*self
            },
            // A symbol that is itself the shifted glyph (`?`, `+`, `(`) is
            // shifted wherever it came from: the shifted key, a layout with it
            // unshifted, the keypad, or a chord written `M-?`.
            KeyCode::Char(c) if chord::is_shifted_symbol(c) => Self {
                shift: true,
                ..*self
            },
            _ => *self,
        }
    }

    /// Whether Shift is held on a key that Shift gives no meaning of its own
    /// (`S-RET`, `S-ESC`, `S-<down>`, ...). A handler that binds
    /// the plain key has not bound this one, so it is left for the window to
    /// match rather than read as the unshifted key. Letters, symbols, `Tab`,
    /// `Space`, `Backspace`, and the function keys all take Shift.
    pub fn is_shifted_plain_key(&self) -> bool {
        self.shift
            && !self.ctrl
            && !self.alt
            && !self.sup
            && matches!(
                self.code,
                KeyCode::Delete
                    | KeyCode::Down
                    | KeyCode::End
                    | KeyCode::Enter
                    | KeyCode::Escape
                    | KeyCode::Home
                    | KeyCode::Insert
                    | KeyCode::Left
                    | KeyCode::PageDown
                    | KeyCode::PageUp
                    | KeyCode::Right
                    | KeyCode::Up
            )
    }

    /// The key as one exact chord, the only way a handler should match it:
    /// every modifier held, and the code. Shift held alone on a character
    /// is the glyph's own, what typed the capital or the symbol, on any
    /// layout (`G` is `(Mods::NONE, Char('G'))`, `?` and `Ä` likewise), and
    /// is kept everywhere else (`C-S-f` is `(Mods::CTRL_SHIFT, Char('f'))`,
    /// `S-<down>` is `(Mods::SHIFT, Down)`). A chord with a modifier more
    /// than a handler names is a different chord, and matches nothing it
    /// does not name.
    pub fn chord(&self) -> (Mods, KeyCode) {
        let key = self.canonical();
        let typed_by_shift = key.mods() == Mods::SHIFT && matches!(key.code, KeyCode::Char(_));
        match typed_by_shift {
            true => (Mods::NONE, key.glyph()),
            false => (key.mods(), key.glyph()),
        }
    }

    /// The character the key types, the space bar's included: a key held
    /// with no modifier but the Shift that typed it. `None` for a chord, or
    /// a key that types nothing.
    pub fn typed_char(&self) -> Option<char> {
        match self.chord() {
            (Mods::NONE, KeyCode::Char(c)) => Some(c),
            (Mods::NONE | Mods::SHIFT, KeyCode::Space) => Some(' '),
            _ => None,
        }
    }

    /// The code this key types: a letter pressed with Shift alone is the
    /// capital, every other key is its code as it stands. The inverse of the
    /// letter folding in [`canonical`](Self::canonical) for keys that type.
    pub fn glyph(&self) -> KeyCode {
        match self.code {
            KeyCode::Char(c) if self.shift && !self.ctrl && !self.alt && !self.sup => {
                KeyCode::Char(c.to_ascii_uppercase())
            }
            code => code,
        }
    }

    /// `code` with no modifier held, which for a letter is the letter as
    /// typed: `Key::plain(KeyCode::Char('S'))` is what S-s arrives as.
    pub fn plain(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    /// `code` with Alt held.
    pub fn with_alt(code: KeyCode) -> Self {
        Self {
            alt: true,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    /// `code` with Ctrl held.
    pub fn with_ctrl(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: true,
            shift: false,
            sup: false,
        }
    }

    /// `code` with both Ctrl and Shift held.
    pub fn with_ctrl_shift(code: KeyCode) -> Self {
        Self {
            alt: false,
            code,
            ctrl: true,
            shift: true,
            sup: false,
        }
    }
}
