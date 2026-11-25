//! The machine's monitors, two tabs kept current on a timer:
//!
//! - [`proc`]: every process, as a tree or a sorted list, with the actions of
//!   a task manager.
//! - [`sys`]: the machine as a whole: processor load per core, memory and
//!   swap, GPUs, and disks.
//!
//! What only the two of them use:
//!
//! - `gpu`: asking `nvidia-smi` about the GPU.
//! - `run`: running a program for the text it prints.
//! - `units`: how a size and a percentage are worded.

mod gpu;
pub mod proc;
mod run;
pub mod sys;
mod units;
