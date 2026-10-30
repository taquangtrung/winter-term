//! Editors: the tabs Winter paints itself, peers of the terminal in a pane's
//! strip.
//!
//! One module per kind of editor (with focused views such as [`keys`]), beside
//! the helpers they share. Everything in an editor stays pure and std-only
//! except the modules named for their side effects, so logic is testable
//! without a window, a filesystem, or a repository.
//!
//! - [`background`]: work an editor runs on a thread of its own.
//! - [`compile`]: a build, its output, and the places it points at.
//! - [`dir`]: a directory listing.
//! - [`docker`]: the containers, images, volumes, and networks docker knows.
//! - [`diff`]: texts compared and merged region by region, as Emacs'
//!   ediff does it.
//! - [`editor`](crate::model::editor): one file as editable text.
//! - [`git`]: the working tree's state.
//! - [`gpu`]: asking `nvidia-smi` about the GPU.
//! - [`grep`]: the lines under a directory holding some text.
//! - [`hex`]: a file's bytes, sixteen to a row, read and written.
//! - [`image`]: one picture, drawn by Winter itself.
//! - [`keys`]: the command list and the chord bound to each command.
//! - [`launcher`]: what a split opens, holding the place for what goes there.
//! - [`list`]: the window over an editor's rows, and the cursor on them.
//! - [`pdf`]: one PDF document, drawn by Winter itself.
//! - [`preview`]: a Markdown document as it reads rendered.
//! - [`proc`]: the machine's processes, with the actions of a task manager.
//! - [`refresh`]: the schedule of an editor that keeps itself current.
//! - [`run`]: running a program for the text it prints.
//! - [`sys`]: the machine's CPU, memory, disks, and GPUs.
//!
//! [`reltime`] is shared rather than an editor of its own: a git log and a list
//! of what was closed recently both have an age to word.
//! [`spell`] is shared the same way: the text editor and the PDF viewer check
//! their words against the one dictionary.

pub mod background;
pub mod compile;
pub mod diff;
pub mod dir;
pub mod dictionary;
pub mod docker;
pub mod extension;
pub mod git;
pub mod git_exec;
pub mod git_parse;
pub mod gpu;
pub mod grep;
pub mod hex;
pub mod highlight;
pub mod image;
pub mod keys;
pub mod launcher;
pub mod list;
pub mod notifications;
pub mod pdf;
#[cfg(feature = "preview")]
pub mod preview;
pub mod proc;
pub mod refresh;
pub mod reltime;
pub mod run;
pub mod spell;
pub mod synctex;
pub mod sys;
pub mod text;
pub mod text_file;
pub mod units;
pub mod word_diff;

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
pub const COMMAND_BLOCKS: &[CommandBlock] = &[
    CommandBlock {
        block: "git",
        commands: git::COMMANDS,
        title: "Git",
    },
    CommandBlock {
        block: "dir",
        commands: dir::COMMANDS,
        title: "Dir",
    },
    CommandBlock {
        block: "editor",
        commands: text::COMMANDS,
        title: "Editor",
    },
];

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
