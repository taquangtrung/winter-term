//! The prose of a document, for spell-checking only what a reader reads:
//! LaTeX and BibTeX without their markup, Markdown without its code, and
//! nothing of source code but its comments.
//!
//! Extraction drops what is not prose and keeps the rest at its original
//! byte offsets. Dropped markup joins the prose around it (`{P}rogram`
//! reads `Program`), except where the source put a space or placeholder
//! in its place (math, code, URLs, citations, `~`), which keeps it apart.
//!
//! - `comments`: comments in code, as the language's grammar reads them
//! - `latex`: LaTeX markup
//! - `markup`: BibTeX and Markdown markup
//! - `scan`: the byte mask, the pass driver, and shared matchers

use self::comments::{comment_ranges, language_named};
use self::latex::drop_latex_markup;
use self::markup::{drop_bibtex_markup, drop_markdown_markup};
use self::scan::{Mask, ProseText};

mod comments;
mod latex;
mod markup;
mod scan;

// ========================================================================
// Constants
// ========================================================================

/// File extensions per kind of prose, matched without case: `latex`,
/// `bibtex`, `markdown`, `plaintext`. Any other is code in the language
/// iris tells by it, if one with comments.
const PROSE_EXTENSIONS: [(&[&str], Prose); 4] = [
    (&["bib"], Prose::Bibtex),
    (&["cls", "latex", "ltx", "sty", "tex"], Prose::Latex),
    (&["markdown", "md", "mdown", "mkd"], Prose::Markdown),
    (&["text", "txt"], Prose::PlainText),
];

// ========================================================================
// Data Structures
// ========================================================================

/// How a document holds its prose.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Prose {
    /// A BibTeX database: field values but people and identifiers.
    Bibtex,
    /// Source code in the language with this id, as iris names it: its
    /// comments only.
    Comments(&'static str),
    /// A LaTeX document or package.
    Latex,
    /// A Markdown document.
    Markdown,
    /// Plain text: all of it.
    PlainText,
}

/// A stretch of a document that is prose. `start..end` are byte offsets
/// into the document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Segment {
    /// One past the last byte.
    pub end: usize,
    /// The first byte.
    pub start: usize,
}

// ========================================================================
// Extraction
// ========================================================================

/// The kind of prose a file holds, by its extension (without the dot,
/// in any case), or `None` for a file not worth checking.
pub fn prose_for_extension(extension: &str) -> Option<Prose> {
    let extension = extension.to_ascii_lowercase();
    PROSE_EXTENSIONS
        .iter()
        .find(|(extensions, _)| extensions.contains(&extension.as_str()))
        .map(|&(_, prose)| prose)
        .or_else(|| language_named(&extension).map(Prose::Comments))
}

/// The prose stretches of `text`, read as `prose`, in order.
pub fn extract_prose(text: &str, prose: Prose) -> Vec<Segment> {
    prose_mask(text, prose).segments()
}

/// The prose of `text` joined up for the tokenizer, with the way back to
/// `text`'s offsets.
pub(crate) fn extract_prose_text(text: &str, prose: Prose) -> ProseText {
    prose_mask(text, prose).prose_text()
}

/// Which bytes of `text` are prose.
fn prose_mask(text: &str, prose: Prose) -> Mask<'_> {
    match prose {
        Prose::Bibtex => {
            let mut mask = Mask::kept_all(text);
            drop_bibtex_markup(&mut mask);
            mask
        }
        Prose::Comments(language) => {
            let mut mask = Mask::kept_none(text);
            for comment in comment_ranges(text, language) {
                mask.keep(comment);
            }
            mask
        }
        Prose::Latex => {
            let mut mask = Mask::kept_all(text);
            drop_latex_markup(&mut mask);
            mask
        }
        Prose::Markdown => {
            let mut mask = Mask::kept_all(text);
            drop_markdown_markup(&mut mask);
            mask
        }
        Prose::PlainText => Mask::kept_all(text),
    }
}

/// The language a Markdown fence's info string names by its first word,
/// as a language id, an alias, or a file extension, if one with comments.
fn fence_language(info: &str) -> Option<&'static str> {
    language_named(&info.split_whitespace().next()?.to_ascii_lowercase())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prose_for_extension_ignores_case_and_tells_code_by_its_language() {
        assert_eq!(prose_for_extension("RS"), Some(Prose::Comments("rust")));
        assert_eq!(prose_for_extension("Tex"), Some(Prose::Latex));
        assert_eq!(prose_for_extension("ml"), Some(Prose::Comments("ocaml")));
        assert_eq!(prose_for_extension("no-such-kind"), None);
    }

    #[test]
    fn test_extract_prose_gives_byte_ranges_of_comments() {
        let code = "let s = \"é\"; // naïve note\nlet t = 1;";
        let segments = extract_prose(code, Prose::Comments("rust"));
        let texts: Vec<&str> = segments.iter().map(|s| &code[s.start..s.end]).collect();
        assert_eq!(texts, ["// naïve note"]);
    }

    #[test]
    fn test_extract_prose_text_joins_braces_but_parts_placeholders() {
        let bibtex = extract_prose_text("{P}rogram notes", Prose::Bibtex);
        assert_eq!(bibtex.text(), "Program notes");
        let latex = extract_prose_text("Table~one", Prose::Latex);
        assert_eq!(latex.text(), "Table one");
        let one = latex.text().find("one").unwrap();
        assert_eq!(latex.original_range(one..one + 3), 6..9);
    }
}
