//! Finding the places a build's output points at: the `path:line:col` a
//! compiler, a test runner, or a linter prints beside what it found there.
//!
//! Each finished line is read once, in order, by a [`Locator`] that carries
//! what the lines before it said: the directory `make` last entered, and the
//! kind of message `rustc` opened before printing the `-->` line that places
//! it. The formats read are the ones nearly every tool prints one of:
//!
//! - GNU, `path:line[:col]: [error|warning|note]: message`, which gcc,
//!   clang, go, TeX's `-file-line-error`, pytest, and most linters print;
//! - rustc's, a `--> path:line:col` under an `error:` or `warning:` line;
//! - MSVC's and the .NET tools', `path(line[,col]): error CODE: message`;
//! - Python's tracebacks, `File "path", line N`.

use std::ops::Range;
use std::path::PathBuf;

use regex::Regex;

use crate::model::paint::DiagnosticLevel;

// ========================================================================
// Constants
// ========================================================================

/// The message that opens a rustc diagnostic: its kind, and maybe a code.
const RUST_HEADER: &str = r"^(error|warning|note|help)(?:\[[A-Za-z0-9]+\])?:";

/// Where rustc places what the message above it is about. `:::` marks a
/// place that is only related, a note's.
const RUST_ARROW: &str = r"^\s*(-->|:::)\s+(\S.*?):(\d+):(\d+)\s*$";

/// A Python traceback's frame.
const PYTHON_FRAME: &str = r#"^\s*File "([^"]+)", line (\d+)"#;

/// MSVC's `path(line,col): error C2065: ...`.
const MSVC: &str = r"^\s*([^\s(][^(]*?)\((\d+)(?:,(\d+))?\)\s*:\s*(fatal error|error|warning|note)";

/// GNU's `path:line[:col][-range]: message`, the path as gcc writes it: no
/// blanks, which is what keeps a sentence with a colon in it from reading as
/// one. `In file included from` and its `from` continuations are the same
/// place written as a lead-in.
const GNU: &str = r"^\s*(?:In file included from |\s*from )?([^\s:]+):(\d+)(?:[:.](\d+))?(?:-\d+(?:[:.]\d+)?)?:(?:\s+(.*))?$";

/// `make` saying which directory the paths it prints next are relative to.
const MAKE_DIRECTORY: &str =
    r#"^\S*make(?:\.exe)?(?:\[\d+\])?: (Entering|Leaving) directory [`'‘"](.+?)['’"]\s*$"#;

/// How many directories above the build's own a relative path is looked for
/// in when it is not under the build's: `cargo` prints a workspace member's
/// paths from the workspace root, wherever it was run from.
const ANCESTOR_DEPTH: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// One place the output points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    /// The column, where the tool gave one, counted from 1.
    pub column: Option<usize>,
    /// The directory the path was printed relative to.
    pub dir: PathBuf,
    /// The path as printed.
    pub file: PathBuf,
    /// The output line, by its whole-output index, holding the message a
    /// place printed on a line of its own belongs to: rustc's `error:` over
    /// its `-->`.
    pub heading: Option<usize>,
    /// How bad the tool said the problem there is.
    pub level: DiagnosticLevel,
    /// The line, counted from 1.
    pub line: usize,
    /// What the tool said about it, where it said it on the same line.
    pub message: String,
    /// Where in the output line the place is written, in bytes: what is
    /// painted as the reference.
    pub span: Range<usize>,
}

/// Reads the lines of one build in order, keeping what the lines before
/// say about the ones after.
#[derive(Debug)]
pub struct Locator {
    /// Where the build ran, which paths are relative to until `make` says
    /// otherwise.
    base: PathBuf,
    /// The directories `make` entered, innermost last.
    dirs: Vec<PathBuf>,
    /// The rustc message being placed, until its `-->` line.
    header: Option<Header>,
    patterns: Option<Patterns>,
}

/// A rustc message's opening line, which the `-->` under it places.
#[derive(Debug)]
struct Header {
    level: DiagnosticLevel,
    /// Its whole-output index.
    line: usize,
    message: String,
}

/// The formats, compiled once per build.
#[derive(Debug)]
struct Patterns {
    gnu: Regex,
    make_directory: Regex,
    msvc: Regex,
    python_frame: Regex,
    rust_arrow: Regex,
    rust_header: Regex,
}

// ========================================================================
// Location
// ========================================================================

impl Location {
    /// The file on disk the location names: under the directory it was
    /// printed relative to, else under one of the directories above that.
    /// `None` when no such file exists.
    pub fn resolve(&self) -> Option<PathBuf> {
        if self.file.is_absolute() {
            return self.file.is_file().then(|| self.file.clone());
        }
        self.dir
            .ancestors()
            .take(ANCESTOR_DEPTH + 1)
            .map(|dir| dir.join(&self.file))
            .find(|path| path.is_file())
    }

