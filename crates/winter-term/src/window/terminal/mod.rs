//! What terminal tabs offer over their scrollback: blocks and their native
//! text, modes and visual selection, Vim edits on the prompt, next-error,
//! cursor navigation and search, the pointer, and the flash a yank leaves.

pub(crate) mod blocks;
pub(crate) mod errors;
pub(crate) mod modes;
pub(crate) mod navigation;
pub(crate) mod pointer;
pub(in crate::window) mod prompt_actions;
pub(in crate::window) mod prompt_edit;
pub(crate) mod visual;
pub(in crate::window) mod yank_flash;
