//! What the PDF viewer tells the user, kept typed until the window takes it.
//!
//! The viewer stores a [`PdfNotice`] and turns it into a misty [`Notice`]
//! only in `take_notice`, so its wording and severity are decided in one
//! `match` and tests compare variants rather than sentences.

use std::fmt;

use winter_model::editor::Notice;

// ========================================================================
// Constants
// ========================================================================

/// What a failed `synctex` is reported as, its output under it.
const SYNCTEX_FAILED: &str = "SyncTeX failed";

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PdfNotice {
    /// A word could not be added to the user's list.
    CouldNotAdd(WordFailure),
    /// How many words the selection or the document holds.
    Counted(WordCount),
    /// Link hints were asked for with no link in view.
    NoLinksInView,
    /// Stepping to a misspelling found none.
    NoMisspellings,
    /// Line hints were asked for with no text in view.
    NoTextInView,
    /// Cropping was asked of pages with no text.
    NoTextToCrop,
    /// Fitting the text was asked of pages with no text.
    NoTextToFit,
    /// The file could not be opened.
    OpenFailed(FileFailure),
    /// A page could not be drawn.
    PageFailed(PageFailure),
    /// Where the reader is in the document.
    Position(Position),
    /// The caret refused a motion.
    Refused(CaretRefusal),
    /// The file could not be read again after it changed.
    ReloadFailed(FileFailure),
    /// A word was asked to be taken as right before any was stepped to.
    StepToMisspelling,
    /// A word spelled wrong, and how it might be spelled.
    Suggestions(Suggestions),
    /// SyncTeX named no source for the spot.
    SyncNoSource,
    /// SyncTeX was asked to go back from a spot with no text.
    SyncNoText,
    /// `synctex` failed, with what it said.
    SyncTexFailed(String),
    /// A word was added to the user's list.
    TakenAsRight(String),
    /// A link's address, said rather than shown.
    Uri(String),
}

/// Why the caret did not move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaretRefusal {
    /// The document has no text to put a cursor in or to search.
    NoText,
    /// No text matches the pattern.
    NotFound(String),
    /// A search was repeated before any was made.
    NothingSearched,
}

/// A word the user's list would not take.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WordFailure {
    /// Why it was not added.
    pub(crate) why: String,
    /// The word.
    pub(crate) word: String,
}

/// How many words something holds, and what else it is counted in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WordCount {
    /// What else was counted.
    pub(crate) rest: CountOf,
    /// The words.
    pub(crate) words: usize,
}

/// What a word count is given beside the words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CountOf {
    /// The selection's characters.
    Characters(usize),
    /// The document's pages.
    Pages(usize),
}

/// A file that failed to open or to read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileFailure {
    /// The file's name.
    pub(crate) file: String,
    /// Why it failed.
    pub(crate) why: String,
}

/// A page that failed to draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageFailure {
    /// The file's name.
    pub(crate) file: String,
    /// The page, from 0.
    pub(crate) page: usize,
}

/// Where the reader is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Position {
    /// The file's name.
    pub(crate) file: String,
    /// The page being read, from 0.
    pub(crate) page: usize,
    /// How many pages there are.
    pub(crate) pages: usize,
    /// How far through the document, in percent.
    pub(crate) percent: u32,
}

/// A word spelled wrong, and the spellings offered for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Suggestions {
    /// The spellings offered, best first.
    pub(crate) offered: Vec<String>,
    /// The word.
    pub(crate) word: String,
}

// ========================================================================
// PdfNotice
// ========================================================================

impl PdfNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::CouldNotAdd(WordFailure { why, word }) => {
                Notice::error(format!("could not add \"{word}\": {why}"))
            }
            Self::Counted(count) => Notice::info(count.to_string()),
            Self::NoLinksInView => Notice::info("no links in view"),
            Self::NoMisspellings => Notice::info("no words spelled wrong"),
            Self::NoTextInView => Notice::info("no text in view"),
            Self::NoTextToCrop => Notice::info("no text to crop to"),
            Self::NoTextToFit => Notice::info("no text to fit"),
            Self::OpenFailed(FileFailure { file, why }) => Notice::error(format!("{file}: {why}")),
            Self::PageFailed(PageFailure { file, page }) => {
                Notice::error(format!("{file}: page {} could not be drawn", page + 1))
            }
            Self::Position(Position {
                file,
                page,
                pages,
                percent,
            }) => Notice::info(format!("{file}, page {} of {pages}, {percent}%", page + 1)),
            Self::Refused(refusal) => Notice::refusal(refusal.to_string()),
            Self::ReloadFailed(FileFailure { file, why }) => {
                Notice::error(format!("{file}: could not read it again: {why}"))
            }
            Self::StepToMisspelling => {
                Notice::info("step to a word spelled wrong first, with C-M-n")
            }
            Self::Suggestions(Suggestions { offered, word }) => match offered.is_empty() {
                true => Notice::info(format!("{word}: no suggestions (z g to take it as right)")),
                false => Notice::info(format!("{word}: {}", offered.join(", "))),
            },
            Self::SyncNoSource => {
                Notice::refusal("SyncTeX: no source for this spot (built with -synctex=1?)")
            }
            Self::SyncNoText => Notice::refusal("SyncTeX: no text here to go back from"),
            Self::SyncTexFailed(detail) => Notice::error(SYNCTEX_FAILED).with_detail(detail),
            Self::TakenAsRight(word) => {
                Notice::info(format!("\"{word}\" is taken as spelled right"))
            }
            Self::Uri(uri) => Notice::info(uri),
        }
    }
}

