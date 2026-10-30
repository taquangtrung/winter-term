//! Winter spelling: checks English prose against a bundled Hunspell
//! dictionary, suggests corrections, and keeps the user's own word list.
//!
//! The dictionaries are compiled in, so nothing is read at runtime but the
//! user's list. Text offsets are byte offsets into the text given.
//!
//! - `affix`: the prefix and suffix rules of a Hunspell `.aff` file
//! - `dictionary`: bundled dictionaries and word checks
//! - `extract`: the prose of LaTeX, BibTeX, Markdown, and code comments
//! - `suggest`: corrections by edit distance and compound splits
//! - `tokenizer`: prose to the words worth checking
//! - `user_words`: the user's word list file

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// Checking
mod affix;
mod dictionary;
mod extract;
mod suggest;
mod tokenizer;

// Persistence
mod user_words;

pub use dictionary::{Dictionary, DictionaryId, UnknownDictionaryId, DICTIONARY_LICENSE};
pub use extract::{extract_prose, prose_for_extension, CommentStyle, Prose, Segment};
pub use suggest::DEFAULT_SUGGESTION_LIMIT;
pub use tokenizer::{
    should_check_token, tokenize, WordToken, MAX_UNCHECKED_ACRONYM_LENGTH,
    MIN_CHECKABLE_WORD_LENGTH,
};
pub use user_words::{default_user_words_path, UserWords};
