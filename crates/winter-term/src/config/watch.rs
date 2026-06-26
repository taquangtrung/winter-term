//! Filesystem watcher for the config directory: notifies the app when
//! `settings.kdl`, `keybindings.kdl`, `winter.kdl`, or a `themes/*.kdl` file
//! changes, so edits hot-reload as soon as they're saved instead of on the
//! next poll tick.

use std::path::Path;
use std::sync::mpsc;

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

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

/// Whether a filesystem event represents an actual change to config content,
/// as opposed to something merely reading it.
///
/// This filter is load-bearing, not a tidy-up. The inotify mask `notify`
/// registers includes `IN_OPEN`, and reloading the config *opens*
/// `settings.kdl`, `keybindings.kdl`, and the active theme to read them.
/// Treating every event as a change therefore made the reload self-triggering:
/// reload -> open -> `Access(Open)` event -> reload, a loop that spun the event
/// loop at ~100 reloads/second and burned ~48% of a core on a terminal sitting
/// completely idle (each pass reparses three KDL files, rebuilds the keymap and
/// theme, and resizes every pane and PTY).
///
/// `Access(Close(Write))` is kept because a close-after-write is a real save.
/// Every other save path still reports through `Modify`/`Create`/`Remove`
/// (`IN_MODIFY`, `IN_ATTRIB`, `IN_CREATE`, `IN_MOVED_TO`), so dropping
/// read-only access events costs no hot-reload coverage.
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        // Open, read, and close-after-read: caused by observers, including
        // this process reloading its own config.
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// [`spawn_watcher`], parameterized on the watched directory for testing.
fn spawn_watcher_for(dir: &Path) -> Option<(RecommendedWatcher, mpsc::Receiver<()>)> {
    std::fs::create_dir_all(dir).ok()?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            if is_content_change(&event.kind) {
                let _ = tx.send(());
            }
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

    /// Reading a watched file must not report as a change. Config reload opens
    /// the files it reloads, so if reads were reported the reload would
    /// re-trigger itself forever (see [`is_content_change`]).
    #[test]
    fn test_reading_a_watched_file_does_not_notify() {
        let dir = std::env::temp_dir().join(format!(
            "winter-watch-read-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.kdl");
        std::fs::write(&file, "theme \"dark\"").unwrap();

        let (_watcher, rx) = spawn_watcher_for(&dir).expect("watcher should start");
        std::thread::sleep(Duration::from_millis(100));
        // Drain anything the setup itself raised.
        while rx.try_recv().is_ok() {}

        // Exactly what `reload_config_if_changed` does: open and read.
        for _ in 0..5 {
            let _ = std::fs::read_to_string(&file).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));

        assert!(
            rx.try_recv().is_err(),
            "reading a watched file must not be reported as a change; \
             reporting it makes config reload self-triggering"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_content_change_classification() {
        use notify::event::{DataChange, ModifyKind, RemoveKind};

        // Reads must be ignored: these are what the reload itself provokes.
        assert!(!is_content_change(&EventKind::Access(AccessKind::Open(
            AccessMode::Read
        ))));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));

        // Real saves must still reload.
        assert!(is_content_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_content_change(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
    }
}
