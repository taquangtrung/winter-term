//! Tools: self-contained pages a pane can hold instead of a terminal.
//!
//! One module per tool. Everything in a tool stays pure and std-only except
//! the modules named for their side effects, so a tool's logic is testable
//! without a window, a filesystem, or a repository.
//!
//! - [`dir`]: a directory listing.
//! - [`editor`]: one file as editable text.
//! - [`git`]: the working tree's state.
//! - [`gpu`]: asking `nvidia-smi` about the GPU.
//! - [`grep`]: the lines under a directory holding some text.
//! - [`keys`]: the command list and the chord bound to each command.
//! - [`pdf`]: one PDF document, drawn by a WebView the page owns.
//! - [`proc`]: the machine's processes, with the actions of a task manager.
//! - [`refresh`]: the schedule of a page that keeps itself current.
//! - [`run`]: running a program for the text it prints.
//! - [`sys`]: the machine's CPU, memory, disks, and GPUs.
//!
//! [`reltime`] is shared rather than a tool of its own: a git log and a list
//! of what was closed recently both have an age to word.

pub mod dir;
pub mod editor;
pub mod git;
pub mod gpu;
pub mod grep;
pub mod keys;
pub mod pdf;
pub mod proc;
pub mod refresh;
pub mod run;
pub mod sys;
pub mod reltime;
