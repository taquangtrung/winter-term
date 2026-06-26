//! The palette's one way out to the operating system: the real filesystem
//! behind [`DirSource`], and the shell's history file. Everything else in the
//! palette works on what these hand it.
//!
//! The filesystem includes the remote ones: a path naming a file as root, in
//! a container, or on an SSH host (see [`crate::remote`]) is listed there,
//! and `docker://` and `ssh://` list the containers and hosts to go to.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::path::{clean_path, expand_home};
use crate::remote::{self, Kind, Place, Remote};

use super::{DirItem, DirSource};

// ========================================================================
// Constants
// ========================================================================

/// The history files tried after `$HISTFILE`, under the home directory.
const HISTORY_FILES: [&str; 2] = [".zsh_history", ".bash_history"];

// ========================================================================
// Data Structures
// ========================================================================

/// The real filesystem, and the remote ones a path can name.
#[derive(Debug)]
pub(super) struct DiskDirs;

// ========================================================================
// DiskDirs
// ========================================================================

impl DirSource for DiskDirs {
    fn read_dir(&self, dir: &Path) -> Option<Vec<DirItem>> {
        match remote::parse(dir) {
            Some(Place::Remote(at)) => return remote_items(&at),
            Some(Place::Targets(kind)) => return Some(target_items(kind)),
            None => {}
        }
        let read = fs::read_dir(dir).ok()?;
        let items = read
            .flatten()
            .map(|entry| {
                let meta = entry.metadata().ok();
                DirItem {
                    is_dir: meta.as_ref().is_some_and(fs::Metadata::is_dir),
                    name: entry.file_name().to_string_lossy().to_string(),
                    path: entry.path(),
                }
            })
            .collect();
        Some(items)
    }

    fn is_dir(&self, path: &Path) -> bool {
        match remote::parse(path) {
            Some(Place::Remote(at)) => remote::is_dir(&at),
            Some(Place::Targets(_)) => true,
            None => path.is_dir(),
        }
    }

    /// Up from a container's or a host's root is the list of them, to go
    /// to another. Root's own root, and that list, have nothing above.
    fn parent(&self, dir: &Path) -> Option<PathBuf> {
        match remote::parse(dir) {
            Some(Place::Remote(at)) => match at.parent() {
                Some(up) => Some(up.to_path()),
                None => at.kind().map(remote::targets_path),
            },
            Some(Place::Targets(_)) => None,
            None => dir.parent().map(Path::to_path_buf),
        }
    }

    fn root(&self, dir: &Path) -> PathBuf {
        match remote::remote_of(dir) {
            Some(at) => at.at("/").to_path(),
            None => dir.ancestors().last().unwrap_or(dir).to_path_buf(),
        }
    }

    /// A URI typed anywhere goes where it says. A path typed on a target
    /// stays on it, unless it starts at home (`~`), which is this user's.
    fn join(&self, dir: &Path, query: &str) -> PathBuf {
        if remote::parse(Path::new(query)).is_some() {
            return PathBuf::from(query);
        }
        match remote::remote_of(dir) {
            Some(at) if !query.starts_with('~') => at.join(query).to_path(),
            Some(_) | None => clean_path(&dir.join(expand_home(query))),
        }
    }
}

/// The entries of a remote directory, or `None` where it cannot be listed.
fn remote_items(dir: &Remote) -> Option<Vec<DirItem>> {
    let entries = remote::list(dir).ok()?;
    let items = entries
        .into_iter()
        .map(|entry| DirItem {
            is_dir: entry.is_dir,
            path: dir.child(&entry.name).to_path(),
            name: entry.name,
        })
        .collect();
    Some(items)
}

/// Each target of `kind`, as a directory to go into.
fn target_items(kind: Kind) -> Vec<DirItem> {
    remote::targets(kind)
        .into_iter()
        .map(|name| DirItem {
            is_dir: true,
            path: Remote::target_root(kind, &name).to_path(),
            name,
        })
        .collect()
}

/// The real filesystem, as the palette's [`DirSource`].
pub(super) fn disk_dirs() -> Arc<dyn DirSource> {
    Arc::new(DiskDirs)
}

/// The raw bytes of the shell's history: `$HISTFILE`, else zsh's, else
/// bash's, the first that holds anything. Empty when none does.
pub(super) fn read_shell_history() -> Vec<u8> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let candidates = env::var_os("HISTFILE")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            HISTORY_FILES
                .iter()
                .filter_map(|name| home.as_ref().map(|home| home.join(name))),
        );
    candidates
        .filter_map(|path| fs::read(path).ok())
        .find(|contents| !contents.is_empty())
        .unwrap_or_default()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    #[test]
    fn test_up_from_a_target_root_is_its_list_of_targets() {
        let dirs = DiskDirs;
        assert_eq!(
            dirs.parent(&path("docker://web/etc")),
            Some(path("docker://web/"))
        );
        assert_eq!(dirs.parent(&path("docker://web/")), Some(path("docker://")));
        assert_eq!(
            dirs.parent(&path("ssh://me@host:2222/")),
            Some(path("ssh://"))
        );
        assert_eq!(
            dirs.parent(&path("sudo:///")),
            None,
            "root's machine is this one"
        );
        assert_eq!(dirs.parent(&path("docker://")), None);
    }

    #[test]
    fn test_a_path_typed_on_a_target_stays_on_it() {
        let dirs = DiskDirs;
        let etc = path("docker://web/etc");
        assert_eq!(dirs.join(&etc, "/var"), path("docker://web/var"));
        assert_eq!(dirs.join(&etc, "nginx"), path("docker://web/etc/nginx"));
        assert_eq!(dirs.join(&etc, ".."), path("docker://web/"));
        assert_eq!(
            dirs.join(&etc, "ssh://host"),
            path("ssh://host"),
            "unless it names another"
        );
        assert_eq!(dirs.root(&path("ssh://host/a/b")), path("ssh://host/"));
    }

    #[test]
    fn test_a_uri_typed_in_a_local_listing_goes_where_it_says() {
        let dirs = DiskDirs;
        let here = std::env::temp_dir();
        assert_eq!(dirs.join(&here, "sudo:///etc"), path("sudo:///etc"));
        assert_eq!(dirs.join(&here, "docker://"), path("docker://"));
    }
}
