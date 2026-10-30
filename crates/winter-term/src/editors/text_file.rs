//! Reading a file into editable lines and writing them back: the half of the
//! editor that loses someone's work when it is wrong.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use crate::model::editor::FileShape;

// ========================================================================
// Constants
// ========================================================================

/// The largest file the editor will open. Past this the buffer is read, split,
/// and repainted on the event-loop thread often enough to stall the window, so
/// refusing is kinder than a frozen frame: the terminal underneath still has a
/// shell in it, and `$EDITOR` is one key away.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The byte that says these are not characters. A file holding one is refused
/// rather than opened, since every path through the buffer treats its contents
/// as text and would write back something the original was not.
const NUL: u8 = 0;

/// The byte order mark some editors put at the head of a UTF-8 file. It is no
/// character of the text, so it is kept aside rather than shown, and written
/// back so a save does not quietly change the file's first bytes.
const BOM: char = '\u{feff}';

/// How the file being written is named while it is still half-written. The
/// process id keeps two Winters saving the same path from colliding on it.
const TEMP_STEM: &str = ".winter-save";

// ========================================================================
// Data Structures
// ========================================================================

/// A file read into lines, paired with everything a write has to reproduce.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedFile {
    /// The contents, split on newlines, with no terminator of their own.
    pub lines: Vec<String>,
    /// The layout a write has to reproduce.
    pub shape: FileShape,
    /// What the file read as, for spotting a write from elsewhere.
    pub stamp: FileStamp,
}

/// What a file looked like when it was read, so a write can tell whether
/// something else has touched it in between. Size and mtime rather than a
/// hash: the check runs on every save, and a rewrite that leaves both
/// identical is not something an editor can do anything about anyway.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileStamp {
    /// The file's size in bytes.
    pub len: u64,
    /// When the filesystem last recorded a write, where it reports one.
    pub modified: Option<SystemTime>,
}

// ========================================================================
// Functions
// ========================================================================

/// Read `path` into lines, refusing whatever the editor could not put back
/// unchanged: bytes that are not UTF-8, bytes that are not text at all, and
/// files large enough to stall the frame that opens them.
pub fn load(path: &Path) -> io::Result<LoadedFile> {
    let stamp = stamp_of(path)?;
    check_size(path, stamp.len)?;
    decode(path, fs::read(path)?, stamp)
}

/// Refuse a file of `len` bytes when it is too large to open.
pub(super) fn check_size(path: &Path, len: u64) -> io::Result<()> {
    match len > MAX_BYTES {
        true => Err(refused(format!(
            "{} is {}, over the {} the editor opens",
            path.display(),
            bytes(len),
            bytes(MAX_BYTES)
        ))),
        false => Ok(()),
    }
}

/// `raw`, the bytes read from `path`, as editable lines, refusing what is
/// not UTF-8 text.
pub(super) fn decode(path: &Path, raw: Vec<u8>, stamp: FileStamp) -> io::Result<LoadedFile> {
    check_size(path, raw.len() as u64)?;
    if raw.contains(&NUL) {
        return Err(refused(format!("{} is not a text file", path.display())));
    }
    let text =
        String::from_utf8(raw).map_err(|_| refused(format!("{} is not UTF-8", path.display())))?;
    let (bom, text) = match text.strip_prefix(BOM) {
        Some(rest) => (true, rest.to_string()),
        None => (false, text),
    };
    let crlf = is_all_crlf(&text);
    let shape = FileShape {
        bom,
        crlf,
        trailing_newline: text.ends_with('\n'),
    };
    Ok(LoadedFile {
        lines: split_lines(&text, crlf),
        shape,
        stamp,
    })
}

/// Write `lines` to `path` as one replacement, and report what the file now
/// looks like.
pub fn save(path: &Path, lines: &[String], shape: FileShape) -> io::Result<FileStamp> {
    write_user_file(path, joined(lines, shape).as_bytes())
}

