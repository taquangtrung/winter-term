//! Splits prose into the words worth spell-checking, skipping what is not
//! prose: emails, file paths, words with digits, and likely proper nouns.
//!
//! Offsets are byte offsets into the scanned text, so they slice it directly.

use std::collections::HashSet;

// ========================================================================
// Constants
// ========================================================================

/// The apostrophe the bundled dictionaries spell contractions with.
const STRAIGHT_APOSTROPHE: char = '\'';

/// The typographic apostrophe smart-quote editors and PDFs produce.
const CURLY_APOSTROPHE: char = '\u{2019}';

/// Opens a file name's extension.
const EXTENSION_DOT: char = '.';

/// Joins the parts of a hyphenated compound.
const HYPHEN: char = '-';

/// A chunk holding this is an email address or a handle, not prose.
const EMAIL_MARK: char = '@';

/// Path separators, Unix and Windows.
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

/// A chunk with at least this many separators is a path even without an
/// extension, unlike "and/or".
const MIN_PATH_SEPARATORS: usize = 2;

/// The longest run after a final dot that still reads as a file extension.
const MAX_FILE_EXTENSION_LENGTH: usize = 10;

/// What ends a sentence, so the next capitalized word may open one.
const SENTENCE_TERMINATORS: [char; 3] = ['.', '!', '?'];

/// Words shorter than this (loop variables, initials) are never checked.
pub const MIN_CHECKABLE_WORD_LENGTH: usize = 2;

/// All-caps words up to this long are taken for acronyms (ID, URL, HTML).
pub const MAX_UNCHECKED_ACRONYM_LENGTH: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// A word found in the text. `start..end` are byte offsets into the text
/// as given; `word` has curly apostrophes made straight, so it may be
/// shorter than that range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WordToken {
    /// One past the last byte.
    pub end: usize,
    /// The first byte.
    pub start: usize,
    /// The word as it should be looked up.
    pub word: String,
}

/// What a tokenizer knows about the language: prefixes that only appear
/// joined by a hyphen, and which hyphenated compounds are whole words.
pub(crate) struct TokenizeContext<'a, F> {
    pub hyphen_prefixes: &'a HashSet<String>,
    pub is_known_word: F,
}

// ========================================================================
// Public API
// ========================================================================

/// The words of `text`, with no knowledge of any language: no hyphen
/// prefix is dropped and every hyphenated compound splits at its hyphens.
pub fn tokenize(text: &str) -> Vec<WordToken> {
    let no_prefixes = HashSet::new();
    tokenize_with(
        text,
        &TokenizeContext {
            hyphen_prefixes: &no_prefixes,
            is_known_word: |_: &str| false,
        },
    )
}

/// Whether a word is worth looking up at all: long enough, not a short
/// acronym, and not an identifier-shaped mix of cases such as `getUserId`.
pub fn should_check_token(word: &str) -> bool {
    let length = word.chars().count();
    if length < MIN_CHECKABLE_WORD_LENGTH {
        return false;
    }
    if length <= MAX_UNCHECKED_ACRONYM_LENGTH && word.to_uppercase() == word {
        return false;
    }
    !is_mixed_case_compound(word)
}

// ========================================================================
// Tokenizing
// ========================================================================

