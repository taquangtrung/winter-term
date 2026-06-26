//! A cap on OSC strings (`ESC ] ... BEL`), applied to the bytes on their way
//! into the vte parser. vte keeps an OSC string's payload in a buffer with no
//! limit, so a program printing `ESC ]` and then never ending it, or one huge
//! title or clipboard write, could grow that buffer until memory runs out.

// ========================================================================
// Constants
// ========================================================================

/// The longest OSC string let through, in bytes. Large enough for the
/// largest real ones (an inline image sent through OSC 1337, a big OSC 52
/// clipboard write); past it the string is cancelled.
pub(super) const MAX_OSC_BYTES: usize = 16 * 1024 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// CAN: cancels the escape sequence in progress, OSC strings included.
pub(super) const CAN: u8 = 0x18;
/// SUB: cancels like CAN.
const SUB: u8 = 0x1a;
/// The byte after ESC that opens an OSC string.
const OSC_INTRODUCER: u8 = b']';

// ========================================================================
// Data Structures
// ========================================================================

/// Where the guard is in the byte stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    /// Outside any OSC string, after a byte that was not ESC.
    #[default]
    Ground,
    /// Just after an ESC, which may open an OSC string.
    Escape,
    /// Inside an OSC string, this many bytes into it.
    Osc(usize),
    /// Inside an OSC string already cancelled: its remaining bytes are dropped.
    Discard,
}

/// What to do with a byte on its way to the parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OscVerdict {
    /// Hand the byte to the parser as it is.
    Forward,
    /// The OSC string ran past [`MAX_OSC_BYTES`]: hand the parser [`CAN`]
    /// instead of the byte, ending the string.
    Cancel,
    /// Part of a cancelled string: the parser never sees it.
    Drop,
}

/// Tracks OSC strings in the stream and says when one has run too long.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct OscGuard {
    state: State,
}

// ========================================================================
// OscGuard
// ========================================================================

impl OscGuard {
    /// Judge `byte`, the next byte bound for the parser.
    pub(super) fn judge(&mut self, byte: u8) -> OscVerdict {
        match self.state {
            State::Ground => {
                self.state = Self::after_plain(byte);
                OscVerdict::Forward
            }
            State::Escape => {
                self.state = match byte {
                    OSC_INTRODUCER => State::Osc(0),
                    other => Self::after_plain(other),
                };
                OscVerdict::Forward
            }
            State::Osc(len) => {
                if Self::ends_string(byte) {
                    self.state = Self::after_plain(byte);
                    return OscVerdict::Forward;
                }
                if len + 1 > MAX_OSC_BYTES {
                    self.state = State::Discard;
                    return OscVerdict::Cancel;
                }
                self.state = State::Osc(len + 1);
                OscVerdict::Forward
            }
            State::Discard => match byte {
                // An ESC ends the cancelled string and may begin the next
                // sequence, so the parser has to see it.
                ESC => {
                    self.state = State::Escape;
                    OscVerdict::Forward
                }
                BEL | CAN | SUB => {
                    self.state = State::Ground;
                    OscVerdict::Drop
                }
                _ => OscVerdict::Drop,
            },
        }
    }

    /// The state after a byte outside any OSC string.
    fn after_plain(byte: u8) -> State {
        match byte {
            ESC => State::Escape,
            _ => State::Ground,
        }
    }

    /// Whether `byte` ends an OSC string: BEL, the ESC of ST (or of any
    /// sequence that cuts the string short), and the cancels.
    fn ends_string(byte: u8) -> bool {
        matches!(byte, BEL | ESC | CAN | SUB)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// What the parser would be handed for `bytes`.
    fn forwarded(guard: &mut OscGuard, bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .filter_map(|&byte| match guard.judge(byte) {
                OscVerdict::Forward => Some(byte),
                OscVerdict::Cancel => Some(CAN),
                OscVerdict::Drop => None,
            })
            .collect()
    }

    #[test]
    fn test_an_osc_string_within_the_cap_passes_untouched() {
        let mut guard = OscGuard::default();
        let title = b"\x1b]2;my title\x07after";
        assert_eq!(forwarded(&mut guard, title), title.to_vec());
    }

    #[test]
    fn test_an_endless_osc_string_is_cancelled_at_the_cap_and_the_rest_dropped() {
        let mut guard = OscGuard::default();
        let mut stream = b"\x1b]0;".to_vec();
        stream.extend(std::iter::repeat_n(b'x', MAX_OSC_BYTES + 100));
        stream.extend_from_slice(b"\x07ok");
        let out = forwarded(&mut guard, &stream);
        // ESC ] then the cap's worth, then CAN; the overflow and its BEL are
        // dropped, and what follows the string comes through.
        assert_eq!(out.len(), 2 + MAX_OSC_BYTES + 1 + 2);
        assert_eq!(out[2 + MAX_OSC_BYTES], CAN);
        assert!(out.ends_with(b"ok"));
    }

    #[test]
    fn test_an_esc_after_a_cancelled_string_still_starts_the_next_sequence() {
        let mut guard = OscGuard {
            state: State::Discard,
        };
        assert_eq!(forwarded(&mut guard, b"junk\x1b[1m"), b"\x1b[1m".to_vec());
    }
}