/// Write `contents` over the user's file at `path`, and report what it now
/// looks like. The bytes land in a temporary file beside the original, made
/// with its mode, flushed, and renamed over it, so an interrupted save leaves
/// the previous contents whole. Where a rename would change what the file is
/// rather than what it holds (another owner, other hard links, a directory
/// this user cannot write in), it is rewritten in place instead.
pub fn write_user_file(path: &Path, contents: &[u8]) -> io::Result<FileStamp> {
    // Through the link, never over it: saving a file reached by a symlink
    // replaces what it points at, the way every other editor treats one.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = fs::metadata(&target).ok();
    let temp = temp_beside(&target)?;
    let created = create_temp(&temp, existing.as_ref());
    let file = match created {
        Ok(file) => file,
        // A file the user may write in a directory they may not: the rename
        // cannot happen, but writing the file itself still can.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() => {
            return write_in_place(&target, contents);
        }
        Err(e) => return Err(e),
    };
    if let Some(meta) = &existing {
        if is_kept_in_place(meta, &file) {
            drop(file);
            let _ = fs::remove_file(&temp);
            return write_in_place(&target, contents);
        }
    }
    let written = fill_and_sync(file, contents)
        .and_then(|()| stamp_of(&temp))
        .and_then(|stamp| fs::rename(&temp, &target).map(|()| stamp));
    match written {
        Ok(stamp) => {
            sync_dir(&target);
            Ok(stamp)
        }
        Err(e) => {
            // The half-written file is worth less than the directory it litters.
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Whether the file `existing` describes has to be rewritten where it is to
/// stay the same file: renaming `temp`, just made by this user, over it would
/// hand it to another owner or group, or split it from its other names.
fn is_kept_in_place(existing: &fs::Metadata, temp: &fs::File) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(made) = temp.metadata() else {
            return true;
        };
        existing.nlink() > 1 || existing.uid() != made.uid() || existing.gid() != made.gid()
    }
    #[cfg(not(unix))]
    {
        let _ = (existing, temp);
        false
    }
}

