//! The user's own words: a plain text file, one word per line, kept in
//! case-insensitive alphabetical order so it reads well and diffs cleanly.
//!
//! Lines starting with `#` are comments; they are kept but never words.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// The application's directory under the platform config root, shared with
/// the rest of Winter's settings.
const APP_CONFIG_DIR: &str = "winter-term";

/// The spelling directory under the application's config directory.
const SPELLING_DIR: &str = "spelling";

/// The user word list's file name.
const WORDS_FILE: &str = "words.txt";

/// Opens a comment line.
const COMMENT_MARK: char = '#';

/// The Windows per-user config root.
#[cfg(windows)]
const WINDOWS_CONFIG_VAR: &str = "APPDATA";

/// The XDG per-user config root, when set.
#[cfg(not(windows))]
const XDG_CONFIG_VAR: &str = "XDG_CONFIG_HOME";

/// The home directory, whose config root is the fallback.
#[cfg(not(windows))]
const HOME_VAR: &str = "HOME";

/// The config root under the home directory when XDG names none.
#[cfg(not(windows))]
const HOME_CONFIG_DIR: &str = ".config";

/// Appended to the target's name for the temporary file a save renames.
const TEMP_SUFFIX: &str = ".tmp";

// ========================================================================
// Data Structures
// ========================================================================

/// A user word list backed by a file the caller chose.
#[derive(Clone, Debug)]
pub struct UserWords {
    /// The file's non-blank lines, trimmed, comments included.
    lines: Vec<String>,
    path: PathBuf,
}

// ========================================================================
// UserWords
// ========================================================================

impl UserWords {
    /// Reads the list at `path`; a missing file is an empty list.
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let lines = read_lines(&path)?;
        Ok(Self { lines, path })
    }

    /// Adds `word` and saves at once, merging words another process saved
    /// since this list was read. Returns false when it was already there.
    pub fn add(&mut self, word: &str) -> io::Result<bool> {
        let word = word.trim();
        self.lines = read_lines(&self.path)?;
        if word.is_empty() || self.contains(word) {
            return Ok(false);
        }
        let key = sort_key(word);
        let at = self
            .lines
            .iter()
            .position(|line| sort_key(line) > key)
            .unwrap_or(self.lines.len());
        self.lines.insert(at, word.to_string());
        self.save()?;
        Ok(true)
    }

    /// Whether `word` is in the list, ignoring case.
    pub fn contains(&self, word: &str) -> bool {
        let key = sort_key(word);
        self.words().any(|existing| sort_key(existing) == key)
    }

    /// The file this list reads and saves.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The words, in file order, without comments.
    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.starts_with(COMMENT_MARK))
    }

    /// Writes through a temporary file and a rename, so a crash never
    /// leaves the list half written; a symlinked list stays a link.
    fn save(&self) -> io::Result<()> {
        let target = fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut temp_name = target.file_name().unwrap_or_default().to_os_string();
        temp_name.push(TEMP_SUFFIX);
        let temp = target.with_file_name(temp_name);
        let mut text = self.lines.join("\n");
        text.push('\n');
        fs::write(&temp, text)?;
        fs::rename(&temp, &target)
    }
}

// ========================================================================
// Default Location
// ========================================================================

/// Where the user's words live by default: `spelling/words.txt` under
/// `%APPDATA%\winter-term` on Windows and `$XDG_CONFIG_HOME/winter-term`
/// or `~/.config/winter-term` elsewhere; `None` with no such root set.
pub fn default_user_words_path() -> Option<PathBuf> {
    Some(
        config_root()?
            .join(APP_CONFIG_DIR)
            .join(SPELLING_DIR)
            .join(WORDS_FILE),
    )
}

#[cfg(windows)]
fn config_root() -> Option<PathBuf> {
    env::var_os(WINDOWS_CONFIG_VAR).map(PathBuf::from)
}

#[cfg(not(windows))]
fn config_root() -> Option<PathBuf> {
    env::var_os(XDG_CONFIG_VAR)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os(HOME_VAR).map(|home| PathBuf::from(home).join(HOME_CONFIG_DIR)))
}

// ========================================================================
// Helpers
// ========================================================================

fn read_lines(path: &Path) -> io::Result<Vec<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn sort_key(line: &str) -> String {
    line.to_lowercase()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::process;

    use super::*;

    /// A fresh path under the system temp directory, unique to this test.
    fn scratch_path(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("winter-spell-{}-{test}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested").join(WORDS_FILE)
    }

    #[test]
    fn test_add_keeps_file_sorted_and_skips_duplicates() {
        let path = scratch_path("sorted");
        let mut words = UserWords::load(&path).unwrap();
        assert_eq!(words.words().count(), 0);
        assert!(words.add("tokio").unwrap());
        assert!(words.add("Axum").unwrap());
        assert!(words.add("serde").unwrap());
        assert!(!words.add("SERDE").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Axum\nserde\ntokio\n");
        assert!(words.contains("axum"));
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn test_add_merges_words_saved_elsewhere_and_keeps_comments() {
        let path = scratch_path("merge");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# mine\n\nbeta\n").unwrap();
        let mut words = UserWords::load(&path).unwrap();
        fs::write(&path, "# mine\nalpha\nbeta\n").unwrap();
        words.add("gamma").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# mine\nalpha\nbeta\ngamma\n"
        );
        assert_eq!(
            words.words().collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }
}
