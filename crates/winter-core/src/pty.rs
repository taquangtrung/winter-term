//! PTY runtime: spawn a child under a pseudo-terminal and stream its output.

use std::io::Read;

use anyhow::Result;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::parser::Terminal;

// ============================================================================
// Constants
// ============================================================================

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const READ_CHUNK: usize = 4096;

// ============================================================================
// Runtime
// ============================================================================

/// Run a command to completion under a PTY, feeding all of its output into a
/// fresh [`Terminal`], and return that terminal once the child exits.
///
/// Intended for non-interactive commands: it reads until the PTY reports EOF,
/// which only happens after the child has exited.
pub fn run_to_completion(command: CommandBuilder) -> Result<Terminal> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut terminal = Terminal::new();
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf)? {
            0 => break,
            count => terminal.feed(&buf[..count]),
        }
    }

    child.wait()?;
    Ok(terminal)
}
