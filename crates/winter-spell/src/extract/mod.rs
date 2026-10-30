//! The prose of a document, for spell-checking only what a reader reads:
//! LaTeX and BibTeX without their markup, Markdown without its code, and
//! nothing of source code but its comments. A port of magic-vscode's
//! `contentExtraction.ts`.
//!
//! Extraction drops what is not prose and keeps the rest at its original
//! byte offsets. Dropped markup joins the prose around it (`{P}rogram`
//! reads `Program`), except where the source put a space or placeholder
//! in its place (math, code, URLs, citations, `~`), which keeps it apart.
//!
//! - `comments`: comments in code, by comment style
//! - `latex`: LaTeX markup
//! - `markup`: BibTeX and Markdown markup
//! - `scan`: the byte mask, the pass driver, and shared matchers

use self::comments::comment_ranges;
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

/// File extensions per kind of prose, matched without case. Each mirrors
/// a magic-vscode language: `latex`, `bibtex`, `markdown`, `plaintext`,
/// and the languages of each comment style. Extensions two languages
/// share (`.v` for Verilog and Coq, `.m` for Objective-C and MATLAB) are
/// left out, as are the ML family's `(* *)` comments (`.ml`, `.mli`),
/// which magic-vscode does not extract either.
const PROSE_EXTENSIONS: [(&[&str], Prose); 7] = [
    (&["bib"], Prose::Bibtex),
    (&DASH_EXTENSIONS, Prose::Comments(CommentStyle::Dash)),
    (&HASH_EXTENSIONS, Prose::Comments(CommentStyle::Hash)),
    (&["cls", "latex", "ltx", "sty", "tex"], Prose::Latex),
    (&["markdown", "md", "mdown", "mkd"], Prose::Markdown),
    (&["text", "txt"], Prose::PlainText),
    (&SLASH_EXTENSIONS, Prose::Comments(CommentStyle::Slash)),
];

/// Extensions of languages with `--` line comments.
const DASH_EXTENSIONS: [&str; 14] = [
    "ada",
    "adb",
    "ads",
    "agda",
    "applescript",
    "elm",
    "hs",
    "idr",
    "lean",
    "lua",
    "purs",
    "sql",
    "vhd",
    "vhdl",
];

/// Extensions of languages with `#` line comments.
const HASH_EXTENSIONS: [&str; 26] = [
    "bash",
    "coffee",
    "ex",
    "exs",
    "fish",
    "gql",
    "graphql",
    "ini",
    "jl",
    "mk",
    "nim",
    "pl",
    "pm",
    "properties",
    "ps1",
    "py",
    "pyi",
    "pyw",
    "r",
    "rb",
    "sh",
    "tcl",
    "toml",
    "yaml",
    "yml",
    "zsh",
];

/// Extensions of languages with `//` line and `/* */` block comments.
const SLASH_EXTENSIONS: [&str; 51] = [
    "c", "cairo", "cc", "circom", "cjs", "clar", "cpp", "cs", "css", "cts", "cxx", "dart", "frag",
    "fs", "fsx", "glsl", "go", "groovy", "h", "hh", "hlsl", "hpp", "hxx", "java", "js", "jsonc",
    "jsx", "kt", "kts", "less", "metal", "mjs", "mm", "move", "mts", "pest", "php", "qml", "rs",
    "sass", "scala", "scss", "shader", "sol", "styl", "swift", "ts", "tsx", "vert", "vue", "yul",
];

/// VS Code language ids per comment style, as magic-vscode lists them,
/// for the info string of a Markdown code fence.
const COMMENT_LANGUAGES: [(&[&str], CommentStyle); 3] = [
    (
        &[
            "ada",
            "agda",
            "applescript",
            "coq",
            "eiffel",
            "elm",
            "haskell",
            "idris",
            "lean",
            "lean4",
            "lua",
            "plsql",
            "purescript",
            "sql",
            "vhdl",
        ],
        CommentStyle::Dash,
    ),
    (
        &[
            "bat",
            "coffeescript",
            "dockerfile",
            "elixir",
            "fish",
            "git-commit",
            "git-rebase",
            "graphql",
            "ignore",
            "ini",
            "julia",
            "makefile",
            "nim",
            "perl",
            "powershell",
            "properties",
            "python",
            "r",
            "ruby",
            "shellscript",
            "tcl",
            "toml",
            "yaml",
        ],
        CommentStyle::Hash,
    ),
    (
        &[
            "c",
            "cairo",
            "circom",
            "clarity",
            "cpp",
            "csharp",
            "css",
            "dart",
            "fsharp",
            "glsl",
            "go",
            "groovy",
            "hlsl",
            "java",
            "javascript",
            "javascriptreact",
            "jsonc",
            "kotlin",
            "less",
            "metal",
            "move",
            "objective-c",
            "objective-cpp",
            "pest",
            "php",
            "qml",
            "rust",
            "sass",
            "scala",
            "scss",
            "shaderlab",
            "solidity",
            "stylus",
            "swift",
            "typescript",
            "typescriptreact",
            "verilog",
            "vyper",
            "vue",
            "yul",
        ],
        CommentStyle::Slash,
    ),
];

// ========================================================================
// Data Structures
// ========================================================================

/// How a document holds its prose.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Prose {
    /// A BibTeX database: field values but people and identifiers.
    Bibtex,
    /// Source code: its comments only.
    Comments(CommentStyle),
    /// A LaTeX document or package.
    Latex,
    /// A Markdown document.
    Markdown,
    /// Plain text: all of it.
    PlainText,
}

/// How a programming language writes its comments.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommentStyle {
    /// `--` line comments, as in Haskell, Lua, and SQL.
    Dash,
    /// `#` line comments, as in Python, shell, and YAML.
    Hash,
    /// `//` line and `/* */` block comments, as in C, Rust, and JavaScript.
    Slash,
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
        Prose::Comments(style) => {
            let mut mask = Mask::kept_none(text);
            for comment in comment_ranges(text, style) {
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

/// The comment style of the language a Markdown fence's info string
/// names by its first word, as a VS Code language id or a file extension.
fn comment_style_for_fence(info: &str) -> Option<CommentStyle> {
    let language = info.split_whitespace().next()?.to_ascii_lowercase();
    COMMENT_LANGUAGES
        .iter()
        .find(|(languages, _)| languages.contains(&language.as_str()))
        .map(|&(_, style)| style)
        .or_else(|| match prose_for_extension(&language) {
            Some(Prose::Comments(style)) => Some(style),
            Some(Prose::Bibtex | Prose::Latex | Prose::Markdown | Prose::PlainText) | None => None,
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prose_for_extension_ignores_case_and_skips_unsupported() {
        assert_eq!(
            prose_for_extension("RS"),
            Some(Prose::Comments(CommentStyle::Slash))
        );
        assert_eq!(prose_for_extension("Tex"), Some(Prose::Latex));
        assert_eq!(prose_for_extension("ml"), None);
        assert_eq!(prose_for_extension("json"), None);
    }

    #[test]
    fn test_extract_prose_gives_byte_ranges_of_comments() {
        let code = "let s = \"é\"; // naïve note\nlet t = 1;";
        let segments = extract_prose(code, Prose::Comments(CommentStyle::Slash));
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
