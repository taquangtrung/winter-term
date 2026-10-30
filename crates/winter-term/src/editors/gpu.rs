//! Asking `nvidia-smi` about the GPU. There is no vendor-neutral way to read
//! per-process or per-card video memory without a driver library, so a machine
//! without an NVIDIA GPU simply gets nothing back and its monitors hide the
//! GPU parts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::editors::run::{run_text_within, RunError};

// ========================================================================
// Constants
// ========================================================================

/// How long `nvidia-smi` may take. A busy or waking GPU can hold it for far
/// longer than a monitor can wait, and a refresh stuck behind it is a frozen
/// editor.
const NVIDIA_TIMEOUT: Duration = Duration::from_secs(4);

/// The program asked.
const NVIDIA_SMI: &str = "nvidia-smi";

// ========================================================================
// State
// ========================================================================

/// Set once `nvidia-smi` is known to be absent, so a machine without one does
/// not try to start it on every refresh. A timeout or a failing query is not
/// remembered: a busy GPU may answer next time.
static NVIDIA_MISSING: AtomicBool = AtomicBool::new(false);

// ========================================================================
// Functions
// ========================================================================

/// What `nvidia-smi` printed for `args`, or nothing when there is no NVIDIA
/// GPU, the editor failed, or it did not answer in time.
pub fn query_nvidia(args: &[&str]) -> Option<String> {
    if NVIDIA_MISSING.load(Ordering::Relaxed) {
        return None;
    }
    match run_text_within(NVIDIA_SMI, args, NVIDIA_TIMEOUT) {
        Ok(text) => Some(text),
        Err(RunError::Missing) => {
            NVIDIA_MISSING.store(true, Ordering::Relaxed);
            None
        }
        Err(RunError::Failed(_)) => None,
    }
}
