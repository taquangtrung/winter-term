//! What runs on the far side, and reading back what it prints.
//!
//! Each script is one line of POSIX `sh` run as `sh -c SCRIPT sh ARGS...`,
//! so a path arrives as `$1` and is never part of the script's text. They
//! are written to survive being single-quoted for any login shell an SSH
//! host might have: one line, and no backslash anywhere.

use super::{Entry, Stat};

// ========================================================================
// Constants
// ========================================================================

/// The entries of directory `$1`. The first line says which form follows:
/// `stat` for `TYPE/SIZE/NAME` lines from GNU, busybox, or BSD `stat`, `ls`
/// for bare names (a directory's ending in `/`) where there is no `stat`.
/// A `/` cannot be part of a name, so it is the one safe separator. A link
/// reads as what it leads to; one leading nowhere is left out.
pub(super) const LIST: &str = concat!(
    "cd -- \"$1\" || exit 3; set --; ",
    "for f in * .[!.]* ..?*; do if [ -e \"$f\" ] || [ -L \"$f\" ]; then set -- \"$@\" \"$f\"; fi; done; ",
    "if stat -c %n / >/dev/null 2>&1; then echo stat; [ $# -gt 0 ] && stat -L -c '%F/%s/%n' -- \"$@\" 2>/dev/null; ",
    "elif stat -f %N / >/dev/null 2>&1; then echo stat; [ $# -gt 0 ] && stat -L -f '%HT/%z/%N' -- \"$@\" 2>/dev/null; ",
    "else echo ls; ls -1Ap; fi; exit 0"
);

/// Succeeds when `$1` is a directory.
pub(super) const IS_DIR: &str = "[ -d \"$1\" ]";

/// The bytes of file `$1`.
pub(super) const READ: &str = "[ -d \"$1\" ] && exit 5; exec cat -- \"$1\"";

/// `SIZE MTIME` of file `$1`, in bytes and whole seconds.
pub(super) const STAT: &str = concat!(
    "stat -L -c '%s %Y' -- \"$1\" 2>/dev/null || ",
    "stat -L -f '%z %m' -- \"$1\" 2>/dev/null || exit 3"
);

/// Standard input over file `$1`, which is `$2` bytes long. The bytes land
/// in a temporary file beside it first and are checked for length, so a
/// transfer cut short leaves the file whole; they are then copied over it
/// rather than renamed, which keeps the file's inode, owner, and mode, and
/// any link to it. Where no temporary file can be made beside it, the write
/// goes straight in.
pub(super) const WRITE: &str = concat!(
    "[ -d \"$1\" ] && exit 5; ",
    "t=$(mktemp \"$1.winter.XXXXXX\" 2>/dev/null) || t=; ",
    "if [ -n \"$t\" ]; then trap 'rm -f \"$t\"' EXIT; cat > \"$t\" || exit 1; ",
    "[ \"$(wc -c < \"$t\" | tr -d ' ')\" = \"$2\" ] || exit 8; cat -- \"$t\" > \"$1\"; ",
    "else cat > \"$1\"; fi"
);

/// What the scripts' own exit codes mean.
pub(super) const EXIT_MEANINGS: [(i32, &str); 3] = [
    (3, "no such file or directory, or no way in"),
    (5, "is a directory"),
    (
        8,
        "the new contents arrived cut short; the file is unchanged",
    ),
];

/// The first line of a listing, saying which form the rest is in.
const FORM_STAT: &str = "stat";
const FORM_LS: &str = "ls";

/// What `stat` calls a directory, in either spelling, lower-cased.
const DIRECTORY: &str = "directory";

// ========================================================================
// Functions
// ========================================================================

/// The entries a [`LIST`] printed.
pub(super) fn parse_listing(output: &str) -> Vec<Entry> {
    let mut lines = output.lines();
    let parse_line: fn(&str) -> Option<Entry> = match lines.next() {
        Some(FORM_STAT) => stat_line,
        Some(FORM_LS) => ls_line,
        _ => return Vec::new(),
    };
    lines.filter_map(parse_line).collect()
}

/// A `TYPE/SIZE/NAME` line.
fn stat_line(line: &str) -> Option<Entry> {
    let mut fields = line.splitn(3, '/');
    let kind = fields.next()?;
    let size = fields.next()?.parse().ok();
    let name = fields.next().filter(|name| !name.is_empty())?;
    Some(Entry {
        is_dir: kind.to_lowercase().starts_with(DIRECTORY),
        name: name.to_string(),
        size,
    })
}

/// A bare name, ending in `/` for a directory.
fn ls_line(line: &str) -> Option<Entry> {
    let (name, is_dir) = match line.strip_suffix('/') {
        Some(name) => (name, true),
        None => (line, false),
    };
    (!name.is_empty()).then(|| Entry {
        is_dir,
        name: name.to_string(),
        size: None,
    })
}

/// The `SIZE MTIME` a [`STAT`] printed.
pub(super) fn parse_stat(output: &str) -> Option<Stat> {
    let mut fields = output.split_whitespace();
    let len = fields.next()?.parse().ok()?;
    Some(Stat::from_secs(
        len,
        fields.next().and_then(|secs| secs.parse().ok()),
    ))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_script_holds_a_backslash_or_a_line_break() {
        // Either would come apart inside the single quotes a fish or csh
        // login shell reads the command through.
        for script in [LIST, IS_DIR, READ, STAT, WRITE] {
            assert!(!script.contains('\\'), "{script}");
            assert!(!script.contains('\n'), "{script}");
        }
    }

    #[test]
    fn test_a_listing_reads_either_stat_spelling_and_the_ls_fallback() {
        let gnu = "stat\ndirectory/4096/etc\nregular file/12/a b.txt\nregular empty file/0/empty\n";
        assert_eq!(
            parse_listing(gnu),
            [
                Entry {
                    is_dir: true,
                    name: "etc".into(),
                    size: Some(4096)
                },
                Entry {
                    is_dir: false,
                    name: "a b.txt".into(),
                    size: Some(12)
                },
                Entry {
                    is_dir: false,
                    name: "empty".into(),
                    size: Some(0)
                },
            ]
        );
        let bsd = "stat\nDirectory/64/usr\n";
        assert!(parse_listing(bsd)[0].is_dir);

        let ls = "ls\nbin/\nREADME\n";
        assert_eq!(
            parse_listing(ls),
            [
                Entry {
                    is_dir: true,
                    name: "bin".into(),
                    size: None
                },
                Entry {
                    is_dir: false,
                    name: "README".into(),
                    size: None
                },
            ]
        );
        assert!(parse_listing("garbage\nfile").is_empty());
    }

    #[test]
    fn test_a_stat_reads_size_and_seconds() {
        let stat = parse_stat("1234 1700000000\n").expect("parses");
        assert_eq!(stat.len, 1234);
        assert!(stat.modified.is_some());
        assert_eq!(parse_stat(""), None);
    }
}
