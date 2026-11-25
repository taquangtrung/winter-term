//! The desktop trash: where a delete sends an entry by default, so a slip of
//! the finger can be taken back from the file manager.
//!
//! On Linux and the BSDs this is the freedesktop.org trash: the entry moves to
//! `Trash/files`, and a `.trashinfo` beside it in `Trash/info` records where it
//! came from and when, which is what a file manager's "Restore" reads. An
//! entry on another filesystem than the home trash goes to the trash at the
//! top of its own filesystem (`$topdir/.Trash-$uid`), so trashing is always
//! a rename and never a copy of the whole tree into the home directory.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::ops;

// ========================================================================
// Constants
// ========================================================================

/// Seconds in a day, for turning a timestamp into a date.
const SECS_PER_DAY: u64 = 86_400;

/// Days from 0000-03-01 to 1970-01-01 in the proleptic Gregorian calendar,
/// the offset the civil-date conversion below counts from.
const EPOCH_DAYS: i64 = 719_468;

/// Days in a 400-year Gregorian cycle, and the years and days it splits into.
const ERA_DAYS: i64 = 146_097;
const ERA_YEARS: i64 = 400;

/// What a trash at the top of a filesystem other than home's is called,
/// before the user id it belongs to.
const TOPDIR_TRASH: &str = ".Trash-";

/// Who may read a trash at the top of a filesystem: its owner alone.
const TOPDIR_TRASH_MODE: u32 = 0o700;

/// What a trash info file's extension is.
const INFO_EXTENSION: &str = "trashinfo";

/// The bytes a trash info path may carry as they are; every other byte is
/// written `%XX`, as the specification asks.
const UNRESERVED: &[u8] = b"-_.~/";

// ========================================================================
// Functions
// ========================================================================

/// Move `path` to the trash, recording where it came from.
pub fn trash(path: &Path) -> io::Result<()> {
    let home = trash_root().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "there is no trash here; X deletes for good",
        )
    })?;
    let original = absolute(path)?;
    let root = trash_for(&original, &home)?;
    trash_into(&original, &root, SystemTime::now())
}

/// Which trash `original` goes to: the home trash when it is on the home
/// trash's filesystem, else the trash at the top of its own filesystem, made
/// when it is not there yet. Refused, rather than copied into the home trash,
/// when that one cannot be made: a copy of a large tree across filesystems is
/// slow, can fill the home disk, and leaves half a copy behind when it fails.
#[cfg(unix)]
fn trash_for(original: &Path, home: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    let device = fs::symlink_metadata(original)?.dev();
    let home_meta = nearest_existing(home).and_then(|dir| fs::metadata(dir).ok());
    let Some(home_meta) = home_meta else {
        return Ok(home.to_path_buf());
    };
    if home_meta.dev() == device {
        return Ok(home.to_path_buf());
    }
    let top = mount_top(original, device);
    let trash = top.join(format!("{TOPDIR_TRASH}{}", home_meta.uid()));
    if !trash.is_dir() {
        fs::DirBuilder::new()
            .mode(TOPDIR_TRASH_MODE)
            .create(&trash)
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("no trash on {} ({e}); X deletes for good", top.display()),
                )
            })?;
    }
    Ok(trash)
}

/// Everything goes to the one trash where there is no telling filesystems
/// apart.
#[cfg(not(unix))]
fn trash_for(_original: &Path, home: &Path) -> io::Result<PathBuf> {
    Ok(home.to_path_buf())
}

/// The highest directory above `path` still on the filesystem `device`: the
/// top of the filesystem `path` is on.
#[cfg(unix)]
fn mount_top(path: &Path, device: u64) -> PathBuf {
    use std::os::unix::fs::MetadataExt;

    let mut top = path.parent().unwrap_or(path).to_path_buf();
    for dir in path.ancestors().skip(1) {
        match fs::metadata(dir) {
            Ok(meta) if meta.dev() == device => top = dir.to_path_buf(),
            Ok(_) | Err(_) => break,
        }
    }
    top
}

/// `path`, or the nearest directory above it that exists: where a trash not
/// made yet would be made.
fn nearest_existing(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| dir.exists())
}

/// Move `original`, an absolute path, into the trash at `root` as of `now`.
fn trash_into(original: &Path, root: &Path, now: SystemTime) -> io::Result<()> {
    let original = original.to_path_buf();
    let name = original
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "nothing to trash"))?;
    let files = root.join("files");
    let info = root.join("info");
    fs::create_dir_all(&files)?;
    fs::create_dir_all(&info)?;
    let free = free_name(&files, &info, &name);
    // The info file is written first, so a file manager never finds an
    // entry in the trash it cannot say how to restore.
    let info_path = info.join(format!("{free}.{INFO_EXTENSION}"));
    // Made new rather than written over: another trash at the same moment
    // must not have its record replaced by this one's.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&info_path)
        .and_then(|mut file| file.write_all(trash_info(&original, now).as_bytes()))?;
    if let Err(e) = ops::move_entry(&original, &files.join(&free)) {
        let _ = fs::remove_file(&info_path);
        return Err(e);
    }
    Ok(())
}

