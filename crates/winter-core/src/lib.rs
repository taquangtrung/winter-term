//! Winter core: a terminal that parses a session into a block list.
//!
//! The pipeline is byte stream → [`Terminal`] (a `vte` parser) → [`Scrollback`]
//! (a list of [`CommandBlock`]s). Scrollback is a block list, not a flat line
//! buffer: OSC 133 marks delimit per-command blocks and OSC 9001 (TBP) emits rich
//! content blocks nested in a command's output; the wire form is in
//! `docs/terminal-block-protocol-spec.md`.
//!
//! The screen model lives here too: [`Grid`] holds styled cells and a cursor,
//! and [`Screen`] drives it from a byte stream via `vte` (printing, cursor
//! motion, SGR colors, erase, scroll). `winter-render` draws a [`Grid`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod block;
mod grid;
pub mod osc_guard;
mod parser;
mod patch;
mod screen;
mod scrollback;

pub use block::{CommandBlock, LinkSpan, LiveBlock, Segment};
pub use grid::{
    AnsiMode, Cell, CellWidth, Charset, CharsetSlot, Color, CursorShape, DecMode, EraseMode, Grid,
    RgbColor, RowRemap, Style, UnderlineStyle, MAX_SCROLLBACK,
};
pub use parser::{Performer, Terminal, MAX_LINK_URL_BYTES};
pub use screen::Screen;
pub use scrollback::Scrollback;
