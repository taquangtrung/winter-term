//! Writing the app's own files (the session, window state, mux definitions,
//! settings) so an interrupted write never leaves a truncated one: a crash,
//! a full disk, or power loss mid-write keeps the previous contents whole.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// The mode of a file only its owner can read or write.
#[cfg(unix)]
const OWNER_ONLY_MODE: u32 = 0o600;

/// What a file that did not read is renamed with, set aside rather than
/// written over.
const CORRUPT_SUFFIX: &str = ".corrupt";

// ========================================================================
// Atomic writes
// ========================================================================

/// Replace `path` with `contents` in one step: written to a temporary file
/// beside it, flushed to disk, then renamed over it. A reader sees the old
/// file or the new one, never half of one. The directory is created if
/// missing; the temporary file is removed again if the write fails.
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_beside(path);
    let written = write_synced(&temp, contents.as_ref()).and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// Write `contents` to a new file at `path` and flush it to the disk, so the
/// rename that follows never publishes a file whose bytes are still in flight.
///
/// The file is readable by this user alone: the session snapshot carries
/// what every terminal's screen showed, and the rename keeps the mode. A
/// leftover temporary file from a crashed write is replaced rather than
/// reused, so whatever mode or link it had does not carry over.
fn write_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(OWNER_ONLY_MODE);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// A temporary path beside `path`, named for this process so two windows
/// writing the same file never share one.
fn temp_beside(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

// ========================================================================
// Reading back
// ========================================================================

/// Read the JSON file at `path`, or the default when there is none. A file
/// that does not read is set aside as `<name>.corrupt` and said so: the
/// default would otherwise be saved over it at the next write, and with it
/// every entry it held, without a word.
pub(crate) fn read_json_or_set_aside<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(text) = fs::read_to_string(path) else {
        return T::default();
    };
    match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            let mut aside = path.as_os_str().to_owned();
            aside.push(CORRUPT_SUFFIX);
            let _ = fs::rename(path, &aside);
            eprintln!("winter: {} did not read ({e}); kept as {}", path.display(), PathBuf::from(&aside).display());
            T::default()
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
    fn test_a_file_that_does_not_read_is_set_aside_not_lost() {
        // One bad byte loaded as the default, and the next save wrote that
        // over every entry the file had.
        let dir = std::env::temp_dir().join(format!("winter-persist-corrupt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("state.json");
        fs::write(&path, "{ not json").expect("file");
        let read: std::collections::BTreeMap<String, String> = read_json_or_set_aside(&path);
        assert!(read.is_empty());
        assert!(!path.exists());
        assert_eq!(fs::read_to_string(dir.join("state.json.corrupt")).expect("kept"), "{ not json");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_an_atomic_write_replaces_the_file_and_leaves_no_temporary_behind() {
        let dir = std::env::temp_dir().join(format!("winter-persist-{}", std::process::id()));
        let path = dir.join("nested").join("state.json");
        write_atomic(&path, "first").expect("written");
        write_atomic(&path, "second").expect("rewritten");
        assert_eq!(fs::read_to_string(&path).expect("read"), "second");
        let leftovers: Vec<_> = fs::read_dir(path.parent().expect("dir"))
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        #[cfg(unix)]
        {
            // Regression: the file was created with the umask's mode,
            // usually world-readable, while it holds terminal screens.
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, OWNER_ONLY_MODE);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
