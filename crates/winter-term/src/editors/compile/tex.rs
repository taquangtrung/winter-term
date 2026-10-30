//! LaTeX: the command a document is built with, and what its `.log` says
//! went wrong.
//!
//! What TeX prints while it runs says little about where: an error is
//! `! Undefined control sequence.` with its line on a later `l.42`, and a
//! warning names a line but not the file it is in. The file is known only
//! from the log's parentheses, `(./sections/intro.tex` opening it and a
//! `)` closing it, interleaved with everything else. [`parse_log`] follows
//! them to place each message in its file.
//!
//! The build is run with `max_print_line` raised, so TeX does not fold its
//! lines at 79 columns: a folded line splits a path, and a split path is
//! one the parentheses cannot be followed through.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::model::paint::DiagnosticLevel;

// ========================================================================
// Constants
// ========================================================================

/// What TeX is told about its line lengths, through the environment its
/// configuration reads them from: long enough that no line is folded.
pub const TEX_ENVIRONMENT: [(&str, &str); 3] = [
    ("max_print_line", "10000"),
    ("error_line", "254"),
    ("half_error_line", "238"),
];

/// How TeX ends a run, naming the log it wrote.
const TRANSCRIPT: &str = r"Transcript written on (.+)\.\s*$";

/// A file TeX opened, as the token after a `(`: a path with an extension
/// that starts with a letter, which keeps `(15.0pt too wide)` from reading
/// as one.
const OPENED_FILE: &str = r"^(?:[A-Za-z]:)?[^\s()]*\.[A-Za-z][A-Za-z0-9]*$";

/// An error placed by `-file-line-error`.
const FILE_LINE_ERROR: &str = r"^(.+?\.[A-Za-z]+):(\d+): (.+)$";

/// An error as TeX prints it without `-file-line-error`.
const BANG_ERROR: &str = r"^! (.+)$";

/// The line an error happened on, a few lines after it.
const ERROR_LINE: &str = r"^l\.(\d+)";

/// A warning: LaTeX's, a package's, a class's, or pdfTeX's.
const WARNING: &str = r"^((?:LaTeX|LaTeX Font|Package (\S+)|Class (\S+)) Warning|pdfTeX warning(?: \([^)]*\))?): (.*)$";

/// The line a warning says it is about.
const ON_INPUT_LINE: &str = r"on input line (\d+)";

/// A box TeX could not set as asked, and the lines it was set from.
const BAD_BOX: &str = r"^((?:Overfull|Underfull) \\[hv]box .*?)(?: in (?:paragraph|alignment) at lines (\d+)--\d+| detected at line (\d+)|$)";

/// How many lines after an error its `l.N` is looked for in.
const ERROR_LINE_REACH: usize = 12;

// ========================================================================
// Data Structures
// ========================================================================

/// One thing the log reported.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TexMessage {
    /// The file it is about, as the log named it, relative to the build's
    /// directory.
    pub file: PathBuf,
    /// How bad it is: an error, a warning, or a box that came out badly.
    pub level: DiagnosticLevel,
    /// The line, where the log gave one.
    pub line: Option<usize>,
    /// What the log said.
    pub message: String,
}

/// The log's formats, compiled once per read.
struct Patterns {
    bad_box: Regex,
    bang_error: Regex,
    error_line: Regex,
    file_line_error: Regex,
    on_input_line: Regex,
    opened_file: Regex,
    warning: Regex,
}

// ========================================================================
// Functions
// ========================================================================

/// The shell command that builds `root`: `command` with the root's file
/// name after it, quoted for the shell.
pub fn latex_command(command: &str, root: &Path) -> String {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("{} {}", command.trim(), shell_quoted(&name))
}

/// The log a build wrote: the one TeX's last `Transcript written on` names,
/// relative to `dir`, else the root's own beside it.
pub fn log_path<'a>(
    output: impl DoubleEndedIterator<Item = &'a str>,
    dir: &Path,
    root: &Path,
) -> PathBuf {
    let named = Regex::new(TRANSCRIPT).ok().and_then(|pattern| {
        output.rev().find_map(|line| {
            pattern
                .captures(line)
                .map(|caps| dir.join(caps[1].trim_matches('"')))
        })
    });
    named.unwrap_or_else(|| root.with_extension("log"))
}

