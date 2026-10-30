//! Language servers, started only when asked: a project's server reads the
//! documents the editors have open and answers what they ask of it
//! (completions, definitions, references, hovers, renames, fixes, and
//! formatting), and says what it finds wrong.
//!
//! - [`servers`]: which server serves a language, and a file's project.
//! - [`client`]: a running server, the [`Port`] an editor talks to it
//!   through, and the [`Hub`] of every server the window started.
//! - [`protocol`]: what the messages say, read into plain values.
//! - `rpc`: the framing on the wire.
//! - [`diagnostics`]: what is wrong in the files, from every back-end that
//!   checks them, servers and others alike, in one store.
//!
//! Nothing here starts a server by itself: the window starts one when the
//! user asks, or for a language the `lsp { auto-start }` setting names.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod client;
pub mod diagnostics;
pub mod protocol;
mod rpc;
pub mod servers;

pub use client::{Event, Hub, Incoming, Port, ServerId};
