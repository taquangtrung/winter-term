//! Character set designation and shifting (ISO 2022 as VT100s do it): the G0
//! and G1 slots, `ESC ( F` / `ESC ) F` to designate them, and SO/SI to pick
//! which one prints. The only non-ASCII set in practical use is DEC Special
//! Graphics, which ncurses switches to (`smacs`, `ESC ( 0`) to draw borders
//! from plain letters: `lqqk` is `┌──┐`.

use super::Grid;

// ========================================================================
// Constants
// ========================================================================

/// The first character DEC Special Graphics replaces (`_`).
const DEC_GRAPHICS_FIRST: char = '\u{5f}';
/// What DEC Special Graphics draws for `_` through `~`, in order.
const DEC_GRAPHICS: [char; 32] = [
    ' ', '◆', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─',
    '⎼', '⎽', '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£', '·',
];
/// Final byte designating DEC Special Graphics (`ESC ( 0`).
const DESIGNATOR_DEC_GRAPHICS: u8 = b'0';
/// Final byte designating US ASCII (`ESC ( B`).
const DESIGNATOR_ASCII: u8 = b'B';
/// Intermediate byte addressing the G0 slot.
const SLOT_G0: u8 = b'(';
/// Intermediate byte addressing the G1 slot.
const SLOT_G1: u8 = b')';
const ESC: char = '\x1b';
/// Shift Out: print from G1.
const SHIFT_OUT: char = '\x0e';
/// Shift In: print from G0.
const SHIFT_IN: char = '\x0f';

// ========================================================================
// Data Structures
// ========================================================================

/// A 94-character set a slot can hold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Charset {
    /// US ASCII, printed as is.
    #[default]
    Ascii,
    /// DEC Special Graphics: `_` through `~` become line-drawing glyphs.
    DecSpecialGraphics,
}

/// One of the two slots a set is designated into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CharsetSlot {
    /// Printed from by default, and after SI.
    G0,
    /// Printed from after SO.
    G1,
}

/// The sets designated into G0 and G1, and which of them prints.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Charsets {
    g0: Charset,
    g1: Charset,
    /// Whether SO is in effect, so G1 prints.
    shifted_out: bool,
}

// ========================================================================
// Charset
// ========================================================================

impl Charset {
    /// The set designated by final byte `byte`. National replacement sets
    /// (UK, German, ...) are read as ASCII: they swap a handful of
    /// punctuation glyphs no current program relies on.
    pub fn from_designator(byte: u8) -> Self {
        match byte {
            DESIGNATOR_DEC_GRAPHICS => Self::DecSpecialGraphics,
            _ => Self::Ascii,
        }
    }

    fn designator(self) -> u8 {
        match self {
            Self::Ascii => DESIGNATOR_ASCII,
            Self::DecSpecialGraphics => DESIGNATOR_DEC_GRAPHICS,
        }
    }

    fn translate(self, ch: char) -> char {
        match self {
            Self::Ascii => ch,
            Self::DecSpecialGraphics => (ch as u32)
                .checked_sub(DEC_GRAPHICS_FIRST as u32)
                .and_then(|offset| DEC_GRAPHICS.get(offset as usize).copied())
                .unwrap_or(ch),
        }
    }
}

// ========================================================================
// CharsetSlot
// ========================================================================

impl CharsetSlot {
    /// The slot an `ESC <intermediate> F` designation addresses, or `None`
    /// for an intermediate that is not a G0/G1 designation.
    pub fn from_intermediate(byte: u8) -> Option<Self> {
        match byte {
            SLOT_G0 => Some(Self::G0),
            SLOT_G1 => Some(Self::G1),
            _ => None,
        }
    }

    fn intermediate(self) -> char {
        match self {
            Self::G0 => SLOT_G0 as char,
            Self::G1 => SLOT_G1 as char,
        }
    }
}

// ========================================================================
// Charsets
// ========================================================================

impl Charsets {
    /// The glyph `ch` prints as through the active slot.
    pub(super) fn translate(&self, ch: char) -> char {
        let active = if self.shifted_out { self.g1 } else { self.g0 };
        active.translate(ch)
    }

    /// The escape sequences that put a fresh terminal's charsets into this
    /// state, or an empty string when it is the default.
    pub(super) fn restore_sequence(&self) -> String {
        if *self == Self::default() {
            String::new()
        } else {
            self.explicit_sequence()
        }
    }

    /// The escape sequences that put any terminal's charsets into this
    /// state, whatever they were before: both designations and the shift.
    pub(super) fn explicit_sequence(&self) -> String {
        let mut out = String::new();
        for (slot, set) in [(CharsetSlot::G0, self.g0), (CharsetSlot::G1, self.g1)] {
            out.push(ESC);
            out.push(slot.intermediate());
            out.push(set.designator() as char);
        }
        out.push(if self.shifted_out {
            SHIFT_OUT
        } else {
            SHIFT_IN
        });
        out
    }
}

// ========================================================================
// Grid: character sets
// ========================================================================

impl Grid {
    /// Designate `set` into `slot` (SCS, `ESC ( F` / `ESC ) F`).
    pub fn designate_charset(&mut self, slot: CharsetSlot, set: Charset) {
        match slot {
            CharsetSlot::G0 => self.charsets.g0 = set,
            CharsetSlot::G1 => self.charsets.g1 = set,
        }
    }

    /// Print from G1 (SO, `true`) or from G0 (SI, `false`).
    pub fn shift_charset(&mut self, shifted_out: bool) {
        self.charsets.shifted_out = shifted_out;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dec_graphics_draws_borders_and_passes_other_characters_through() {
        let set = Charset::DecSpecialGraphics;
        let border: String = "lqkxmj".chars().map(|ch| set.translate(ch)).collect();
        assert_eq!(border, "┌─┐│└┘");
        assert_eq!(set.translate('~'), '·', "the last mapped character");
        assert_eq!(set.translate('A'), 'A', "below the mapped range");
        assert_eq!(set.translate('é'), 'é', "above the mapped range");
    }

    #[test]
    fn test_restore_sequence_rebuilds_the_same_state() {
        let charsets = Charsets {
            g0: Charset::DecSpecialGraphics,
            g1: Charset::Ascii,
            shifted_out: true,
        };
        assert_eq!(charsets.restore_sequence(), "\x1b(0\x1b)B\x0e");
        assert_eq!(Charsets::default().restore_sequence(), "");
        assert_eq!(Charsets::default().explicit_sequence(), "\x1b(B\x1b)B\x0f");
    }
}
