//! Where the app keeps its per-user files: the private runtime directory
//! every IPC socket (mux, control channel) and side-channel directory lives
//! in, so they agree on where to look, and the state directory its
//! snapshots (session, mux definitions) are written to.

use std::path::PathBuf;

// ========================================================================
// Constants
// ========================================================================

/// Where IPC sockets and other per-user files go when no private directory
/// can be had: unusable on purpose, so binding there fails with a clear
/// error, rather than falling back to a shared path another local user
/// could have created first and so receive this user's keystrokes.
pub(crate) const UNUSABLE_DIR: &str = "/nonexistent/winter-no-private-dir";

/// The app's directory name under the platform's state directory.
const APP_DIR_NAME: &str = "winter-term";

/// The runtime directory made under `$HOME` when `$XDG_RUNTIME_DIR` is unset
/// (always on macOS, often under ssh or cron).
#[cfg(unix)]
const HOME_RUNTIME_DIR: &str = ".run";

/// The mode of a directory only its owner can use.
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o700;

/// Mode bits letting anyone but the owner write.
#[cfg(unix)]
const OTHERS_WRITE: u32 = 0o022;

/// Mode bits letting anyone but the owner do anything.
#[cfg(unix)]
const OTHERS_ANY: u32 = 0o077;

/// The state directory under `$HOME` when `$XDG_STATE_HOME` is unset.
#[cfg(not(windows))]
const HOME_STATE_DIR: &str = ".local/state";

// ========================================================================
// Private directory
// ========================================================================

/// A directory only this user can use, for sockets and side-channel files:
/// the first of `$XDG_RUNTIME_DIR`, `$HOME/.run`, and `winter-<uid>` under
/// the system temp directory that is (or can be made) private to this user.
/// Each is created 0700 when missing, and an existing one is used only if
/// this user owns it and nobody else can write to it; one only others
/// cannot read is tightened to 0700, since its contents are this user's.
#[cfg(unix)]
pub(crate) fn private_dir() -> Option<String> {
    resolve_private_dir(
        std::env::var("XDG_RUNTIME_DIR").ok(),
        std::env::var("HOME").ok(),
        current_uid(),
    )
}

/// `%LOCALAPPDATA%`, then `%TEMP%`, or `None` if neither is set.
#[cfg(not(unix))]
pub(crate) fn private_dir() -> Option<String> {
    std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("TEMP"))
        .ok()
}

/// The directory `name` inside [`private_dir`], created private (0700) if
/// missing, or `None` when there is no private directory or it cannot be
/// made there.
pub(crate) fn private_subdir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(private_dir()?).join(name);
    #[cfg(unix)]
    {
        ensure_private(&dir, current_uid())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir).ok()?;
    }
    Some(dir)
}

/// [`private_dir`], with its inputs passed in so tests need not touch the
/// process environment.
#[cfg(unix)]
fn resolve_private_dir(xdg: Option<String>, home: Option<String>, uid: u32) -> Option<String> {
    let temp = std::env::temp_dir().join(format!("winter-{uid}"));
    xdg.map(PathBuf::from)
        .into_iter()
        .chain(home.map(|home| PathBuf::from(home).join(HOME_RUNTIME_DIR)))
        .chain(std::iter::once(temp))
        .find_map(|dir| ensure_private(&dir, uid))
}

/// `dir`, created private (0700) if missing, and only if it is a directory
/// `uid` owns that nobody else can write to. One that others can merely
/// read or enter is tightened to 0700 rather than refused.
#[cfg(unix)]
fn ensure_private(dir: &std::path::Path, uid: u32) -> Option<String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    match std::fs::DirBuilder::new().mode(PRIVATE_MODE).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    // Not followed through a link: a link planted there is someone else's.
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & OTHERS_WRITE != 0 {
        return None;
    }
    if meta.mode() & OTHERS_ANY != 0 {
        let private = std::fs::Permissions::from_mode(PRIVATE_MODE);
        std::fs::set_permissions(dir, private).ok()?;
    }
    Some(dir.to_string_lossy().into_owned())
}

