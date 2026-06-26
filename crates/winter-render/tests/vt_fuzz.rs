//! Adversarial VT stream fuzzing for [`Screen`] and [`Grid`].
//!
//! The escape parser is the one surface driven entirely by untrusted input:
//! every byte any program writes to a PTY lands here. The existing tests cover
//! sequences someone thought to write down, which by construction excludes the
//! ones that break it.
//!
//! This generates streams from a deterministic PRNG, biased toward shapes that
//! historically break terminals (extreme CSI parameters, inverted scroll
//! regions, truncated OSC, wide characters at the right margin, resizes
//! interleaved with output), and asserts the structural invariants the rest of
//! the crate relies on after every chunk. A failure prints its seed, so any
//! case it finds is reproducible and can be promoted to a named regression.

use winter_render::{Grid, Screen};

// ========================================================================
// Constants
// ========================================================================

/// Streams generated per test: enough to explore the generator's space while
/// staying fast enough to run on every push.
const STREAMS: usize = 2000;

/// Chunks fed per stream. Feeding in chunks rather than one buffer exercises
/// the parser's resumption across call boundaries, where a half-consumed
/// escape has to survive between calls.
const CHUNKS_PER_STREAM: usize = 12;

/// Grid sizes to run against, including the degenerate 1x1 case.
const SIZES: &[(usize, usize)] = &[(1, 1), (2, 3), (80, 24), (13, 7)];

// ========================================================================
// Deterministic PRNG
// ========================================================================

/// xorshift64*, so a failing case reproduces from its seed alone. Not
/// cryptographic and not meant to be; it only has to be stable across runs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Zero is a fixed point for xorshift, so never start there.
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

/// Every structural guarantee the renderer, the app layer, and `Grid`'s own
/// accessors depend on. Checked after each chunk rather than at the end, so a
/// violation is attributed to the chunk that caused it.
fn assert_invariants(grid: &Grid, context: &str) {
    let (cols, rows) = (grid.cols(), grid.rows());
    assert!(
        cols > 0 && rows > 0,
        "{context}: grid collapsed to {cols}x{rows}"
    );

    // The renderer reads the cursor unguarded to place the caret.
    let (row, col) = grid.cursor();
    assert!(row < rows, "{context}: cursor row {row} >= rows {rows}");
    assert!(
        col <= cols,
        "{context}: cursor col {col} > cols {cols} (a deferred wrap parks at cols)"
    );

    // `scroll_up`/`scroll_down` index the region directly.
    let (top, bottom) = (grid.scroll_top(), grid.scroll_bottom());
    assert!(
        top <= bottom,
        "{context}: inverted scroll region {top}..={bottom}"
    );
    assert!(
        bottom < rows,
        "{context}: scroll bottom {bottom} >= rows {rows}"
    );

    // Every visible cell must be addressable: selection, search, and block
    // navigation all walk this rectangle.
    for r in 0..rows {
        for c in 0..cols {
            assert!(
                grid.cell(r, c).is_some(),
                "{context}: missing cell at ({r}, {c}) in {cols}x{rows}"
            );
        }
    }

    // Absolute addressing spans scrollback plus the live grid; the block layer
    // converts between the two constantly.
    let total = grid.scrollback_len() + rows;
    for abs in 0..total {
        assert!(
            grid.absolute_cell(abs, 0).is_some(),
            "{context}: missing absolute cell at row {abs} of {total}"
        );
    }

    // Used for search, yank, and export. Asserted structurally rather than by
    // byte count: a cell carries combining marks in its tail, so one column can
    // legitimately hold an emoji ZWJ sequence of 25-odd bytes. What must hold
    // is the row count, since callers map lines back onto grid rows.
    let text = grid.to_text();
    let line_count = if text.is_empty() {
        0
    } else {
        text.split('\n').count()
    };
    assert!(
        line_count <= rows,
        "{context}: to_text produced {line_count} lines for a {rows}-row grid"
    );
}

// ========================================================================
// Stream generation
// ========================================================================