/// Every error, warning, and bad box in `log`, each once, in the order the
/// log reports them.
pub fn parse_log(log: &str) -> Vec<TexMessage> {
    let Some(patterns) = Patterns::compile() else {
        return Vec::new();
    };
    let lines: Vec<&str> = log.lines().collect();
    let mut files: Vec<Option<PathBuf>> = Vec::new();
    let mut found: Vec<TexMessage> = Vec::new();
    let mut seen: HashSet<TexMessage> = HashSet::new();
    let mut at = 0;
    while at < lines.len() {
        let current = files.iter().rev().find_map(Clone::clone);
        let (message, used) = read_message(&patterns, &lines, at, current);
        if let Some(message) = message {
            if seen.insert(message.clone()) {
                found.push(message);
            }
        }
        for line in &lines[at..at + used.max(1)] {
            follow_parentheses(&patterns, line, &mut files);
        }
        at += used.max(1);
    }
    found
}

/// The message starting at line `at`, if one does, and how many lines it
/// takes.
fn read_message(
    patterns: &Patterns,
    lines: &[&str],
    at: usize,
    current: Option<PathBuf>,
) -> (Option<TexMessage>, usize) {
    let text = lines[at];
    if let Some(caps) = patterns.file_line_error.captures(text) {
        let message = TexMessage {
            file: PathBuf::from(&caps[1]),
            level: DiagnosticLevel::Error,
            line: caps[2].parse().ok(),
            message: caps[3].trim().to_string(),
        };
        return (Some(message), 1);
    }
    let Some(file) = current else {
        return (None, 1);
    };
    if let Some(caps) = patterns.bang_error.captures(text) {
        let line = lines[at + 1..]
            .iter()
            .take(ERROR_LINE_REACH)
            .find_map(|next| patterns.error_line.captures(next))
            .and_then(|caps| caps[1].parse().ok());
        let message = TexMessage {
            file,
            level: DiagnosticLevel::Error,
            line,
            message: caps[1].trim().to_string(),
        };
        return (Some(message), 1);
    }
    if let Some(caps) = patterns.warning.captures(text) {
        // A package's warning goes on over lines that open with its name in
        // parentheses, which are part of it rather than files.
        let owner = caps.get(2).or_else(|| caps.get(3)).map(|m| m.as_str());
        let mut message = caps[4].trim().to_string();
        let mut used = 1;
        if let Some(owner) = owner {
            let lead = format!("({owner})");
            while let Some(next) = lines.get(at + used).and_then(|l| l.strip_prefix(&lead)) {
                message.push(' ');
                message.push_str(next.trim());
                used += 1;
            }
        }
        let line = patterns
            .on_input_line
            .captures(&message)
            .and_then(|caps| caps[1].parse().ok());
        let message = TexMessage {
            file,
            level: DiagnosticLevel::Warning,
            line,
            message,
        };
        return (Some(message), used);
    }
    if let Some(caps) = patterns.bad_box.captures(text) {
        let line = caps
            .get(2)
            .or_else(|| caps.get(3))
            .and_then(|m| m.as_str().parse().ok());
        let message = TexMessage {
            file,
            level: DiagnosticLevel::Info,
            line,
            message: caps[1].trim().to_string(),
        };
        return (Some(message), 1);
    }
    (None, 1)
}

/// Follow `line`'s parentheses: a `(` opening a file pushes it, any other
/// `(` pushes a placeholder so that its `)` closes nothing, and a `)` pops.
fn follow_parentheses(patterns: &Patterns, line: &str, files: &mut Vec<Option<PathBuf>>) {
    let mut rest = line;
    while let Some(at) = rest.find(['(', ')']) {
        let after = &rest[at + 1..];
        match rest.as_bytes()[at] {
            b'(' => {
                let token_end = after.find(|c: char| c.is_whitespace() || c == '(' || c == ')');
                let token = &after[..token_end.unwrap_or(after.len())];
                let file = patterns
                    .opened_file
                    .is_match(token)
                    .then(|| PathBuf::from(token.trim_matches('"')));
                files.push(file);
            }
            _ => {
                files.pop();
            }
        }
        rest = after;
    }
}

/// `text` quoted for a POSIX shell, unless it needs no quoting.
fn shell_quoted(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/+".contains(c));
    match plain {
        true => text.to_string(),
        false => format!("'{}'", text.replace('\'', r"'\''")),
    }
}

