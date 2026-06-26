//! Winter core: a PTY-driven terminal that parses a session into a block list.
//!
//! The pipeline is byte stream → [`Terminal`] (a `vte` parser) → [`Scrollback`]
//! (a list of [`CommandBlock`]s). Scrollback is a block list, not a flat line
//! buffer: OSC 133 marks delimit per-command blocks and OSC 9001 (TBP) emits rich
//! content blocks nested in a command's output. See `docs/Winter-implementation-plan.md`.
//!
//! [`run_to_completion`] is the headless entry point used for testing and the
//! `dump_session` example.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod block;
mod parser;
mod patch;
mod pty;
mod scrollback;

pub use block::{CommandBlock, LiveBlock, Segment};
pub use parser::{Performer, Terminal};
pub use patch::apply as apply_json_patch;
pub use pty::{run_to_completion, stream_with_callback};
pub use scrollback::Scrollback;

pub use winter_proto;