/// This process's real user id.
#[cfg(unix)]
#[allow(unsafe_code)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, touches no memory, and cannot fail.
    unsafe { libc::getuid() }
}

// ========================================================================
// State directory
// ========================================================================

/// The file `name` in the app's state directory, or `None` when there is no
/// state directory to put it in (rather than a path relative to wherever
/// the process happens to run).
pub(crate) fn state_file(name: &str) -> Option<PathBuf> {
    state_dir().map(|dir| dir.join(name))
}

/// `%LOCALAPPDATA%\winter-term` on Windows (machine-local, non-roaming).
#[cfg(windows)]
fn state_dir() -> Option<PathBuf> {
    let local_appdata = std::env::var("LOCALAPPDATA").ok()?;
    Some(PathBuf::from(local_appdata).join(APP_DIR_NAME))
}

/// `$XDG_STATE_HOME/winter-term`, or `~/.local/state/winter-term`.
#[cfg(not(windows))]
fn state_dir() -> Option<PathBuf> {
    resolve_state_dir(
        std::env::var("XDG_STATE_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

#[cfg(not(windows))]
fn resolve_state_dir(xdg: Option<String>, home: Option<String>) -> Option<PathBuf> {
    let base = xdg
        .filter(|xdg| !xdg.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(HOME_STATE_DIR)))?;
    Some(base.join(APP_DIR_NAME))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("winter-paths-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("base");
        base
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path)
            .expect("exists")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn test_a_private_dir_is_made_private_and_a_shared_one_refused() {
        let base = scratch("private");
        let uid = current_uid();

        let fresh = base.join("fresh");
        assert!(ensure_private(&fresh, uid).is_some());
        assert_eq!(mode(&fresh), 0o700);

        // One that others can write to may have been planted: refused.
        let shared = base.join("shared");
        std::fs::create_dir(&shared).expect("shared");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        assert_eq!(ensure_private(&shared, uid), None);

        // One others can only read is this user's, and is tightened.
        let readable = base.join("readable");
        std::fs::create_dir(&readable).expect("readable");
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(ensure_private(&readable, uid).is_some());
        assert_eq!(mode(&readable), 0o700);

        // A link in its place is someone else's too.
        let link = base.join("link");
        std::os::unix::fs::symlink(&fresh, &link).expect("link");
        assert_eq!(ensure_private(&link, uid), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_without_a_runtime_dir_the_home_fallback_is_created_private() {
        // Regression: `$HOME/.run` was returned without being created or
        // checked, so without `$XDG_RUNTIME_DIR` (macOS, ssh, cron) the mux
        // socket could not be bound and the side-channel directory under it
        // was made world-readable.
        let home = scratch("home");
        let uid = current_uid();
        let dir = resolve_private_dir(None, Some(home.to_string_lossy().into_owned()), uid);
        let run = home.join(HOME_RUNTIME_DIR);
        assert_eq!(dir.as_deref(), Some(run.to_string_lossy().as_ref()));
        assert_eq!(mode(&run), 0o700);

        // A runtime dir others can write to is passed over.
        let shared = home.join("shared-runtime");
        std::fs::create_dir(&shared).expect("shared");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        let dir = resolve_private_dir(
            Some(shared.to_string_lossy().into_owned()),
            Some(home.to_string_lossy().into_owned()),
            uid,
        );
        assert_eq!(dir.as_deref(), Some(run.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn test_the_state_dir_never_falls_back_to_the_working_directory() {
        assert_eq!(resolve_state_dir(None, None), None);
        assert_eq!(
            resolve_state_dir(Some(String::new()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/winter-term"))
        );
        assert_eq!(
            resolve_state_dir(Some("/state".into()), Some("/home/u".into())),
            Some(PathBuf::from("/state/winter-term"))
        );
    }
}
