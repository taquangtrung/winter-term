//! Editors: the tabs Winter paints itself, peers of the terminal in a pane's
//! strip.
//!
//! One module per kind of editor (with focused views such as [`keys`]), beside
//! the helpers they share. Everything in an editor stays pure and std-only
//! except the modules named for their side effects, so logic is testable
//! without a window, a filesystem, or a repository.
//!
//! - [`background`]: work an editor runs on a thread of its own.
//! - [`dir`]: a directory listing.
//! - [`editor`]: one file as editable text.
//! - [`git`]: the working tree's state.
//! - [`gpu`]: asking `nvidia-smi` about the GPU.
//! - [`grep`]: the lines under a directory holding some text.
//! - [`image`]: one picture, drawn by Winter itself.
//! - [`keys`]: the command list and the chord bound to each command.
//! - [`list`]: the window over an editor's rows, and the cursor on them.
//! - [`pdf`]: one PDF document, drawn by Winter itself.
//! - [`proc`]: the machine's processes, with the actions of a task manager.
//! - [`refresh`]: the schedule of an editor that keeps itself current.
//! - [`run`]: running a program for the text it prints.
//! - [`sys`]: the machine's CPU, memory, disks, and GPUs.
//!
//! [`reltime`] is shared rather than an editor of its own: a git log and a list
//! of what was closed recently both have an age to word.

pub mod background;
pub mod dir;
pub mod git;
pub mod gpu;
pub mod grep;
pub mod image;
pub mod keys;
pub mod list;
pub mod notifications;
pub mod pdf;
pub mod proc;
pub mod refresh;
pub mod reltime;
pub mod run;
pub mod sys;
pub mod text;

use crate::model::editor::EditorCommand;

// ========================================================================
// Data Structures
// ========================================================================

/// One editor's block in `keybindings.kdl`: the commands it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandBlock {
    /// The block's name, the editor kind's own (`git`).
    pub block: &'static str,
    /// The commands the editor runs by name.
    pub commands: &'static [EditorCommand],
    /// How the Keys editor heads the editor's commands.
    pub title: &'static str,
}

// ========================================================================
// Constants
// ========================================================================

/// Every editor that has commands to bind. An editor missing here has a block
/// that `keybindings.kdl` reports as unknown.
pub const COMMAND_BLOCKS: &[CommandBlock] = &[CommandBlock {
    block: "git",
    commands: git::COMMANDS,
    title: "Git",
}];

// ========================================================================
// Lookups
// ========================================================================

/// The commands an editor's block may bind, or `None` for no editor's block.
pub fn block_commands(block: &str) -> Option<&'static [EditorCommand]> {
    COMMAND_BLOCKS
        .iter()
        .find(|entry| entry.block == block)
        .map(|entry| entry.commands)
}

/// What an editor's command is called, or `None` for no command of that editor's.
pub fn command_label(block: &str, name: &str) -> Option<&'static str> {
    block_commands(block)?
        .iter()
        .find(|command| command.name == name)
        .map(|command| command.label)
}
