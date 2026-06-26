//! Files somewhere other than this user's own view of the disk: as root,
//! inside a Docker container, or on a host over SSH.
//!
//! A place is named by a URI, the way the desktop file managers name one
//! (see [`uri`]): `sudo:///etc/hosts`, `docker://web/etc/nginx.conf`,
//! `ssh://deploy@example.com:2222/srv/app`, with `docker://` and `ssh://`
//! alone naming the containers and hosts there are to go to. The URI is what
//! the rest of the app carries as the file's path, so it reaches the recent
//! list and a saved session as it is. Nothing takes it apart as a local path:
//! going up, down, and across a remote filesystem is [`Remote`]'s own work,
//! which keeps a Windows path separator out of it.
//!
//! Every operation is one fixed shell script run on the far side, with the
//! path handed to it as an argument rather than spliced into its text (see
//! [`scripts`]), through `sudo -n`, `docker exec`, or a non-interactive
//! `ssh` whose connection is shared between calls (see [`run`]). Docker and
//! SSH work from Linux, macOS, and Windows, since the far side is the one
//! that runs the scripts; root needs a Unix `sudo` here.
//!
//! - [`run`]: starting the commands, with a deadline.
//! - [`scripts`]: what runs on the far side, and reading what it prints.
//! - [`targets`]: the containers running and the hosts configured.
//! - [`uri`]: reading and printing the URIs.

mod run;
mod scripts;
mod targets;
mod uri;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// ========================================================================
// Constants
// ========================================================================

/// The root of every target's filesystem.
const ROOT: &str = "/";

/// Said where root is asked for on a system without a `sudo` to ask.
pub const SUDO_UNAVAILABLE: &str = "root needs sudo, which this system does not have";

// ========================================================================
// Data Structures
// ========================================================================

/// A file or directory reached through something other than the local disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Remote {
    /// How it is reached.
    pub method: Method,
    /// Its absolute path on the far side, decoded and normalized, with no
    /// trailing `/` unless it is the root.
    pub path: String,
}

/// How a remote file is reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Method {
    /// On this machine, as root, through `sudo`.
    Sudo,
    /// Inside a running container.
    Docker(Container),
    /// On a host over SSH.
    Ssh(SshHost),
}

/// A running container, and who to be inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Container {
    pub name: String,
    /// The user to run as, else the container's own default.
    pub user: Option<String>,
}

/// An SSH host, and how to log in to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshHost {
    /// A name, an address, or an alias from the SSH configuration.
    pub host: String,
    /// The port, else SSH's own choice.
    pub port: Option<u16>,
    /// The user to log in as, else SSH's own choice.
    pub user: Option<String>,
}

/// The kinds of place with more than one target to choose from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Docker,
    Ssh,
}

/// What a path names, when it is not a local one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Place {
    /// A file or directory on a target.
    Remote(Remote),
    /// The list of targets of a kind: the containers, or the hosts.
    Targets(Kind),
}

/// One entry of a remote directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Whether it is a directory, or a link to one.
    pub is_dir: bool,
    pub name: String,
    /// Its size in bytes, where the far side could say.
    pub size: Option<u64>,
}

/// A remote file's size and modification time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stat {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// Why a remote operation did not happen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteError {
    /// It was refused or failed, for the reason given.
    Failed(String),
    /// `sudo` wants a password before it will act: see [`sudo_authenticate`].
    NeedsPassword,
}

// ========================================================================
// Functions
// ========================================================================

/// What `path` names, or `None` for an ordinary local path.
pub fn parse(path: &Path) -> Option<Place> {
    uri::parse(path.to_str()?)
}

/// `path` as a remote file, or `None` for anything else.
pub fn remote_of(path: &Path) -> Option<Remote> {
    match parse(path)? {
        Place::Remote(remote) => Some(remote),
        Place::Targets(_) => None,
    }
}

/// The path naming every target of `kind`, to list them.
pub fn targets_path(kind: Kind) -> PathBuf {
    PathBuf::from(uri::targets_uri(kind))
}

/// The names of every target of `kind` there is to go to.
pub fn targets(kind: Kind) -> Vec<String> {
    match kind {
        Kind::Docker => targets::containers(),
        Kind::Ssh => targets::ssh_hosts(),
    }
}