    /// `path:line[:col]`, as a reference to copy.
    pub fn reference(&self) -> String {
        let path = self.resolve().unwrap_or_else(|| self.dir.join(&self.file));
        match self.column {
            Some(column) => format!("{}:{}:{column}", path.display(), self.line),
            None => format!("{}:{}", path.display(), self.line),
        }
    }
}

// ========================================================================
// Locator
// ========================================================================

impl Locator {
    /// A reader for a build that ran in `base`.
    pub fn new(base: PathBuf) -> Self {
        Self {
            base,
            dirs: Vec::new(),
            header: None,
            patterns: Patterns::compile(),
        }
    }

    /// The place `text`, the build's next line, at whole-output index
    /// `index`, points at, if any.
    pub fn scan(&mut self, index: usize, text: &str) -> Option<Location> {
        let patterns = self.patterns.as_ref()?;
        if let Some(caps) = patterns.make_directory.captures(text) {
            match &caps[1] {
                "Entering" => self.dirs.push(PathBuf::from(&caps[2])),
                _ => {
                    self.dirs.pop();
                }
            }
            return None;
        }
        if let Some(caps) = patterns.rust_header.captures(text) {
            let level = match &caps[1] {
                "error" => DiagnosticLevel::Error,
                "warning" => DiagnosticLevel::Warning,
                _ => DiagnosticLevel::Info,
            };
            let message = text[caps.get(0)?.end()..].trim().to_string();
            self.header = Some(Header {
                level,
                line: index,
                message,
            });
            return None;
        }
        let dir = self
            .dirs
            .last()
            .map_or_else(|| self.base.clone(), |dir| self.base.join(dir));
        if let Some(caps) = patterns.rust_arrow.captures(text) {
            let header = self.header.as_ref();
            let level = match &caps[1] {
                ":::" => DiagnosticLevel::Info,
                _ => header.map_or(DiagnosticLevel::Error, |header| header.level),
            };
            let whole = caps.get(0)?;
            let start = caps.get(2)?.start();
            return Some(Location {
                column: caps[4].parse().ok(),
                dir,
                file: PathBuf::from(&caps[2]),
                heading: header.map(|header| header.line),
                level,
                line: caps[3].parse().ok()?,
                message: header.map_or_else(String::new, |header| header.message.clone()),
                span: start..whole.as_str().trim_end().len() + whole.start(),
            });
        }
        if let Some(caps) = patterns.python_frame.captures(text) {
            let span = caps.get(1)?.start()..caps.get(0)?.end();
            return Some(Location {
                column: None,
                dir,
                file: PathBuf::from(&caps[1]),
                heading: None,
                level: DiagnosticLevel::Error,
                line: caps[2].parse().ok()?,
                message: String::new(),
                span,
            });
        }
        if let Some(caps) = patterns.msvc.captures(text) {
            let path = caps.get(1)?;
            let close = text[path.end()..].find(')')? + path.end() + 1;
            return Some(Location {
                column: caps.get(3).and_then(|c| c.as_str().parse().ok()),
                dir,
                file: PathBuf::from(path.as_str()),
                heading: None,
                level: level_of(&caps[4]),
                line: caps[2].parse().ok()?,
                message: text[caps.get(4)?.start()..].trim().to_string(),
                span: path.start()..close,
            });
        }
        let caps = patterns.gnu.captures(text)?;
        let path = caps.get(1)?;
        if !looks_like_path(path.as_str()) {
            return None;
        }
        let place_end = caps
            .get(3)
            .or_else(|| caps.get(2))
            .map_or(path.end(), |m| m.end());
        let message = caps.get(4).map_or("", |m| m.as_str()).trim().to_string();
        Some(Location {
            column: caps.get(3).and_then(|c| c.as_str().parse().ok()),
            dir,
            file: PathBuf::from(path.as_str()),
            heading: None,
            level: match message.is_empty() {
                // A bare place is a lead-in (`In file included from`).
                true => DiagnosticLevel::Info,
                false => level_of(&message),
            },
            line: caps[2].parse().ok()?,
            message,
            span: path.start()..place_end,
        })
    }
}

// ========================================================================
// Patterns
// ========================================================================

