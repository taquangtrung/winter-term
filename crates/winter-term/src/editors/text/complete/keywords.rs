//! Keywords: the reserved words of the language being written.

use super::{Candidate, Context, Kind, Reply};

/// Each language's keywords, as settings name the language.
const KEYWORDS: &[(&str, &[&str])] = &[
    ("c", &["auto", "break", "case", "char", "const", "continue", "default", "double", "else", "enum", "extern", "float", "for", "goto", "inline", "int", "long", "register", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile", "while"]),
    ("cpp", &["auto", "bool", "break", "case", "catch", "class", "const", "constexpr", "continue", "default", "delete", "else", "enum", "explicit", "false", "for", "friend", "if", "inline", "namespace", "new", "noexcept", "nullptr", "operator", "override", "private", "protected", "public", "return", "static", "struct", "switch", "template", "this", "throw", "true", "try", "typename", "using", "virtual", "void", "while"]),
    ("go", &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var"]),
    ("javascript", &["async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "else", "export", "extends", "false", "finally", "for", "function", "if", "import", "instanceof", "let", "new", "null", "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "while", "yield"]),
    ("python", &["False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "lambda", "nonlocal", "not", "pass", "raise", "return", "try", "while", "with", "yield"]),
    ("rust", &["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while"]),
    ("shell", &["case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "readonly", "return", "then", "until", "while"]),
    ("typescript", &["abstract", "any", "as", "async", "await", "boolean", "break", "case", "catch", "class", "const", "continue", "declare", "default", "else", "enum", "export", "extends", "false", "for", "function", "if", "implements", "import", "interface", "keyof", "let", "never", "new", "null", "number", "private", "protected", "public", "readonly", "return", "string", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "unknown", "void", "while"]),
];

/// The language's keywords, where a word is being typed.
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    let start = context.word_start();
    if start == context.col {
        return None;
    }
    let (_, words) = KEYWORDS.iter().find(|(language, _)| *language == context.language)?;
    let candidates = words
        .iter()
        .map(|word| Candidate {
            detail: None,
            insert: word.to_string(),
            kind: Kind::Keyword,
            label: word.to_string(),
            source: "keyword",
        })
        .collect();
    Some(Reply { candidates, start })
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    #[test]
    fn test_a_language_offers_its_own_keywords_and_an_unknown_one_none() {
        let lines = vec![String::new()];
        let reply = complete(&context("    ma", &lines, "rust")).expect("rust has keywords");
        assert!(reply.candidates.iter().any(|c| c.label == "match"));
        assert!(complete(&context("ma", &lines, "kdl")).is_none());
    }
}
