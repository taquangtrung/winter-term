//! Words a grammar leaves unscoped, scoped by what stands around them.
//!
//! Sublime's Rust grammar, and the Lean 4 grammar VS Code also reads with,
//! give most names no scope of their own: a type where it is used, a call,
//! a field, a variable where it is bound, a Lean tactic or namespace. VS
//! Code colors those through its own Rust grammar's guesses and the
//! language servers' semantic tokens. This pass makes the same guesses from
//! the line itself: the tokens next to a word, and the scopes the grammar
//! put it inside. Only words the grammar left in the text's color are
//! scoped, so nothing a grammar does name is ever overruled.
//!
//! A variable is colored where it is bound and left plain where it is used:
//! a binding can be told from the code alone, a use only guessed at.

use std::ops::Range;

use syntect::parsing::Scope;

// ========================================================================
// Constants
// ========================================================================

/// The scope each kind of word is given, from the names VS Code's own Rust
/// grammar and the semantic tokens' fallbacks use, which the theme colors.
const TYPE: &str = "entity.name.type";
const FUNCTION: &str = "entity.name.function";
const NAMESPACE: &str = "entity.name.namespace";
const CONSTANT: &str = "constant.other.caps";
const VARIANT: &str = "variable.other.enummember";
const FIELD: &str = "variable.other.member";
const BINDING: &str = "variable.declaration";
const KEYWORD: &str = "keyword.other.tactic";
const OPERATOR: &str = "keyword.operator";

/// Rust's keywords, which the grammar colors itself, and the words that
/// stand where a name might but are not one.
const RUST_KEYWORDS: &[&str] = &[
    "_", "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else",
    "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "type", "union", "unsafe", "use", "where", "while", "yield",
];

/// The Lean keywords that open a name of a namespace: what follows them on
/// the line is one.
const LEAN_NAMESPACE_OPENERS: &[&str] = &["end", "export", "import", "namespace", "open", "section"];

/// Lean's tactics, colored as keywords the way the Lean server's semantic
/// tokens have VS Code color them.
const LEAN_TACTICS: &[&str] = &[
    "aesop", "all_goals", "any_goals", "apply", "assumption", "by_cases", "by_contra", "calc",
    "case", "cases", "congr", "constructor", "contradiction", "conv", "decide", "done", "exact",
    "exfalso", "exists", "ext", "first", "funext", "generalize", "have", "induction", "intro",
    "intros", "left", "linarith", "next", "nlinarith", "norm_num", "obtain", "omega", "positivity",
    "rcases", "refine", "rename_i", "repeat", "rfl", "right", "ring", "rintro", "rw", "rwa",
    "show", "simp", "simp_all", "simpa", "sorry", "specialize", "split", "subst", "suffices",
    "trivial", "try", "unfold", "use",
];

/// Lean's other keywords, which the grammar colors itself or which stand
/// where a binder's name might.
const LEAN_KEYWORDS: &[&str] = &[
    "by", "def", "deriving", "do", "else", "example", "fun", "if", "in", "inductive", "instance",
    "lemma", "let", "match", "mut", "structure", "then", "theorem", "variable", "where", "with",
];

/// Lean's operators, which its grammar leaves in the text's color.
const LEAN_OPERATORS: &[&str] = &[
    ":=", "=>", "↦", "→", "←", "↔", "⟨", "⟩", "×", "∘", "¬", "∧", "∨", "≠", "≤", "≥", "∀", "∃", "λ",
    "▸", "·", "+", "-", "*", "/", "=", "<", ">", "|",
];

/// The letters Lean writes number types with, which name types.
const LEAN_TYPE_LETTERS: &[char] = &['ℕ', 'ℤ', 'ℚ', 'ℝ', 'ℂ'];

/// The symbols read as one token, longest first.
const SYMBOLS: &[&str] = &["::", "=>", "->", ":=", "..", "|"];

// ========================================================================
// Data Structures
// ========================================================================

/// A language whose unscoped words this pass names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Language {
    Lean,
    Rust,
}

/// A word of a line and the scope it is given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Word {
    /// Its bytes in the line.
    pub(super) range: Range<usize>,
    /// The scope it is given, under those the grammar put it inside.
    pub(super) scope: &'static str,
}

/// A run of a line the grammar scoped as one: its bytes, the scopes it sits
/// inside, and whether the theme colors it.
pub(super) struct Run<'a> {
    pub(super) plain: bool,
    pub(super) range: Range<usize>,
    pub(super) scopes: &'a [Scope],
}

