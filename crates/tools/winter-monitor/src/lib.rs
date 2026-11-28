//! The machine's monitors, each a tab of its own:
//!
//! - [`proc`]: every process, as a tree or a sorted list, with the actions of
//!   a task manager, and any one of them looked into.
//! - [`sys`]: the machine as a whole: processor load per core, memory and
//!   swap, network and disk traffic, GPUs, and disks.
//! - [`ports`]: every socket and the process holding it.
//! - [`startup`]: what starts on its own at boot or login.
//!
//! What only they use:
//!
//! - `gpu`: asking `nvidia-smi` about the GPU.
//! - `header`: the header every System page draws.
//! - `net`: listing the sockets, for Ports and a process's details.
//! - `run`: running a program for the text it prints.
//! - `table`: the cells every list monitor draws.
//! - `units`: how a size and a percentage are worded.

mod gpu;
mod header;
mod net;
pub mod ports;
pub mod proc;
mod run;
pub mod startup;
pub mod sys;
mod table;
mod units;

use winter_model::monitor::{MonitorAnswer, MonitorQuery};

/// Read what `query` asks for: the host runs it off the event loop.
pub fn answer(query: &MonitorQuery) -> MonitorAnswer {
    match query {
        MonitorQuery::Details(pid) => MonitorAnswer::Details(proc::details::details(*pid)),
        MonitorQuery::Ports => MonitorAnswer::Ports(net::sockets()),
        MonitorQuery::Startup => MonitorAnswer::Startup(startup::collect::collect()),
    }
}