// ========================================================================
// Patterns
// ========================================================================

impl Patterns {
    fn compile() -> Option<Self> {
        Some(Self {
            bad_box: Regex::new(BAD_BOX).ok()?,
            bang_error: Regex::new(BANG_ERROR).ok()?,
            error_line: Regex::new(ERROR_LINE).ok()?,
            file_line_error: Regex::new(FILE_LINE_ERROR).ok()?,
            on_input_line: Regex::new(ON_INPUT_LINE).ok()?,
            opened_file: Regex::new(OPENED_FILE).ok()?,
            warning: Regex::new(WARNING).ok()?,
        })
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r"This is pdfTeX, Version 3.141592653-2.6-1.40.25 (TeX Live 2023) (preloaded format=pdflatex)
(./main.tex
LaTeX2e <2022-11-01>
(/usr/share/texlive/texmf-dist/tex/latex/base/article.cls
Document Class: article 2022/07/02 v1.4n Standard LaTeX document class
(/usr/share/texlive/texmf-dist/tex/latex/base/size10.clo))
(./sections/overview.tex
LaTeX Warning: Reference `fig:x' on page 1 undefined on input line 12.

Overfull \hbox (15.0pt too wide) in paragraph at lines 20--22
[]\OT1/cmr/m/n/10 text
! Undefined control sequence.
l.31 \foo

)
Package hyperref Warning: Token not allowed in a PDF string (Unicode):
(hyperref)                removing `math shift' on input line 40.

./main.tex:50: Missing $ inserted.
LaTeX Warning: There were undefined references.
)
Output written on main.pdf (1 page, 1234 bytes).
Transcript written on main.log.";

    #[test]
    fn test_messages_are_placed_in_the_file_the_parentheses_say_tex_was_reading() {
        let found = parse_log(LOG);
        let placed: Vec<(String, Option<usize>, DiagnosticLevel)> = found
            .iter()
            .map(|m| (m.file.display().to_string(), m.line, m.level))
            .collect();
        assert_eq!(
            placed,
            [
                (
                    "./sections/overview.tex".into(),
                    Some(12),
                    DiagnosticLevel::Warning
                ),
                (
                    "./sections/overview.tex".into(),
                    Some(20),
                    DiagnosticLevel::Info
                ),
                (
                    "./sections/overview.tex".into(),
                    Some(31),
                    DiagnosticLevel::Error
                ),
                ("./main.tex".into(), Some(40), DiagnosticLevel::Warning),
                ("./main.tex".into(), Some(50), DiagnosticLevel::Error),
                ("./main.tex".into(), None, DiagnosticLevel::Warning),
            ]
        );
    }

    #[test]
    fn test_a_package_warning_takes_in_its_continuation_lines() {
        let found = parse_log(LOG);
        let hyperref = found.iter().find(|m| m.line == Some(40)).expect("found it");
        assert_eq!(
            hyperref.message,
            "Token not allowed in a PDF string (Unicode): removing `math shift' on input line 40."
        );
    }

    #[test]
    fn test_the_same_message_twice_is_reported_once() {
        let twice = "(./a.tex\n! Missing $ inserted.\nl.3 x\n! Missing $ inserted.\nl.3 x\n)";
        assert_eq!(parse_log(twice).len(), 1);
    }

    #[test]
    fn test_the_log_is_the_one_tex_says_it_wrote() {
        let output = [
            "Latexmk: ...",
            "Transcript written on build/main.log.",
            "Latexmk: done",
        ];
        let dir = Path::new("/doc");
        assert_eq!(
            log_path(output.into_iter(), dir, Path::new("/doc/main.tex")),
            PathBuf::from("/doc/build/main.log")
        );
        assert_eq!(
            log_path(["nothing"].into_iter(), dir, Path::new("/doc/main.tex")),
            PathBuf::from("/doc/main.log")
        );
    }

    #[test]
    fn test_the_root_is_quoted_for_the_shell_only_when_it_needs_it() {
        assert_eq!(
            latex_command("latexmk -pdf ", Path::new("/doc/main.tex")),
            "latexmk -pdf main.tex"
        );
        assert_eq!(
            latex_command("latexmk", Path::new("/doc/my thesis's.tex")),
            r"latexmk 'my thesis'\''s.tex'"
        );
    }
}