/// One token of a line: a name, or a symbol.
#[derive(Clone, Debug)]
struct Token<'a> {
    range: Range<usize>,
    text: &'a str,
    word: bool,
}

// ========================================================================
// Language
// ========================================================================

impl Language {
    /// The language a grammar's top scope (`source.rust`) is, where this
    /// pass knows it.
    pub(super) fn of(top: &str) -> Option<Self> {
        match top {
            "source.rust" => Some(Self::Rust),
            "source.lean4" => Some(Self::Lean),
            _ => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The words of `line` in `language` the grammar left plain, each with the
/// scope what surrounds it gives it. `runs` are the line's runs as the
/// grammar scoped them, in order.
pub(super) fn scope_words(language: Language, line: &str, runs: &[Run]) -> Vec<Word> {
    let tokens = tokenize(line, language);
    let guesses = match language {
        Language::Rust => rust(&tokens, runs),
        Language::Lean => lean(&tokens),
    };
    // Only where the grammar left the whole word plain.
    guesses
        .into_iter()
        .filter(|word| {
            runs.iter()
                .find(|run| run.range.start <= word.range.start && word.range.end <= run.range.end)
                .is_some_and(|run| run.plain)
        })
        .collect()
}

/// The names and symbols of `line`, spaces dropped.
fn tokenize(line: &str, language: Language) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if is_name_start(c, language) {
            let mut end = start + c.len_utf8();
            while let Some(&(at, next)) = chars.peek() {
                // A Lean name runs on through a dot into its next part.
                let dotted = language == Language::Lean
                    && next == '.'
                    && line[at + 1..].chars().next().is_some_and(|c| is_name_start(c, language));
                if !is_name_char(next, language) && !dotted {
                    break;
                }
                end = at + next.len_utf8();
                chars.next();
            }
            tokens.push(Token { range: start..end, text: &line[start..end], word: true });
            continue;
        }
        let rest = &line[start..];
        let symbol = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol)).copied();
        let end = start + symbol.map_or(c.len_utf8(), str::len);
        for _ in 1..line[start..end].chars().count() {
            chars.next();
        }
        tokens.push(Token { range: start..end, text: &line[start..end], word: false });
    }
    tokens
}

/// Whether `c` starts a name.
fn is_name_start(c: char, language: Language) -> bool {
    match language {
        Language::Rust => c.is_ascii_alphabetic() || c == '_',
        Language::Lean => (c.is_alphabetic() && c != 'λ') || c == '_' || LEAN_TYPE_LETTERS.contains(&c),
    }
}

/// Whether `c` goes on a name.
fn is_name_char(c: char, language: Language) -> bool {
    match language {
        Language::Rust => c.is_ascii_alphanumeric() || c == '_',
        Language::Lean => {
            (c.is_alphanumeric() && c != 'λ') || matches!(c, '_' | '\'' | '!' | '?') || ('₀'..='₉').contains(&c)
        }
    }
}

/// Whether `name` reads as a type's: a capital, and a small letter after.
fn is_camel(name: &str) -> bool {
    name.starts_with(|c: char| c.is_uppercase()) && (name.chars().count() == 1 || name.chars().any(char::is_lowercase))
}

