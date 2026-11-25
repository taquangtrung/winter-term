//! What Winter's own pages tell the user, kept typed until the window takes
//! it: one enum per page, each turned into a misty [`Notice`] only in the
//! page's `take_notice`, so the wording and severity are decided here and
//! tests compare variants rather than sentences.
//!
//! What the system or a program said (an error reading a file, a build's
//! end) travels as a variant's data.

use std::path::PathBuf;

use winter_model::editor::Notice;
use winter_model::notice::SearchMiss;

// ========================================================================
// Data Structures
// ========================================================================

/// What the Compile page says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompileNotice {
    /// The build ended badly, by its name, how it ended, and what it found.
    BuildFailed(BuildEnd),
    /// The build finished, by its name and what it found.
    BuildFinished(BuildEnd),
    /// The build was stopped, by its name.
    BuildStopped(String),
    /// The page shows only errors and warnings (`true`) or all the output.
    ErrorsOnly(bool),
    /// No build is running.
    NoBuild,
    /// The line under the cursor names no error.
    NoErrorHere,
    /// Stepping found no more errors.
    NoMoreErrors,
    /// The build makes no PDF.
    NoPdf,
    /// The build's PDF is not written yet.
    NoPdfYet(PathBuf),
    /// A place the build named is no file.
    NoSuchFile(PathBuf),
    /// The build is not taking input.
    NotTakingInput,
    /// The build pointed at nothing, while it runs (`true`) or once done.
    PointedAtNothing(bool),
    /// A search over the output that moved nowhere.
    Search(SearchMiss),
    /// The build is being stopped.
    Stopping,
    /// Stepping opened a place the build named.
    Walked(Walked),
}

/// How a build ended, as its report names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildEnd {
    /// The errors and warnings found, as the page counts them.
    pub(crate) counts: String,
    /// How it ended, as the page says it (`failed with exit code 1 after
    /// 2.0s`).
    pub(crate) how: String,
    /// The build's name.
    pub(crate) name: String,
}

/// A place the build named, opened by stepping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Walked {
    /// Which, from 0.
    pub(crate) at: usize,
    /// How many places there are.
    pub(crate) count: usize,
    /// The place, as `file:line:column`.
    pub(crate) reference: String,
}

/// What an extension's view says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExtensionViewNotice {
    /// The extension panicked and was stopped, by its name.
    Stopped(String),
}

/// What the Extensions page says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExtensionsNotice {
    /// A change went through, as it reported itself; it takes effect after
    /// a restart.
    Changed(String),
    /// Reading or changing what is installed failed, with why.
    Failed(String),
}

/// What the Hex page says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HexNotice {
    /// Editing was turned on (`true`) or off.
    Editing(bool),
    /// The file could not be read or written, with the system's error.
    Failed(String),
    /// The file was never read, so a save has nothing to write.
    NeverRead,
    /// The offset typed is not in the file.
    NoSuchOffset,
    /// Undo found nothing to take back.
    NothingToTakeBack,
    /// The file is too large to show.
    TooLarge,
    /// The bytes were written back, this many.
    Wrote(usize),
}

/// What the image viewer says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ImageNotice {
    /// The picture and where the view is, as the viewer describes them.
    Described(String),
    /// The picture could not be read.
    Unreadable(FileFailure),
}

/// A file that could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileFailure {
    /// The system's error.
    pub(crate) error: String,
    /// The file.
    pub(crate) path: PathBuf,
}

/// What the Keys page says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum KeysNotice {
    /// The binding is already the built-in one.
    AlreadyBuiltIn,
    /// Vim's motions are built in and cannot be rebound.
    BuiltIn,
    /// A key capture ended with no keys pressed.
    NoKeysPressed,
    /// The command runs only in an editor of this kind.
    RunsIn(String),
}

/// What a saved list says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ListNotice {
    /// The line under the cursor names no place on this machine.
    NoPath,
    /// A search over the lines that moved nowhere.
    Search(SearchMiss),
}

/// What the Markdown preview says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PreviewNotice {
    /// Following the source was turned on (`true`) or off.
    Following(bool),
    /// A search found no match, by the query.
    NoMatch(String),
    /// A link names no file beside this one.
    NoSuchFile(String),
}

// ========================================================================
// CompileNotice
// ========================================================================

impl CompileNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::BuildFailed(BuildEnd { counts, how, name }) => {
                Notice::error(format!("{name}: {how} ({counts})"))
            }
            Self::BuildFinished(BuildEnd { counts, name, .. }) => {
                Notice::info(format!("{name}: finished ({counts})"))
            }
            Self::BuildStopped(name) => Notice::info(format!("{name}: stopped")),
            Self::ErrorsOnly(true) => Notice::info("showing only errors and warnings"),
            Self::ErrorsOnly(false) => Notice::info("showing all the output"),
            Self::NoBuild => Notice::info("no build is running"),
            Self::NoErrorHere => Notice::info("no error on this line"),
            Self::NoMoreErrors => Notice::info("no more errors"),
            Self::NoPdf => Notice::refusal("this build makes no PDF"),
            Self::NoPdfYet(pdf) => Notice::refusal(format!("no PDF yet: {}", pdf.display())),
            Self::NoSuchFile(file) => Notice::refusal(format!("no such file: {}", file.display())),
            Self::NotTakingInput => Notice::refusal("the build is not taking input"),
            Self::PointedAtNothing(true) => Notice::refusal("the build has pointed at nothing yet"),
            Self::PointedAtNothing(false) => Notice::refusal("the build pointed at nothing"),
            Self::Search(miss) => miss.into_notice(),
            Self::Stopping => Notice::info("stopping the build"),
            Self::Walked(Walked {
                at,
                count,
                reference,
            }) => Notice::info(format!("{} of {count}: {reference}", at + 1)),
        }
    }
}

