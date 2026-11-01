//! Version control as every tool shares it, the Git view and the directory
//! listing among them: nothing here draws anything.
//!
//! - [`exec`]: the git command lines run, as requests a host carries out.
//! - [`parse`]: reading git's own output.
//! - [`watch`]: noticing that a working tree changed.

pub mod exec;
pub mod parse;
pub mod watch;