impl Patterns {
    /// Every format, or `None` should one fail to compile, which reads no
    /// places at all rather than some of them.
    fn compile() -> Option<Self> {
        Some(Self {
            gnu: Regex::new(GNU).ok()?,
            make_directory: Regex::new(MAKE_DIRECTORY).ok()?,
            msvc: Regex::new(MSVC).ok()?,
            python_frame: Regex::new(PYTHON_FRAME).ok()?,
            rust_arrow: Regex::new(RUST_ARROW).ok()?,
            rust_header: Regex::new(RUST_HEADER).ok()?,
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How bad a message says it is by the word it opens with. A message that
/// names no kind is an error: that is what a tool printing a bare place
/// before what it found there means, and what Emacs reads it as.
fn level_of(message: &str) -> DiagnosticLevel {
    let lower = message.trim_start().to_ascii_lowercase();
    let opens = |word: &str| {
        lower.starts_with(word)
            && lower[word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
    };
    if opens("warning") {
        return DiagnosticLevel::Warning;
    }
    if ["note", "info", "remark", "help"].into_iter().any(opens) {
        return DiagnosticLevel::Info;
    }
    DiagnosticLevel::Error
}

/// Whether what came before a `:line` is plausibly a path rather than a
/// word or a time of day: it has a separator or an extension, and is not a
/// number.
fn looks_like_path(text: &str) -> bool {
    !text.starts_with('-')
        && !text.chars().all(|c| c.is_ascii_digit() || c == '.')
        && (text.contains(['/', '\\', '.']))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn located(lines: &[&str]) -> Vec<Location> {
        let mut locator = Locator::new(PathBuf::from("/work"));
        lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| locator.scan(index, line))
            .collect()
    }

    #[test]
    fn test_gnu_places_carry_their_column_and_the_kind_their_message_names() {
        let found = located(&[
            "src/main.c:12:5: error: expected ';'",
            "src/main.c:20: warning: unused variable",
            "lib/x.go:7:2: note: declared here",
        ]);
        assert_eq!(found.len(), 3);
        assert_eq!((found[0].line, found[0].column), (12, Some(5)));
        assert_eq!(found[0].level, DiagnosticLevel::Error);
        assert_eq!(found[0].span, 0..15);
        assert_eq!(found[1].level, DiagnosticLevel::Warning);
        assert_eq!(found[1].column, None);
        assert_eq!(found[2].level, DiagnosticLevel::Info);
    }

    #[test]
    fn test_a_tex_file_line_error_is_an_error_at_that_line() {
        let found = located(&["./sections/overview.tex:121: Undefined control sequence."]);
        assert_eq!(found[0].file, PathBuf::from("./sections/overview.tex"));
        assert_eq!(found[0].line, 121);
        assert_eq!(found[0].level, DiagnosticLevel::Error);
        assert_eq!(found[0].message, "Undefined control sequence.");
    }

    #[test]
    fn test_rustc_places_take_the_kind_of_the_message_above_them() {
        let found = located(&[
            "warning: unused import: `Path`",
            " --> crates/a/src/lib.rs:3:5",
            "error[E0425]: cannot find value `x` in this scope",
            "   --> crates/a/src/lib.rs:9:13",
            "   ::: crates/b/src/other.rs:1:1",
        ]);
        let levels: Vec<DiagnosticLevel> = found.iter().map(|l| l.level).collect();
        assert_eq!(
            levels,
            [
                DiagnosticLevel::Warning,
                DiagnosticLevel::Error,
                DiagnosticLevel::Info
            ]
        );
        assert_eq!((found[1].line, found[1].column), (9, Some(13)));
        // The `-->` knows the line its message opened on, and what it said.
        assert_eq!(found[1].heading, Some(2));
        assert_eq!(found[1].message, "cannot find value `x` in this scope");
        assert_eq!(
            &"   --> crates/a/src/lib.rs:9:13"[found[1].span.clone()],
            "crates/a/src/lib.rs:9:13"
        );
    }

    #[test]
    fn test_python_and_msvc_places_are_read() {
        let found = located(&[
            r#"  File "/work/app.py", line 42, in main"#,
            r"src\widget.cpp(17,9): error C2065: 'x': undeclared identifier",
        ]);
        assert_eq!(found[0].file, PathBuf::from("/work/app.py"));
        assert_eq!(found[0].line, 42);
        assert_eq!(found[1].line, 17);
        assert_eq!(found[1].column, Some(9));
        assert_eq!(found[1].level, DiagnosticLevel::Error);
    }

    #[test]
    fn test_paths_after_make_enters_a_directory_are_relative_to_it() {
        let found = located(&[
            "make[1]: Entering directory '/work/sub'",
            "a.c:1: error: x",
            "make[1]: Leaving directory '/work/sub'",
            "b.c:2: error: y",
        ]);
        assert_eq!(found[0].dir, PathBuf::from("/work/sub"));
        assert_eq!(found[1].dir, PathBuf::from("/work"));
    }

    #[test]
    fn test_times_words_and_urls_are_not_places() {
        let found = located(&[
            "12:30:45: started",
            "Error:12: nothing",
            "see https://example.com:8080/path",
            "Finished `dev` profile in 3.2s",
        ]);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn test_a_relative_path_is_found_under_a_directory_above_the_build() {
        let root = std::env::temp_dir().join(format!("winter-locate-{}", std::process::id()));
        let member = root.join("crates").join("a");
        std::fs::create_dir_all(&member).expect("creates the tree");
        std::fs::write(root.join("top.rs"), "").expect("writes the file");
        let location = Location {
            column: None,
            dir: member.clone(),
            file: PathBuf::from("top.rs"),
            heading: None,
            level: DiagnosticLevel::Error,
            line: 1,
            message: String::new(),
            span: 0..0,
        };
        assert_eq!(location.resolve(), Some(root.join("top.rs")));
        let missing = Location {
            file: PathBuf::from("nowhere.rs"),
            ..location
        };
        assert_eq!(missing.resolve(), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
