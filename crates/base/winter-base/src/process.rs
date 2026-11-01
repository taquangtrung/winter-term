//! Starting the helper programs Winter asks things of (`ssh`, `curl`, `ps`),
//! without a console window flashing up for each one on Windows, and
//! stopping a group of them at once.

use std::io::Write;
use std::process::Command;
use std::sync::mpsc;
use std::thread;

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
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(NO_WINDOW);
    }
    command
}

/// Send `signal` to every process in the group `group` leads: a job's
/// children and theirs, which a signal to the leader alone would leave
/// running.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn signal_group(group: libc::pid_t, signal: libc::c_int) {
    // SAFETY: `kill` takes no pointers and touches no memory of ours; a
    // negative pid names the group.
    unsafe { libc::kill(-group, signal) };
}

/// The processes this one started that are still its children, read from
/// `/proc`; empty where the system does not say.
pub fn child_pids() -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
            return Vec::new();
        };
        let mut pids: Vec<u32> = tasks
            .flatten()
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .flat_map(|text| {
                text.split_whitespace()
                    .filter_map(|pid| pid.parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Reap the child `pid` if it has ended, without waiting. Says whether it
/// is gone: reaped now, or no child of this process any more.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn reap_if_ended(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    let mut status = 0;
    // SAFETY: `waitpid` writes only to `status`, a local that outlives the
    // call; `WNOHANG` makes it return at once.
    let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    reaped != 0
}

/// Let go of a program a reload handed over that no tab took: close its
/// terminal, which nothing in this process has taken, and hang it up, as
/// closing its tab would, rather than leave it running with no one reading.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn abandon_program(fd: std::os::fd::RawFd, pid: u32) {
    // SAFETY: `close` takes a descriptor this process inherited and nothing
    // in it has taken, so no owner closes it again.
    unsafe { libc::close(fd) };
    hang_up(pid);
}

/// Hang up the program `pid`, as closing its terminal does.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn hang_up(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: `kill` takes no pointers and touches no memory of ours.
        unsafe { libc::kill(pid, libc::SIGHUP) };
    }
}

/// Hand `writer`, a PTY's input, to a thread of its own, and return the
/// channel that feeds it. A program that stops reading blocks writes once the
/// kernel's buffer fills; on its own thread that blocks nobody else. The
/// thread ends when the sender is dropped, or when the PTY refuses a write
/// because the program is gone.
pub fn spawn_pty_writer(
    mut writer: Box<dyn Write + Send>,
) -> std::io::Result<mpsc::Sender<Vec<u8>>> {
    let (input, queued) = mpsc::channel::<Vec<u8>>();
    thread::Builder::new()
        .name("winter pty write".into())
        .spawn(move || {
            for bytes in queued {
                if writer
                    .write_all(&bytes)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })?;
    Ok(input)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_child_whose_handle_was_lost_is_listed_and_reaped_when_it_ends() {
        // After a reload's exec, a child's reaper thread is gone with the old
        // image: unless it is reaped by pid, it stays a zombie.
        let child = Command::new("true").spawn().expect("spawns");
        let pid = child.id();
        std::mem::forget(child);
        assert!(child_pids().contains(&pid));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !reap_if_ended(pid) {
            assert!(std::time::Instant::now() < deadline, "never reaped");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!child_pids().contains(&pid), "gone, not a zombie");
    }
}