/// The words of `text`, one per letter run, scanned a whitespace-delimited
/// chunk at a time. A run followed by a hyphen is first tried as a whole
/// compound; a known hyphen prefix, a run with digits, and a capitalized
/// word in mid-sentence are dropped.
pub(crate) fn tokenize_with<F>(text: &str, context: &TokenizeContext<'_, F>) -> Vec<WordToken>
where
    F: Fn(&str) -> bool,
{
    let mut tokens = Vec::new();
    for (chunk_start, chunk) in whitespace_chunks(text) {
        if chunk.contains(EMAIL_MARK) || looks_like_file_path(chunk) {
            continue;
        }
        let mut cursor = 0;
        while let Some(run_start) = next_run_start(chunk, cursor) {
            let run_end = word_run_end(chunk, run_start);
            cursor = run_end;
            let run = &chunk[run_start..run_end];
            if run.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            let followed_by_hyphen = chunk[run_end..].starts_with(HYPHEN);
            if followed_by_hyphen {
                let chain_end = hyphen_chain_end(chunk, run_start);
                if chain_end > run_end {
                    let compound = normalize_apostrophes(&chunk[run_start..chain_end]);
                    if (context.is_known_word)(&compound) {
                        tokens.push(WordToken {
                            end: chunk_start + chain_end,
                            start: chunk_start + run_start,
                            word: compound,
                        });
                        cursor = chain_end;
                        continue;
                    }
                }
                if context.hyphen_prefixes.contains(&run.to_lowercase()) {
                    continue;
                }
            }
            let start = chunk_start + run_start;
            if is_title_case_word(run) && !is_sentence_initial_position(text, start) {
                continue;
            }
            tokens.push(WordToken {
                end: start + run.len(),
                start,
                word: normalize_apostrophes(run),
            });
        }
    }
    tokens
}

/// Each maximal run of non-whitespace in `text`, with its byte offset.
fn whitespace_chunks(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split(char::is_whitespace)
        .filter(|chunk| !chunk.is_empty())
        .map(move |chunk| (chunk.as_ptr() as usize - text.as_ptr() as usize, chunk))
}

/// Where the next word run begins at or after `from`: the next letter.
fn next_run_start(chunk: &str, from: usize) -> Option<usize> {
    chunk[from..]
        .char_indices()
        .find(|(_, c)| c.is_alphabetic())
        .map(|(offset, _)| from + offset)
}

/// The end of the run of letters and digits starting at `start`, taking in
/// an apostrophe only when a letter follows it, so "isn't" stays whole but
/// a closing quote does not join the word.
fn word_run_end(chunk: &str, start: usize) -> usize {
    let mut chars = chunk[start..].char_indices().peekable();
    let mut end = start;
    while let Some((offset, c)) = chars.next() {
        if c.is_alphabetic() || c.is_ascii_digit() {
            end = start + offset + c.len_utf8();
            continue;
        }
        let is_apostrophe = c == STRAIGHT_APOSTROPHE || c == CURLY_APOSTROPHE;
        match chars.peek() {
            Some(&(next_offset, next)) if is_apostrophe && next.is_alphabetic() => {
                end = start + next_offset + next.len_utf8();
                chars.next();
            }
            Some(_) | None => break,
        }
    }
    end
}

/// The end of the longest `word(-word)+` chain starting at `start`, or the
/// end of the first run when no hyphen and word follow it.
fn hyphen_chain_end(chunk: &str, start: usize) -> usize {
    let mut end = word_run_end(chunk, start);
    while chunk[end..].starts_with(HYPHEN) {
        let next = end + HYPHEN.len_utf8();
        let starts_word = chunk[next..]
            .chars()
            .next()
            .is_some_and(char::is_alphabetic);
        if !starts_word {
            break;
        }
        end = word_run_end(chunk, next);
    }
    end
}

// ========================================================================
// Classification
// ========================================================================

/// Whether a chunk is a file name or path rather than prose that happens
/// to hold a slash: it ends in an extension, or has several separators.
fn looks_like_file_path(chunk: &str) -> bool {
    if has_file_extension(chunk) {
        return true;
    }
    chunk.matches(PATH_SEPARATORS).count() >= MIN_PATH_SEPARATORS
}

fn has_file_extension(chunk: &str) -> bool {
    let Some((_, extension)) = chunk.rsplit_once(EXTENSION_DOT) else {
        return false;
    };
    let length = extension.len();
    (1..=MAX_FILE_EXTENSION_LENGTH).contains(&length)
        && extension.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A single leading capital and the rest lowercase: a sentence opener or
/// a proper noun.
fn is_title_case_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let rest = chars.as_str();
    !rest.is_empty() && first.is_uppercase() && rest.to_lowercase() == rest
}

/// Upper and lower case mixed in any shape but Title Case: camelCase,
/// PascalCase, or an acronym fused onto a word (`MCSat`, `APIs`).
fn is_mixed_case_compound(word: &str) -> bool {
    let has_upper = word.chars().any(char::is_uppercase);
    let has_lower = word.chars().any(char::is_lowercase);
    has_upper && has_lower && !is_title_case_word(word)
}

/// Whether only whitespace lies between `start` and either the text's
/// beginning or a sentence terminator. Closing quotes after the terminator
/// are not looked past.
fn is_sentence_initial_position(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .rev()
        .find(|c| !c.is_whitespace())
        .is_none_or(|c| SENTENCE_TERMINATORS.contains(&c))
}

fn normalize_apostrophes(word: &str) -> String {
    word.replace(CURLY_APOSTROPHE, &STRAIGHT_APOSTROPHE.to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        tokenize(text).into_iter().map(|token| token.word).collect()
    }

    fn spans(tokens: Vec<WordToken>) -> Vec<(String, usize, usize)> {
        tokens
            .into_iter()
            .map(|token| (token.word, token.start, token.end))
            .collect()
    }

    fn words_with_prefixes(text: &str, prefixes: &[&str]) -> Vec<String> {
        let prefixes: HashSet<String> = prefixes.iter().map(|p| p.to_string()).collect();
        let context = TokenizeContext {
            hyphen_prefixes: &prefixes,
            is_known_word: |_: &str| false,
        };
        tokenize_with(text, &context)
            .into_iter()
            .map(|token| token.word)
            .collect()
    }

    fn tokens_knowing(text: &str, known: &str) -> Vec<WordToken> {
        let prefixes = HashSet::new();
        let context = TokenizeContext {
            hyphen_prefixes: &prefixes,
            is_known_word: |word: &str| word.eq_ignore_ascii_case(known),
        };
        tokenize_with(text, &context)
    }

    #[test]
    fn test_tokenize_keeps_camel_case_whole_with_offsets() {
        assert_eq!(
            spans(tokenize("getUserId")),
            vec![("getUserId".to_string(), 0, 9)]
        );
        assert_eq!(
            words("I study at VinUniversity now"),
            ["I", "study", "at", "VinUniversity", "now"]
        );
        assert_eq!(words("XMLHttpRequest"), ["XMLHttpRequest"]);
    }

    #[test]
    fn test_tokenize_splits_snake_and_kebab_case() {
        assert_eq!(words("get_user-id"), ["get", "user", "id"]);
    }

    #[test]
    fn test_tokenize_drops_runs_with_digits() {
        assert_eq!(words("utf8 item2 plainword"), ["plainword"]);
    }

    #[test]
    fn test_tokenize_skips_email_chunks() {
        assert_eq!(words("contact info@vldb.org please"), ["contact", "please"]);
    }

    #[test]
    fn test_tokenize_skips_paths_and_file_names() {
        assert_eq!(
            words("see crates/verifier/src/infer/rules.rs please"),
            ["see", "please"]
        );
        assert_eq!(words("see src/rules.rs please"), ["see", "please"]);
        assert_eq!(words("see redesign-symexec.md please"), ["see", "please"]);
        assert_eq!(
            words("see crates\\verifier\\src\\infer\\rules.rs please"),
            ["see", "please"]
        );
    }

    #[test]
    fn test_tokenize_keeps_single_slash_without_extension() {
        assert_eq!(
            words("bring cats and/or dogs"),
            ["bring", "cats", "and", "or", "dogs"]
        );
    }

    #[test]
    fn test_tokenize_drops_hyphen_prefix_runs() {
        assert_eq!(
            words_with_prefixes("a pre-configured post-processing step", &["pre", "post"]),
            ["a", "configured", "processing", "step"]
        );
        assert_eq!(
            words_with_prefixes("Pre-existing conditions apply", &["pre"]),
            ["existing", "conditions", "apply"]
        );
        assert_eq!(
            words_with_prefixes("pre and post are words too", &["pre", "post"]),
            ["pre", "and", "post", "are", "words", "too"]
        );
        assert_eq!(
            words_with_prefixes("a non-existant option", &["non"]),
            ["a", "existant", "option"]
        );
        assert_eq!(
            words("a pre-configured option"),
            ["a", "pre", "configured", "option"]
        );
    }

    #[test]
    fn test_tokenize_emits_known_hyphen_compound_whole() {
        assert_eq!(
            spans(tokens_knowing("a de-facto standard", "de-facto")),
            vec![
                ("a".to_string(), 0, 1),
                ("de-facto".to_string(), 2, 10),
                ("standard".to_string(), 11, 19),
            ]
        );
        let long = tokens_knowing("a twenty-one-year-old car", "twenty-one-year-old");
        assert_eq!(long[1].word, "twenty-one-year-old");
        assert_eq!(
            words("a de-facto standard"),
            ["a", "de", "facto", "standard"]
        );
    }

    #[test]
    fn test_tokenize_drops_mid_sentence_title_case_only() {
        assert_eq!(
            words("if we see a name like Rollups here"),
            ["if", "we", "see", "a", "name", "like", "here"]
        );
        assert_eq!(
            words("Teh quick fox jumps"),
            ["Teh", "quick", "fox", "jumps"]
        );
        assert_eq!(
            words("Done. Susan left early."),
            ["Done", "Susan", "left", "early"]
        );
        assert_eq!(
            words("please call GetUser now"),
            ["please", "call", "GetUser", "now"]
        );
    }

    #[test]
    fn test_tokenize_keeps_accented_words_whole_with_byte_offsets() {
        assert_eq!(
            words("the Gröbner basis algorithm"),
            ["the", "basis", "algorithm"]
        );
        assert_eq!(words("order a café here"), ["order", "a", "café", "here"]);
        // "ö" is two bytes, so every later offset is one past its char index.
        assert_eq!(
            spans(tokenize("Gröbner bases are used here.")),
            vec![
                ("Gröbner".to_string(), 0, 8),
                ("bases".to_string(), 9, 14),
                ("are".to_string(), 15, 18),
                ("used".to_string(), 19, 23),
                ("here".to_string(), 24, 28),
            ]
        );
    }

    #[test]
    fn test_tokenize_keeps_contractions_and_normalizes_curly_apostrophe() {
        let expected = vec![
            ("it".to_string(), 0, 2),
            ("isn't".to_string(), 3, 8),
            ("ready".to_string(), 9, 14),
        ];
        assert_eq!(spans(tokenize("it isn't ready")), expected);
        // The curly apostrophe is three bytes: the span covers the source
        // text while the word holds the straight form.
        let curly = spans(tokenize("it isn\u{2019}t ready"));
        assert_eq!(curly[1], ("isn't".to_string(), 3, 10));
        assert_eq!(words("say 'hello' now"), ["say", "hello", "now"]);
    }

    #[test]
    fn test_should_check_token_rules() {
        assert!(!should_check_token("a"));
        assert!(should_check_token("ab"));
        assert!(!should_check_token("ID"));
        assert!(!should_check_token("HTML"));
        assert!(should_check_token("HELLO"));
        for identifier in [
            "MCSat",
            "XMLHttpRequest",
            "ZKPs",
            "APIs",
            "getUserId",
            "VinUniversity",
            "zkSNARK",
            "parseJSON",
            "toURL",
        ] {
            assert!(!should_check_token(identifier), "{identifier}");
        }
        assert!(should_check_token("Rollups"));
        assert!(should_check_token("hello"));
    }
}
