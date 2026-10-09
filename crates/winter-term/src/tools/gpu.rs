//! Asking `nvidia-smi` about the GPU. There is no vendor-neutral way to read
//! per-process or per-card video memory without a driver library, so a machine
//! without an NVIDIA GPU simply gets nothing back and its monitors hide the
//! GPU parts.

use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// ========================================================================
// Constants
// ========================================================================

/// How long `nvidia-smi` may take. A busy or waking GPU can hold it for far
/// longer than a monitor can wait, and a refresh stuck behind it is a frozen
/// page.
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
/// GPU, the tool failed, or it did not answer in time.
pub fn query_nvidia(args: &[&str]) -> Option<String> {
    if NVIDIA_MISSING.load(Ordering::Relaxed) {
        return None;
    }
    let spawned = Command::new(NVIDIA_SMI)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                NVIDIA_MISSING.store(true, Ordering::Relaxed);
            }
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    // Read on a thread of its own so the wait below can give up: the pipe
    // would otherwise be read to its end however long the tool takes.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let Ok(bytes) = rx.recv_timeout(NVIDIA_TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let succeeded = child.wait().is_ok_and(|status| status.success());
    succeeded.then(|| String::from_utf8_lossy(&bytes).into_owned())
}
