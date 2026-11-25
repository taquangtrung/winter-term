//! What a terminal tab tells the user, kept typed until the window posts
//! it: the one place its wording and severity are decided, so callers and
//! tests name a variant rather than a sentence. What the system said (an
//! error writing a file, opening a link) travels as a variant's data.

use std::fmt;
use std::path::PathBuf;

use winter_model::editor::Notice;

// ========================================================================
// Constants
// ========================================================================

/// Said when the system clipboard would not take the text.
const CLIPBOARD_FAILED: &str = "could not copy to clipboard";

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum TerminalNotice {
    /// The ANSI scrollback opened in an editor.
    AnsiOpened,
    /// The ANSI scrollback could not be written, with the system's error.
    AnsiWriteFailed(String),
    /// A block's text, its command line above it, was copied.
    BlockCopied,
    /// Typing goes to every terminal shown (`true`) or the focused one.
    Broadcast(bool),
    /// The system clipboard would not take the text.
    ClipboardFailed,
    /// A block's output was copied.
    Copied,
    /// A reference under the cursor was copied, as it reads.
    CopiedReference(String),
    /// A selection was copied, by its size.
    CopiedText(TextSize),
    /// Stepping opened a place the output named.
    ErrorStep(ErrorStep),
    /// The scrollback could not be written as HTML.
    HtmlWriteFailed(String),
    /// The block focused has no content to export.
    NoBlockContent,
    /// No terminal output to read errors from.
    NoErrorOutput,
    /// The prompt line holds no match for the search.
    NoPromptMatch,
    /// The last commands printed no file:line references.
    NoReferences,
    /// The block focused has no SVG graphic.
    NoSvg,
    /// A link is not to a file on this machine.
    NotLocalFile(String),
    /// The cursor is off the line the shell is editing.
    NotOnPrompt(PromptOp),
    /// A link is neither a web nor a mail link.
    NotWebLink(String),
    /// Nothing under the cursor to open.
    NothingToOpen,
    /// A file an export was written to could not be opened.
    OpenFailed(String),
    /// A file was opened in the system's viewer.
    Opened(PathBuf),
    /// A link was opened.
    OpenedUrl(String),
    /// Prompt editing is off, the shell's own editor handling the line.
    PromptEditingOff,
    /// The scrollback was copied.
    ScrollbackCopied,
    /// A selection meant for an edit lies off the line the shell is editing.
    SelectionNotOnPrompt,
    /// The cursor is on the shell's own prompt, not its command line.
    ShellPrompt,
    /// A program could not be started.
    StartFailed(Failure),
    /// A block's SVG could not be written, with the system's error.
    SvgWriteFailed(String),
    /// The scrollback opened in an editor.
    TextOpened,
    /// The scrollback could not be written as text.
    TextWriteFailed(String),
    /// A link could not be opened.
    UrlFailed(Failure),
    /// A yank went into a register, by its name.
    Yanked(char),
}

/// An edit on the shell's command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum PromptOp {
    /// `c`.
    Change,
    /// `cs`.
    ChangeSurround,
    /// `d`.
    Delete,
    /// `ds`.
    DeleteSurround,
    /// `r`.
    Replace,
    /// `ys`.
    Surround,
    /// `~`.
    ToggleCase,
}

/// Something that failed, by its name, with the system's error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Failure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// What failed: a program's name, or a link.
    pub(in crate::window) what: String,
}

/// A place the output named, opened by stepping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ErrorStep {
    /// Which, from 0.
    pub(in crate::window) at: usize,
    /// How many places there are.
    pub(in crate::window) count: usize,
    /// The line, when one was named.
    pub(in crate::window) line: Option<usize>,
    /// The file.
    pub(in crate::window) path: PathBuf,
}

/// How much text was copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) struct TextSize {
    /// Its characters.
    pub(in crate::window) chars: usize,
    /// Its lines.
    pub(in crate::window) lines: usize,
}

// ========================================================================
// TerminalNotice
// ========================================================================