// ========================================================================
// ExtensionViewNotice
// ========================================================================

impl ExtensionViewNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Stopped(extension) => Notice::error(super::extension::failure(&extension)),
        }
    }
}

// ========================================================================
// ExtensionsNotice
// ========================================================================

impl ExtensionsNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Changed(said) => {
                Notice::info(format!("{said}: restart Winter for it to take effect"))
            }
            Self::Failed(why) => Notice::error(why),
        }
    }
}

// ========================================================================
// HexNotice
// ========================================================================

impl HexNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Editing(on) => {
                let state = if on { "on" } else { "off" };
                Notice::info(format!("editing {state}")).unlogged()
            }
            Self::Failed(error) => Notice::error(format!("hex: {error}")),
            Self::NeverRead => {
                Notice::refusal("hex: the file was never read, so there is nothing to write")
            }
            Self::NoSuchOffset => Notice::info("no such offset").unlogged(),
            Self::NothingToTakeBack => Notice::info("nothing to take back").unlogged(),
            Self::TooLarge => Notice::refusal("hex: the file is too large to show"),
            Self::Wrote(bytes) => Notice::info(format!("wrote {bytes} bytes")),
        }
    }
}

// ========================================================================
// ImageNotice
// ========================================================================

impl ImageNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Described(description) => Notice::info(description),
            Self::Unreadable(FileFailure { error, path }) => {
                Notice::error(format!("{}: {error}", path.display()))
            }
        }
    }
}

// ========================================================================
// KeysNotice
// ========================================================================

impl KeysNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::AlreadyBuiltIn => Notice::info("already the built-in binding"),
            Self::BuiltIn => Notice::info(super::keys::BUILT_IN),
            Self::NoKeysPressed => Notice::info("no keys pressed, so the binding is unchanged"),
            Self::RunsIn(editor) => Notice::info(format!("runs in a {editor} tab")),
        }
    }
}

// ========================================================================
// ListNotice
// ========================================================================

impl ListNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::NoPath => Notice::refusal("no path on this line"),
            Self::Search(miss) => miss.into_notice(),
        }
    }
}

// ========================================================================
// PreviewNotice
// ========================================================================

impl PreviewNotice {
    /// The notice the status bar shows.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Following(on) => {
                let state = if on { "on" } else { "off" };
                Notice::info(format!("following the source {state}")).unlogged()
            }
            Self::NoMatch(query) => Notice::info(format!("no match for {query}")).unlogged(),
            Self::NoSuchFile(local) => Notice::warning(format!("no such file: {local}")),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every page's notices, so a wording rule is checked against them all.
    fn every_notice() -> Vec<Notice> {
        let end = || BuildEnd {
            counts: "1 error, 0 warnings".to_string(),
            how: "failed with exit code 1 after 2.0s".to_string(),
            name: "cargo build".to_string(),
        };
        let compile = [
            CompileNotice::BuildFailed(end()),
            CompileNotice::BuildFinished(end()),
            CompileNotice::BuildStopped("cargo build".to_string()),
            CompileNotice::ErrorsOnly(true),
            CompileNotice::ErrorsOnly(false),
            CompileNotice::NoBuild,
            CompileNotice::NoErrorHere,
            CompileNotice::NoMoreErrors,
            CompileNotice::NoPdf,
            CompileNotice::NoPdfYet(PathBuf::from("a.pdf")),
            CompileNotice::NoSuchFile(PathBuf::from("a.rs")),
            CompileNotice::NotTakingInput,
            CompileNotice::PointedAtNothing(true),
            CompileNotice::PointedAtNothing(false),
            CompileNotice::Stopping,
            CompileNotice::Walked(Walked {
                at: 0,
                count: 2,
                reference: "a.rs:1:1".to_string(),
            }),
        ];
        let hex = [
            HexNotice::Editing(true),
            HexNotice::NeverRead,
            HexNotice::NoSuchOffset,
            HexNotice::NothingToTakeBack,
            HexNotice::TooLarge,
            HexNotice::Wrote(16),
        ];
        let keys = [
            KeysNotice::AlreadyBuiltIn,
            KeysNotice::BuiltIn,
            KeysNotice::NoKeysPressed,
            KeysNotice::RunsIn("text editor".to_string()),
        ];
        let preview = [
            PreviewNotice::Following(true),
            PreviewNotice::NoMatch("x".to_string()),
            PreviewNotice::NoSuchFile("b.md".to_string()),
        ];
        let mut every: Vec<Notice> = Vec::new();
        every.extend(compile.into_iter().map(CompileNotice::into_notice));
        every.extend(hex.into_iter().map(HexNotice::into_notice));
        every.extend(keys.into_iter().map(KeysNotice::into_notice));
        every.extend(preview.into_iter().map(PreviewNotice::into_notice));
        every.push(ExtensionViewNotice::Stopped("docker".to_string()).into_notice());
        every.push(ExtensionsNotice::Changed("installed docker".to_string()).into_notice());
        every.push(ListNotice::NoPath.into_notice());
        every
    }

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        for notice in every_notice() {
            let text = &notice.text;
            assert!(!text.is_empty(), "{notice:?} says nothing");
            assert!(
                !text.contains("..."),
                "{notice:?} writes `...`, not `…`: {text}"
            );
            assert!(!text.ends_with('.'), "{notice:?} ends in a period: {text}");
        }
    }
}
