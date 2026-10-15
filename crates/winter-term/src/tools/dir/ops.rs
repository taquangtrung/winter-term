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

/// What an entry is called while it waits for another to leave the name it
/// is taking, in a batch of renames that swaps or cycles names.
const PARKED_STEM: &str = ".winter-rename";

// ========================================================================
// Data Structures
// ========================================================================

/// One rename in a batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rename {
    /// How deep the entry sits below the listing's root, which orders the
    /// batch: an entry is renamed before the directory holding it.
    pub depth: usize,
    /// Where the entry is now.
    pub from: PathBuf,
    /// Where it goes.
    pub to: PathBuf,
}

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

/// Whether `renames` can all be carried out without one landing on an entry
/// that stays: two taking the same name, or one taking the name of an entry
/// the batch is not moving. Checked before any of them runs, so a refused
/// batch has changed nothing.
pub fn check_renames(renames: &[Rename]) -> Result<(), String> {
    for (at, rename) in renames.iter().enumerate() {
        if renames[..at].iter().any(|other| other.to == rename.to) {
            return Err(format!("two entries renamed to {}", rename.to.display()));
        }
        let leaving = renames.iter().any(|other| other.from == rename.to);
        if rename.to.exists() && !leaving {
            return Err(format!("{} exists", rename.to.display()));
        }
    }
    Ok(())
}

/// Carry out a checked batch, deepest entries first. At each depth, an entry
/// whose new name another entry still holds is parked under a temporary name
/// first, so a swap (`a` to `b`, `b` to `a`) or a longer cycle goes through
/// rather than failing on its first rename. Reports how many were renamed,
/// and why it stopped where it did.
pub fn rename_all(renames: &[Rename]) -> (usize, Option<String>) {
    let mut ordered = renames.to_vec();
    ordered.sort_by_key(|rename| std::cmp::Reverse(rename.depth));
    let mut done = 0;
    for level in ordered.chunk_by_mut(|one, other| one.depth == other.depth) {
        // Which renames take a name another entry of the batch still holds,
        // decided before anything moves: those wait under a parked name.
        let waits: Vec<bool> = level
            .iter()
            .map(|rename| level.iter().any(|other| other.from == rename.to))
            .collect();
        for (index, rename) in level.iter_mut().enumerate() {
            if !waits[index] {
                continue;
            }
            let parked = rename
                .from
                .with_file_name(format!("{PARKED_STEM}-{}-{index}", std::process::id()));
            if let Err(e) = fs::rename(&rename.from, &parked) {
                return (done, Some(format!("{e} ({done} done)")));
            }
            rename.from = parked;
        }
        // The ones taking a free name go first, which frees the names the
        // parked ones are waiting for.
        let order = (0..level.len())
            .filter(|i| !waits[*i])
            .chain((0..level.len()).filter(|i| waits[*i]));
        for index in order {
            let rename = &level[index];
            if let Err(e) = move_entry(&rename.from, &rename.to) {
                return (done, Some(format!("{e} ({done} done)")));
            }
            done += 1;
        }
    }
    (done, None)
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

    fn rename(tree: &TempTree, from: &str, to: &str) -> Rename {
        Rename {
            depth: 0,
            from: tree.path(from),
            to: tree.path(to),
        }
    }

    #[test]
    fn test_a_batch_swaps_and_cycles_names_without_losing_an_entry() {
        let tree = TempTree::new("rename-cycle");
        for (name, body) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")] {
            fs::write(tree.path(name), body).expect("write");
        }
        let batch = [
            rename(&tree, "a", "b"),
            rename(&tree, "b", "a"),
            rename(&tree, "c", "d"),
            rename(&tree, "d", "e"),
        ];
        assert_eq!(check_renames(&batch), Ok(()));
        assert_eq!(rename_all(&batch), (4, None));
        let read = |name| fs::read_to_string(tree.path(name)).expect("read");
        assert_eq!(
            (read("a"), read("b"), read("d"), read("e")),
            ("B".into(), "A".into(), "C".into(), "D".into())
        );
        assert!(!tree.path("c").exists());
    }

    #[test]
    fn test_a_batch_landing_on_an_entry_that_stays_is_refused_before_anything_moves() {
        let tree = TempTree::new("rename-collide");
        tree.touch("a");
        tree.touch("b");
        tree.touch("keep");
        let onto_kept = [rename(&tree, "a", "keep")];
        assert!(check_renames(&onto_kept).is_err());
        let doubled = [rename(&tree, "a", "x"), rename(&tree, "b", "x")];
        assert!(check_renames(&doubled).is_err());
        assert!(tree.path("a").exists() && tree.path("b").exists());
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