impl From<TerminalNotice> for Notice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    fn from(notice: TerminalNotice) -> Self {
        use TerminalNotice::*;
        match notice {
            AnsiOpened => Notice::info("opened ANSI scrollback in editor"),
            AnsiWriteFailed(error) => {
                Notice::error(format!("could not write ANSI scrollback file: {error}"))
            }
            BlockCopied => Notice::info("copied block text to clipboard").unlogged(),
            Broadcast(true) => Notice::info("typing goes to every terminal shown").unlogged(),
            Broadcast(false) => {
                Notice::info("typing goes to the focused terminal alone").unlogged()
            }
            ClipboardFailed => Notice::error(CLIPBOARD_FAILED),
            Copied => Notice::info("copied to clipboard").unlogged(),
            CopiedReference(display) => Notice::info(format!("copied {display}")).unlogged(),
            CopiedText(TextSize { chars, lines }) => Notice::info(format!(
                "copied {lines} line{}, {chars} character{}",
                if lines == 1 { "" } else { "s" },
                if chars == 1 { "" } else { "s" },
            ))
            .unlogged(),
            ErrorStep(step) => Notice::info(step.to_string()),
            HtmlWriteFailed(error) => {
                Notice::error(format!("could not write scrollback HTML: {error}"))
            }
            NoBlockContent => Notice::refusal("no block content to export"),
            NoErrorOutput => Notice::refusal("no terminal output to read errors from"),
            NoPromptMatch => Notice::refusal("no search match on the prompt line"),
            NoReferences => Notice::refusal("the last commands printed no file:line references"),
            NoSvg => Notice::refusal("the focused block has no SVG graphic"),
            NotLocalFile(url) => Notice::refusal(format!("not opening {url}: not a local file")),
            NotOnPrompt(op) => {
                Notice::refusal(format!("cannot {op}: not on the editable prompt line"))
            }
            NotWebLink(url) => {
                Notice::refusal(format!("not opening {url}: not a web or mail link"))
            }
            NothingToOpen => Notice::info("nothing to open under the cursor"),
            OpenFailed(error) => Notice::error(format!("could not open the file: {error}")),
            Opened(path) => Notice::info(format!("opened {}", path.display())),
            OpenedUrl(url) => Notice::info(format!("opened {url}")),
            PromptEditingOff => Notice::refusal(
                "prompt editing is off (prompt-edit-bindings); your shell's own editor handles \
                 the line",
            ),
            ScrollbackCopied => Notice::info("copied scrollback to clipboard").unlogged(),
            SelectionNotOnPrompt => {
                Notice::refusal("cannot delete: the selection is not on the editable prompt line")
            }
            ShellPrompt => {
                Notice::refusal("cannot edit: that is the shell's own prompt, not the command line")
            }
            StartFailed(Failure { error, what }) => {
                Notice::error(format!("could not start {what}: {error}"))
            }
            SvgWriteFailed(error) => Notice::error(format!("could not write SVG file: {error}")),
            TextOpened => Notice::info("opened scrollback in editor"),
            TextWriteFailed(error) => {
                Notice::error(format!("could not write scrollback file: {error}"))
            }
            UrlFailed(Failure { error, what }) => {
                Notice::error(format!("could not open {what}: {error}"))
            }
            Yanked(register) => {
                Notice::info(format!("yanked into register \"{register}")).unlogged()
            }
        }
    }
}

// ========================================================================
// PromptOp
// ========================================================================

impl fmt::Display for PromptOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Change => "change",
            Self::ChangeSurround => "change surround",
            Self::Delete => "delete",
            Self::DeleteSurround => "delete surround",
            Self::Replace => "replace",
            Self::Surround => "surround",
            Self::ToggleCase => "toggle case",
        })
    }
}

// ========================================================================
// ErrorStep
// ========================================================================

impl fmt::Display for ErrorStep {
    /// `2 of 5: src/lib.rs:12`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} of {}: {}",
            self.at + 1,
            self.count,
            self.path.display()
        )?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        Ok(())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The size of `text` as a copy reports it.
    fn copied(text: &str) -> String {
        let size = TextSize {
            chars: text.chars().count(),
            lines: text.lines().count(),
        };
        Notice::from(TerminalNotice::CopiedText(size)).text
    }

    #[test]
    fn test_a_copy_counts_lines_and_unicode_characters_in_the_plural_past_one() {
        assert_eq!(copied("ab\ncd"), "copied 2 lines, 5 characters");
        assert_eq!(copied("a"), "copied 1 line, 1 character");
        assert_eq!(copied("héllo"), "copied 1 line, 5 characters");
    }

    #[test]
    fn test_a_step_names_the_line_only_when_one_was_named() {
        let step = |line| ErrorStep {
            at: 1,
            count: 5,
            line,
            path: PathBuf::from("src/lib.rs"),
        };
        assert_eq!(step(Some(12)).to_string(), "2 of 5: src/lib.rs:12");
        assert_eq!(step(None).to_string(), "2 of 5: src/lib.rs");
    }

    #[test]
    fn test_every_prompt_refusal_names_its_edit() {
        let text = Notice::from(TerminalNotice::NotOnPrompt(PromptOp::ToggleCase)).text;
        assert_eq!(text, "cannot toggle case: not on the editable prompt line");
    }
}
