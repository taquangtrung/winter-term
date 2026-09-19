//! Coloring source as it is read: which run of characters is a comment, a
//! string, a number, a keyword, or the name of a type, for the languages
//! named by a file's extension.
//!
//! A lexer rather than a parser, and a small one: it colors what can be told
//! from the characters themselves, gets out of the way for a file it does not
//! know, and never decides that something is wrong with the code.

use std::path::Path;

// ========================================================================
// Constants
// ========================================================================

/// How far a `'` may look for its partner before it is read as something
/// other than a quote. Rust's lifetimes and OCaml's type variables are a `'`
/// that opens nothing, and running a string from one to the end of the line
/// paints most of a file the color of a string.
const QUOTE_REACH: usize = 4;

/// The languages named by extension, and what each one spells its comments,
/// strings, keywords, and built-in types with.
const LANGUAGES: [(&[&str], Syntax); 8] = [
    (
        &["rs"],
        Syntax {
            block: Some(("/*", "*/")),
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super",
                "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str",
                "u8", "u16", "u32", "u64", "u128", "usize",
            ],
        },
    ),
    (
        &["py"],
        Syntax {
            block: None,
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "try", "while", "with", "yield", "False", "True",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[
                "bool", "bytes", "dict", "float", "int", "list", "set", "str", "tuple",
            ],
        },
    ),
    (
        &["go"],
        Syntax {
            block: Some(("/*", "*/")),
            keywords: &[
                "break",
                "case",
                "chan",
                "const",
                "continue",
                "default",
                "defer",
                "else",
                "fallthrough",
                "false",
                "for",
                "func",
                "go",
                "goto",
                "if",
                "import",
                "interface",
                "map",
                "nil",
                "package",
                "range",
                "return",
                "select",
                "struct",
                "switch",
                "true",
                "type",
                "var",
            ],
            line: &["//"],
            quotes: &['"', '`', '\''],
            types: &[
                "bool",
                "byte",
                "complex64",
                "complex128",
                "error",
                "float32",
                "float64",
                "int",
                "int8",
                "int16",
                "int32",
                "int64",
                "rune",
                "string",
                "uint",
                "uint8",
                "uint16",
                "uint32",
                "uint64",
                "uintptr",
            ],
        },
    ),
    (
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        Syntax {
            block: Some(("/*", "*/")),
            keywords: &[
                "as",
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "default",
                "delete",
                "do",
                "else",
                "export",
                "extends",
                "false",
                "finally",
                "for",
                "from",
                "function",
                "if",
                "implements",
                "import",
                "in",
                "instanceof",
                "interface",
                "let",
                "new",
                "null",
                "of",
                "return",
                "static",
                "super",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "type",
                "typeof",
                "undefined",
                "var",
                "void",
                "while",
                "yield",
            ],
            line: &["//"],
            quotes: &['"', '\'', '`'],
            types: &[
                "any", "bigint", "boolean", "never", "number", "object", "string", "symbol",
                "unknown",
            ],
        },
    ),
    (
        &["c", "h", "cc", "cpp", "hpp", "cxx"],
        Syntax {
            block: Some(("/*", "*/")),
            keywords: &[
                "auto",
                "break",
                "case",
                "class",
                "const",
                "constexpr",
                "continue",
                "default",
                "delete",
                "do",
                "else",
                "enum",
                "extern",
                "false",
                "for",
                "goto",
                "if",
                "inline",
                "namespace",
                "new",
                "nullptr",
                "operator",
                "private",
                "protected",
                "public",
                "return",
                "sizeof",
                "static",
                "struct",
                "switch",
                "template",
                "this",
                "throw",
                "true",
                "typedef",
                "typename",
                "union",
                "using",
                "virtual",
                "volatile",
                "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "double", "float", "int", "long", "short", "signed", "size_t",
                "unsigned", "void",
            ],
        },
    ),
    (
        &["sh", "bash", "zsh"],
        Syntax {
            block: None,
            keywords: &[
                "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
                "if", "in", "local", "return", "then", "until", "while",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["json"],
        Syntax {
            block: None,
            keywords: &["false", "null", "true"],
            line: &[],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["toml", "kdl", "ini", "conf"],
        Syntax {
            block: None,
            keywords: &["false", "null", "true"],
            line: &["#", "//"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
];

// ========================================================================
// Data Structures
// ========================================================================

/// One language's spelling of the things worth coloring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Syntax {
    /// What opens and closes a comment that may run past the line it starts
    /// on, for a language that has one.
    block: Option<(&'static str, &'static str)>,
    /// Words that are the language rather than the program.
    keywords: &'static [&'static str],
    /// What opens a comment that runs to the end of the line.
    line: &'static [&'static str],
    /// What opens, and closes, a string.
    quotes: &'static [char],
    /// Words naming a type the language already has. Anything else opening
    /// with a capital is taken for a type as well, which is the convention
    /// every language here is written in.
    types: &'static [&'static str],
}

/// What one line of a file was colored as, and what the next line inherits.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Highlighted {
    /// What the line after this one begins inside.
    pub next: LineState,
    /// What each character of the line is, one entry per character.
    pub tokens: Vec<Token>,
}