/// Make the temporary file at `temp` with `existing`'s permission bits from
/// the start, so a private file is never readable by others while it is
/// being written.
fn create_temp(temp: &Path, existing: Option<&fs::Metadata>) -> io::Result<fs::File> {
    match fs::remove_file(temp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(meta) = existing {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(meta.permissions().mode());
    }
    let file = options.open(temp)?;
    // The mode at creation is cut by the umask; the bits the file had are
    // what it keeps.
    if let Some(meta) = existing {
        file.set_permissions(meta.permissions())?;
    }
    Ok(file)
}

/// Write `contents` into `file` and flush them to the disk, so the rename
/// that follows never publishes a file whose bytes are still in flight.
fn fill_and_sync(mut file: fs::File, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    file.write_all(contents)?;
    file.sync_all()
}

/// Rewrite `target` where it is, keeping its owner, links, and mode.
fn write_in_place(target: &Path, contents: &[u8]) -> io::Result<FileStamp> {
    let file = fs::OpenOptions::new().write(true).truncate(true).open(target)?;
    fill_and_sync(file, contents)?;
    stamp_of(target)
}

/// Flush the directory holding `target`, so the rename survives a crash.
/// Best effort: not every platform can open a directory to flush it.
fn sync_dir(target: &Path) {
    if let Some(parent) = target.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

/// Whether something else has written to `path` since `stamp` was taken, in
/// which case saving would throw that away without either side knowing.
pub fn is_stale(path: &Path, stamp: &FileStamp) -> bool {
    match stamp_of(path) {
        Ok(now) => now != *stamp,
        // A file that has gone missing is not what was opened either, but a
        // save recreates it, which is the more useful answer than refusing.
        Err(_) => false,
    }
}

/// What `path` currently reads as, for comparing against what it read as when
/// it was opened.
pub(super) fn stamp_of(path: &Path) -> io::Result<FileStamp> {
    let meta = fs::metadata(path)?;
    Ok(FileStamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Whether every line of `text` ends `\r\n`. A file mixing the two endings
/// is not a CRLF file: its carriage returns stay in its lines, so a save puts
/// each line back with the ending it had.
fn is_all_crlf(text: &str) -> bool {
    let newlines = text.matches('\n').count();
    newlines > 0 && text.matches("\r\n").count() == newlines
}

/// The text split into editable lines, with the line terminators dropped: a
/// trailing newline ends the last line rather than starting an empty one, so
/// the count on screen matches what every other editor reports. In a CRLF
/// file the carriage return before each newline is part of the terminator; a
/// last line with no newline keeps any carriage return it ends with.
fn split_lines(text: &str, crlf: bool) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let terminated = lines.len() - 1;
    if crlf {
        for line in &mut lines[..terminated] {
            line.pop();
        }
    }
    if text.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// The lines back as bytes, in the layout the file had when it was read.
pub(super) fn joined(lines: &[String], shape: FileShape) -> String {
    let ending = if shape.crlf { "\r\n" } else { "\n" };
    let mut text = match shape.bom {
        true => BOM.to_string(),
        false => String::new(),
    };
    text.push_str(&lines.join(ending));
    if shape.trailing_newline {
        text.push_str(ending);
    }
    text
}

/// A path beside `target` to assemble the new contents in. The same directory,
/// so the rename that follows stays within one filesystem and is therefore
/// atomic; `/tmp` may well be another one.
fn temp_beside(target: &Path) -> io::Result<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| refused(format!("{} has no directory to write in", target.display())))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(parent.join(format!("{TEMP_STEM}-{}-{name}", std::process::id())))
}

/// A refusal to open or write something, in the form the editors report.
fn refused(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A byte count in the largest unit that leaves it readable.
fn bytes(count: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    match count {
        n if n >= MIB => format!("{:.1}M", n as f64 / MIB as f64),
        n if n >= KIB => format!("{:.1}K", n as f64 / KIB as f64),
        n => format!("{n}B"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory that removes itself on drop, so a test that
    /// writes files never leaks state into the next run.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("winter-editor-file-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).expect("temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_a_trailing_newline_ends_the_last_line_rather_than_starting_one() {
        // Splitting on '\n' naively leaves a phantom empty last line, which
        // shows as a line the file does not have and grows one per save.
        let loaded = split_lines("one\ntwo\n", false);
        assert_eq!(loaded, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(split_lines("one\ntwo", false), vec!["one", "two"]);
        assert_eq!(split_lines("", false), vec![""], "an empty file is one empty line");
    }

    #[test]
    fn test_a_file_saves_back_byte_for_byte_when_nothing_was_edited() {
        // The shapes a naive save silently rewrites: CRLF endings, and a file
        // that deliberately ends without a newline. Either one turns a
        // one-word edit into a whole-file diff.
        let tmp = TempDir::new("shape");
        for (name, raw) in [
            ("crlf.txt", "one\r\ntwo\r\n"),
            ("nolf.txt", "one\ntwo"),
            ("plain.txt", "one\ntwo\n"),
            ("crlf-nolf.txt", "one\r\ntwo"),
            ("bom.txt", "\u{feff}one\ntwo\n"),
            ("mixed-lf.txt", "one\ntwo\r\nthree\n"),
            ("mixed-crlf.txt", "one\r\ntwo\nthree\r\n"),
            ("lone-cr.txt", "one\r\ntwo\r"),
        ] {
            let path = tmp.write(name, raw);
            let loaded = load(&path).expect("loads");
            assert_eq!(loaded.lines[0].trim_end_matches('\r'), "one", "{name}: the mark is not text");
            save(&path, &loaded.lines, loaded.shape).expect("saves");
            let after = fs::read_to_string(&path).expect("reads back");
            assert_eq!(after, raw, "{name} came back changed");
        }
    }

    #[test]
    fn test_binary_and_non_utf8_files_are_refused_rather_than_mangled() {
        // Opening either one and saving it writes back something the original
        // was not: the NUL bytes gone, the invalid sequences replaced.
        let tmp = TempDir::new("binary");
        let binary = tmp.write("a.bin", [0x7f, 0x45, NUL, 0x02]);
        assert!(load(&binary).is_err(), "a file holding NUL is not text");

        let latin1 = tmp.write("b.txt", [0x63, 0x61, 0x66, 0xe9, 0x0a]);
        assert!(load(&latin1).is_err(), "not UTF-8, so not ours to rewrite");
    }

    #[test]
    fn test_saving_through_a_symlink_writes_what_it_points_at() {
        // Writing the link's own path would replace the link with a regular
        // file, silently detaching it from the file it stood for.
        #[cfg(unix)]
        {
            let tmp = TempDir::new("symlink");
            let real = tmp.write("real.txt", "before\n");
            let link = tmp.0.join("link.txt");
            std::os::unix::fs::symlink(&real, &link).expect("symlink");

            save(
                &link,
                &["after".to_string()],
                FileShape {
                    bom: false,
                    crlf: false,
                    trailing_newline: true,
                },
            )
            .expect("saves");

            assert_eq!(fs::read_to_string(&real).expect("real"), "after\n");
            assert!(
                fs::symlink_metadata(&link)
                    .expect("link")
                    .file_type()
                    .is_symlink(),
                "the link is still a link"
            );
        }
    }

    #[test]
    fn test_saving_keeps_the_permission_bits_the_file_had() {
        // A save that drops the executable bit turns a script into a file that
        // no longer runs, and nothing on screen says so.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let tmp = TempDir::new("mode");
            let path = tmp.write("run.sh", "echo hi\n");
            let mut mode = fs::metadata(&path).expect("meta").permissions();
            mode.set_mode(0o755);
            fs::set_permissions(&path, mode).expect("chmod");

            save(
                &path,
                &["echo bye".to_string()],
                FileShape {
                    bom: false,
                    crlf: false,
                    trailing_newline: true,
                },
            )
            .expect("saves");

            let after = fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(after & 0o777, 0o755, "the executable bit survived");
        }
    }

    #[test]
    fn test_saving_a_hard_linked_file_keeps_its_other_name_in_step() {
        // A rename over one name would leave the other holding the old bytes,
        // splitting what was one file into two.
        let tmp = TempDir::new("hardlink");
        let path = tmp.write("a.txt", "before\n");
        let other = tmp.0.join("b.txt");
        fs::hard_link(&path, &other).expect("link");
        write_user_file(&path, b"after\n").expect("saves");
        assert_eq!(fs::read_to_string(&other).expect("other"), "after\n");
    }

    #[test]
    fn test_a_writable_file_in_a_read_only_directory_still_saves() {
        // The temporary file has nowhere to go, but the file itself may be
        // written, and refusing would push the user toward sudo for nothing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let tmp = TempDir::new("rodir");
            let dir = tmp.0.join("locked");
            fs::create_dir(&dir).expect("dir");
            let path = dir.join("a.txt");
            fs::write(&path, "before\n").expect("file");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("chmod");
            let saved = write_user_file(&path, b"after\n");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod back");
            saved.expect("saves in place");
            assert_eq!(fs::read_to_string(&path).expect("reads"), "after\n");
        }
    }

    #[test]
    fn test_a_write_from_elsewhere_shows_up_as_stale() {
        // Without this a save silently throws away whatever the other writer
        // put there, which is the one unrecoverable thing an editor can do.
        let tmp = TempDir::new("stale");
        let path = tmp.write("a.txt", "one\n");
        let loaded = load(&path).expect("loads");
        assert!(!is_stale(&path, &loaded.stamp));

        fs::write(&path, "one\ntwo\n").expect("outside write");
        assert!(is_stale(&path, &loaded.stamp));
    }

    #[test]
    fn test_a_file_too_large_to_repaint_is_refused() {
        let tmp = TempDir::new("large");
        let path = tmp.write("big.txt", vec![b'x'; (MAX_BYTES + 1) as usize]);
        let error = load(&path).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
