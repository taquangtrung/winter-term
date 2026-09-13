//! Filesystem operations a listing performs: create, rename, copy, move,
//! delete, and change mode.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// How deep a recursive copy will nest. Symlinks are copied as links rather
/// than followed, so this guards against a pathological tree, not a cycle.
const MAX_COPY_DEPTH: usize = 64;

// ========================================================================
// Functions
// ========================================================================

/// Create an empty file at `path`, refusing to truncate an existing one.
pub fn create_file(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Err(already_exists(path));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(path).map(|_| ())
}

/// Create a directory at `path`, parents included.
pub fn create_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Err(already_exists(path));
    }
    fs::create_dir_all(path)
}

/// Move `from` to `to`, refusing to overwrite. Falls back to copy-then-delete
/// when the two are on different filesystems, which `rename` cannot cross.
pub fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(already_exists(to));
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_entry(from, to)?;
            remove_entry(from)
        }
    }
}

/// Copy `from` to `to`, recursing into a directory. Refuses to overwrite, and
/// refuses to copy a directory into itself.
pub fn copy_entry(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(already_exists(to));
    }
    if to.starts_with(from) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is inside {}", to.display(), from.display()),
        ));
    }
    copy_at(from, to, 0)
}

/// Delete `path`, recursing into a directory.
pub fn remove_entry(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    // A symlinked directory is unlinked, never walked: removing its target's
    // contents is not what deleting a link means.
    if meta.is_dir() && !meta.file_type().is_symlink() {
        return fs::remove_dir_all(path);
    }
    fs::remove_file(path)
}

/// Replace `path`'s permission bits with `mode`.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Changing mode bits needs a platform that has them.
#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "permission bits are a unix concept",
    ))
}

/// Parse an octal mode such as `755` or `0644`.
pub fn parse_mode(text: &str) -> Option<u32> {
    let digits = text.trim();
    if digits.is_empty() || !digits.chars().all(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    u32::from_str_radix(digits, 8).ok()
}

/// Where `name` resolves to, relative to `dir`. A name with a separator in it
/// is rejected rather than quietly writing outside the listed directory.
pub fn resolve_name(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(dir.join(name))
}

fn copy_at(from: &Path, to: &Path, depth: usize) -> io::Result<()> {
    if depth >= MAX_COPY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory nests too deeply to copy",
        ));
    }
    let meta = fs::symlink_metadata(from)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        fs::copy(from, to).map(|_| ())?;
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for child in fs::read_dir(from)?.flatten() {
        copy_at(&child.path(), &to.join(child.file_name()), depth + 1)?;
    }
    Ok(())
}

fn already_exists(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} exists", path.display()),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary tree that removes itself on drop.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-dirops-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn touch(&self, name: &str) -> PathBuf {
            let path = self.path(name);
            fs::write(&path, "contents").expect("temp file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn test_create_refuses_to_truncate_an_existing_file() {
        // `File::create` truncates, so creating over a file the user forgot
        // about would silently destroy it.
        let tree = TempTree::new("create");
        let file = tree.touch("notes.txt");
        assert_eq!(
            create_file(&file).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&file).expect("still there"), "contents");
    }

    #[test]
    fn test_copy_recurses_into_a_directory() {
        let tree = TempTree::new("copy");
        let src = tree.path("src");
        fs::create_dir_all(src.join("inner")).expect("nested dirs");
        fs::write(src.join("inner").join("leaf.txt"), "leaf").expect("leaf");

        copy_entry(&src, &tree.path("dst")).expect("copy");
        assert_eq!(
            fs::read_to_string(tree.path("dst").join("inner").join("leaf.txt")).expect("copied"),
            "leaf"
        );
    }

    #[test]
    fn test_copy_refuses_a_destination_inside_the_source() {
        // Copying `src` into `src/backup` walks what it is writing, forever.
        let tree = TempTree::new("copy-into-self");
        let src = tree.path("src");
        fs::create_dir_all(&src).expect("src");
        assert_eq!(
            copy_entry(&src, &src.join("backup")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn test_move_refuses_to_overwrite() {
        let tree = TempTree::new("move");
        let from = tree.touch("from.txt");
        let onto = tree.touch("onto.txt");
        assert_eq!(
            move_entry(&from, &onto).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(from.exists(), "the source survives a refused move");
    }

    #[test]
    fn test_parse_mode_takes_octal_and_rejects_the_rest() {
        assert_eq!(parse_mode("755"), Some(0o755));
        assert_eq!(parse_mode(" 0644 "), Some(0o644));
        assert_eq!(parse_mode("799"), None, "8 and 9 are not octal digits");
        assert_eq!(parse_mode("rwx"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn test_resolve_name_rejects_anything_that_leaves_the_directory() {
        // A name is a name: `../etc/passwd` typed into a rename prompt must not
        // reach outside the listing.
        let dir = Path::new("/tmp/listing");
        assert_eq!(
            resolve_name(dir, "notes.txt"),
            Some(PathBuf::from("/tmp/listing/notes.txt"))
        );
        assert_eq!(resolve_name(dir, "../escape"), None);
        assert_eq!(resolve_name(dir, "sub/file"), None);
        assert_eq!(resolve_name(dir, ".."), None);
        assert_eq!(resolve_name(dir, "  "), None);
    }
}