/// Whether root can be reached here: there is a `sudo` on Unix, and no
/// way to be root without a dialog on Windows.
pub fn sudo_available() -> bool {
    cfg!(unix)
}

/// The entries of the directory at `remote`.
pub fn list(remote: &Remote) -> Result<Vec<Entry>, RemoteError> {
    let output = run::script(remote, scripts::LIST, &[&remote.path], None, run::QUICK)?;
    Ok(scripts::parse_listing(&String::from_utf8_lossy(&output)))
}

/// Whether `remote` is a directory that can be listed.
pub fn is_dir(remote: &Remote) -> bool {
    run::script(remote, scripts::IS_DIR, &[&remote.path], None, run::QUICK).is_ok()
}

/// The contents of the file at `remote`.
pub fn read(remote: &Remote) -> Result<Vec<u8>, RemoteError> {
    run::script(remote, scripts::READ, &[&remote.path], None, run::TRANSFER)
}

/// Replace the contents of the file at `remote` with `bytes`, keeping the
/// file itself (its owner, its mode, a link leading to it) as it was.
pub fn write(remote: &Remote, bytes: &[u8]) -> Result<(), RemoteError> {
    let len = bytes.len().to_string();
    run::script(
        remote,
        scripts::WRITE,
        &[&remote.path, &len],
        Some(bytes),
        run::TRANSFER,
    )
    .map(|_| ())
}

/// The size and modification time of the file at `remote`, or `None` where
/// it cannot be read.
pub fn stat(remote: &Remote) -> Option<Stat> {
    let output = run::script(remote, scripts::STAT, &[&remote.path], None, run::QUICK).ok()?;
    scripts::parse_stat(&String::from_utf8_lossy(&output))
}

/// Whether `sudo` would act now without asking for a password.
pub fn sudo_ready() -> bool {
    sudo_available() && run::sudo_ready()
}

/// Give `sudo` the password, so it acts without one for as long as its own
/// timestamp lasts. The password is passed on once and not kept.
pub fn sudo_authenticate(password: &str) -> Result<(), RemoteError> {
    match sudo_available() {
        true => run::sudo_authenticate(password),
        false => Err(RemoteError::Failed(SUDO_UNAVAILABLE.to_string())),
    }
}

/// `path` with `.` and empty components dropped and `..` taken back, always
/// absolute, never ending in `/` unless it is the root.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("{ROOT}{}", parts.join("/"))
}

// ========================================================================
// Remote
// ========================================================================

impl Remote {
    /// `path` on the far side `method` reaches.
    fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: normalize(path),
        }
    }

    /// This machine's `path`, as root.
    pub fn sudo(path: &Path) -> Self {
        Self::new(Method::Sudo, &path.to_string_lossy())
    }

    /// The root of the target `kind` lists as `name`.
    pub fn target_root(kind: Kind, name: &str) -> Self {
        let method = match kind {
            Kind::Docker => Method::Docker(Container {
                name: name.to_string(),
                user: None,
            }),
            Kind::Ssh => Method::Ssh(SshHost {
                host: name.to_string(),
                port: None,
                user: None,
            }),
        };
        Self::new(method, ROOT)
    }

    /// The same target, at `path`.
    pub fn at(&self, path: &str) -> Self {
        Self::new(self.method.clone(), path)
    }

    /// The entry `name` inside this directory.
    pub fn child(&self, name: &str) -> Self {
        self.at(&format!("{}/{name}", self.path))
    }

    /// `query`, typed as a path from this directory: absolute on the same
    /// target, or relative to here.
    pub fn join(&self, query: &str) -> Self {
        match query.starts_with(ROOT) {
            true => self.at(query),
            false => self.child(query),
        }
    }

    /// Whether this is the root of its target.
    pub fn is_root(&self) -> bool {
        self.path == ROOT
    }

    /// The directory holding this one, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        (!self.is_root()).then(|| self.at(&format!("{}/..", self.path)))
    }

    /// The URI naming this file, as the path the rest of the app carries.
    pub fn to_path(&self) -> PathBuf {
        PathBuf::from(uri::format(self))
    }

    /// Where it is, without the path: `sudo`, `docker://web`,
    /// `ssh://me@host:2222`.
    pub fn label(&self) -> String {
        match self.method {
            Method::Sudo => uri::SUDO.to_string(),
            Method::Docker(_) | Method::Ssh(_) => uri::authority_of(&self.method),
        }
    }

    /// The kind of target, for a remote that has a list of them to go back
    /// to. `None` for root, which is this one machine.
    pub fn kind(&self) -> Option<Kind> {
        match self.method {
            Method::Sudo => None,
            Method::Docker(_) => Some(Kind::Docker),
            Method::Ssh(_) => Some(Kind::Ssh),
        }
    }
}

