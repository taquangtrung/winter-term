//! Filesystem watcher for the config directory: notifies the app when
//! `settings.kdl`, `keybindings.kdl`, `winter.kdl`, or a `themes/*.kdl` file
//! changes, so edits hot-reload as soon as they're saved instead of on the
//! next poll tick.

use std::path::Path;
use std::sync::mpsc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use super::config_dir;

// ========================================================================
// Watcher
// ========================================================================

/// Watch the config directory for changes, returning a receiver a poller can
/// drain non-blockingly (one message per filesystem event; the exact event
/// is discarded since any change means "re-check everything"). Recursive, so
/// it covers `themes/*.kdl` alongside the top-level files without needing to
/// know which theme is currently active or re-watch on a theme switch.
///
/// The returned `RecommendedWatcher` must be kept alive for as long as
/// watching should continue; dropping it stops delivery. Returns `None` if
/// the platform's watch backend fails to start (e.g. the inotify instance
/// limit is exhausted): the caller simply gets no hot-reload rather than a
/// startup failure.
pub(crate) fn spawn_watcher() -> Option<(RecommendedWatcher, mpsc::Receiver<()>)> {
    spawn_watcher_for(&config_dir())
}

/// [`spawn_watcher`], parameterized on the watched directory for testing.
fn spawn_watcher_for(dir: &Path) -> Option<(RecommendedWatcher, mpsc::Receiver<()>)> {
    std::fs::create_dir_all(dir).ok()?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .ok()?;
    watcher.watch(dir, RecursiveMode::Recursive).ok()?;
    Some((watcher, rx))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_spawn_watcher_for_notifies_on_file_write() {
        let dir = std::env::temp_dir().join(format!(
            "winter-watch-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (_watcher, rx) = spawn_watcher_for(&dir).expect("watcher should start");
        // Give the platform watch backend a moment to actually start
        // observing the directory before the write below.
        std::thread::sleep(Duration::from_millis(100));

        std::fs::write(dir.join("settings.kdl"), "theme \"dark\"").unwrap();

        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "expected a filesystem event after writing into the watched directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
