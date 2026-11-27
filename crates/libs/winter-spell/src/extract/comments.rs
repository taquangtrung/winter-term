//! Comments in source code, as the language's TextMate grammar reads them
//! through iris: a docstring counts as one, a comment marker inside a
//! string does not.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use iris::{Grammar, Language, Languages, Registry, Tokenizer};

// ========================================================================
// Constants
// ========================================================================

/// The file name a language is told from by its extension alone.
const BY_EXTENSION: &str = "code";

// ========================================================================
// Data Structures
// ========================================================================

/// The languages iris knows and their grammars, each compiled once.
struct Grammars {
    /// The grammars compiled so far, by scope; `None` for one that failed.
    compiled: Mutex<HashMap<String, Option<Arc<Grammar>>>>,
    languages: Languages,
    registry: Registry,
}

// ========================================================================
// Grammars Implementations
// ========================================================================

impl Grammars {
    /// The grammar of `language`, compiled the first time it is asked for.
    fn grammar_of(&self, language: &Language) -> Option<Arc<Grammar>> {
        let scope = language.scope.as_deref()?;
        let mut compiled = self.compiled.lock().unwrap_or_else(PoisonError::into_inner);
        compiled
            .entry(scope.to_owned())
            .or_insert_with(|| self.registry.grammar(scope).ok())
            .clone()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The id of the language `name` names, by its id or an alias (`rust`), or
/// as an extension (`rs`), as a Markdown fence names its code's; `None` for
/// one with no grammar or no comments.
pub(super) fn language_named(name: &str) -> Option<&'static str> {
    let languages = &grammars().languages;
    let language = languages
        .get(name)
        .or_else(|| languages.detect(format!("{BY_EXTENSION}.{name}"), ""))?;
    let commented =
        language.config.line_comment.is_some() || language.config.block_comment.is_some();
    (language.scope.is_some() && commented).then_some(language.id.as_str())
}

/// The comments of `code`, in the language with id `language`, in order,
/// each a byte range of `code`; none for a language with no grammar.
pub(super) fn comment_ranges(code: &str, language: &str) -> Vec<Range<usize>> {
    let grammars = grammars();
    grammars
        .languages
        .get(language)
        .and_then(|language| grammars.grammar_of(language))
        .map(|grammar| iris::comment_ranges(&mut Tokenizer::new(grammar), code))
        .unwrap_or_default()
}

/// The languages and grammars every check shares.
fn grammars() -> &'static Grammars {
    static GRAMMARS: OnceLock<Grammars> = OnceLock::new();
    GRAMMARS.get_or_init(|| Grammars {
        compiled: Mutex::default(),
        languages: Languages::builtin(),
        registry: Registry::bundled(),
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn comments<'c>(code: &'c str, name: &str) -> Vec<&'c str> {
        let language = language_named(name).expect("a language with comments");
        comment_ranges(code, language)
            .into_iter()
            .map(|range| &code[range])
            .collect()
    }

    #[test]
    fn test_a_language_is_named_by_its_id_an_alias_or_an_extension() {
        assert_eq!(language_named("rust"), Some("rust"));
        assert_eq!(language_named("rs"), Some("rust"));
        assert_eq!(language_named("py"), Some("python"));
        assert_eq!(language_named("no-such-language"), None);
    }

    #[test]
    fn test_comments_are_read_as_the_languages_grammar_reads_them() {
        let rust = "let s = \"// not one\"; // the count\n/* a block\n note */ g();";
        assert_eq!(
            comments(rust, "rust"),
            ["// the count", "/* a block", " note */"]
        );
        assert_eq!(comments("x = '# no'  # set x", "python"), ["# set x"]);
        assert_eq!(comments("SELECT 1; -- one row\n", "sql"), ["-- one row"]);
        assert_eq!(comments("let x = 1 (* note *)", "ocaml"), ["(* note *)"]);
    }

    #[test]
    fn test_comments_keep_byte_offsets_past_multibyte_text() {
        let code = "let s = \"é\"; // café note";
        assert_eq!(comments(code, "rust"), ["// café note"]);
    }
}
