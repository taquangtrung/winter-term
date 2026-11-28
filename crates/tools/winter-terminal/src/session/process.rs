//! The programs in a terminal's foreground, read from the system: the one
//! the shell handed the terminal to, and what that started under it,
//! nearest first, so an agent run as `make dev` (`make`, `cargo`, then the
//! agent) is found under the programs it runs through. Linux reads `/proc`,
//! macOS asks `libproc` and `sysctl`, and Windows takes one snapshot of its
//! processes for each walk.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::program::program_name;

// ========================================================================
// Constants
// ========================================================================

/// The most programs a walk reads, the foreground one among them: enough
/// for any chain of tools a command runs through, few enough that a build
/// spawning hundreds costs little.
const MAX_PROGRAMS: usize = 32;

// ========================================================================
// Functions
// ========================================================================

/// The programs from `leader` down, nearest first: `leader`, then what it
/// started, breadth first, each by name ([`program_name`] where the command
/// line is known), up to [`MAX_PROGRAMS`].
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn programs_under(leader: u32) -> Vec<String> {
    walk(leader, os::children)
        .into_iter()
        .filter_map(os::name)
        .collect()
}

/// What to call the program `pid`, from its command line where the system
/// says it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn program_of(pid: u32) -> Option<String> {
    os::name(pid)
}

/// The programs under the shell `shell`'s newest child, nearest first:
/// Windows keeps no foreground for a console, so the program the shell ran
/// last is taken for it. Empty while the shell runs nothing.
#[cfg(target_os = "windows")]
pub(super) fn programs_under_shell(shell: u32) -> Vec<String> {
    let Some(table) = os::Snapshot::take() else {
        return Vec::new();
    };
    let Some(leader) = table.children(shell).into_iter().max() else {
        return Vec::new();
    };
    walk(leader, |pid| table.children(pid))
        .into_iter()
        .filter_map(|pid| table.name(pid))
        .collect()
}

/// The process group in the foreground of the terminal whose master end is
/// `tty`, its leader's process id, while one other than the shell `shell`'s
/// owns it.
#[cfg(target_os = "macos")]
pub(super) fn foreground_leader(tty: std::os::fd::RawFd, shell: u32) -> Option<u32> {
    os::foreground_leader(tty, shell)
}

/// `leader` and the processes under it, breadth first by what `children`
/// says each started, up to [`MAX_PROGRAMS`] and never one twice.
#[cfg_attr(not(any(test, unix, windows)), allow(dead_code))]
fn walk(leader: u32, mut children: impl FnMut(u32) -> Vec<u32>) -> Vec<u32> {
    let mut found = vec![leader];
    let mut at = 0;
    while at < found.len() && found.len() < MAX_PROGRAMS {
        for child in children(found[at]) {
            if found.len() < MAX_PROGRAMS && !found.contains(&child) {
                found.push(child);
            }
        }
        at += 1;
    }
    found
}