/// The home trash: `$XDG_DATA_HOME/Trash`, or `~/.local/share/Trash`.
#[cfg(all(unix, not(target_os = "macos"), not(test)))]
fn trash_root() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(data.join("Trash"))
}

/// No freedesktop trash on this platform, so a delete there is for good.
#[cfg(all(not(all(unix, not(target_os = "macos"))), not(test)))]
fn trash_root() -> Option<PathBuf> {
    None
}

/// A trash of the test run's own, so no test ever fills the real one.
#[cfg(test)]
fn trash_root() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("winter-test-trash-{}", std::process::id())))
}

/// `path` made absolute against where it sits, without following a link at
/// its end: trashing a link moves the link.
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = match parent {
        Some(parent) => fs::canonicalize(parent)?,
        None => std::env::current_dir()?,
    };
    Ok(match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    })
}

/// A name for `name` free in both halves of the trash: itself, or itself
/// with ` 2`, ` 3`, ... before any extension.
fn free_name(files: &Path, info: &Path, name: &str) -> String {
    let taken = |candidate: &str| {
        files.join(candidate).exists()
            || info.join(format!("{candidate}.{INFO_EXTENSION}")).exists()
    };
    if !taken(name) {
        return name.to_string();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| format!("{stem} {n}{extension}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

/// The `.trashinfo` contents for `original`, deleted at `now`.
fn trash_info(original: &Path, now: SystemTime) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encoded(&original.to_string_lossy()),
        utc_stamp(now)
    )
}

/// `text` with every byte outside the unreserved set written `%XX`.
fn percent_encoded(text: &str) -> String {
    text.bytes()
        .map(
            |byte| match byte.is_ascii_alphanumeric() || UNRESERVED.contains(&byte) {
                true => (byte as char).to_string(),
                false => format!("%{byte:02X}"),
            },
        )
        .collect()
}

/// `now` as `YYYY-MM-DDThh:mm:ss`, in UTC.
fn utc_stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil_date((secs / SECS_PER_DAY) as i64);
    let of_day = secs % SECS_PER_DAY;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's algorithm.
fn civil_date(days: i64) -> (i64, i64, i64) {
    let z = days + EPOCH_DAYS;
    let era = z.div_euclid(ERA_DAYS);
    let day_of_era = z - era * ERA_DAYS;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / (ERA_DAYS - 1)) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * ERA_YEARS + i64::from(month <= 2);
    (year, month, day)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    #[test]
    fn test_a_trashed_file_can_be_restored_from_what_the_info_records() {
        let dir = std::env::temp_dir().join(format!("winter-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("Trash");
        fs::create_dir_all(&dir).expect("temp dir");
        let doomed = dir.join("my notes.txt");
        fs::write(&doomed, "keep me").expect("write");
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        trash_into(&doomed, &root, when).expect("trashed");
        fs::write(&doomed, "again").expect("write");
        trash_into(&doomed, &root, when).expect("trashed twice");

        assert!(!doomed.exists());
        assert_eq!(
            fs::read_to_string(root.join("files/my notes.txt")).expect("first"),
            "keep me"
        );
        assert!(
            root.join("files/my notes 2.txt").exists(),
            "a second one is renamed"
        );
        let info = fs::read_to_string(root.join("info/my notes.txt.trashinfo")).expect("info");
        let original = percent_encoded(&doomed.to_string_lossy());
        assert!(original.contains("my%20notes.txt"), "{original}");
        assert_eq!(
            info,
            format!("[Trash Info]\nPath={original}\nDeletionDate=2023-11-14T22:13:20\n")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_an_entry_on_the_home_trashs_filesystem_goes_to_the_home_trash() {
        let dir = std::env::temp_dir().join(format!("winter-trash-for-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let entry = dir.join("entry");
        fs::write(&entry, "x").expect("write");
        // A trash not made yet is judged by where it would be made.
        let home = dir.join("not-yet").join("Trash");
        assert_eq!(trash_for(&entry, &home).expect("a trash"), home);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_the_top_of_a_filesystem_is_the_highest_directory_still_on_it() {
        use std::os::unix::fs::MetadataExt;
        let dir = std::env::temp_dir();
        let device = fs::metadata(&dir).expect("meta").dev();
        let top = mount_top(&dir.join("entry"), device);
        assert!(
            dir.starts_with(&top),
            "{} above {}",
            top.display(),
            dir.display()
        );
        assert_eq!(fs::metadata(&top).expect("meta").dev(), device);
        if let Some(parent) = top.parent() {
            assert_ne!(
                fs::metadata(parent).expect("meta").dev(),
                device,
                "one more step up leaves the filesystem"
            );
        }
    }
}
