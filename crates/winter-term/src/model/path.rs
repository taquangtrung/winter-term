//! Path manipulation and home-directory abbreviation.
//!
//! Mirrors `magic-vscode` file reference formatting:
//! - Abbreviates paths under the user's home directory with `~`
//!   (e.g. `~\Workspace\Apps\winter-term` on Windows or `~/workspace/...` on POSIX).
//! - Normalizes path separators and supports case-insensitivity on Windows.
//! - Preserves line/column suffix references (e.g. `:10` or `:10-20`).
//! - Preserves URLs and multi-line strings.

use std::path::{Path, PathBuf};

// ========================================================================
// Functions
// ========================================================================

/// The user's home directory across platforms.
///
/// On Windows, checks `%USERPROFILE%` first, then `%HOME%`.
/// On non-Windows, checks `$HOME` first, then `$USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }
}

/// Abbreviate `path` with `~` if it sits within the user's home directory.
pub fn abbreviate_home(path: &Path) -> String {
    let s = path.to_string_lossy();
    format_file_reference(&s)
}

/// Format a file path or location for copying as a reference.
///
/// Multi-line inputs (e.g. multiple files marked in a directory listing) are
/// split by newline and formatted line by line.
pub fn format_file_reference(location: &str) -> String {
    let Some(home) = home_dir() else {
        return normalize_separators(location);
    };
    format_file_reference_with_home(location, &home)
}

/// Format `location` using the specified `home` directory.
pub fn format_file_reference_with_home(location: &str, home: &Path) -> String {
    if location.is_empty() {
        return String::new();
    }
    location
        .lines()
        .map(|line| format_single_reference(line, home))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_single_reference(location: &str, home: &Path) -> String {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if is_url(trimmed) {
        return trimmed.to_string();
    }

    let (raw_path, suffix) = split_line_suffix(trimmed);
    let abbreviated = abbreviate_single_path(raw_path, home);
    format!("{abbreviated}{suffix}")
}

fn is_url(text: &str) -> bool {
    text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("ws://")
        || text.starts_with("wss://")
}

fn split_line_suffix(location: &str) -> (&str, &str) {
    if let Some(colon_pos) = location.rfind(':') {
        if colon_pos == 1 && location.len() <= 3 {
            return (location, "");
        }
        let after = &location[colon_pos + 1..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ':') {
            let before = &location[..colon_pos];
            if before.len() > 1 || !before.chars().all(|c| c.is_ascii_alphabetic()) {
                return (before, &location[colon_pos..]);
            }
        }
    }
    (location, "")
}

/// Format `path` as a full absolute path, normalized for the current OS.
///
/// On Windows: backslashes `\` and Windows drive letters (`C:\...`).
/// On non-Windows: forward slashes `/` (`/...`).
pub fn format_full_path(path: &Path) -> String {
    normalize_separators(&path.to_string_lossy())
}

/// Normalize a filesystem path (e.g. from OSC 7 or user input) into a valid `PathBuf` for the current OS.
pub fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(normalize_separators(&path.as_ref().to_string_lossy()))
}

fn normalize_separators(path: &str) -> String {
    #[cfg(windows)]
    {
        let s = if path.starts_with('/') && path.len() >= 3 && path.as_bytes()[2] == b':' {
            &path[1..]
        } else if path.starts_with('/')
            && path.len() >= 3
            && (path.as_bytes()[2] == b'/' || path.len() == 2)
            && path.as_bytes()[1].is_ascii_alphabetic()
        {
            let drive = (path.as_bytes()[1] as char).to_ascii_uppercase();
            let rest = if path.len() > 2 { &path[2..] } else { "" };
            return format!("{drive}:{rest}").replace('/', "\\");
        } else {
            path
        };
        s.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        path.replace('\\', "/")
    }
}