/// The command line a macOS `KERN_PROCARGS2` reply holds: its count of
/// arguments, the executable's path and the padding after it, then that many
/// arguments, each ended by a zero byte.
#[cfg(any(test, target_os = "macos"))]
fn procargs_argv(reply: &[u8]) -> Vec<String> {
    let Some(count) = reply
        .get(..4)
        .and_then(|head| <[u8; 4]>::try_from(head).ok())
    else {
        return Vec::new();
    };
    let count = usize::try_from(i32::from_ne_bytes(count)).unwrap_or(0);
    let rest = &reply[4..];
    let Some(path_end) = rest.iter().position(|byte| *byte == 0) else {
        return Vec::new();
    };
    rest[path_end..]
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .take(count)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

// ========================================================================
// Linux
// ========================================================================

#[cfg(target_os = "linux")]
mod os {
    use super::program_name;

    /// The processes `pid` started, from each of its threads'
    /// `/proc/<pid>/task/<tid>/children`.
    pub(super) fn children(pid: u32) -> Vec<u32> {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return Vec::new();
        };
        tasks
            .filter_map(Result::ok)
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .flat_map(|listed| {
                listed
                    .split_whitespace()
                    .filter_map(|child| child.parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect()
    }

    /// What to call `pid`, from its command line and its name.
    pub(super) fn name(pid: u32) -> Option<String> {
        let argv: Vec<String> = std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|cmdline| {
                cmdline
                    .split(|byte| *byte == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        program_name(&argv, &comm)
    }
}

// ========================================================================
// macOS
// ========================================================================

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod os {
    use std::ffi::c_void;

    use super::{procargs_argv, program_name};

    /// `sysctl`'s `CTL_KERN`, its `KERN_ARGMAX`, and its `KERN_PROCARGS2`.
    const CTL_KERN: i32 = 1;
    const KERN_ARGMAX: i32 = 8;
    const KERN_PROCARGS2: i32 = 49;

    /// The longest name `proc_name` gives, with room for its end.
    const NAME_BYTES: usize = 256;

    /// The most children of one process read.
    const MAX_CHILDREN: usize = 256;

    unsafe extern "C" {
        fn getpgid(pid: i32) -> i32;
        fn proc_listchildpids(ppid: i32, buffer: *mut c_void, buffersize: i32) -> i32;
        fn proc_name(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
        fn sysctl(
            name: *mut i32,
            namelen: u32,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> i32;
        fn tcgetpgrp(fd: i32) -> i32;
    }

    /// The process group in the foreground of the terminal `tty`, while one
    /// other than the shell `shell`'s owns it.
    pub(super) fn foreground_leader(tty: std::os::fd::RawFd, shell: u32) -> Option<u32> {
        // SAFETY: both only read the kernel's tables; a closed descriptor or
        // a gone process answers -1.
        let (group, shells) = unsafe { (tcgetpgrp(tty), getpgid(i32::try_from(shell).ok()?)) };
        (group > 0 && group != shells).then(|| u32::try_from(group).ok())?
    }

    /// The processes `pid` started.
    pub(super) fn children(pid: u32) -> Vec<u32> {
        let Ok(pid) = i32::try_from(pid) else {
            return Vec::new();
        };
        let mut pids = vec![0i32; MAX_CHILDREN];
        let bytes = i32::try_from(pids.len() * size_of::<i32>()).unwrap_or(i32::MAX);
        // SAFETY: the buffer is as long as the size given, and the call
        // writes no more than that.
        let count = unsafe { proc_listchildpids(pid, pids.as_mut_ptr().cast(), bytes) };
        let count = usize::try_from(count).unwrap_or(0).min(pids.len());
        pids[..count]
            .iter()
            .filter_map(|child| u32::try_from(*child).ok())
            .filter(|child| *child > 0)
            .collect()
    }

    /// What to call `pid`, from its command line and its name.
    pub(super) fn name(pid: u32) -> Option<String> {
        let pid = i32::try_from(pid).ok()?;
        let mut comm = [0u8; NAME_BYTES];
        // SAFETY: the buffer is as long as the size given.
        let written = unsafe { proc_name(pid, comm.as_mut_ptr().cast(), NAME_BYTES as u32) };
        let comm = match usize::try_from(written) {
            Ok(length) => String::from_utf8_lossy(&comm[..length.min(NAME_BYTES)]).into_owned(),
            Err(_) => String::new(),
        };
        program_name(&argv(pid), &comm)
    }

    /// The command line `pid` was started with, from `KERN_PROCARGS2`.
    fn argv(pid: i32) -> Vec<String> {
        let Some(room) = arg_max() else {
            return Vec::new();
        };
        let mut reply = vec![0u8; room];
        let mut length = reply.len();
        let mut name = [CTL_KERN, KERN_PROCARGS2, pid];
        // SAFETY: `length` says how long the buffer is, and the call writes
        // no more than that, setting it to what it wrote.
        let read = unsafe {
            sysctl(
                name.as_mut_ptr(),
                3,
                reply.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        match read {
            0 => procargs_argv(&reply[..length.min(reply.len())]),
            _ => Vec::new(),
        }
    }

    /// How long a process's arguments may be, which a `KERN_PROCARGS2`
    /// reply needs room for.
    fn arg_max() -> Option<usize> {
        let mut most: i32 = 0;
        let mut length = size_of::<i32>();
        let mut name = [CTL_KERN, KERN_ARGMAX];
        // SAFETY: `most` is as long as `length` says.
        let read = unsafe {
            sysctl(
                name.as_mut_ptr(),
                2,
                (&mut most as *mut i32).cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        (read == 0).then(|| usize::try_from(most).ok())?
    }
}

// ========================================================================
// Windows
// ========================================================================

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod os {
    use std::ffi::c_void;

    use super::super::program::without_exe;

    /// `CreateToolhelp32Snapshot`'s `TH32CS_SNAPPROCESS`.
    const SNAP_PROCESSES: u32 = 0x2;

    /// How long a process entry's executable name may be, in UTF-16 units.
    const MAX_PATH: usize = 260;

    /// What `CreateToolhelp32Snapshot` answers when it fails.
    const INVALID_HANDLE: isize = -1;

    /// One process of a snapshot, as `Process32FirstW` fills it.
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        priority: i32,
        flags: u32,
        exe_file: [u16; MAX_PATH],
    }

    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// Every process at one moment: its id, its parent's, and its name.
    pub(super) struct Snapshot(Vec<(u32, u32, String)>);

    impl Snapshot {
        /// The processes running now, or `None` when Windows will not say.
        pub(super) fn take() -> Option<Self> {
            // SAFETY: the handle is checked before use and closed after; the
            // entry is sized as the calls ask.
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(SNAP_PROCESSES, 0);
                if snapshot.is_null() || snapshot as isize == INVALID_HANDLE {
                    return None;
                }
                let mut entry: ProcessEntry = std::mem::zeroed();
                entry.size = size_of::<ProcessEntry>() as u32;
                let mut processes = Vec::new();
                let mut more = Process32FirstW(snapshot, &mut entry) != 0;
                while more {
                    let length = entry
                        .exe_file
                        .iter()
                        .position(|unit| *unit == 0)
                        .unwrap_or(MAX_PATH);
                    let exe = String::from_utf16_lossy(&entry.exe_file[..length]);
                    processes.push((entry.process_id, entry.parent_process_id, exe));
                    more = Process32NextW(snapshot, &mut entry) != 0;
                }
                CloseHandle(snapshot);
                Some(Self(processes))
            }
        }

        /// The processes `pid` started.
        pub(super) fn children(&self, pid: u32) -> Vec<u32> {
            self.0
                .iter()
                .filter(|(child, parent, _)| *parent == pid && *child != pid)
                .map(|(child, ..)| *child)
                .collect()
        }

        /// What to call `pid`: its executable's name, without `.exe`.
        pub(super) fn name(&self, pid: u32) -> Option<String> {
            let (.., exe) = self.0.iter().find(|(id, ..)| *id == pid)?;
            let name = without_exe(exe);
            (!name.is_empty()).then(|| name.to_string())
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_walk_goes_down_breadth_first_once_each_and_stops_at_its_most() {
        let tree = |pid: u32| match pid {
            1 => vec![2],
            2 => vec![3, 4],
            3 => vec![5, 1],
            _ => Vec::new(),
        };
        assert_eq!(walk(1, tree), [1, 2, 3, 4, 5], "a loop back is not taken");
        let wide = |pid: u32| match pid {
            1 => (2..200).collect(),
            _ => Vec::new(),
        };
        assert_eq!(walk(1, wide).len(), MAX_PROGRAMS);
    }

    #[test]
    fn test_a_procargs_reply_gives_the_command_line_after_the_path() {
        let mut reply = 2i32.to_ne_bytes().to_vec();
        reply.extend_from_slice(b"/usr/local/bin/node\0\0\0node\0/opt/claude/cli.js\0PATH=/bin\0");
        assert_eq!(procargs_argv(&reply), ["node", "/opt/claude/cli.js"]);
        assert!(procargs_argv(b"\x01").is_empty(), "too short");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_programs_under_a_process_are_read_down_its_tree() {
        use std::process::Command;
        let mut shell = Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .expect("a shell");
        let pid = shell.id();
        // The shell starts `sleep` a moment after it starts.
        let programs = (0..50)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                programs_under(pid)
            })
            .find(|programs| programs.len() > 1)
            .unwrap_or_default();
        let _ = shell.kill();
        let _ = shell.wait();
        assert_eq!(programs.first().map(String::as_str), Some("sh"));
        assert!(programs.iter().any(|name| name == "sleep"), "{programs:?}");
    }
}
