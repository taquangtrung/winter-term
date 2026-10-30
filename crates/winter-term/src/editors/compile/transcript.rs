//! A build's output as the lines it means, decoded from what the program
//! wrote to its terminal.
//!
//! A compiler run under a pty writes for a screen: a progress line redrawn
//! in place with `\r`, a line wiped with `CSI K`, a status block redrawn by
//! moving the cursor up over it, colors set with SGR. None of that is text a
//! reader wants twice. The transcript plays those moves out over a short
//! window of the newest lines and keeps what is left, so `cargo` printing
//! `Compiling foo` forty times over one line leaves one line, while lines
//! keep their own length whatever the pane's width: a pane folds them when
//! it paints, which a screen grid would already have done for it, wrongly.
//!
//! Lines that have scrolled out of reach of the cursor are frozen into a
//! compact form, and past [`MAX_LINES`] the oldest are let go, so a runaway
//! build costs a bounded amount of memory.

use std::collections::VecDeque;

use vte::{Params, Parser, Perform};
use winter_core::RgbColor;

use crate::model::paint::{TerminalColor, TerminalStyle};

// ========================================================================
// Constants
// ========================================================================

/// How many of the newest lines the cursor can still move back into and
/// rewrite. Progress displays redraw a handful of lines at most; anything
/// further up is history and is frozen.
const LIVE_LINES: usize = 64;

/// The most lines kept. Past this the oldest go, a tenth at a time, so the
/// trimming happens rarely.
pub const MAX_LINES: usize = 200_000;

/// How far apart tab stops are, as a terminal sets them.
const TAB_STOP: usize = 8;

/// The palette slot SGR's bright colors (90-97, 100-107) start at.
const BRIGHT_BASE: u16 = 8;

// ========================================================================
// Data Structures
// ========================================================================

/// Everything a build wrote, as lines.
pub struct Transcript {
    parser: Parser,
    screen: Screen,
}

/// The decoded lines and the cursor the program is writing at.
#[derive(Debug, Default)]
struct Screen {
    /// The column the next character lands in, counted in characters.
    col: usize,
    /// How many lines have been let go from the front.
    dropped: usize,
    /// Lines out of the cursor's reach, oldest first.
    frozen: VecDeque<FrozenLine>,
    /// The newest lines, which the cursor can still rewrite.
    live: VecDeque<Vec<Cell>>,
    /// The live line the cursor is on.
    row: usize,
    /// The attributes the next character is written in.
    style: TerminalStyle,
}

/// One character of a live line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cell {
    ch: char,
    style: TerminalStyle,
}

/// A line no longer in the cursor's reach: its text, and where along it the
/// style changes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct FrozenLine {
    /// Each run's first byte in `text`, and its style, in order.
    runs: Vec<(usize, TerminalStyle)>,
    text: String,
}

/// Where a line is kept.
enum Stored<'a> {
    Frozen(&'a FrozenLine),
    Live(&'a Vec<Cell>),
}

// ========================================================================
// Transcript
// ========================================================================

impl std::fmt::Debug for Transcript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcript")
            .field("lines", &self.len())
            .field("dropped", &self.screen.dropped)
            .finish_non_exhaustive()
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    /// An empty transcript, the cursor at the start of its first line.
    pub fn new() -> Self {
        let mut screen = Screen::default();
        screen.live.push_back(Vec::new());
        Self {
            parser: Parser::new(),
            screen,
        }
    }

    /// Play `bytes` out. A sequence split across two calls is finished by the
    /// second.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    /// How many lines there are, the one being written included.
    pub fn len(&self) -> usize {
        self.screen.frozen.len() + self.screen.live.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 1 && self.screen.live.front().is_some_and(Vec::is_empty)
    }

    /// How many lines have been let go from the front, so a line's index in
    /// the whole output is its index here plus this.
    pub fn dropped(&self) -> usize {
        self.screen.dropped
    }

    /// How many lines are done with: every line the cursor has left by a
    /// newline. The line it is on may still be rewritten.
    pub fn finished(&self) -> usize {
        self.screen.frozen.len() + self.screen.row
    }

    /// Line `index`'s text.
    pub fn text(&self, index: usize) -> String {
        match self.screen.line(index) {
            Some(Stored::Frozen(line)) => line.text.clone(),
            Some(Stored::Live(cells)) => cells.iter().map(|cell| cell.ch).collect(),
            None => String::new(),
        }
    }

    /// Line `index` as runs of text sharing one style, in order.
    pub fn runs(&self, index: usize) -> Vec<(TerminalStyle, String)> {
        match self.screen.line(index) {
            Some(Stored::Frozen(line)) => line
                .runs
                .iter()
                .enumerate()
                .map(|(at, (start, style))| {
                    let end = line.runs.get(at + 1).map_or(line.text.len(), |next| next.0);
                    (*style, line.text[*start..end].to_string())
                })
                .collect(),
            Some(Stored::Live(cells)) => runs_of(cells),
            None => Vec::new(),
        }
    }
}

