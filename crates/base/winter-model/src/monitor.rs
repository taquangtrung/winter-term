//! What the monitors past the process list and the dashboard read from the
//! machine: one process looked into, every socket, and what starts on its
//! own. Asked for as a [`MonitorQuery`] and answered as a [`MonitorAnswer`],
//! so a monitor added later costs the host one variant.

use std::path::PathBuf;

use crate::process::ProcState;

// ========================================================================
// Data Structures
// ========================================================================

/// What a monitor asks the host to read off the event loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MonitorQuery {
    /// Look into the process with this id.
    Details(u32),
    /// List every socket, and the process holding each.
    Ports,
    /// List what starts on its own at boot or login.
    Startup,
}

/// The answer to a [`MonitorQuery`], or why the system would not give one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MonitorAnswer {
    /// The process looked into.
    Details(ProcessDetails),
    /// Every socket.
    Ports(Result<Vec<Socket>, String>),
    /// What starts on its own.
    Startup(Result<Vec<StartupEntry>, String>),
}

/// One process looked into. Each part is read on its own, and says why
/// where it could not be: another user's process keeps most of them to
/// itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessDetails {
    /// Its environment variables, as it was started with them.
    pub environment: Result<Vec<(String, String)>, String>,
    /// What it has open.
    pub files: Result<Vec<OpenFile>, String>,
    /// The shared libraries it loaded, by path.
    pub libraries: Result<Vec<String>, String>,
    /// Its memory, by what each part maps, largest first.
    pub memory: Result<Vec<MemoryRegion>, String>,
    /// The process.
    pub pid: u32,
    /// Its network connections.
    pub sockets: Result<Vec<Socket>, String>,
    /// Its threads.
    pub threads: Result<Vec<ThreadInfo>, String>,
}

/// One thing a process has open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenFile {
    /// Its descriptor number, where the system numbers them.
    pub fd: Option<u32>,
    /// Whether it is a file, a pipe, a socket, or something else.
    pub kind: FileKind,
    /// What it is: a path, or the system's name for what has none
    /// (`pipe:[1234]`).
    pub target: String,
}

/// What kind of thing an [`OpenFile`] is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FileKind {
    /// A file or directory on disk, or a device.
    File,
    /// One end of a pipe.
    Pipe,
    /// A network or local socket.
    Socket,
    /// Anything else the kernel hands out (an event or timer handle).
    Other,
}

/// The memory a process maps from one source: a file, the heap, the stack,
/// or anonymous memory, all its mappings of it together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    /// How many mappings were added up.
    pub mappings: usize,
    /// What is mapped: a path, `[heap]`, `[stack]`, or `[anon]`.
    pub name: String,
    /// Kilobytes of it in memory now, where the system says.
    pub resident_kb: Option<u64>,
    /// Kilobytes of address space it takes.
    pub size_kb: u64,
}

/// One thread of a process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadInfo {
    /// Its name, which a program may set per thread.
    pub name: String,
    /// Whether it runs, sleeps, or waits.
    pub state: ProcState,
    /// Its id.
    pub tid: u32,
}

/// One socket, listening or connected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Socket {
    /// Whether it waits for connections: a TCP socket listening, or a UDP
    /// socket bound to no peer.
    pub listening: bool,
    /// The local address and port (`127.0.0.1:3000`, `[::]:22`).
    pub local: String,
    /// The local port, for sorting and searching.
    pub port: u16,
    /// The process holding it, where the system says.
    pub pid: Option<u32>,
    /// That process's name.
    pub program: Option<String>,
    /// TCP or UDP.
    pub protocol: Protocol,
    /// The address it is connected to, `None` for one connected to nothing.
    pub remote: Option<String>,
    /// Its state as the system names it (`LISTEN`, `ESTABLISHED`), empty
    /// for UDP.
    pub state: String,
}

/// A socket's protocol.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Protocol {
    /// A connection, with a state.
    Tcp,
    /// Datagrams, with no state.
    Udp,
}

/// One thing that starts on its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupEntry {
    /// What it runs, or what it is for where it runs nothing itself.
    pub command: String,
    /// Whether it is set to start.
    pub enabled: bool,
    /// What starts it.
    pub kind: StartupKind,
    /// Its name: a unit, an entry's name, a file's.
    pub name: String,
    /// Whether it starts for this user alone or for the whole machine.
    pub scope: StartupScope,
    /// The file that sets it up, where there is one to open.
    pub source: Option<PathBuf>,
}

/// How a [`StartupEntry`] is started.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StartupKind {
    /// Started by the desktop at login (an XDG autostart entry, a Windows
    /// startup command).
    Autostart,
    /// A line of a crontab.
    Cron,
    /// A launch agent or daemon (macOS).
    Launchd,
    /// A file every new shell reads.
    Shell,
    /// A systemd unit enabled to start.
    Systemd,
}

/// Whose a [`StartupEntry`] is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StartupScope {
    /// The whole machine's: changing it needs an administrator.
    System,
    /// This user's own.
    User,
}

// ========================================================================
// ProcessDetails
// ========================================================================

impl ProcessDetails {
    /// Process `pid` with none of its parts read, each saying `why`.
    pub fn unread(pid: u32, why: &str) -> Self {
        Self {
            environment: Err(why.to_string()),
            files: Err(why.to_string()),
            libraries: Err(why.to_string()),
            memory: Err(why.to_string()),
            pid,
            sockets: Err(why.to_string()),
            threads: Err(why.to_string()),
        }
    }
}