/// Whether `name` reads as a constant's: capitals, digits, and underscores.
fn is_caps(name: &str) -> bool {
    name.chars().count() > 1
        && name.chars().any(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `name` reads as a variable's or a function's: a small letter
/// first.
fn is_lower(name: &str) -> bool {
    name.starts_with(|c: char| c.is_lowercase() || c == '_') && name != "_"
}

/// How deep in brackets each token sits, counted before it.
fn depths(tokens: &[Token]) -> Vec<i32> {
    let mut depth = 0;
    tokens
        .iter()
        .map(|token| {
            let at = depth;
            match token.text {
                "(" | "[" | "{" | "⟨" | "⦃" => depth += 1,
                ")" | "]" | "}" | "⟩" | "⦄" => depth -= 1,
                _ => {}
            }
            at
        })
        .collect()
}

// ------------------------------------------------------------------------
// Rust
// ------------------------------------------------------------------------

/// Rust's unscoped words: a type, a variant, a call, a path's namespace, a
/// field, a constant, and a variable where a pattern binds it.
fn rust(tokens: &[Token], runs: &[Run]) -> Vec<Word> {
    let binds = rust_patterns(tokens);
    let depth = depths(tokens);
    // For each `{`, whether a type's name came before it: a struct literal,
    // whose `name:` is a field.
    let mut literal = Vec::new();
    let mut in_literal = vec![false; tokens.len()];
    for (at, token) in tokens.iter().enumerate() {
        in_literal[at] = literal.last().copied().unwrap_or(false);
        match token.text {
            "{" => literal.push(at > 0 && tokens[at - 1].word && is_camel(tokens[at - 1].text)),
            "}" => {
                literal.pop();
            }
            _ => {}
        }
    }
    let mut words = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        if !token.word || RUST_KEYWORDS.contains(&token.text) {
            continue;
        }
        let before = at.checked_sub(1).map(|at| tokens[at].text);
        let after = tokens.get(at + 1).map(|token| token.text);
        let name = token.text;
        let scope = if is_caps(name) {
            Some(CONSTANT)
        } else if is_camel(name) {
            let pathed = before == Some("::") && at >= 2 && tokens[at - 2].word && is_camel(tokens[at - 2].text);
            match pathed || in_enum_body(runs, token.range.start) {
                true => Some(VARIANT),
                false => Some(TYPE),
            }
        } else if !is_lower(name) {
            None
        } else if is_call(tokens, at) {
            Some(FUNCTION)
        } else if before == Some(".") {
            Some(FIELD)
        } else if after == Some("::") {
            Some(NAMESPACE)
        } else if matches!(before, Some("::" | "'")) {
            // A path's last part that is not called, or a lifetime's name.
            None
        } else if after == Some(":") && (in_literal[at] || binds[at] && depth[at] > 0) {
            Some(FIELD)
        } else if binds[at] && after != Some("!") {
            Some(BINDING)
        } else {
            None
        };
        if let Some(scope) = scope {
            words.push(Word { range: token.range.clone(), scope });
        }
    }
    words
}

/// Whether the name at token `at` is called: `(` after it, or a turbofish
/// (`::<`) before its arguments.
fn is_call(tokens: &[Token], at: usize) -> bool {
    match tokens.get(at + 1).map(|token| token.text) {
        Some("(") => true,
        Some("::") => tokens.get(at + 2).is_some_and(|next| next.text == "<"),
        _ => false,
    }
}

/// Whether byte `at` is in a run directly inside an enum's body, where a
/// name is one of its variants.
fn in_enum_body(runs: &[Run], at: usize) -> bool {
    let Some(run) = runs.iter().find(|run| run.range.contains(&at)) else {
        return false;
    };
    let names: Vec<String> = run.scopes.iter().rev().take(2).map(|scope| scope.build_string()).collect();
    matches!(names.as_slice(), [block, owner] if block.starts_with("meta.block") && owner.starts_with("meta.enum"))
}

/// Which tokens sit in a pattern that binds names: after `let` to its `=`
/// or type, between `for` and `in`, a closure's `|..|`, and a match arm
/// before its `=>`.
fn rust_patterns(tokens: &[Token]) -> Vec<bool> {
    let depth = depths(tokens);
    let mut binds = vec![false; tokens.len()];
    let mut at = 0;
    while at < tokens.len() {
        let text = tokens[at].text;
        let before = at.checked_sub(1).map(|at| tokens[at].text);
        if text == "let" {
            let level = depth[at];
            let mut next = at + 1;
            while next < tokens.len() && !(depth[next] == level && matches!(tokens[next].text, "=" | ":" | ";" | "else")) {
                binds[next] = true;
                next += 1;
            }
            at = next;
            continue;
        }
        if text == "for" && tokens.get(at + 1).is_some_and(|next| next.text != "<") {
            let mut next = at + 1;
            while next < tokens.len() && tokens[next].text != "in" {
                binds[next] = true;
                next += 1;
            }
            at = next;
            continue;
        }
        if text == "|" && matches!(before, None | Some("(" | "," | "=" | "move" | "return" | "{" | ":" | "=>")) {
            let mut next = at + 1;
            while next < tokens.len() && tokens[next].text != "|" {
                binds[next] = true;
                next += 1;
            }
            at = next + 1;
            continue;
        }
        if text == "=>" {
            // Back to where the arm starts: a `,` or a `{` as deep as the
            // arrow, or the line's start.
            let level = depth[at];
            let mut back = at;
            while back > 0 {
                let previous = &tokens[back - 1];
                if depth[back - 1] == level && previous.text == "," || depth[back - 1] < level {
                    break;
                }
                back -= 1;
                binds[back] = true;
            }
        }
        at += 1;
    }
    binds
}

// ------------------------------------------------------------------------
// Lean
// ------------------------------------------------------------------------

/// Lean's unscoped words: a namespace, a tactic, a type, a constructor, an
/// operator, and a name where a binder or a pattern binds it.
fn lean(tokens: &[Token]) -> Vec<Word> {
    let depth = depths(tokens);
    let binds = lean_binders(tokens, &depth);
    let opens_namespace = tokens.first().is_some_and(|first| LEAN_NAMESPACE_OPENERS.contains(&first.text));
    let mut words = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        let before = at.checked_sub(1).map(|at| tokens[at].text);
        let text = token.text;
        let scope = if !token.word {
            LEAN_OPERATORS.contains(&text).then_some(OPERATOR)
        } else if LEAN_KEYWORDS.contains(&text) || LEAN_NAMESPACE_OPENERS.contains(&text) {
            None
        } else if opens_namespace && text != "in" {
            Some(NAMESPACE)
        } else if before == Some("|") && (at == 1 || tokens.get(at.wrapping_sub(2)).is_some_and(|t| t.text == "with")) {
            // A constructor, where it is declared or matched on.
            Some(FUNCTION)
        } else if binds[at] {
            Some(BINDING)
        } else if LEAN_TACTICS.contains(&text) {
            Some(KEYWORD)
        } else if is_camel(last_part(text)) || text.chars().all(|c| LEAN_TYPE_LETTERS.contains(&c)) {
            Some(TYPE)
        } else {
            None
        };
        if let Some(scope) = scope {
            words.push(Word { range: token.range.clone(), scope });
        }
    }
    words
}