// ========================================================================
// Screen
// ========================================================================

impl Screen {
    // --------------------------------------------------------------------
    // Lines
    // --------------------------------------------------------------------

    fn line(&self, index: usize) -> Option<Stored<'_>> {
        match index.checked_sub(self.frozen.len()) {
            None => self.frozen.get(index).map(Stored::Frozen),
            Some(live) => self.live.get(live).map(Stored::Live),
        }
    }

    /// The line the cursor is on.
    fn current(&mut self) -> &mut Vec<Cell> {
        &mut self.live[self.row]
    }

    /// Move to the start of the next line, making one if the cursor was on
    /// the last.
    fn newline(&mut self) {
        self.row += 1;
        self.col = 0;
        if self.row == self.live.len() {
            self.live.push_back(Vec::new());
        }
        self.freeze_out_of_reach();
    }

    /// Freeze the lines the cursor can no longer reach, and let the oldest
    /// go once there are too many.
    fn freeze_out_of_reach(&mut self) {
        while self.live.len() > LIVE_LINES && self.row > 0 {
            let Some(cells) = self.live.pop_front() else {
                break;
            };
            self.row -= 1;
            self.frozen.push_back(freeze(&cells));
        }
        if self.frozen.len() + self.live.len() > MAX_LINES {
            let excess = MAX_LINES / 10;
            let gone = excess.min(self.frozen.len());
            self.frozen.drain(..gone);
            self.dropped += gone;
        }
    }

    // --------------------------------------------------------------------
    // Writing
    // --------------------------------------------------------------------

    /// Write `ch` at the cursor and step past it.
    fn put(&mut self, ch: char) {
        let (col, style) = (self.col, self.style);
        let line = self.current();
        if line.len() < col {
            line.resize(col, blank(TerminalStyle::default()));
        }
        let cell = Cell { ch, style };
        match line.get_mut(col) {
            Some(slot) => *slot = cell,
            None => line.push(cell),
        }
        self.col += 1;
    }

    /// `CSI K`: wipe the line after the cursor (`0`), before it (`1`), or
    /// all of it (`2`).
    fn erase_line(&mut self, mode: u16) {
        let col = self.col;
        let line = self.current();
        match mode {
            1 => {
                let end = (col + 1).min(line.len());
                line[..end].fill(blank(TerminalStyle::default()));
            }
            2 => line.clear(),
            _ => line.truncate(col),
        }
        trim_blank_tail(line);
    }

    /// `CSI J`: wipe from the cursor down. Wiping the whole screen is how a
    /// program starts its display afresh, and the output before it is still
    /// the build's, so that is left alone.
    fn erase_below(&mut self, mode: u16) {
        if mode != 0 {
            return;
        }
        self.erase_line(0);
        self.live.truncate(self.row + 1);
    }

    /// Move the cursor `delta` lines, never above the lines it can reach nor
    /// below the last line written.
    fn move_rows(&mut self, delta: isize) {
        let last = self.live.len() - 1;
        self.row = self.row.saturating_add_signed(delta).min(last);
    }

    // --------------------------------------------------------------------
    // SGR
    // --------------------------------------------------------------------

    /// Apply one `CSI ... m`.
    fn select_graphic_rendition(&mut self, params: &Params) {
        let mut flat: Vec<Vec<u16>> = params.iter().map(<[u16]>::to_vec).collect();
        if flat.is_empty() {
            flat.push(vec![0]);
        }
        let mut at = 0;
        while at < flat.len() {
            let group = &flat[at];
            let code = group.first().copied().unwrap_or(0);
            at += 1;
            match code {
                0 => self.style = TerminalStyle::default(),
                1 => self.style.bold = true,
                2 => self.style.faint = true,
                3 => self.style.italic = true,
                4 => self.style.underline = group.get(1).is_none_or(|kind| *kind != 0),
                7 => self.style.inverse = true,
                21 => self.style.underline = true,
                22 => {
                    self.style.bold = false;
                    self.style.faint = false;
                }
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.foreground = Some(TerminalColor::Indexed((code - 30) as u8)),
                38 => self.style.foreground = extended_color(group, &flat, &mut at),
                39 => self.style.foreground = None,
                40..=47 => self.style.background = Some(TerminalColor::Indexed((code - 40) as u8)),
                48 => self.style.background = extended_color(group, &flat, &mut at),
                49 => self.style.background = None,
                90..=97 => {
                    self.style.foreground =
                        Some(TerminalColor::Indexed((code - 90 + BRIGHT_BASE) as u8))
                }
                100..=107 => {
                    self.style.background =
                        Some(TerminalColor::Indexed((code - 100 + BRIGHT_BASE) as u8))
                }
                _ => {}
            }
        }
    }
}

