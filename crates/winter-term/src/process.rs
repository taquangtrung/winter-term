//! Starting the helper programs Winter asks things of (`ssh`, `curl`, `ps`),
//! without a console window flashing up for each one on Windows.

use std::process::Command;

// ========================================================================
// Constants
// ========================================================================

/// Starts a console program on Windows without a console window of its
/// own (`CREATE_NO_WINDOW`).
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

// ========================================================================
// Functions
// ========================================================================

/// `program`, ready to be given arguments, started without a window of
/// its own on Windows.
pub(crate) fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(NO_WINDOW);
    }
    command
}