/// The last part of a dotted Lean name: `Point` of `Foo.Point`.
fn last_part(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Which tokens a Lean binder binds: the names before the `:` of a `(..)`,
/// `{..}`, `[..]`, or `⦃..⦄`; after `fun` or `λ` up to its `=>`; after `∀`
/// or `∃` up to its `,`; a field at the start of a line, before its `:`;
/// and the names a match arm's constructor binds, before its `=>`.
fn lean_binders(tokens: &[Token], depth: &[i32]) -> Vec<bool> {
    let mut binds = vec![false; tokens.len()];
    let names_until = |binds: &mut Vec<bool>, from: usize, stop: &dyn Fn(&Token) -> bool| {
        let mut at = from;
        while at < tokens.len() && !stop(&tokens[at]) {
            binds[at] = tokens[at].word;
            at += 1;
        }
    };
    for (at, token) in tokens.iter().enumerate() {
        match token.text {
            "(" | "{" | "[" | "⦃" => {
                // Only a binder: names, then `:` before the group closes.
                let level = depth[at] + 1;
                let mut next = at + 1;
                while next < tokens.len() && tokens[next].word && depth[next] == level {
                    next += 1;
                }
                if next > at + 1 && tokens.get(next).is_some_and(|t| t.text == ":") {
                    names_until(&mut binds, at + 1, &|t: &Token| t.text == ":");
                }
            }
            "fun" | "λ" => names_until(&mut binds, at + 1, &|t: &Token| matches!(t.text, "=>" | "↦" | ":")),
            "∀" | "∃" => names_until(&mut binds, at + 1, &|t: &Token| matches!(t.text, "," | ":")),
            // A match arm: the constructor, then the names it binds.
            "|" if tokens.iter().skip(at).any(|t| t.text == "=>") => {
                names_until(&mut binds, at + 2, &|t: &Token| t.text == "=>");
            }
            _ => {}
        }
    }
    // A field, or a structure's argument, at the start of a line: names
    // then `:`.
    let leading = tokens.iter().take_while(|token| token.word).count();
    if leading > 0
        && tokens.get(leading).is_some_and(|t| t.text == ":")
        && !LEAN_KEYWORDS.contains(&tokens[0].text)
        && !LEAN_NAMESPACE_OPENERS.contains(&tokens[0].text)
    {
        for bind in binds.iter_mut().take(leading) {
            *bind = true;
        }
    }
    binds
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// What each word of `line` is scoped as, taking the whole line as one
    /// plain run the grammar left unscoped.
    fn named(language: Language, line: &str) -> Vec<(&str, &'static str)> {
        let runs = [Run { plain: true, range: 0..line.len(), scopes: &[] }];
        scope_words(language, line, &runs)
            .into_iter()
            .map(|word| (&line[word.range], word.scope))
            .collect()
    }

    fn scope_of(language: Language, line: &str, name: &str) -> Option<&'static str> {
        named(language, line).into_iter().find(|(word, _)| *word == name).map(|(_, scope)| scope)
    }

    #[test]
    fn test_rust_s_unscoped_names_are_read_from_what_surrounds_them() {
        let rust = |line, name| scope_of(Language::Rust, line, name);
        let line = "let mut map: HashMap<String, u8> = HashMap::new();";
        assert_eq!(rust(line, "map"), Some(BINDING), "bound by let");
        assert_eq!(rust(line, "HashMap"), Some(TYPE));
        assert_eq!(rust(line, "new"), Some(FUNCTION), "a call through a path");
        assert_eq!(rust("std::collections::HashMap", "collections"), Some(NAMESPACE));
        assert_eq!(rust("total = self.y + other.y;", "y"), Some(FIELD));
        assert_eq!(rust("total = self.y + other.y;", "total"), None, "a use stays plain");
        assert_eq!(rust("helper(x)", "helper"), Some(FUNCTION));
        assert_eq!(rust("items.iter().collect::<Vec<_>>()", "collect"), Some(FUNCTION), "a turbofish call");
        assert_eq!(rust("if n > MAX {", "MAX"), Some(CONSTANT));
        assert_eq!(rust("Shape::Circle(r) => r,", "Circle"), Some(VARIANT));
        assert_eq!(rust("Shape::Circle(r) => r,", "r"), Some(BINDING), "bound by the arm");
        assert_eq!(rust("for (i, v) in xs.iter() {", "v"), Some(BINDING));
        assert_eq!(rust("xs.filter(|v| **v > 0)", "v"), Some(BINDING), "a closure's parameter");
        assert_eq!(rust("Point { x: 1, y }", "x"), Some(FIELD), "a struct literal's field");
        assert_eq!(rust("let Some(x) = opt else { return };", "x"), Some(BINDING));
        assert_eq!(rust("println!(\"{}\", v)", "println"), None, "a macro is the grammar's");
    }

    #[test]
    fn test_only_words_the_grammar_left_plain_are_scoped() {
        let line = "HashMap";
        let runs = [Run { plain: false, range: 0..line.len(), scopes: &[] }];
        assert!(scope_words(Language::Rust, line, &runs).is_empty());
    }

    #[test]
    fn test_lean_s_unscoped_names_are_read_from_what_surrounds_them() {
        let lean = |line, name| scope_of(Language::Lean, line, name);
        assert_eq!(lean("import Mathlib.Data.Nat.Basic", "Mathlib.Data.Nat.Basic"), Some(NAMESPACE));
        assert_eq!(lean("namespace Foo", "Foo"), Some(NAMESPACE));
        assert_eq!(lean("def add (a b : Nat) : Nat := a + b", "a"), Some(BINDING), "a binder");
        assert_eq!(lean("def add (a b : Nat) : Nat := a + b", "Nat"), Some(TYPE));
        assert_eq!(lean("def add (a b : Nat) : Nat := a + b", ":="), Some(OPERATOR));
        assert_eq!(lean("  | zero => simp", "zero"), Some(FUNCTION), "a constructor matched on");
        assert_eq!(lean("  | zero => simp", "simp"), Some(KEYWORD), "a tactic");
        assert_eq!(lean("  | succ n ih => omega", "ih"), Some(BINDING));
        assert_eq!(lean("  | node : Tree α → α → Tree α → Tree α", "node"), Some(FUNCTION));
        assert_eq!(lean("  | node : Tree α → α → Tree α → Tree α", "→"), Some(OPERATOR));
        assert_eq!(lean("fun p q => p.x + q.x", "q"), Some(BINDING));
        assert_eq!(lean("variable {n : ℕ} (h : n > 0)", "ℕ"), Some(TYPE));
        assert_eq!(lean("  x : Nat", "x"), Some(BINDING), "a structure's field");
        assert_eq!(lean("theorem t : ∀ x y, x + y = y + x := by", "y"), Some(BINDING));
    }
}