/// Whether a line begins inside something an earlier line opened.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineState {
    /// Inside a comment that has not been closed yet.
    Comment,
    /// Ordinary code.
    #[default]
    Code,
}

/// What a character is, in the vocabulary a page paints.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Token {
    /// Prose about the code rather than the code.
    Comment,
    /// A word of the language rather than of the program.
    Keyword,
    /// Anything the lexer has nothing to say about.
    #[default]
    Normal,
    /// A literal number.
    Number,
    /// A literal string, quotes included.
    String,
    /// The name of a type.
    Type,
}

// ========================================================================
// Syntax
// ========================================================================

impl Syntax {
    /// What opens and closes a comment that may run past its own line. A line
    /// holding neither leaves the next one exactly as it found it, whatever
    /// else is written on it, which is what makes most of a file skippable
    /// when all that is wanted is what it leaves open.
    pub fn block(&self) -> Option<(&'static str, &'static str)> {
        self.block
    }

    /// Whether anything in the language can run past the line it starts on,
    /// which is the only reason a line has to know what precedes it.
    pub fn spans_lines(&self) -> bool {
        self.block.is_some()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The language `path`'s extension names, or `None` for a file whose language
/// this does not know, which is then painted as the plain text it may well be.
pub fn of_path(path: &Path) -> Option<&'static Syntax> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|(names, _)| names.contains(&extension.as_str()))
        .map(|(_, syntax)| syntax)
}

/// Color one line, given what the line before it left open.
pub fn highlight(line: &[char], syntax: &Syntax, state: LineState) -> Highlighted {
    let mut tokens = vec![Token::Normal; line.len()];
    let next = scan(line, syntax, state, Some(&mut tokens));
    Highlighted { next, tokens }
}

/// What a line leaves open for the next one, without working out its colors.
/// The same walk as [`highlight`], reading rather than writing, for the lines
/// between the top of a file and the part of it being looked at: they are
/// only worth what they say about the lines below them.
pub fn next_state(line: &[char], syntax: &Syntax, state: LineState) -> LineState {
    scan(line, syntax, state, None)
}

/// Walk a line, coloring it where somewhere to put the colors is given. One
/// walk rather than two, so what a line leaves open cannot come out different
/// from what it was painted as.
fn scan(
    line: &[char],
    syntax: &Syntax,
    state: LineState,
    mut tokens: Option<&mut Vec<Token>>,
) -> LineState {
    let mut at = 0;
    let mut state = state;
    while at < line.len() {
        if state == LineState::Comment {
            let closer = syntax.block.map(|(_, close)| close).unwrap_or_default();
            match find_from(line, at, closer) {
                Some(end) => {
                    paint(
                        &mut tokens,
                        at,
                        end + closer.chars().count(),
                        Token::Comment,
                    );
                    at = end + closer.chars().count();
                    state = LineState::Code;
                }
                None => {
                    paint(&mut tokens, at, line.len(), Token::Comment);
                    return LineState::Comment;
                }
            }
            continue;
        }
        if syntax.line.iter().any(|mark| starts_at(line, at, mark)) {
            paint(&mut tokens, at, line.len(), Token::Comment);
            break;
        }
        if let Some((open, close)) = syntax.block {
            if starts_at(line, at, open) {
                let from = at + open.chars().count();
                match find_from(line, from, close) {
                    Some(end) => {
                        let stop = end + close.chars().count();
                        paint(&mut tokens, at, stop, Token::Comment);
                        at = stop;
                    }
                    None => {
                        paint(&mut tokens, at, line.len(), Token::Comment);
                        return LineState::Comment;
                    }
                }
                continue;
            }
        }
        let c = line[at];
        if syntax.quotes.contains(&c) {
            match string_end(line, at, c) {
                Some(end) => {
                    paint(&mut tokens, at, end, Token::String);
                    at = end;
                }
                // An unterminated double quote is a string still being typed;
                // an unterminated `'` is a lifetime or an apostrophe, and is
                // left as the one ordinary character it is.
                None if c == '\'' => at += 1,
                None => {
                    paint(&mut tokens, at, line.len(), Token::String);
                    break;
                }
            }
            continue;
        }
        if c.is_ascii_digit() && !at_word_tail(line, at) {
            let end = number_end(line, at);
            paint(&mut tokens, at, end, Token::Number);
            at = end;
            continue;
        }
        if is_word_start(c) {
            let end = word_end(line, at);
            // What a word is only matters where the colors are wanted: it is
            // the one place a line's own contents are looked up in a list.
            if tokens.is_some() {
                let word: String = line[at..end].iter().collect();
                paint(&mut tokens, at, end, classify(&word, syntax));
            }
            at = end;
            continue;
        }
        at += 1;
    }
    state
}

/// What a word is: the language's own, one of its types, a name written the
/// way types are written, or nothing in particular.
fn classify(word: &str, syntax: &Syntax) -> Token {
    if syntax.keywords.contains(&word) {
        return Token::Keyword;
    }
    if syntax.types.contains(&word) {
        return Token::Type;
    }
    let capitalized = word.chars().next().is_some_and(char::is_uppercase);
    // A run of capitals is a constant rather than a type: `MAX_UNDO` names a
    // value, and coloring it as a type says something untrue about it.
    let shouting = word
        .chars()
        .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_');
    match capitalized && !shouting {
        true => Token::Type,
        false => Token::Normal,
    }
}

/// Where the string opened at `at` ends, one past its closing quote, or `None`
/// when the line ends first. A backslash escapes whatever follows it, so an
/// escaped quote does not close the string.
fn string_end(line: &[char], at: usize, quote: char) -> Option<usize> {
    let mut index = at + 1;
    // A `'` is only a quote when its partner is close enough to be one.
    let reach = match quote {
        '\'' => (index + QUOTE_REACH).min(line.len()),
        _ => line.len(),
    };
    while index < reach {
        match line[index] {
            '\\' => index += 2,
            c if c == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Where the number starting at `at` ends. Digits, the separators and radix
/// letters a literal may carry, and one trailing type suffix: near enough for
/// every language here without parsing any of them.
fn number_end(line: &[char], at: usize) -> usize {
    let mut index = at;
    while index < line.len() {
        let c = line[index];
        let part = c.is_ascii_alphanumeric() || c == '_';
        // A dot belongs to the number only with a digit after it, so the `.`
        // of a method call ends the literal rather than joining it.
        let decimal = c == '.' && line.get(index + 1).is_some_and(char::is_ascii_digit);
        if !part && !decimal {
            break;
        }
        index += 1;
    }
    index
}

/// Where the word starting at `at` ends.
fn word_end(line: &[char], at: usize) -> usize {
    let mut index = at;
    while index < line.len() && is_word_char(line[index]) {
        index += 1;
    }
    index
}

/// Whether `at` sits inside a word rather than at its start, which is what
/// keeps the `1` of `utf8_len` from being read as a number.
fn at_word_tail(line: &[char], at: usize) -> bool {
    at > 0 && is_word_char(line[at - 1])
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `needle` is written at `at`.
fn starts_at(line: &[char], at: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, c)| line.get(at + offset) == Some(&c))
}

/// Where `needle` next appears at or after `at`.
fn find_from(line: &[char], at: usize, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    (at..line.len()).find(|&index| starts_at(line, index, needle))
}

/// Mark every character of a half-open range as one thing, where anything is
/// keeping the marks.
fn paint(tokens: &mut Option<&mut Vec<Token>>, from: usize, to: usize, token: Token) {
    let Some(tokens) = tokens else {
        return;
    };
    let to = to.min(tokens.len());
    for slot in &mut tokens[from.min(to)..to] {
        *slot = token;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokens of `line` in `syntax`, as one letter per character, so a
    /// test reads as the line it is about: `c`omment, `k`eyword, `n`umber,
    /// `s`tring, `t`ype, and `.` for everything else, spaces included.
    fn marks(line: &str, syntax: &Syntax, state: LineState) -> (String, LineState) {
        let chars: Vec<char> = line.chars().collect();
        let out = highlight(&chars, syntax, state);
        let text = out
            .tokens
            .iter()
            .map(|token| match token {
                Token::Comment => 'c',
                Token::Keyword => 'k',
                Token::Normal => '.',
                Token::Number => 'n',
                Token::String => 's',
                Token::Type => 't',
            })
            .collect();
        (text, out.next)
    }

    fn rust() -> &'static Syntax {
        of_path(Path::new("a.rs")).expect("rust")
    }

    #[test]
    fn test_a_comment_marker_inside_a_string_does_not_open_a_comment() {
        // Scanning for `//` without reading the string first greys out the
        // rest of every line holding a URL.
        let (marks, _) = marks(r#"let u = "http://x"; // go"#, rust(), LineState::Code);
        assert_eq!(marks, "kkk.....ssssssssss..ccccc");
    }

    #[test]
    fn test_a_block_comment_carries_to_the_lines_under_it_and_closes() {
        let (first, state) = marks("code /* open", rust(), LineState::Code);
        assert_eq!(first, ".....ccccccc");
        assert_eq!(state, LineState::Comment, "the next line is inside it");

        let (middle, state) = marks("still inside", rust(), state);
        assert_eq!(middle, "cccccccccccc");
        assert_eq!(state, LineState::Comment);

        let (last, state) = marks("done */ code", rust(), state);
        assert_eq!(last, "ccccccc.....");
        assert_eq!(state, LineState::Code, "and code follows it again");
    }

    #[test]
    fn test_a_lifetime_is_not_a_string_that_never_ends() {
        // A `'` read as a quote with no partner paints the rest of the line,
        // and most of the rest of the file, the color of a string.
        let (lifetime, state) = marks("fn f<'a>(x: &'a str) {", rust(), LineState::Code);
        assert!(
            !lifetime.contains('s'),
            "nothing here is a string, got {lifetime}"
        );
        assert_eq!(state, LineState::Code);

        // A character literal is short enough to still be one.
        let (literal, _) = marks("let c = 'x';", rust(), LineState::Code);
        assert_eq!(literal, "kkk.....sss.");
    }

    #[test]
    fn test_an_escaped_quote_does_not_close_the_string() {
        let (marks, _) = marks(r#"("a\"b") x"#, rust(), LineState::Code);
        assert_eq!(marks, ".ssssss...");
    }

    #[test]
    fn test_digits_inside_a_name_are_part_of_the_name() {
        // `utf8_len` and `i32` are words, not a word with a number in it.
        let (marks, _) = marks("utf8_len + 42", rust(), LineState::Code);
        assert_eq!(marks, "...........nn");
    }

    #[test]
    fn test_a_shouting_name_is_a_constant_rather_than_a_type() {
        // Capitalized reads as a type, but a name in full capitals is a value
        // and saying otherwise is saying something untrue about it.
        let (marks, _) = marks("MAX_UNDO = Buffer", rust(), LineState::Code);
        assert_eq!(marks, "...........tttttt");
    }

    #[test]
    fn test_a_language_is_chosen_by_extension_and_an_unknown_one_is_left_alone() {
        assert!(of_path(Path::new("src/main.rs")).is_some());
        assert!(of_path(Path::new("script.PY")).is_some(), "case is not it");
        assert!(of_path(Path::new("notes.txt")).is_none());
        assert!(of_path(Path::new("Makefile")).is_none());
    }

    #[test]
    fn test_a_number_ends_where_a_method_call_begins() {
        // `1.max(2)` is a number and a call, not one long number.
        let (marks, _) = marks("1.max(2) 1.5", rust(), LineState::Code);
        assert_eq!(marks, "n.....n..nnn");
    }
}