fn abbreviate_single_path(raw_path: &str, home: &Path) -> String {
    let normalized_raw = normalize_separators(raw_path);
    let home_str = normalize_separators(&home.to_string_lossy());

    let target_trimmed = normalized_raw.trim_end_matches(['\\', '/']);
    let home_trimmed = home_str.trim_end_matches(['\\', '/']);

    #[cfg(windows)]
    {
        let is_same = target_trimmed.eq_ignore_ascii_case(home_trimmed);
        if is_same {
            return "~".to_string();
        }
        let prefix_len = home_trimmed.len();
        if normalized_raw.len() > prefix_len {
            let matches_prefix = normalized_raw[..prefix_len].eq_ignore_ascii_case(home_trimmed);
            let sep = normalized_raw.as_bytes()[prefix_len];
            if matches_prefix && (sep == b'\\' || sep == b'/') {
                let rest = &normalized_raw[prefix_len + 1..];
                return format!("~\\{rest}");
            }
        }
    }

    #[cfg(not(windows))]
    {
        if target_trimmed == home_trimmed {
            return "~".to_string();
        }
        let prefix_len = home_trimmed.len();
        if normalized_raw.len() > prefix_len {
            let matches_prefix = &normalized_raw[..prefix_len] == home_trimmed;
            let sep = normalized_raw.as_bytes()[prefix_len];
            if matches_prefix && (sep == b'/' || sep == b'\\') {
                let rest = &normalized_raw[prefix_len + 1..];
                return format!("~/{rest}");
            }
        }
    }

    normalized_raw
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_home_abbreviates_to_tilde() {
        #[cfg(windows)]
        {
            let home = Path::new(r"C:\Users\Trung");
            assert_eq!(
                format_file_reference_with_home(r"C:\Users\Trung", home),
                "~"
            );
            assert_eq!(
                format_file_reference_with_home(r"c:\users\trung\", home),
                "~"
            );
        }
        #[cfg(not(windows))]
        {
            let home = Path::new("/home/user");
            assert_eq!(
                format_file_reference_with_home("/home/user", home),
                "~"
            );
            assert_eq!(
                format_file_reference_with_home("/home/user/", home),
                "~"
            );
        }
    }

    #[test]
    fn test_subpath_under_home_abbreviates_with_tilde_prefix() {
        #[cfg(windows)]
        {
            let home = Path::new(r"C:\Users\Trung");
            assert_eq!(
                format_file_reference_with_home(r"C:\Users\Trung\Workspace\Apps\winter-term", home),
                r"~\Workspace\Apps\winter-term"
            );
            // Forward slashes normalized to backslashes on Windows
            assert_eq!(
                format_file_reference_with_home("C:/Users/Trung/Workspace/Apps/winter-term", home),
                r"~\Workspace\Apps\winter-term"
            );
            // Case-insensitive drive letter and username on Windows
            assert_eq!(
                format_file_reference_with_home(r"c:\users\trung\docs\file.txt", home),
                r"~\docs\file.txt"
            );
        }
        #[cfg(not(windows))]
        {
            let home = Path::new("/home/user");
            assert_eq!(
                format_file_reference_with_home("/home/user/workspace/app", home),
                "~/workspace/app"
            );
            // Backslashes normalized to forward slashes on non-Windows
            assert_eq!(
                format_file_reference_with_home(r"/home/user\workspace\app", home),
                "~/workspace/app"
            );
        }
    }

    #[test]
    fn test_line_suffix_preserved() {
        #[cfg(windows)]
        {
            let home = Path::new(r"C:\Users\Trung");
            assert_eq!(
                format_file_reference_with_home(r"C:\Users\Trung\file.rs:42", home),
                r"~\file.rs:42"
            );
            assert_eq!(
                format_file_reference_with_home(r"C:\Users\Trung\file.rs:10-25", home),
                r"~\file.rs:10-25"
            );
        }
        #[cfg(not(windows))]
        {
            let home = Path::new("/home/user");
            assert_eq!(
                format_file_reference_with_home("/home/user/file.rs:42", home),
                "~/file.rs:42"
            );
            assert_eq!(
                format_file_reference_with_home("/home/user/file.rs:10-25", home),
                "~/file.rs:10-25"
            );
        }
    }

    #[test]
    fn test_path_outside_home_retained() {
        #[cfg(windows)]
        {
            let home = Path::new(r"C:\Users\Trung");
            assert_eq!(
                format_file_reference_with_home(r"C:\Windows\System32", home),
                r"C:\Windows\System32"
            );
            assert_eq!(
                format_file_reference_with_home(r"D:\Projects\other", home),
                r"D:\Projects\other"
            );
        }
        #[cfg(not(windows))]
        {
            let home = Path::new("/home/user");
            assert_eq!(
                format_file_reference_with_home("/var/log/syslog", home),
                "/var/log/syslog"
            );
        }
    }

    #[test]
    fn test_format_full_path_and_normalize_path() {
        #[cfg(windows)]
        {
            let p1 = Path::new("C:/Users/Trung/Workspace");
            assert_eq!(format_full_path(p1), r"C:\Users\Trung\Workspace");
            assert_eq!(normalize_path(p1), PathBuf::from(r"C:\Users\Trung\Workspace"));

            let p2 = Path::new("/C:/Users/Trung/Workspace");
            assert_eq!(format_full_path(p2), r"C:\Users\Trung\Workspace");
            assert_eq!(normalize_path(p2), PathBuf::from(r"C:\Users\Trung\Workspace"));

            let p3 = Path::new("/c/Users/Trung/Workspace");
            assert_eq!(format_full_path(p3), r"C:\Users\Trung\Workspace");
            assert_eq!(normalize_path(p3), PathBuf::from(r"C:\Users\Trung\Workspace"));
        }
        #[cfg(not(windows))]
        {
            let p1 = Path::new("/home/user/workspace");
            assert_eq!(format_full_path(p1), "/home/user/workspace");
            assert_eq!(normalize_path(p1), PathBuf::from("/home/user/workspace"));

            let p2 = Path::new(r"/home/user\workspace");
            assert_eq!(format_full_path(p2), "/home/user/workspace");
            assert_eq!(normalize_path(p2), PathBuf::from("/home/user/workspace"));
        }
    }
}