// ========================================================================
// Stat
// ========================================================================

impl Stat {
    /// From a size and a modification time in whole seconds since the epoch.
    fn from_secs(len: u64, secs: Option<u64>) -> Self {
        Self {
            len,
            modified: secs.map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(text: &str) -> Remote {
        remote_of(Path::new(text)).unwrap_or_else(|| panic!("{text} is not a remote file"))
    }

    #[test]
    fn test_a_remote_path_walks_up_down_and_across_its_own_target() {
        let dir = remote("docker://web/etc/nginx");
        assert_eq!(
            dir.child("nginx.conf").to_path(),
            PathBuf::from("docker://web/etc/nginx/nginx.conf")
        );
        assert_eq!(dir.join("/var/log").path, "/var/log");
        assert_eq!(dir.join("../ssl").path, "/etc/ssl");
        let up = dir.parent().expect("a parent");
        assert_eq!(up.path, "/etc");
        assert_eq!(
            up.parent().expect("the root").to_path(),
            PathBuf::from("docker://web/")
        );
        assert!(remote("docker://web/").parent().is_none());
    }

    #[test]
    fn test_a_label_names_the_target_without_the_path() {
        assert_eq!(remote("sudo:///etc/hosts").label(), "sudo");
        assert_eq!(
            remote("ssh://me@host:2222/srv").label(),
            "ssh://me@host:2222"
        );
        assert_eq!(remote("docker://web/x").label(), "docker://web");
    }

    /// End to end against a real container: `WINTER_TEST_CONTAINER=<name>
    /// cargo test -- --ignored remote::tests::test_a_container`.
    #[test]
    #[ignore = "needs a running container named in WINTER_TEST_CONTAINER"]
    fn test_a_container_lists_reads_and_writes_through_docker_exec() {
        let container = std::env::var("WINTER_TEST_CONTAINER").expect("a container");
        let root = Remote::target_root(Kind::Docker, &container);
        assert!(targets(Kind::Docker).contains(&container));
        assert!(list(&root)
            .expect("lists /")
            .iter()
            .any(|e| e.name == "tmp" && e.is_dir));
        assert!(is_dir(&root.at("/tmp")));

        let file = root.at("/tmp/winter remote 'test'.txt");
        write(&file, b"first\n").expect("creates");
        write(&file, b"second line\n").expect("overwrites");
        assert_eq!(read(&file).expect("reads"), b"second line\n");
        assert_eq!(stat(&file).expect("stats").len, 12);
        assert_eq!(
            remote(&file.to_path().to_string_lossy()),
            file,
            "its URI reads back"
        );
        assert!(matches!(
            read(&root.at("/no/such")),
            Err(RemoteError::Failed(_))
        ));
    }

    /// End to end against a real host: `WINTER_TEST_SSH_HOST=<[user@]host[:port]>
    /// cargo test -- --ignored remote::tests::test_an_ssh_host`.
    #[test]
    #[ignore = "needs a host Winter can reach without a prompt, in WINTER_TEST_SSH_HOST"]
    fn test_an_ssh_host_lists_reads_and_writes_over_ssh() {
        let host = std::env::var("WINTER_TEST_SSH_HOST").expect("a host");
        let root = remote(&format!("ssh://{host}/"));
        let listed = list(&root);
        assert!(
            listed.as_ref().is_ok_and(|entries| !entries.is_empty()),
            "{listed:?}"
        );
        let file = root.at("/tmp/winter-remote-test.txt");
        write(&file, b"over ssh\n").expect("writes");
        assert_eq!(read(&file).expect("reads"), b"over ssh\n");
    }
}