impl Perform for Screen {
    fn print(&mut self, ch: char) {
        self.put(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // A pty turns `\n` into `\r\n` on the way out; a bare one is
            // still the end of a line to a reader.
            b'\n' | 0x0b | 0x0c => self.newline(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => {
                let stop = (self.col / TAB_STOP + 1) * TAB_STOP;
                while self.col < stop {
                    self.put(' ');
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // A private mode (`CSI ? 25 l`, hiding the cursor) says nothing about
        // the text.
        if ignore || !intermediates.is_empty() {
            return;
        }
        let first = params
            .iter()
            .next()
            .and_then(|group| group.first().copied())
            .unwrap_or(0);
        let count = first.max(1) as usize;
        match action {
            'm' => self.select_graphic_rendition(params),
            'K' => self.erase_line(first),
            'J' => self.erase_below(first),
            'A' => self.move_rows(-(count as isize)),
            'B' => self.move_rows(count as isize),
            'C' => self.col += count,
            'D' => self.col = self.col.saturating_sub(count),
            'E' => {
                self.move_rows(count as isize);
                self.col = 0;
            }
            'F' => {
                self.move_rows(-(count as isize));
                self.col = 0;
            }
            'G' => self.col = count - 1,
            _ => {}
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A space in `style`, what a line is padded with.
fn blank(style: TerminalStyle) -> Cell {
    Cell { ch: ' ', style }
}

/// Drop the unstyled spaces an erase left at a line's end.
fn trim_blank_tail(line: &mut Vec<Cell>) {
    while line
        .last()
        .is_some_and(|cell| *cell == blank(TerminalStyle::default()))
    {
        line.pop();
    }
}

/// A live line in its compact form.
fn freeze(cells: &[Cell]) -> FrozenLine {
    let mut frozen = FrozenLine::default();
    for cell in cells {
        if frozen
            .runs
            .last()
            .is_none_or(|(_, style)| *style != cell.style)
        {
            frozen.runs.push((frozen.text.len(), cell.style));
        }
        frozen.text.push(cell.ch);
    }
    frozen
}

/// A live line as runs of one style each.
fn runs_of(cells: &[Cell]) -> Vec<(TerminalStyle, String)> {
    let mut runs: Vec<(TerminalStyle, String)> = Vec::new();
    for cell in cells {
        match runs.last_mut() {
            Some((style, text)) if *style == cell.style => text.push(cell.ch),
            _ => runs.push((cell.style, cell.ch.to_string())),
        }
    }
    runs
}

/// The color a `38` or `48` names, in either spelling: its own colon-joined
/// subparameters (`38:5:n`, `38:2::r:g:b`), or the parameters after it
/// (`38;5;n`, `38;2;r;g;b`), which are consumed.
fn extended_color(group: &[u16], all: &[Vec<u16>], at: &mut usize) -> Option<TerminalColor> {
    let values: Vec<u16> = match group.len() > 1 {
        true => group[1..].to_vec(),
        false => {
            let kind = all.get(*at).and_then(|g| g.first().copied())?;
            let wanted = match kind {
                5 => 2,
                2 => 4,
                _ => return None,
            };
            let taken: Vec<u16> = all[*at..]
                .iter()
                .take(wanted)
                .filter_map(|g| g.first().copied())
                .collect();
            *at += taken.len();
            taken
        }
    };
    let byte = |value: u16| u8::try_from(value).unwrap_or(u8::MAX);
    match values.as_slice() {
        [5, index, ..] => Some(TerminalColor::Indexed(byte(*index))),
        // The colon spelling may carry a color-space id before the channels.
        [2, _, r, g, b] | [2, r, g, b] => Some(TerminalColor::Rgb(RgbColor {
            r: byte(*r),
            g: byte(*g),
            b: byte(*b),
        })),
        _ => None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(bytes: &[u8]) -> Transcript {
        let mut transcript = Transcript::new();
        transcript.feed(bytes);
        transcript
    }

    fn lines(transcript: &Transcript) -> Vec<String> {
        (0..transcript.len()).map(|i| transcript.text(i)).collect()
    }

    #[test]
    fn test_a_progress_line_redrawn_with_cr_leaves_only_its_last_state() {
        let transcript = decoded(b"Compiling a\r\x1b[KCompiling b\r\x1b[K   Finished\r\nnext\r\n");
        assert_eq!(lines(&transcript), ["   Finished", "next", ""]);
        assert_eq!(transcript.finished(), 2);
    }

    #[test]
    fn test_a_shorter_rewrite_without_erase_keeps_the_old_tail_as_a_terminal_would() {
        let transcript = decoded(b"abcdef\rxy\r\n");
        assert_eq!(transcript.text(0), "xycdef");
    }

    #[test]
    fn test_moving_up_rewrites_a_line_already_written() {
        let transcript = decoded(b"one\r\ntwo\r\n\x1b[2A\x1b[2Kuno\r\n\x1b[B");
        assert_eq!(lines(&transcript), ["uno", "two", ""]);
    }

    #[test]
    fn test_sgr_colors_split_a_line_into_runs_and_reset_ends_them() {
        let transcript = decoded(b"\x1b[1;31merror\x1b[0m: bad\r\n");
        let red = TerminalStyle {
            bold: true,
            foreground: Some(TerminalColor::Indexed(1)),
            ..TerminalStyle::default()
        };
        assert_eq!(
            transcript.runs(0),
            [
                (red, "error".to_string()),
                (TerminalStyle::default(), ": bad".to_string())
            ]
        );
    }

    #[test]
    fn test_extended_colors_read_in_both_spellings() {
        let transcript = decoded(b"\x1b[38;5;196ma\x1b[38:2::1:2:3mb\x1b[48;2;4;5;6mc\x1b[93md");
        let runs = transcript.runs(0);
        assert_eq!(runs[0].0.foreground, Some(TerminalColor::Indexed(196)));
        let rgb = |r, g, b| Some(TerminalColor::Rgb(RgbColor { r, g, b }));
        assert_eq!(runs[1].0.foreground, rgb(1, 2, 3));
        assert_eq!(runs[2].0.background, rgb(4, 5, 6));
        assert_eq!(runs[3].0.foreground, Some(TerminalColor::Indexed(11)));
    }

    #[test]
    fn test_a_sequence_split_across_reads_is_finished_by_the_next() {
        let mut transcript = Transcript::new();
        transcript.feed(b"\x1b[3");
        transcript.feed(b"1mred \xe2\x9c");
        transcript.feed(b"\x93\r\n");
        assert_eq!(transcript.text(0), "red ✓");
        assert_eq!(
            transcript.runs(0)[0].0.foreground,
            Some(TerminalColor::Indexed(1))
        );
    }

    #[test]
    fn test_frozen_lines_read_back_as_they_were_written() {
        let mut bytes = Vec::new();
        for i in 0..LIVE_LINES * 2 {
            bytes.extend(format!("\x1b[32mline\x1b[0m {i}\r\n").bytes());
        }
        let transcript = decoded(&bytes);
        assert_eq!(transcript.text(0), "line 0");
        assert_eq!(
            transcript.runs(0)[1],
            (TerminalStyle::default(), " 0".to_string())
        );
        assert_eq!(transcript.finished(), LIVE_LINES * 2);
    }

    #[test]
    fn test_the_oldest_lines_go_once_there_are_too_many() {
        let mut transcript = Transcript::new();
        for _ in 0..MAX_LINES + 1 {
            transcript.feed(b"x\n");
        }
        assert!(transcript.len() <= MAX_LINES);
        assert_eq!(transcript.dropped() + transcript.len(), MAX_LINES + 2);
    }

    #[test]
    fn test_a_tab_opens_out_to_the_next_stop() {
        assert_eq!(decoded(b"ab\tc").text(0), "ab      c");
    }
}