// ========================================================================
// CaretRefusal
// ========================================================================

impl fmt::Display for CaretRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoText => write!(f, "this document has no text"),
            Self::NotFound(pattern) => write!(f, "`{pattern}` is not in this document"),
            Self::NothingSearched => write!(f, "nothing has been searched for"),
        }
    }
}

// ========================================================================
// WordCount
// ========================================================================

impl fmt::Display for WordCount {
    /// `70 words, 10 pages`, or `3 words, 15 characters selected`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = counted(self.words, "word");
        match self.rest {
            CountOf::Characters(chars) => {
                write!(f, "{words}, {} selected", counted(chars, "character"))
            }
            CountOf::Pages(pages) => write!(f, "{words}, {}", counted(pages, "page")),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `count` of `one`, made plural past one: `1 page`, `2 pages`.
fn counted(count: usize, one: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        _ => format!("{count} {one}s"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One of each variant Winter words itself, so a wording rule is checked
    /// against them all.
    fn every_notice() -> Vec<PdfNotice> {
        let file = || "paper.pdf".to_string();
        vec![
            PdfNotice::CouldNotAdd(WordFailure {
                why: "read-only".to_string(),
                word: "teh".to_string(),
            }),
            PdfNotice::Counted(WordCount {
                rest: CountOf::Pages(1),
                words: 1,
            }),
            PdfNotice::NoLinksInView,
            PdfNotice::NoMisspellings,
            PdfNotice::NoTextInView,
            PdfNotice::NoTextToCrop,
            PdfNotice::NoTextToFit,
            PdfNotice::Refused(CaretRefusal::NoText),
            PdfNotice::Refused(CaretRefusal::NothingSearched),
            PdfNotice::Refused(CaretRefusal::NotFound("x".to_string())),
            PdfNotice::OpenFailed(FileFailure {
                file: file(),
                why: "damaged".to_string(),
            }),
            PdfNotice::PageFailed(PageFailure {
                file: file(),
                page: 0,
            }),
            PdfNotice::Position(Position {
                file: file(),
                page: 0,
                pages: 10,
                percent: 0,
            }),
            PdfNotice::ReloadFailed(FileFailure {
                file: file(),
                why: "gone".to_string(),
            }),
            PdfNotice::StepToMisspelling,
            PdfNotice::Suggestions(Suggestions {
                offered: Vec::new(),
                word: "teh".to_string(),
            }),
            PdfNotice::SyncNoSource,
            PdfNotice::SyncNoText,
            PdfNotice::SyncTexFailed("synctex: error".to_string()),
            PdfNotice::TakenAsRight("teh".to_string()),
        ]
    }

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        for notice in every_notice() {
            let text = notice.clone().into_notice().text;
            assert!(!text.is_empty(), "{notice:?} says nothing");
            assert!(
                !text.contains("..."),
                "{notice:?} writes `...`, not `…`: {text}"
            );
            assert!(!text.ends_with('.'), "{notice:?} ends in a period: {text}");
        }
    }

    #[test]
    fn test_pages_are_counted_from_one() {
        let position = Position {
            file: "paper.pdf".to_string(),
            page: 0,
            pages: 10,
            percent: 0,
        };
        let text = PdfNotice::Position(position).into_notice().text;
        assert_eq!(text, "paper.pdf, page 1 of 10, 0%");
    }

    #[test]
    fn test_a_count_is_plural_past_one() {
        let count = |words, rest| WordCount { rest, words }.to_string();
        assert_eq!(count(1, CountOf::Pages(1)), "1 word, 1 page");
        assert_eq!(count(70, CountOf::Pages(10)), "70 words, 10 pages");
        assert_eq!(
            count(3, CountOf::Characters(15)),
            "3 words, 15 characters selected"
        );
    }
}
