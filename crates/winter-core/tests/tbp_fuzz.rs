//! Adversarial stream fuzzing for the block parser and the TBP codec.
//!
//! Companion to `winter-render`'s `vt_fuzz`, which covers the cell grid. This
//! covers the other half of what a PTY byte stream reaches: the OSC 133 block
//! state machine, OSC 7 working directories, OSC 8 hyperlinks, and the OSC 9001
//! TBP payloads, all of which are attacker-controlled and none of which the
//! terminal may crash on.

use winter_core::Terminal;

// ========================================================================
// Constants
// ========================================================================

const STREAMS: usize = 2000;
const CHUNKS_PER_STREAM: usize = 10;

// ========================================================================
// Deterministic PRNG
// ========================================================================

/// xorshift64*, so a failing case reproduces from its seed alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

// ========================================================================
// Invariants
// ========================================================================

/// What every consumer of the block list assumes. The app layer treats a
/// block's index as a stable identifier and walks its segments unguarded.
fn assert_invariants(term: &Terminal, context: &str) {
    let blocks = term.scrollback().blocks();
    assert!(
        !blocks.is_empty(),
        "{context}: the block list must always hold at least one block"
    );

    // These are the accessors the GUI calls every frame; none may panic.
    let _ = term.scrollback().plain_text();
    let _ = term.scrollback().to_json();
    let _ = term.scrollback().block_row_offsets(80);
    let _ = term.scrollback().search("x");

    for (index, block) in blocks.iter().enumerate() {
        // `row_count` divides by the column width; a zero-width call is what
        // a pane collapsed to nothing would produce.
        let _ = block.row_count(0);
        let _ = block.row_count(80);
        let _ = block.plain_text();
        assert!(
            !block.elided || block.output.is_empty(),
            "{context}: block {index} is elided but kept its output"
        );
    }
}

// ========================================================================
// Stream generation
// ========================================================================

fn push_sequence(out: &mut Vec<u8>, rng: &mut Rng) {
    // OSC 133 marks, in and out of order: a stream may open a command without
    // closing it, close one that never opened, or nest them.
    const MARKS: &[&[u8]] = &[
        b"\x1b]133;A\x1b\\",
        b"\x1b]133;B\x1b\\",
        b"\x1b]133;C\x1b\\",
        b"\x1b]133;D;0\x1b\\",
        b"\x1b]133;D;1\x1b\\",
        b"\x1b]133;D;-1\x1b\\",
        b"\x1b]133;D;99999999999999999999\x1b\\",
        b"\x1b]133;D\x1b\\",
        b"\x1b]133;Z\x1b\\",
        b"\x1b]133\x1b\\",
    ];
    // OSC 7 cwd, including percent-escapes that are truncated or invalid.
    const CWDS: &[&[u8]] = &[
        b"\x1b]7;file://host/tmp\x1b\\",
        b"\x1b]7;file://host/My%20Docs\x1b\\",
        b"\x1b]7;file://host/bad%zz\x1b\\",
        b"\x1b]7;file://host/trunc%\x1b\\",
        b"\x1b]7;file://host/%C3\x1b\\",
        b"\x1b]7;\x1b\\",
        b"\x1b]7\x1b\\",
    ];
    // OSC 8 hyperlinks, opened without closing and closed without opening.
    const LINKS: &[&[u8]] = &[
        b"\x1b]8;;http://example.com\x1b\\",
        b"\x1b]8;;\x1b\\",
        b"\x1b]8\x1b\\",
        b"\x1b]8;;;;;\x1b\\",
    ];
    // TBP: malformed base64, malformed JSON, unknown verbs, absent params, and
    // a trust tier the wire is not allowed to be granted.
    const TBP: &[&[u8]] = &[
        b"\x1b]9001;emit;v=1,id=1,trust=trusted;bm90IGpzb24=\x1b\\",
        b"\x1b]9001;emit;v=1,id=1;!!!notbase64!!!\x1b\\",
        b"\x1b]9001;emit\x1b\\",
        b"\x1b]9001;bogusverb;id=1;\x1b\\",
        b"\x1b]9001;open;id=1,mime=text/html;eyJhIjoxfQ==\x1b\\",
        b"\x1b]9001;patch;id=1;W3sib3AiOiJhZGQifV0=\x1b\\",
        b"\x1b]9001;patch;id=999;W10=\x1b\\",
        b"\x1b]9001;close;id=1\x1b\\",
        b"\x1b]9001;close;id=404\x1b\\",
        b"\x1b]9001;emit;v=99,id=1;e30=\x1b\\",
    ];

    match rng.below(6) {
        0 => out.extend_from_slice(rng.pick(MARKS)),
        1 => out.extend_from_slice(rng.pick(CWDS)),
        2 => out.extend_from_slice(rng.pick(LINKS)),
        3 => out.extend_from_slice(rng.pick(TBP)),
        4 => out.extend_from_slice(rng.pick(&[
            b"hello".as_ref(),
            b"\n",
            b"\r\n",
            b"\t",
            "ありがとう".as_bytes(),
            "👨‍👩‍👧‍👦".as_bytes(),
            b"\xff\xfe",
        ])),
        _ => out.push(*rng.pick(b"\x07\x08\x09\x0a\x0d\x00\x1b")),
    }
}

fn build_chunk(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.below(20) + 1 {
        push_sequence(&mut out, rng);
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_adversarial_streams_keep_the_block_list_consistent() {
    for stream in 0..STREAMS {
        let seed = 0xa076_1d64_78bd_642fu64.wrapping_mul(stream as u64 + 1);
        let mut rng = Rng::new(seed);
        let mut term = Terminal::new();

        for chunk_index in 0..CHUNKS_PER_STREAM {
            term.feed(&build_chunk(&mut rng));
            assert_invariants(&term, &format!("seed {seed:#x} chunk {chunk_index}"));
        }
    }
}

#[test]
fn test_a_stream_split_at_every_byte_parses_the_same() {
    // The parser is fed whatever the PTY read returned, so an escape can arrive
    // split across any byte boundary. Feeding one byte at a time must reach the
    // same state as feeding the whole buffer at once.
    for stream in 0..200 {
        let seed = 0x2545_f491_4f6c_dd1du64.wrapping_mul(stream as u64 + 1);
        let mut rng = Rng::new(seed);
        let stream_bytes = build_chunk(&mut rng);

        let mut whole = Terminal::new();
        whole.feed(&stream_bytes);

        let mut split = Terminal::new();
        for byte in &stream_bytes {
            split.feed(&[*byte]);
        }

        assert_eq!(
            whole.scrollback().to_json(),
            split.scrollback().to_json(),
            "seed {seed:#x}: byte-at-a-time parse diverged from the whole-buffer parse"
        );
    }
}
