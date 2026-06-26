//! The bytes of a program's output on their way into the vte parser.
//!
//! Two things are taken out of the stream before vte sees it. APC strings
//! (`ESC _ ... ST`) are lifted out whole, since vte drops them and the Kitty
//! graphics protocol lives in them; and an OSC string that runs past its cap is
//! cut off (see [`OscGuard`]). Everything else passes through untouched, and
//! is handed on as runs of the caller's own bytes rather than one byte at a
//! time, so vte parses printable text in bulk.

use std::ops::Range;

use super::osc_guard::{OscGuard, OscVerdict, CAN};

// ========================================================================
// Constants
// ========================================================================

/// The most payload one APC string may carry. Past it the string is dropped
/// to its end, so an unterminated or oversized one cannot grow memory.
pub(super) const APC_MAX_PAYLOAD: usize = 4 * 1024 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// The 8-bit String Terminator.
const ST_C1: u8 = 0x9c;
/// The byte after ESC that opens an APC string.
const APC_INTRODUCER: u8 = b'_';
/// The byte after ESC that makes it the 7-bit String Terminator.
const ST_FINAL: u8 = b'\\';
/// The first byte of an APC payload that is a Kitty graphics command.
const KITTY_GRAPHICS: u8 = b'G';

/// The bytes handed on where the stream's own cannot be: an ESC held back to
/// see whether it opened an APC string, and the CAN that cuts off an OSC one.
const ESC_BYTES: &[u8] = &[ESC];
const CAN_BYTES: &[u8] = &[CAN];

// ========================================================================
// Data Structures
// ========================================================================

/// One piece of the filtered stream, in stream order.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Piece<'a> {
    /// Bytes for the parser.
    Bytes(&'a [u8]),
    /// A Kitty graphics command: the payload of `APC G ... ST`, after the `G`.
    KittyGraphics(Vec<u8>),
}

/// Lifts APC strings out of the stream and caps OSC strings in it.
#[derive(Debug, Default)]
pub(super) struct ByteFilter {
    /// The payload of the APC string being read.
    apc: Vec<u8>,
    osc: OscGuard,
    state: State,
}

/// Where the filter is in the stream, as far as APC strings go.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    /// Inside an APC string.
    Apc,
    /// Just after an ESC inside an APC string, which may end it.
    ApcEscape,
    /// Inside an APC string past [`APC_MAX_PAYLOAD`]: dropped to its end.
    Discard,
    /// Just after an ESC inside a dropped APC string.
    DiscardEscape,
    /// Just after an ESC outside any APC string, held back until the next
    /// byte says whether it opens one.
    Escape,
    /// Outside any APC string.
    #[default]
    Ground,
}

/// Finds a fixed escape sequence in a stream that arrives in pieces, so a
/// sequence split across two reads is still found.
#[derive(Clone, Copy, Debug)]
pub(super) struct SequenceMatcher {
    /// How many of `sequence`'s bytes the stream has ended on so far.
    matched: usize,
    sequence: &'static [u8],
}

// ========================================================================
// ByteFilter
// ========================================================================