/// Escape sequences chosen because these shapes have broken terminals before:
/// parameters at and past the `u16`/`u32`/`u64` limits, empty and over-long
/// parameter lists, private modes, and escapes that never terminate.
fn push_escape(out: &mut Vec<u8>, rng: &mut Rng) {
    const PARAMS: &[&str] = &[
        "",
        "0",
        "1",
        "2",
        "7",
        "24",
        "80",
        "999",
        "65535",
        "65536",
        "4294967295",
        "18446744073709551615",
        "-1",
        "1;1",
        "0;0",
        "24;80",
        "80;24",
        "999;999",
        "1;2;3;4;5;6;7;8;9;10",
        ";",
        ";;;;",
        "1;",
        ";1",
    ];
    // CSI finals: cursor motion, erase, scroll region, insert/delete, SGR,
    // device status, and the mode setters.
    const FINALS: &[u8] = b"ABCDEFGHJKLMPSTXdfghlmnrsu@`";
    const MODES: &[&[u8]] = &[
        b"\x1b[?1049h",
        b"\x1b[?1049l",
        b"\x1b[?25l",
        b"\x1b[?25h",
        b"\x1b[?2004h",
        b"\x1b[?1000h",
        b"\x1b[?6h",
        b"\x1b[?7l",
    ];
    const OSC_BODIES: &[&str] = &[
        "0;title",
        "7;file:///tmp",
        "8;;http://x",
        "133;A",
        "9001;emit",
        "52;c;?",
    ];

    match rng.below(10) {
        0..=5 => {
            out.extend_from_slice(b"\x1b[");
            if rng.below(4) == 0 {
                out.push(b'?');
            }
            out.extend_from_slice(rng.pick(PARAMS).as_bytes());
            out.push(*rng.pick(FINALS));
        }
        6 => {
            // Half of these are deliberately unterminated, so the parser has to
            // survive an escape that never closes.
            out.extend_from_slice(b"\x1b]");
            out.extend_from_slice(rng.pick(OSC_BODIES).as_bytes());
            if rng.below(2) == 0 {
                out.extend_from_slice(b"\x1b\\");
            }
        }
        7 => {
            // Bare ESC plus one final: DECSC, DECRC, RI, and several that are
            // not defined at all.
            out.push(0x1b);
            out.push(*rng.pick(b"78MDEHcZ()#%*+"));
        }
        8 => out.extend_from_slice(rng.pick(MODES)),
        _ => out.push(*rng.pick(b"\x07\x08\x09\x0a\x0b\x0c\x0d\x00\x7f")),
    }
}

/// Text bytes: ASCII, multi-byte UTF-8, double-width CJK, emoji with ZWJ and
/// skin-tone modifiers, combining marks, and deliberately invalid UTF-8.
fn push_text(out: &mut Vec<u8>, rng: &mut Rng) {
    const TEXTS: &[&str] = &[
        "a",
        "hello",
        " ",
        "\t",
        "0123456789",
        "ありがとう",
        "🇺🇸",
        "👍🏽",
        "👨‍👩‍👧‍👦",
        "e\u{0301}",
        "\u{200b}",
        "ｆｕｌｌｗｉｄｔｈ",
        "~",
    ];
    const INVALID_UTF8: &[&[u8]] = &[b"\xff", b"\x80", b"\xc3", b"\xe2\x82", b"\xf0\x9f\x92"];

    if rng.below(12) == 0 {
        out.extend_from_slice(rng.pick(INVALID_UTF8));
        return;
    }
    out.extend_from_slice(rng.pick(TEXTS).as_bytes());
}

fn build_chunk(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.below(24) + 1 {
        if rng.below(3) == 0 {
            push_escape(&mut out, rng);
        } else {
            push_text(&mut out, rng);
        }
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_adversarial_streams_keep_the_grid_consistent() {
    for stream in 0..STREAMS {
        let seed = 0x9e37_79b9_7f4a_7c15u64.wrapping_mul(stream as u64 + 1);
        let mut rng = Rng::new(seed);
        let &(cols, rows) = rng.pick(SIZES);
        let mut screen = Screen::new(cols, rows);

        for chunk_index in 0..CHUNKS_PER_STREAM {
            let chunk = build_chunk(&mut rng);
            screen.feed(&chunk);
            assert_invariants(
                screen.grid(),
                &format!("seed {seed:#x} chunk {chunk_index} ({cols}x{rows})"),
            );
        }
    }
}

#[test]
fn test_resize_interleaved_with_output_keeps_the_grid_consistent() {
    // Resize reflows logical lines, rebuilding the cell buffer and the parallel
    // wrap-flag vectors. Doing it mid-stream, while a wrap is pending and the
    // scroll region is non-default, is where those vectors fall out of step.
    for stream in 0..STREAMS {
        let seed = 0xd1b5_4a32_d192_ed03u64.wrapping_mul(stream as u64 + 1);
        let mut rng = Rng::new(seed);
        let mut screen = Screen::new(20, 8);

        for chunk_index in 0..CHUNKS_PER_STREAM {
            screen.feed(&build_chunk(&mut rng));
            let cols = rng.below(40) + 1;
            let rows = rng.below(20) + 1;
            screen.resize(cols, rows);
            assert_invariants(
                screen.grid(),
                &format!("seed {seed:#x} chunk {chunk_index} after resize to {cols}x{rows}"),
            );
        }
    }
}
