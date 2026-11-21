//! Terminal runtime: PTY terminals, the block content queue, and the image
//! protocols programs draw with, with no window in it. The terminal tab the
//! window shows over it is `winter-term`'s.
//!
//! - [`session`]: an interactive terminal, its bytes from a local PTY child
//!   or a session on a mux server.
//! - [`block_queue`]: the rich content blocks its programs surface.
//! - [`chart`]: Vega-Lite charts a block holds, sized and drawn as SVG.
//! - [`kitty`] and [`sixel`]: the image protocols.
//! - [`headless`]: a command run to its end under a PTY, with no window.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod block_queue;
pub mod chart;
pub mod headless;
pub mod kitty;
pub mod session;
pub mod sixel;