impl ByteFilter {
    /// Filter `bytes`, handing each piece of what comes out to `sink` in
    /// order. A sequence cut short at the end of `bytes` is carried on to the
    /// next call.
    pub(super) fn feed(&mut self, bytes: &[u8], sink: &mut impl FnMut(Piece<'_>)) {
        let mut run: Option<Range<usize>> = None;
        // Where in `bytes` the ESC being held back sits, when it is in them
        // rather than at the end of an earlier call.
        let mut held_esc_at: Option<usize> = None;
        for (at, &byte) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => match byte {
                    ESC => {
                        self.state = State::Escape;
                        held_esc_at = Some(at);
                    }
                    _ => self.forward(bytes, at, &mut run, sink),
                },
                State::Escape if byte == APC_INTRODUCER => {
                    self.state = State::Apc;
                    self.apc.clear();
                }
                State::Escape => {
                    // The held ESC opened something else: it goes on first.
                    match held_esc_at {
                        Some(esc) if esc + 1 == at => self.forward(bytes, esc, &mut run, sink),
                        Some(_) | None => {
                            flush(bytes, &mut run, sink);
                            // An ESC always ends what the guard is in, so it
                            // always passes.
                            self.osc.judge(ESC);
                            sink(Piece::Bytes(ESC_BYTES));
                        }
                    }
                    match byte {
                        ESC => held_esc_at = Some(at),
                        _ => {
                            self.state = State::Ground;
                            self.forward(bytes, at, &mut run, sink);
                        }
                    }
                }
                State::Apc => match byte {
                    ESC => self.state = State::ApcEscape,
                    BEL | ST_C1 => self.finish_apc(bytes, &mut run, sink),
                    _ => self.push_apc(&[byte]),
                },
                State::ApcEscape => match byte {
                    ST_FINAL => self.finish_apc(bytes, &mut run, sink),
                    // Another ESC may yet be the one that ends the string.
                    ESC => self.push_apc(ESC_BYTES),
                    _ => {
                        self.state = State::Apc;
                        self.push_apc(&[ESC, byte]);
                    }
                },
                State::Discard => match byte {
                    ESC => self.state = State::DiscardEscape,
                    BEL | ST_C1 => self.state = State::Ground,
                    _ => {}
                },
                State::DiscardEscape => match byte {
                    ST_FINAL => self.state = State::Ground,
                    ESC => {}
                    _ => self.state = State::Discard,
                },
            }
        }
        flush(bytes, &mut run, sink);
    }

    /// Pass the byte at `at` on through the OSC guard: into the run of
    /// stream bytes being built, as a CAN, or not at all.
    fn forward(
        &mut self,
        bytes: &[u8],
        at: usize,
        run: &mut Option<Range<usize>>,
        sink: &mut impl FnMut(Piece<'_>),
    ) {
        match self.osc.judge(bytes[at]) {
            OscVerdict::Forward => match run {
                Some(range) if range.end == at => range.end = at + 1,
                Some(_) | None => {
                    flush(bytes, run, sink);
                    *run = Some(at..at + 1);
                }
            },
            OscVerdict::Cancel => {
                flush(bytes, run, sink);
                sink(Piece::Bytes(CAN_BYTES));
            }
            OscVerdict::Drop => {}
        }
    }

    /// Add `bytes` to the APC payload, or give the string up once it is too
    /// long, dropping the rest of it.
    fn push_apc(&mut self, bytes: &[u8]) {
        if self.apc.len() + bytes.len() > APC_MAX_PAYLOAD {
            self.apc = Vec::new();
            self.state = match self.state {
                State::ApcEscape => State::DiscardEscape,
                _ => State::Discard,
            };
            return;
        }
        self.apc.extend_from_slice(bytes);
    }

    /// End the APC string, handing it on if it is a Kitty graphics command.
    /// What came before it in the stream goes on first.
    fn finish_apc(
        &mut self,
        bytes: &[u8],
        run: &mut Option<Range<usize>>,
        sink: &mut impl FnMut(Piece<'_>),
    ) {
        self.state = State::Ground;
        let mut payload = std::mem::take(&mut self.apc);
        if payload.first() == Some(&KITTY_GRAPHICS) {
            flush(bytes, run, sink);
            payload.remove(0);
            sink(Piece::KittyGraphics(payload));
        }
    }
}

/// Hand the run of stream bytes built so far on, if there is one.
fn flush(bytes: &[u8], run: &mut Option<Range<usize>>, sink: &mut impl FnMut(Piece<'_>)) {
    if let Some(range) = run.take() {
        sink(Piece::Bytes(&bytes[range]));
    }
}

// ========================================================================
// SequenceMatcher
// ========================================================================

impl SequenceMatcher {
    /// A matcher for `sequence`, whose first byte must appear nowhere else in
    /// it (true of every escape sequence: only its first byte is ESC).
    pub(super) const fn new(sequence: &'static [u8]) -> Self {
        Self {
            matched: 0,
            sequence,
        }
    }

    /// Where in `bytes` the sequence next ends, as the index just past its
    /// last byte, counting what earlier calls ended on.
    pub(super) fn find_end(&mut self, bytes: &[u8]) -> Option<usize> {
        for (at, &byte) in bytes.iter().enumerate() {
            if byte == self.sequence[self.matched] {
                self.matched += 1;
                if self.matched == self.sequence.len() {
                    self.matched = 0;
                    return Some(at + 1);
                }
            } else {
                self.matched = usize::from(byte == self.sequence[0]);
            }
        }
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// What the parser and the graphics handler would be given for each of
    /// `reads`, fed in turn: the parser's bytes joined, and each command.
    fn filtered(reads: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>, usize) {
        let mut filter = ByteFilter::default();
        let mut parsed = Vec::new();
        let mut commands = Vec::new();
        let mut pieces = 0;
        for read in reads {
            filter.feed(read, &mut |piece| {
                pieces += 1;
                match piece {
                    Piece::Bytes(bytes) => parsed.extend_from_slice(bytes),
                    Piece::KittyGraphics(payload) => commands.push(payload),
                }
            });
        }
        (parsed, commands, pieces)
    }

    #[test]
    fn test_plain_output_and_escapes_reach_the_parser_as_one_run() {
        let text: &[u8] = b"ls\r\n\x1b[1;31mred\x1b[0m done";
        let (parsed, commands, pieces) = filtered(&[text]);
        assert_eq!(parsed, text);
        assert!(commands.is_empty());
        assert_eq!(pieces, 1, "a read with no APC is not cut up");
    }

    #[test]
    fn test_a_graphics_command_is_lifted_out_between_the_text_around_it() {
        let (parsed, commands, _) = filtered(&[b"before\x1b_Ga=T;AAAA\x1b\\after"]);
        assert_eq!(parsed, b"beforeafter");
        assert_eq!(commands, vec![b"a=T;AAAA".to_vec()]);
    }

    #[test]
    fn test_an_escape_split_across_reads_is_still_read_whole() {
        let (parsed, commands, _) = filtered(&[b"a\x1b", b"_Gm=0\x1b", b"\\b\x1b", b"[1m"]);
        assert_eq!(parsed, b"ab\x1b[1m");
        assert_eq!(commands, vec![b"m=0".to_vec()]);
    }

    #[test]
    fn test_a_stray_esc_before_an_apc_does_not_hide_it() {
        let (parsed, commands, _) = filtered(&[b"\x1b\x1b_Gi=1\x1b\\"]);
        assert_eq!(parsed, b"\x1b");
        assert_eq!(commands, vec![b"i=1".to_vec()]);
    }

    #[test]
    fn test_an_oversized_apc_is_dropped_to_its_end_not_printed() {
        // Regression: past the cap the rest of the payload went to the
        // parser, and printed as text.
        let mut stream = b"\x1b_G".to_vec();
        stream.extend(std::iter::repeat_n(b'A', APC_MAX_PAYLOAD + 10));
        stream.extend_from_slice(b"\x1b\\visible");
        let (parsed, commands, _) = filtered(&[&stream]);
        assert_eq!(parsed, b"visible");
        assert!(commands.is_empty());
    }

    #[test]
    fn test_an_apc_that_is_not_graphics_is_dropped_quietly() {
        let (parsed, commands, _) = filtered(&[b"x\x1b_other\x07y"]);
        assert_eq!(parsed, b"xy");
        assert!(commands.is_empty());
    }

    #[test]
    fn test_a_sequence_is_found_across_reads_and_not_in_lookalikes() {
        let mut matcher = SequenceMatcher::new(b"\x1b[?2026h");
        assert_eq!(matcher.find_end(b"\x1b[?2025h \x1b[?20"), None);
        assert_eq!(matcher.find_end(b"26hrest"), Some(3));
        assert_eq!(matcher.find_end(b"\x1b\x1b[?2026h"), Some(9));
    }
}
